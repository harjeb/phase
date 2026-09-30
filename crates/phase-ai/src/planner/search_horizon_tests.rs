use super::*;
use crate::config::{create_config, AiDifficulty, Platform};
use engine::ai_support::ActionMetadata;
use engine::game::scenario::{GameScenario, P0, P1};
use engine::types::ability::TargetRef;
use engine::types::game_state::CastPaymentMode;
use engine::types::identifiers::ObjectId;
use engine::types::mana::{ManaCost, ManaType, ManaUnit};
use engine::types::phase::Phase;
use engine::types::zones::Zone;

fn target_payment_chain() -> (GameState, ObjectId) {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let victim = scenario.add_creature(P1, "Extension victim", 10, 10).id();
    scenario.add_creature(P0, "Alternative target", 1, 1);
    let spell = scenario
        .add_spell_to_hand_from_oracle(P0, "Extension witness", true, "Destroy target creature.")
        .with_mana_cost(ManaCost::Cost {
            shards: vec![],
            generic: 1,
        })
        .id();
    let mut runner = scenario.build();
    runner.state_mut().add_mana_to_pool(
        P0,
        ManaUnit::new(ManaType::Colorless, ObjectId(0), false, vec![]),
    );
    let card_id = runner.state().objects[&spell].card_id;
    runner
        .act(GameAction::CastSpell {
            object_id: spell,
            card_id,
            targets: vec![],
            payment_mode: CastPaymentMode::Manual,
        })
        .unwrap();
    assert!(
        matches!(
            runner.state().waiting_for,
            WaitingFor::TargetSelection { .. }
        ),
        "actual prompt: {:?}",
        runner.state().waiting_for
    );
    (runner.state().clone(), victim)
}

fn search_at_horizon(state: &GameState, allowance: u8, nodes: u32) -> (f64, u32, u32) {
    let mut config = create_config(AiDifficulty::Hard, Platform::Native).into_measurement(42);
    config.search.max_branching = 1;
    let policies = PolicyRegistry::default();
    let mut services = PlannerServices::new_default(P0, &config, &policies);
    let mut budget = SearchBudget::new(nodes);
    let planner = BeamContinuationPlanner {
        depth: 1,
        rollout_depth: 0,
    };
    let value = planner.search_with_extension(
        state,
        0,
        allowance,
        0,
        f64::NEG_INFINITY,
        f64::INFINITY,
        &mut services,
        &mut budget,
    );
    (
        value,
        services.announcement_extensions,
        budget.nodes_evaluated,
    )
}

#[test]
fn announcement_extension_sees_target_payment_and_resolved_removal() {
    let (state, victim) = target_payment_chain();
    let mut payment = state.clone();
    apply_as_current_for_simulation(
        &mut payment,
        GameAction::ChooseTarget {
            target: Some(TargetRef::Object(victim)),
        },
    )
    .unwrap();
    assert!(matches!(
        payment.waiting_for,
        WaitingFor::ManaPayment { .. }
    ));
    let prepared =
        prepare_payment_candidates(&payment, build_decision_context(&payment).candidates);
    let finalized = prepared
        .into_iter()
        .filter_map(|p| p.payment_successor)
        .find(|s| s.pending_cast.is_none() && !s.stack.is_empty())
        .expect("a certified edge finalizes the real payment");
    let config = create_config(AiDifficulty::Hard, Platform::Native).into_measurement(42);
    let policies = PolicyRegistry::default();
    let services = PlannerServices::new_default(P0, &config, &policies);
    let resolved = services.quiesce(&finalized);
    assert_eq!(resolved.objects[&victim].zone, Zone::Graveyard);

    let (unextended, zero_extensions, _) = search_at_horizon(&state, 0, 100);
    let (extended, extensions, _) = search_at_horizon(&state, 2, 100);
    assert_eq!(zero_extensions, 0);
    assert_eq!(extensions, 2, "target and payment each spend one extension");
    assert!(
        extended > unextended + 5.0,
        "must see removal, not merely an edge bonus: {extended} vs {unextended}"
    );
    let (_, one_extension, _) = search_at_horizon(&state, 1, 100);
    assert_eq!(one_extension, 1, "the allowance must not reset at payment");
    assert_eq!(
        search_at_horizon(&state, 2, 100),
        search_at_horizon(&state, 2, 100)
    );
}

