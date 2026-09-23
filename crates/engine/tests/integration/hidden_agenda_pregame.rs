//! CR 702.106: secret, per-copy pregame commitments, not ETB choices.
use engine::database::CardDatabase;
use engine::game::conspiracy::{functions_from_command_zone, turn_hidden_agenda_face_up};
use engine::game::deck_loading::{
    load_deck_with_conspiracy_choices, ConspiracyChoice, DeckEntry, DeckPayload, PlayerDeckPayload,
};
use engine::game::filter::{matches_target_filter, FilterContext};
use engine::game::visibility::filter_state_for_viewer;
use engine::types::ability::{ChosenAttribute, TargetFilter};
use engine::types::card::CardFace;
use engine::types::game_state::GameState;
use engine::types::player::PlayerId;

const HIDDEN: &str = "Hidden agenda (Start the game with this conspiracy face down in the command zone and secretly choose a card name. You may turn this conspiracy face up any time and reveal that name.)\nSpells with the chosen name you cast cost {1} less to cast.";
const DOUBLE: &str = "Double agenda (Start the game with this conspiracy face down in the command zone and secretly choose two different card names. You may turn this conspiracy face up any time and reveal those names.)\nWhenever you cast a creature spell with one of the chosen names, you may search your library for a creature card with the other chosen name, reveal it, put it into your hand, then shuffle.";

fn face(name: &str, kind: &str, oracle: Option<&str>) -> CardFace {
    serde_json::from_value(serde_json::json!({
        "name": name, "mana_cost": {"type":"NoCost"},
        "card_type": {"supertypes":[], "core_types":[kind], "subtypes":[]},
        "power":null,"toughness":null,"loyalty":null,"defense":null,
        "oracle_text":oracle,"non_ability_text":null,"flavor_name":null,
        "keywords":[],"abilities":[],"triggers":[],"static_abilities":[],"replacements":[],
        "color_override":null,"scryfall_oracle_id":null
    }))
    .unwrap()
}

fn fixture(double: bool) -> (CardDatabase, DeckPayload) {
    let forest = face("Forest", "Land", None);
    let island = face("Island", "Land", None);
    let db = CardDatabase::from_json_str(
        &serde_json::json!({
            "forest": forest, "island": island
        })
        .to_string(),
    )
    .unwrap();
    let payload = DeckPayload {
        player: PlayerDeckPayload {
            main_deck: vec![DeckEntry {
                card: forest,
                count: 8,
            }],
            conspiracy: vec![DeckEntry {
                card: if double {
                    face("Summoner's Bond", "Conspiracy", Some(DOUBLE))
                } else {
                    face("Brago's Favor", "Conspiracy", Some(HIDDEN))
                },
                count: 2,
            }],
            ..Default::default()
        },
        ..Default::default()
    };
    (db, payload)
}

fn commitment(index: usize, names: &[&str]) -> ConspiracyChoice {
    ConspiracyChoice {
        player: PlayerId(0),
        index,
        choices: names
            .iter()
            .map(|name| ChosenAttribute::CardName((*name).into()))
            .collect(),
    }
}

