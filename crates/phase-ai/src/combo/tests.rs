use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use engine::game::printed_cards::apply_card_face_to_object;
use engine::game::zones::create_object;
use engine::types::ability::{AbilityDefinition, AbilityKind, Effect};
use engine::types::actions::GameAction;
use engine::types::card::CardFace;
use engine::types::counter::CounterType;
use engine::types::game_state::{GameState, WaitingFor};
use engine::types::identifiers::{CardId, ObjectId};
use engine::types::mana::{ManaType, ManaUnit};
use engine::types::phase::Phase;
use engine::types::player::PlayerId;
use engine::types::zones::Zone;

use super::{plan_combos, ComboPlanOutcome};
use super::{ComboReachability, ComboRegistry};

pub(crate) fn card(name: &str) -> CardFace {
    static CARDS: OnceLock<HashMap<String, CardFace>> = OnceLock::new();
    CARDS
        .get_or_init(|| serde_json::from_str(include_str!("fixtures/cards.json")).unwrap())
        .get(name)
        .unwrap()
        .clone()
}

pub(crate) fn position() -> GameState {
    let mut state = GameState::new_two_player(29);
    state.turn_number = 3;
    state.phase = Phase::PreCombatMain;
    state.active_player = PlayerId(0);
    state.priority_player = PlayerId(0);
    state.waiting_for = WaitingFor::Priority {
        player: PlayerId(0),
    };
    state
}

pub(crate) fn place(state: &mut GameState, face: &CardFace, zone: Zone) -> ObjectId {
    let identifier = CardId(state.next_object_id);
    let object = create_object(state, identifier, PlayerId(0), face.name.clone(), zone);
    let permanent = state.objects.get_mut(&object).unwrap();
    apply_card_face_to_object(permanent, face);
    permanent.entered_battlefield_turn = Some(0);
    permanent.summoning_sick = false;
    object
}

pub(crate) fn heliod_position() -> (GameState, ObjectId, ObjectId) {
    let mut state = position();
    let heliod = place(&mut state, &card("heliod, sun-crowned"), Zone::Battlefield);
    let ballista = place(&mut state, &card("walking ballista"), Zone::Battlefield);
    state
        .objects
        .get_mut(&ballista)
        .unwrap()
        .counters
        .insert(CounterType::Plus1Plus1, 2);
    for _ in 0..2 {
        state.players[0]
            .mana_pool
            .add(ManaUnit::new(ManaType::White, heliod, false, Vec::new()));
    }
    (state, heliod, ballista)
}

#[test]
fn component_recognition_does_not_depend_on_card_names() {
    let (mut state, _, _) = heliod_position();
    for (_, object) in state.objects.iter_mut() {
        object.name = format!("Renamed {}", object.id.0);
    }
    assert!(ComboRegistry::default()
        .reachable_lines(&state, PlayerId(0))
        .iter()
        .any(|(_, reach)| matches!(reach, ComboReachability::ReachableThisTurn { .. })));
}

#[test]
fn component_recognition_resolves_reordered_ability_indices() {
    let (mut state, _, ballista) = heliod_position();
    let placeholder = AbilityDefinition::new(AbilityKind::Activated, Effect::NoOp);
    Arc::make_mut(&mut state.objects.get_mut(&ballista).unwrap().abilities).insert(0, placeholder);
    let reachable = ComboRegistry::default().reachable_lines(&state, PlayerId(0));
    let actions = reachable
        .iter()
        .find_map(|(_, reach)| match reach {
            ComboReachability::ReachableThisTurn {
                required_actions, ..
            } => Some(required_actions),
            _ => None,
        })
        .expect("the assembled combo must be recognized");
    assert!(actions.contains(&GameAction::ActivateAbility {
        source_id: ballista,
        ability_index: 2
    }));
}

#[test]
fn printed_names_without_matching_effects_do_not_form_a_combo() {
    let (mut state, _, ballista) = heliod_position();
    state.objects.get_mut(&ballista).unwrap().abilities = Arc::new(Vec::new());
    assert!(ComboRegistry::default()
        .reachable_lines(&state, PlayerId(0))
        .is_empty());
}

fn replay(state: &GameState, plan: &super::ComboPlan) -> GameState {
    let mut result = state.clone();
    for step in &plan.actions {
        assert_eq!(result.waiting_for.acting_player(), Some(step.actor));
        engine::game::engine::apply_as_current_for_simulation(&mut result, step.action.clone())
            .expect("every planned action must apply through the engine");
    }
    result
}