#[test]
fn announcement_extension_does_not_expand_quiet_priority() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let runner = scenario.build();
    let (_, extensions, nodes) = search_at_horizon(runner.state(), 2, 100);
    assert_eq!(extensions, 0);
    assert_eq!(nodes, 1);
}

#[test]
fn announcement_extension_honors_zero_and_last_node_budgets() {
    let (state, _) = target_payment_chain();
    for nodes in 0..=1 {
        let (_, extensions, used) = search_at_horizon(&state, 2, nodes);
        assert_eq!(extensions, 0);
        assert!(used <= nodes);
    }
    for nodes in 2..=5 {
        let (_, _, used) = search_at_horizon(&state, 2, nodes);
        assert!(used <= nodes);
    }
}

#[test]
fn unfinished_announcements_bypass_candidate_cache_in_rollouts() {
    let (state, _) = target_payment_chain();
    let config = create_config(AiDifficulty::Hard, Platform::Native).into_measurement(42);
    let policies = PolicyRegistry::default();
    let mut services = PlannerServices::new_default(P0, &config, &policies);
    let mut poisoned = build_decision_context(&state);
    assert!(!poisoned.candidates.is_empty());
    poisoned.candidates.clear();
    services
        .candidate_cache
        .insert(candidate_cache_key(&state), std::sync::Arc::new(poisoned));
    assert!(
        !services.planner_evaluation(&state).priors.is_empty(),
        "rollout must regenerate pending candidates, not use the stale cache"
    );
}

#[test]
fn beam_backfills_illegal_candidate_without_spending_width() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let runner = scenario.build();
    let state = runner.state();
    let mut config = create_config(AiDifficulty::Hard, Platform::Native).into_measurement(42);
    config.search.max_branching = 1;
    let policies = PolicyRegistry::default();
    let mut services = PlannerServices::new_default(P0, &config, &policies);
    let illegal = CandidateAction {
        action: GameAction::ActivateAbility {
            source_id: ObjectId(99999),
            ability_index: 0,
        },
        metadata: ActionMetadata::for_actor(Some(P0), TacticalClass::Utility),
    };
    let pass = CandidateAction {
        action: GameAction::PassPriority,
        metadata: ActionMetadata::for_actor(Some(P0), TacticalClass::Pass),
    };
    assert!(apply_candidate(state, &illegal).is_none());
    assert!(apply_candidate(state, &pass).is_some());
    let mut ctx = build_decision_context(state);
    ctx.candidates = vec![illegal.clone(), pass];
    let ranked = rank_prepared_candidates(
        prepare_payment_candidates(state, ctx.candidates.clone()),
        |c| services.tactical_score(state, &ctx, c, P0, SearchDepth::Lookahead),
        usize::MAX,
    );
    assert_eq!(
        ranked[0].candidate.action, illegal.action,
        "hostile candidate must occupy the original beam"
    );
    services
        .candidate_cache
        .insert(candidate_cache_key(state), std::sync::Arc::new(ctx));
    let planner = BeamContinuationPlanner {
        depth: 1,
        rollout_depth: 0,
    };
    let mut budget = SearchBudget::new(100);
    let result = planner.search_value(
        state,
        1,
        0,
        f64::NEG_INFINITY,
        f64::INFINITY,
        &mut services,
        &mut budget,
    );
    assert!(result.is_finite());
    assert_eq!(
        budget.nodes_evaluated, 2,
        "the legal tail must be searched after the invalid top is skipped"
    );
}