#[test]
fn names_are_per_copy_private_until_reveal_and_feed_existing_name_filter() {
    let (db, payload) = fixture(false);
    let mut state = GameState::new_two_player(42);
    load_deck_with_conspiracy_choices(
        &mut state,
        &payload,
        &[commitment(0, &["forest"]), commitment(1, &["Island"])],
        &db,
    )
    .unwrap();
    let ids: Vec<_> = state.command_zone.iter().copied().collect();
    assert_eq!(state.objects[&ids[0]].chosen_card_name(), Some("Forest"));
    assert_eq!(state.objects[&ids[1]].chosen_card_name(), Some("Island"));
    assert!(!functions_from_command_zone(&state.objects[&ids[0]]));
    let owner = filter_state_for_viewer(&state, PlayerId(0));
    assert_eq!(owner.objects[&ids[0]].chosen_card_name(), Some("Forest"));
    let observer = filter_state_for_viewer(&state, PlayerId(1));
    assert!(observer.objects[&ids[0]].chosen_attributes.is_empty());
    assert_ne!(observer.objects[&ids[0]].name, "Brago's Favor");
    assert!(observer.deck_pools[0].registered_conspiracy.is_empty());
    assert!(!turn_hidden_agenda_face_up(&mut state, ids[0], PlayerId(1)));
    assert!(turn_hidden_agenda_face_up(&mut state, ids[0], PlayerId(0)));
    assert!(functions_from_command_zone(&state.objects[&ids[0]]));
    let observer = filter_state_for_viewer(&state, PlayerId(1));
    assert_eq!(observer.objects[&ids[0]].chosen_card_name(), Some("Forest"));
    assert!(observer.objects[&ids[1]].chosen_attributes.is_empty());
    // CR 702.106d: the existing effect filter reads the committed source name.
    let forest = state
        .objects
        .iter()
        .find(|(_, obj)| obj.name == "Forest")
        .unwrap()
        .0;
    assert!(matches_target_filter(
        &state,
        *forest,
        &TargetFilter::HasChosenName,
        &FilterContext::from_source(&state, ids[0])
    ));
    assert!(!matches_target_filter(
        &state,
        *forest,
        &TargetFilter::HasChosenName,
        &FilterContext::from_source(&state, ids[1])
    ));
}

#[test]
fn invalid_commitments_are_atomic_and_a_new_game_requires_fresh_choices() {
    let (db, payload) = fixture(false);
    for choices in [
        vec![],
        vec![commitment(0, &["Forest"])],
        vec![commitment(0, &["Forest"]), commitment(0, &["Island"])],
        vec![
            commitment(0, &["Not an Oracle card"]),
            commitment(1, &["Forest"]),
        ],
        vec![
            commitment(0, &["Forest", "Island"]),
            commitment(1, &["Forest"]),
        ],
        vec![commitment(0, &["Forest"]), commitment(2, &["Island"])],
    ] {
        let mut state = GameState::new_two_player(42);
        let before = serde_json::to_value(&state).unwrap();
        assert!(load_deck_with_conspiracy_choices(&mut state, &payload, &choices, &db).is_err());
        assert_eq!(serde_json::to_value(&state).unwrap(), before);
    }
    let mut next_game = GameState::new_two_player(43);
    load_deck_with_conspiracy_choices(
        &mut next_game,
        &payload,
        &[commitment(0, &["Island"]), commitment(1, &["Forest"])],
        &db,
    )
    .unwrap();
    let id = next_game.command_zone[0];
    assert_eq!(next_game.objects[&id].chosen_card_name(), Some("Island"));
    assert!(next_game.objects[&id].face_down);
}

#[test]
fn double_agenda_keeps_two_distinct_names_secret_until_reveal() {
    let (db, payload) = fixture(true);
    let mut state = GameState::new_two_player(42);
    assert!(load_deck_with_conspiracy_choices(
        &mut state,
        &payload,
        &[
            commitment(0, &["Forest", "forest"]),
            commitment(1, &["Forest", "Island"])
        ],
        &db
    )
    .is_err());
    load_deck_with_conspiracy_choices(
        &mut state,
        &payload,
        &[
            commitment(0, &["Forest", "Island"]),
            commitment(1, &["Island", "Forest"]),
        ],
        &db,
    )
    .unwrap();
    let id = state.command_zone[0];
    assert_eq!(state.objects[&id].chosen_attributes.len(), 2);
    assert!(filter_state_for_viewer(&state, PlayerId(1)).objects[&id]
        .chosen_attributes
        .is_empty());
    assert!(!turn_hidden_agenda_face_up(&mut state, id, PlayerId(1)));
    assert!(turn_hidden_agenda_face_up(&mut state, id, PlayerId(0)));
    assert!(!state.objects[&id].face_down);
    let observer = filter_state_for_viewer(&state, PlayerId(1));
    assert_eq!(
        observer.objects[&id].chosen_attributes,
        state.objects[&id].chosen_attributes
    );
    assert_eq!(observer.objects[&id].chosen_attributes.len(), 2);
    let other = state.command_zone[1];
    assert!(observer.objects[&other].chosen_attributes.is_empty());
    assert!(state.objects[&other].face_down);
}