#[test]
fn complete_damage_cycle_restores_its_counter() {
    let (state, heliod, ballista) = heliod_position();
    let result = plan_combos(&state, PlayerId(0), 256, engine::util::Deadline::none());
    let plan = result
        .plan
        .expect("a complete damage cycle must be planned");
    assert_eq!(plan.outcome, ComboPlanOutcome::CompletedCycle);
    assert_eq!(
        plan.actions[0].action,
        GameAction::ActivateAbility {
            source_id: heliod,
            ability_index: 0
        }
    );
    let after = replay(&state, &plan);
    assert_eq!(
        after.objects[&ballista].counters[&CounterType::Plus1Plus1],
        2
    );
    assert!(after.players[1].life < state.players[1].life);
    assert!(result.nodes_used <= 256);
}

#[test]
fn one_counter_is_not_misreported_as_a_damage_cycle() {
    let (mut state, _, ballista) = heliod_position();
    state
        .objects
        .get_mut(&ballista)
        .unwrap()
        .counters
        .insert(CounterType::Plus1Plus1, 1);
    assert!(
        plan_combos(&state, PlayerId(0), 256, engine::util::Deadline::none())
            .plan
            .is_none()
    );
}

#[test]
fn mana_shortfall_does_not_produce_a_plan() {
    let (mut state, _, _) = heliod_position();
    state.players[0].mana_pool = Default::default();
    assert!(
        plan_combos(&state, PlayerId(0), 128, engine::util::Deadline::none())
            .plan
            .is_none()
    );
}

#[test]
fn zero_or_expired_budget_does_not_expand_combo_actions() {
    let (state, _, _) = heliod_position();
    let empty = plan_combos(&state, PlayerId(0), 0, engine::util::Deadline::none());
    assert_eq!(empty.nodes_used, 0);
    assert!(empty.plan.is_none());
    let expired = plan_combos(&state, PlayerId(0), 128, engine::util::Deadline::after(0));
    assert_eq!(expired.nodes_used, 0);
    assert!(expired.plan.is_none());
}

pub(crate) fn oracle_position() -> GameState {
    let mut state = position();
    state.all_card_names = Arc::from(vec![
        "Island".to_string(),
        "Library Filler".to_string(),
        "Thassa's Oracle".to_string(),
        "Demonic Consultation".to_string(),
    ]);
    let oracle = place(&mut state, &card("thassa's oracle"), Zone::Hand);
    place(&mut state, &card("demonic consultation"), Zone::Hand);
    for _ in 0..12 {
        place(
            &mut state,
            &CardFace {
                name: "Library Filler".to_string(),
                ..Default::default()
            },
            Zone::Library,
        );
    }
    for color in [ManaType::Blue, ManaType::Blue, ManaType::Black] {
        state.players[0]
            .mana_pool
            .add(ManaUnit::new(color, oracle, false, Vec::new()));
    }
    state
}

#[test]
fn library_combo_plan_includes_casts_choices_and_terminal_resolution() {
    let state = oracle_position();
    let result = plan_combos(&state, PlayerId(0), 512, engine::util::Deadline::none());
    assert!(
        result.plan.is_some(),
        "library planning result: {result:?}; oracle role: {}; exile role: {}",
        super::components::matches_face(
            &super::CardPredicate::Role(super::ComponentRole::LibraryWin),
            &card("thassa's oracle")
        ),
        super::components::matches_face(
            &super::CardPredicate::Role(super::ComponentRole::LibraryExile),
            &card("demonic consultation")
        )
    );
    let plan = result
        .plan
        .expect("the library combo must be planned through terminal resolution");
    assert_eq!(plan.outcome, ComboPlanOutcome::WinUnderPassResponses);
    assert!(plan
        .actions
        .iter()
        .any(|step| matches!(step.action, GameAction::ChooseOption { .. })));
    let after = replay(&state, &plan);
    assert_eq!(after.game_end.unwrap().winner, Some(PlayerId(0)));
    assert!(after.players[0].library.is_empty());
}

#[test]
fn copy_blink_plan_executes_a_complete_token_cycle() {
    let mut state = position();
    place(
        &mut state,
        &card("kiki-jiki, mirror breaker"),
        Zone::Battlefield,
    );
    place(&mut state, &card("felidar guardian"), Zone::Battlefield);
    let result = plan_combos(&state, PlayerId(0), 256, engine::util::Deadline::none());
    let plan = result
        .plan
        .expect("copy and blink must form a complete cycle");
    assert_eq!(plan.outcome, ComboPlanOutcome::CompletedCycle);
    let after = replay(&state, &plan);
    assert!(after
        .objects
        .values()
        .any(|object| object.is_token && object.zone == Zone::Battlefield));
}

#[test]
fn tapped_copy_source_does_not_produce_a_cycle() {
    let mut state = position();
    let source = place(
        &mut state,
        &card("kiki-jiki, mirror breaker"),
        Zone::Battlefield,
    );
    place(&mut state, &card("felidar guardian"), Zone::Battlefield);
    state.objects.get_mut(&source).unwrap().tapped = true;
    assert!(
        plan_combos(&state, PlayerId(0), 64, engine::util::Deadline::none())
            .plan
            .is_none()
    );
}

