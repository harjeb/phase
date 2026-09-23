//! Real-card hidden-agenda effects through production Oracle synthesis.
use engine::database::mtgjson::AtomicCard;
use engine::database::synthesis::build_oracle_face;
use engine::game::casting::display_spell_cost;
use engine::game::conspiracy::{start_with_conspiracy, turn_hidden_agenda_face_up};
use engine::game::layers::evaluate_layers;
use engine::game::scenario::{GameScenario, P0, P1};
use engine::game::zones::create_object;
use engine::types::ability::ChosenAttribute;
use engine::types::game_state::GameState;
use engine::types::identifiers::{CardId, ObjectId};
use engine::types::keywords::Keyword;
use engine::types::mana::{ManaCost, ManaCostShard};
use engine::types::phase::Phase;
use engine::types::zones::Zone;

// data/mtgjson/AtomicCards.json, Summoner's Bond (Oracle id
// 07b6326a-b73d-422b-bcb5-c508aa3fc558). Keep the complete Oracle entry text.
const BOND_ORACLE: &str = "Double agenda (Start the game with this conspiracy face down in the command zone and secretly choose two different card names. You may turn this conspiracy face up any time and reveal those names.)\nWhenever you cast a creature spell with one of the chosen names, you may search your library for a creature card with the other chosen name, reveal it, put it into your hand, then shuffle.";

fn bond(state: &mut GameState, revealed: bool) -> ObjectId {
    let atomic: AtomicCard = serde_json::from_value(serde_json::json!({
        "name": "Summoner's Bond", "colors": [], "colorIdentity": [], "layout": "normal",
        "type": "Conspiracy", "types": ["Conspiracy"], "manaValue": 0,
        "identifiers": {}, "keywords": ["Double agenda", "Hidden agenda"],
        "text": BOND_ORACLE
    }))
    .unwrap();
    let face = build_oracle_face(&atomic, None);
    assert_eq!(face.triggers.len(), 1, "{face:#?}");
    let id =
        engine::game::deck_loading::create_conspiracy_from_card_face(state, &face, P0).unwrap();
    let obj = state.objects.get_mut(&id).unwrap();
    obj.chosen_attributes = vec![
        ChosenAttribute::CardName("Grizzly Bears".into()),
        ChosenAttribute::CardName("Runeclaw Bear".into()),
    ];
    if revealed {
        assert!(turn_hidden_agenda_face_up(state, id, P0));
    }
    id
}

fn bond_scenario(
    cast_name: &str,
    revealed: bool,
) -> (
    engine::game::scenario::GameRunner,
    ObjectId,
    ObjectId,
    ObjectId,
    ObjectId,
) {
    let mut scenario = GameScenario::new_n_player(2, 42);
    scenario.at_phase(Phase::PreCombatMain);
    let spell = scenario
        .add_creature_to_hand(P0, cast_name, 2, 2)
        .with_mana_cost(ManaCost::Cost {
            generic: 0,
            shards: vec![],
        })
        .id();
    let first = scenario.add_card_to_library_top(P0, "Grizzly Bears");
    let second = scenario.add_card_to_library_top(P0, "Runeclaw Bear");
    let unrelated = scenario.add_card_to_library_top(P0, "Forest Bear");
    // A named noncreature card must never be offered by this creature tutor.
    scenario.add_card_to_library_top(P0, "Grizzly Bears");
    scenario.add_card_to_library_top(P0, "Runeclaw Bear");
    let mut runner = scenario.build();
    for id in [first, second, unrelated] {
        let obj = runner.state_mut().objects.get_mut(&id).unwrap();
        obj.card_types.core_types = vec![engine::types::card_type::CoreType::Creature];
        obj.base_card_types = obj.card_types.clone();
    }
    bond(runner.state_mut(), revealed);
    (runner, spell, first, second, unrelated)
}

#[test]
fn summoners_bond_searches_only_the_other_chosen_name_in_both_directions() {
    use engine::types::game_state::WaitingFor;
    for (cast_name, find_second) in [("Grizzly Bears", true), ("Runeclaw Bear", false)] {
        let (mut runner, spell, first, second, _) = bond_scenario(cast_name, true);
        let outcome = runner.cast(spell).accept_optional().resolve();
        let expected = if find_second { second } else { first };
        match outcome.final_waiting_for() {
            WaitingFor::SearchChoice {
                player,
                cards,
                count,
                ..
            } => {
                assert_eq!(*player, P0);
                assert_eq!(*count, 1);
                assert_eq!(cards.as_slice(), &[expected], "{cast_name}");
            }
            other => panic!("{cast_name} must search for the other name: {other:?}"),
        }
    }
}