#[test]
fn between_games_cannot_silently_start_without_fresh_agenda_choices() {
    use engine::types::match_config::MatchPhase;
    for double in [false, true] {
        let (db, payload) = fixture(double);
        let mut state = GameState::new_two_player(42);
        let names = if double {
            vec!["Forest", "Island"]
        } else {
            vec!["Forest"]
        };
        load_deck_with_conspiracy_choices(
            &mut state,
            &payload,
            &[commitment(0, &names), commitment(1, &names)],
            &db,
        )
        .unwrap();
        state.match_phase = MatchPhase::BetweenGames;
        state.next_game_chooser = Some(PlayerId(0));
        let before = serde_json::to_value(&state).unwrap();
        let mut events = vec![];
        let error = engine::game::match_flow::handle_choose_play_draw(
            &mut state,
            PlayerId(0),
            true,
            &mut events,
        )
        .unwrap_err();
        assert!(error.contains("fresh secret choices"), "{error}");
        assert!(events.is_empty());
        assert_eq!(serde_json::to_value(&state).unwrap(), before);
    }
}

#[test]
fn rematch_rejects_missing_choices_and_accepts_new_secret_names() {
    for double in [false, true] {
        let (db, payload) = fixture(double);
        let mut state = GameState::new_two_player(42);
        let original = if double {
            vec!["Forest", "Island"]
        } else {
            vec!["Forest"]
        };
        load_deck_with_conspiracy_choices(
            &mut state,
            &payload,
            &[commitment(0, &original), commitment(1, &original)],
            &db,
        )
        .unwrap();
        let old = state.command_zone[0];
        assert!(turn_hidden_agenda_face_up(&mut state, old, PlayerId(0)));
        // Reusing a game container cannot silently reuse its old decisions.
        let before = serde_json::to_value(&state).unwrap();
        assert!(load_deck_with_conspiracy_choices(&mut state, &payload, &[], &db).is_err());
        assert_eq!(serde_json::to_value(&state).unwrap(), before);
        let fresh = if double {
            vec!["Island", "Forest"]
        } else {
            vec!["Island"]
        };
        assert!(load_deck_with_conspiracy_choices(
            &mut state,
            &payload,
            &[commitment(0, &fresh), commitment(1, &fresh)],
            &db,
        )
        .unwrap_err()
        .contains("fresh game state"));
        assert_eq!(serde_json::to_value(&state).unwrap(), before);
        state = GameState::new_two_player(43);
        assert!(load_deck_with_conspiracy_choices(&mut state, &payload, &[], &db).is_err());
        assert!(state.objects.is_empty());
        load_deck_with_conspiracy_choices(
            &mut state,
            &payload,
            &[commitment(0, &fresh), commitment(1, &fresh)],
            &db,
        )
        .unwrap();
        let id = state.command_zone[0];
        assert!(state.objects[&id].face_down);
        assert_eq!(
            state.objects[&id].chosen_attributes,
            fresh
                .iter()
                .map(|name| ChosenAttribute::CardName((*name).into()))
                .collect::<Vec<_>>()
        );
        assert!(filter_state_for_viewer(&state, PlayerId(1)).objects[&id]
            .chosen_attributes
            .is_empty());
    }
}

#[test]
fn reveal_rejects_empty_duplicate_or_excess_names() {
    let (db, payload) = fixture(true);
    let mut state = GameState::new_two_player(42);
    load_deck_with_conspiracy_choices(
        &mut state,
        &payload,
        &[
            commitment(0, &["Forest", "Island"]),
            commitment(1, &["Island", "Forest"]),
        ],
        &db,
    )
    .unwrap();
    let id = state.command_zone[0];
    for names in [
        vec![],
        vec![""],
        vec!["Forest", ""],
        vec!["Forest", "forest"],
        vec!["Forest", "Island", "Plains"],
    ] {
        state.objects.get_mut(&id).unwrap().chosen_attributes = names
            .into_iter()
            .map(|name| ChosenAttribute::CardName(name.into()))
            .collect();
        assert!(!turn_hidden_agenda_face_up(&mut state, id, PlayerId(0)));
        assert!(state.objects[&id].face_down);
    }
}