#[test]
fn changed_state_invalidates_a_prepared_plan() {
    let (mut state, _, ballista) = heliod_position();
    let plan = plan_combos(&state, PlayerId(0), 256, engine::util::Deadline::none())
        .plan
        .unwrap();
    assert!(plan.next_action(&state, PlayerId(0)).is_some());
    assert!(plan.next_action(&state, PlayerId(1)).is_none());
    state.objects.get_mut(&ballista).unwrap().abilities = Arc::new(Vec::new());
    assert!(plan.next_action(&state, PlayerId(0)).is_none());
    assert!(
        plan_combos(&state, PlayerId(0), 256, engine::util::Deadline::none())
            .plan
            .is_none()
    );
}

#[test]
fn protected_component_is_rejected_by_engine_targeting() {
    let (mut state, _, ballista) = heliod_position();
    let object = state.objects.get_mut(&ballista).unwrap();
    object
        .keywords
        .push(engine::types::keywords::Keyword::Shroud);
    object
        .base_keywords
        .push(engine::types::keywords::Keyword::Shroud);
    assert!(
        plan_combos(&state, PlayerId(0), 256, engine::util::Deadline::none())
            .plan
            .is_none()
    );
}

#[test]
fn tight_node_budget_is_respected() {
    let (state, _, _) = heliod_position();
    for limit in [1, 2, 8, 24, 48] {
        let result = plan_combos(&state, PlayerId(0), limit, engine::util::Deadline::none());
        assert!(result.nodes_used <= limit);
        if let Some(plan) = result.plan {
            replay(&state, &plan);
        }
    }
}

#[test]
fn all_supported_families_fit_the_production_combo_quota() {
    let (damage, _, _) = heliod_position();
    let mut copy = position();
    place(
        &mut copy,
        &card("kiki-jiki, mirror breaker"),
        Zone::Battlefield,
    );
    place(&mut copy, &card("felidar guardian"), Zone::Battlefield);
    for state in [damage, oracle_position(), copy] {
        let result = plan_combos(&state, PlayerId(0), 48, engine::util::Deadline::none());
        assert!(result.nodes_used <= 48);
        assert!(
            result.plan.is_some(),
            "a supported family must fit the production quota: {result:?}"
        );
    }
}

#[test]
fn cycle_witness_is_bound_to_the_actual_damage_source() {
    let (mut state, _, first) = heliod_position();
    let second = place(&mut state, &card("walking ballista"), Zone::Battlefield);
    let object = state.objects.get_mut(&second).unwrap();
    object.counters.insert(CounterType::Plus1Plus1, 2);
    object
        .keywords
        .push(engine::types::keywords::Keyword::Lifelink);
    object
        .base_keywords
        .push(engine::types::keywords::Keyword::Lifelink);
    let plan = plan_combos(&state, PlayerId(0), 256, engine::util::Deadline::none())
        .plan
        .unwrap();
    assert!(matches!(plan.actions[0].action,
        GameAction::ActivateAbility { source_id, .. } if source_id == second && source_id != first));
    replay(&state, &plan);
}

#[test]
fn terminal_win_is_preferred_over_an_available_cycle() {
    let mut state = oracle_position();
    let heliod = place(&mut state, &card("heliod, sun-crowned"), Zone::Battlefield);
    let ballista = place(&mut state, &card("walking ballista"), Zone::Battlefield);
    state
        .objects
        .get_mut(&ballista)
        .unwrap()
        .counters
        .insert(CounterType::Plus1Plus1, 2);
    for _ in 0..2 {
        state.players[0]
            .mana_pool
            .add(ManaUnit::new(ManaType::White, heliod, false, Vec::new()));
    }
    let result = plan_combos(&state, PlayerId(0), 96, engine::util::Deadline::none());
    assert!(result.nodes_used <= 96);
    let plan = result
        .plan
        .expect("coexisting supported combos must produce a plan");
    assert_eq!(plan.outcome, ComboPlanOutcome::WinUnderPassResponses);
    assert_eq!(
        replay(&state, &plan).game_end.map(|end| end.winner),
        Some(Some(PlayerId(0)))
    );
}

