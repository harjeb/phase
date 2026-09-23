#![cfg(feature = "manabrew")]

use engine::game::deck_loading::{load_deck_into_state, DeckEntry, DeckPayload, PlayerDeckPayload};
use engine::game::game_object::GameObject;
use engine::game::match_flow::handle_game_over_transition;
use engine::types::card::CardFace;
use engine::types::card_type::{CardType, CoreType};
use engine::types::game_state::WaitingFor;
use engine::types::mana::ManaCost;
use engine::types::match_config::{MatchPhase, MatchType};
use engine::types::PlayerId;
use manabrew_compat::{ClientToServerMessage, PromptInput};
use server_core::session::{GameSession, SessionManager};
use serde_json::json;

fn entry(name: &str, count: u32) -> DeckEntry {
    DeckEntry {
        count,
        card: CardFace {
            name: name.into(), mana_cost: ManaCost::NoCost,
            card_type: CardType { supertypes: vec![], core_types: vec![CoreType::Land], subtypes: vec![] },
            power: None, toughness: None, loyalty: None, defense: None,
            oracle_text: None, non_ability_text: None, flavor_name: None,
            keywords: vec![], abilities: vec![], triggers: vec![], static_abilities: vec![], replacements: vec![],
            cleave_variant: None, color_override: None, color_identity: vec![], scryfall_oracle_id: None,
            modal: None, additional_cost: None, strive_cost: None, casting_restrictions: vec![], casting_options: vec![],
            solve_condition: None, parse_warnings: vec![], brawl_commander: false, is_commander: false,
            is_oathbreaker: false, deck_copy_limit: None, metadata: Default::default(), rarities: Default::default(), attraction_lights: vec![],
        },
    }
}

fn lookup(_: &GameObject) -> Option<String> { Some(String::new()) }

fn fixture() -> (SessionManager, String, String, String) {
    let player = PlayerDeckPayload { main_deck: vec![entry("Forest", 60)], sideboard: vec![entry("Island", 1)], ..Default::default() };
    let opponent = PlayerDeckPayload { main_deck: vec![entry("Mountain", 60)], sideboard: vec![entry("Swamp", 1)], ..Default::default() };
    let mut manager = SessionManager::new();
    let (code, token) = manager.create_game(player.clone(), None);
    let other = "second-seat-token".to_string();
    {
        let mut session = manager.try_session(&code).unwrap();
        session.player_tokens[1] = other.clone();
        session.connected[1] = true;
        session.game_started = true;
        session.state.match_config.match_type = MatchType::Bo3;
        load_deck_into_state(&mut session.state, &DeckPayload { player, opponent, ..Default::default() });
        session.state.match_phase = MatchPhase::InGame;
        session.state.waiting_for = WaitingFor::GameOver { winner: Some(PlayerId(0)) };
        handle_game_over_transition(&mut session.state);
    }
    (manager, code, token, other)
}

fn unchanged_submission(session: &GameSession, token: &str) -> ClientToServerMessage {
    let prompt = session.manabrew_snapshot(token, &lookup).unwrap().prompt.unwrap();
    let PromptInput::Sideboard(input) = prompt.input else { panic!("expected sideboard") };
    serde_json::from_value(json!({
        "kind": "response", "promptId": prompt.prompt_id,
        "action": {"type": "sideboard", "output": {"type": "submitSideboard", "main": input.main, "sideboard": input.sideboard}}
    })).unwrap()
}

#[test]
fn bo3_private_pool_wrong_seat_stale_and_full_restart() {
    let (manager, code, token, other) = fixture();
    let mut session = manager.try_session(&code).unwrap();
    let own = session.manabrew_snapshot(&token, &lookup).unwrap();
    let waiting = session.manabrew_snapshot(&other, &lookup).unwrap();
    assert!(waiting.prompt.is_none());
    let wire = serde_json::to_string(&own).unwrap();
    assert!(wire.contains("Island"));
    assert!(!wire.contains("Swamp"));
    assert!(!wire.contains("Mountain"));
    assert!(!own.update.game_view.game_over);
    let first = unchanged_submission(&session, &token);
    assert!(session.handle_manabrew_message(&other, first.clone()).is_err());
    assert_eq!(session.state_revision, 0);
    session.handle_manabrew_message(&token, first.clone()).unwrap();
    assert_eq!(session.state_revision, 1);
    assert!(session.handle_manabrew_message(&token, first).is_err());
    assert!(session.manabrew_snapshot(&token, &lookup).unwrap().prompt.is_none());
    let second = unchanged_submission(&session, &other);
    session.handle_manabrew_message(&other, second.clone()).unwrap();
    let prompt = session.manabrew_snapshot(&other, &lookup).unwrap().prompt.unwrap();
    assert_eq!(serde_json::to_value(&prompt.input).unwrap()["type"], "chooseBoolean");
    let play: ClientToServerMessage = serde_json::from_value(json!({
        "kind": "response", "promptId": prompt.prompt_id,
        "action": {"type": "chooseBoolean", "output": {"type": "decision", "value": false}}
    })).unwrap();
    assert!(session.handle_manabrew_message(&token, play.clone()).is_err());
    session.handle_manabrew_message(&other, play.clone()).unwrap();
    assert_eq!(session.state.match_phase, MatchPhase::InGame);
    assert_eq!(session.state.game_number, 2);
    assert_eq!(session.state.match_score.p0_wins, 1);
    assert_eq!(session.state.current_starting_player, PlayerId(0));
    assert_eq!(session.state_revision, 3);
    assert!(session.handle_manabrew_message(&other, play).is_err());
    assert!(session.handle_manabrew_message(&other, second).is_err());
    assert!(matches!(session.state.waiting_for, WaitingFor::MulliganDecision { .. }));
}

#[test]
fn bo3_rejects_forged_pool_and_size_without_advancing_revision() {
    let (manager, code, token, _) = fixture();
    let mut session = manager.try_session(&code).unwrap();
    let original = serde_json::to_value(unchanged_submission(&session, &token)).unwrap();
    for replacement in [
        json!({"main": [{"name":"Forest", "count":59}], "sideboard":[{"name":"Island", "count":1}, {"name":"Forest", "count":1}]}),
        json!({"main": [{"name":"Forest", "count":60}], "sideboard":[{"name":"Swamp", "count":1}]}),
        json!({"main": [{"name":"Forest", "count":61}], "sideboard":[{"name":"Island", "count":1}]}),
    ] {
        let mut wire = original.clone();
        wire["action"]["output"]["main"] = replacement["main"].clone();
        wire["action"]["output"]["sideboard"] = replacement["sideboard"].clone();
        let before = serde_json::to_value(&session.state).unwrap();
        assert!(session.handle_manabrew_message(&token, serde_json::from_value(wire).unwrap()).is_err());
        assert_eq!(session.state_revision, 0);
        assert_eq!(serde_json::to_value(&session.state).unwrap(), before);
    }
    let valid = unchanged_submission(&session, &token);
    session.handle_manabrew_message(&token, valid).unwrap();
}