#[test]
fn summoners_bond_does_not_trigger_face_down_or_for_unrelated_spells() {
    for (cast_name, revealed) in [
        ("Grizzly Bears", false),
        ("Runeclaw Bear", false),
        ("Forest Bear", true),
    ] {
        let (mut runner, spell, first, second, unrelated) = bond_scenario(cast_name, revealed);
        let outcome = runner.cast(spell).accept_optional().resolve();
        assert!(matches!(
            outcome.final_waiting_for(),
            engine::types::game_state::WaitingFor::Priority { .. }
        ));
        for id in [first, second, unrelated] {
            assert_eq!(outcome.zone_of(id), Zone::Library);
        }
    }
}

#[test]
fn summoners_bond_search_is_optional_and_puts_the_revealed_creature_in_hand() {
    for accept in [false, true] {
        let (mut runner, spell, first, second, unrelated) = bond_scenario("Grizzly Bears", true);
        let cast = runner.cast(spell).search_first_legal();
        let outcome = if accept {
            cast.accept_optional()
        } else {
            cast.decline_optional()
        }
        .resolve();
        assert_eq!(
            outcome.zone_of(second),
            if accept { Zone::Hand } else { Zone::Library }
        );
        assert_eq!(outcome.zone_of(first), Zone::Library);
        assert_eq!(outcome.zone_of(unrelated), Zone::Library);
        use engine::types::events::{GameEvent, PlayerActionKind};
        assert_eq!(outcome.events().iter().any(|event| matches!(event,
            GameEvent::CardsRevealed { player, card_ids, .. } if *player == P0 && card_ids.contains(&second)
        )), accept, "search must reveal exactly the found card");
        assert_eq!(outcome.events().iter().any(|event| matches!(event,
            GameEvent::PlayerPerformedAction { player_id, action: PlayerActionKind::ShuffledLibrary, .. } if *player_id == P0
        )), accept, "shuffle is part of the optional search");
    }
}

#[test]
fn summoners_bond_does_not_trigger_for_an_opponents_spell_or_a_named_noncreature() {
    use engine::types::card_type::CoreType;
    use engine::types::game_state::WaitingFor;
    for opponent in [false, true] {
        let (mut runner, spell, _, _, _) = bond_scenario("Grizzly Bears", true);
        let state = runner.state_mut();
        if opponent {
            state.active_player = P1;
            state.priority_player = P1;
            state.waiting_for = WaitingFor::Priority { player: P1 };
            state.players[0].hand.retain(|id| *id != spell);
            state.players[1].hand.push_back(spell);
            let obj = state.objects.get_mut(&spell).unwrap();
            obj.owner = P1;
            obj.controller = P1;
        } else {
            let obj = state.objects.get_mut(&spell).unwrap();
            obj.card_types.core_types = vec![CoreType::Artifact];
            obj.base_card_types = obj.card_types.clone();
        }
        let outcome = runner.cast(spell).accept_optional().resolve();
        assert!(matches!(
            outcome.final_waiting_for(),
            WaitingFor::Priority { .. }
        ));
    }
}

#[test]
fn summoners_bond_remembers_the_triggering_name_if_the_spell_leaves_the_stack() {
    use engine::types::game_state::WaitingFor;
    let (mut runner, spell, _, second, _) = bond_scenario("Grizzly Bears", true);
    let mut cast = runner.cast(spell).accept_optional().commit();
    engine::game::zones::move_to_zone(cast.state_mut(), spell, Zone::Graveyard, &mut vec![]);
    let outcome = cast.resolve();
    match outcome.final_waiting_for() {
        WaitingFor::SearchChoice { cards, .. } => assert_eq!(cards.as_slice(), &[second]),
        other => panic!("the independently resolving trigger must retain its name link: {other:?}"),
    }
}

#[test]
fn summoners_bond_revealed_after_casting_does_not_trigger_retroactively() {
    let (mut runner, spell, _, _, _) = bond_scenario("Grizzly Bears", false);
    let mut cast = runner.cast(spell).accept_optional().commit();
    let state = cast.state_mut();
    let id = state.command_zone[0];
    assert!(turn_hidden_agenda_face_up(state, id, P0));
    let outcome = cast.resolve();
    assert!(matches!(
        outcome.final_waiting_for(),
        engine::types::game_state::WaitingFor::Priority { .. }
    ));
}