#[test]
fn public_ai_pipeline_finishes_the_library_combo() {
    use engine::game::bracket_estimate::CommanderBracketTier;
    use engine::game::deck_loading::DeckEntry;
    use engine::types::game_state::PlayerDeckPool;
    use rand::{rngs::SmallRng, SeedableRng};

    let mut state = oracle_position();
    state.deck_pools.push(PlayerDeckPool {
        player: PlayerId(0),
        current_main: Arc::new(vec![DeckEntry {
            card: card("thassa's oracle"),
            count: 1,
        }]),
        bracket_tier: CommanderBracketTier::Cedh,
        ..Default::default()
    });
    let mut config = crate::config::create_config(
        crate::config::AiDifficulty::CEDH,
        crate::config::Platform::Native,
    )
    .into_measurement(29);
    config.search.max_depth = 0;
    config.search.rollout_depth = 0;
    let mut random = SmallRng::seed_from_u64(29);
    for _ in 0..40 {
        if state.game_end.is_some() {
            break;
        }
        let player = state.waiting_for.acting_player().unwrap();
        let action = if player == PlayerId(0) {
            let scores = crate::search::score_candidates(&state, player, &config);
            crate::search::select_safe_action_from_scores(&state, &scores, 0.0, &mut random)
                .expect("the full AI pipeline must select an issued action")
        } else {
            GameAction::PassPriority
        };
        engine::game::engine::apply_as_current_for_simulation(&mut state, action).unwrap();
    }
    assert_eq!(
        state.game_end.map(|end| end.winner),
        Some(Some(PlayerId(0)))
    );
}

fn execute_ai_until_cycle(
    mut state: GameState,
    complete: impl Fn(&GameState) -> bool,
) -> GameState {
    use engine::game::bracket_estimate::CommanderBracketTier;
    use engine::game::deck_loading::DeckEntry;
    use engine::types::game_state::PlayerDeckPool;
    use rand::{rngs::SmallRng, SeedableRng};

    let entries = state
        .objects
        .values()
        .filter(|object| object.controller == PlayerId(0))
        .filter_map(|object| {
            let name = object.name.to_lowercase();
            matches!(
                name.as_str(),
                "heliod, sun-crowned"
                    | "walking ballista"
                    | "kiki-jiki, mirror breaker"
                    | "felidar guardian"
            )
            .then(|| DeckEntry {
                card: card(&name),
                count: 1,
            })
        })
        .collect();
    state.deck_pools.push(PlayerDeckPool {
        player: PlayerId(0),
        current_main: Arc::new(entries),
        bracket_tier: CommanderBracketTier::Cedh,
        ..Default::default()
    });
    for player in [PlayerId(0), PlayerId(1)] {
        for _ in 0..12 {
            let card_id = CardId(state.next_object_id);
            create_object(
                &mut state,
                card_id,
                player,
                "Library Filler".to_string(),
                Zone::Library,
            );
        }
    }
    let mut config = crate::config::create_config(
        crate::config::AiDifficulty::CEDH,
        crate::config::Platform::Native,
    )
    .into_measurement(29);
    config.search.max_depth = 0;
    config.search.rollout_depth = 0;
    let mut random = SmallRng::seed_from_u64(29);
    let mut actions = Vec::new();
    for _ in 0..40 {
        if complete(&state) {
            return state;
        }
        if state.game_end.is_some() {
            break;
        }
        let player = state.waiting_for.acting_player().unwrap();
        let action = if player == PlayerId(0) {
            let scores = crate::search::score_candidates(&state, player, &config);
            crate::search::select_safe_action_from_scores(&state, &scores, 0.0, &mut random)
                .expect("the full AI pipeline must select an issued action")
        } else {
            GameAction::PassPriority
        };
        actions.push((player, action.clone()));
        engine::game::engine::apply_as_current_for_simulation(&mut state, action).unwrap();
    }
    assert!(
        complete(&state),
        "the AI must finish its cycle: {actions:?}"
    );
    state
}

#[test]
fn public_ai_pipeline_finishes_the_damage_cycle() {
    let (state, _, ballista) = heliod_position();
    let life_before = state.players[1].life;
    execute_ai_until_cycle(state, |position| {
        position.players[1].life < life_before
            && position.stack.is_empty()
            && position.objects.get(&ballista).is_some_and(|object| {
                object
                    .counters
                    .get(&CounterType::Plus1Plus1)
                    .copied()
                    .unwrap_or(0)
                    >= 2
            })
    });
}

#[test]
fn public_ai_pipeline_finishes_the_copy_blink_cycle() {
    let mut state = position();
    let source = place(
        &mut state,
        &card("kiki-jiki, mirror breaker"),
        Zone::Battlefield,
    );
    let card_id = state.objects[&source].card_id;
    place(&mut state, &card("felidar guardian"), Zone::Battlefield);
    execute_ai_until_cycle(state, |position| {
        position.stack.is_empty()
            && position
                .objects
                .values()
                .any(|object| object.is_token && object.zone == Zone::Battlefield)
            && position.objects.values().any(|object| {
                object.card_id == card_id && object.zone == Zone::Battlefield && !object.tapped
            })
    });
}