#[test]
fn summoners_bond_may_fail_to_find_but_still_shuffles() {
    use engine::types::events::{GameEvent, PlayerActionKind};
    let (mut runner, spell, first, second, unrelated) = bond_scenario("Grizzly Bears", true);
    let outcome = runner.cast(spell).accept_optional().search_none().resolve();
    for id in [first, second, unrelated] {
        assert_eq!(outcome.zone_of(id), Zone::Library);
    }
    assert!(!outcome
        .events()
        .iter()
        .any(|event| matches!(event, GameEvent::CardsRevealed { .. })));
    assert!(outcome.events().iter().any(|event| matches!(event,
        GameEvent::PlayerPerformedAction { player_id, action: PlayerActionKind::ShuffledLibrary, .. } if *player_id == P0
    )));
}

fn agenda(state: &mut GameState, name: &str, effect: &str, chosen: &str) -> ObjectId {
    let atomic: AtomicCard = serde_json::from_value(serde_json::json!({
        "name": name, "colors": [], "colorIdentity": [], "layout": "normal",
        "type": "Conspiracy", "types": ["Conspiracy"], "manaValue": 0,
        "identifiers": {}, "keywords": ["Hidden agenda"],
        "text": format!("Hidden agenda (Start the game with this conspiracy face down in the command zone and secretly choose a card name. You may turn this conspiracy face up any time and reveal that name.)\n{effect}")
    })).unwrap();
    let face = build_oracle_face(&atomic, None);
    assert!(
        !face.static_abilities.is_empty(),
        "{name} must parse its effect"
    );
    let id = create_object(state, CardId(1000), P0, name.into(), Zone::Command);
    let obj = state.objects.get_mut(&id).unwrap();
    obj.card_types = face.card_type;
    obj.static_definitions = face.static_abilities.into();
    obj.trigger_definitions = face.triggers.into();
    obj.chosen_attributes
        .push(ChosenAttribute::CardName(chosen.into()));
    start_with_conspiracy(state, id, true);
    id
}

#[test]
fn immediate_action_grants_haste_only_to_its_owners_named_creatures_after_reveal() {
    let mut scenario = GameScenario::new_n_player(2, 42);
    let named = scenario.add_creature(P0, "Grizzly Bears", 2, 2).id();
    let other = scenario.add_creature(P0, "Runeclaw Bear", 2, 2).id();
    let opposing = scenario.add_creature(P1, "Grizzly Bears", 2, 2).id();
    let mut runner = scenario.build();
    let state = runner.state_mut();
    let id = agenda(
        state,
        "Immediate Action",
        "Creatures you control with the chosen name have haste.",
        "Grizzly Bears",
    );
    evaluate_layers(state);
    for creature in [named, other, opposing] {
        assert!(!state.objects[&creature].keywords.contains(&Keyword::Haste));
    }
    assert!(turn_hidden_agenda_face_up(state, id, P0));
    evaluate_layers(state);
    assert!(state.objects[&named].keywords.contains(&Keyword::Haste));
    for creature in [other, opposing] {
        assert!(!state.objects[&creature].keywords.contains(&Keyword::Haste));
    }
}

#[test]
fn bragos_favor_reduces_only_its_owners_named_spells_after_reveal() {
    let mut scenario = GameScenario::new_n_player(2, 42);
    scenario.at_phase(Phase::PreCombatMain);
    let cost = ManaCost::Cost {
        generic: 1,
        shards: vec![ManaCostShard::Green],
    };
    let named = scenario
        .add_creature_to_hand(P0, "Grizzly Bears", 2, 2)
        .with_mana_cost(cost.clone())
        .id();
    let other = scenario
        .add_creature_to_hand(P0, "Runeclaw Bear", 2, 2)
        .with_mana_cost(cost.clone())
        .id();
    let opposing = scenario
        .add_creature_to_hand(P1, "Grizzly Bears", 2, 2)
        .with_mana_cost(cost.clone())
        .id();
    let mut runner = scenario.build();
    let state = runner.state_mut();
    let id = agenda(
        state,
        "Brago's Favor",
        "Spells with the chosen name you cast cost {1} less to cast.",
        "Grizzly Bears",
    );
    evaluate_layers(state);
    for (player, spell) in [(P0, named), (P0, other), (P1, opposing)] {
        assert_eq!(display_spell_cost(state, player, spell), Some(cost.clone()));
    }
    assert!(turn_hidden_agenda_face_up(state, id, P0));
    evaluate_layers(state);
    assert_eq!(
        display_spell_cost(state, P0, named),
        Some(ManaCost::Cost {
            generic: 0,
            shards: vec![ManaCostShard::Green]
        })
    );
    for (player, spell) in [(P0, other), (P1, opposing)] {
        assert_eq!(display_spell_cost(state, player, spell), Some(cost.clone()));
    }
}
