//! End-to-end smoke test for cEDH difficulty wiring.
//!
//! Verifies that all layers wired across Phases 1-8 of the cEDH implementation
//! are correctly connected: config preset values, 4-player paranoid-scaling
//! bypass, `DeckFeatures::is_cedh`, `ComboLinePolicy` registration, and the
//! structural combo registry and engine-verified action plans.

use std::sync::Arc;

use engine::ai_support::legal_actions;
use engine::game::bracket_estimate::CommanderBracketTier;
use engine::game::deck_loading::DeckEntry;
use engine::game::zones::create_object;
use engine::types::actions::GameAction;
use engine::types::card::CardFace;
use engine::types::card_type::CoreType;
use engine::types::game_state::{GameState, PlayerDeckPool, WaitingFor};
use engine::types::identifiers::{CardId, ObjectId};
use engine::types::phase::Phase;
use engine::types::player::PlayerId;
use engine::types::zones::Zone;

use phase_ai::combo::ComboRegistry;
use phase_ai::config::{create_config, create_config_for_players, AiDifficulty, Platform};
use phase_ai::features::DeckFeatures;
use phase_ai::policies::registry::{PolicyId, PolicyRegistry};
use phase_ai::search::{choose_action, score_candidates, select_safe_action_from_scores};

/// Builds the minimal `GameState` shared by the `score_candidates` and
/// `choose_action` cEDH combo tests.
///
/// `tier` controls `PlayerId(0)`'s `PlayerDeckPool::bracket_tier`; pass
/// `CommanderBracketTier::Cedh` for the positive tests and
/// `CommanderBracketTier::Core` (or any non-cEDH tier) for negative tests that
/// verify the combo bonus is absent.
///
/// Returns `(state, heliod_id, ballista_id)`.
///
/// Invariants guaranteed by this builder:
///
/// - `Phase::PreCombatMain`, `PlayerId(0)` has priority.
/// - Two untapped Plains satisfy Heliod's `{1}{W}` cost via the legacy
///   land-mana fallback (no `AbilityDefinition` needed on the land objects).
/// - Heliod and Walking Ballista use their actual parsed costs and effects.
/// - Walking Ballista starts with two +1/+1 counters and can pay its damage cost.
/// - `PlayerId(0)`'s `PlayerDeckPool` has a non-empty `current_main` so
///   `build_ai_context` propagates the tier through `DeckFeatures::analyze`.
fn cedh_combo_state_with_parsed_cards(
    tier: CommanderBracketTier,
) -> (GameState, ObjectId, ObjectId) {
    use engine::game::printed_cards::apply_card_face_to_object;
    use engine::types::counter::CounterType;
    let cards: std::collections::HashMap<String, CardFace> =
        serde_json::from_str(include_str!("../src/combo/fixtures/cards.json")).unwrap();
    let mut state = GameState::new_two_player(42);
    state.turn_number = 3;
    state.phase = Phase::PreCombatMain;
    state.active_player = PlayerId(0);
    state.priority_player = PlayerId(0);
    state.waiting_for = WaitingFor::Priority {
        player: PlayerId(0),
    };
    for index in 0..2 {
        let land = create_object(
            &mut state,
            CardId(100 + index),
            PlayerId(0),
            "Plains".to_string(),
            Zone::Battlefield,
        );
        let object = state.objects.get_mut(&land).unwrap();
        object.card_types.core_types.push(CoreType::Land);
        object.card_types.subtypes.push("Plains".to_string());
    }
    let heliod_id = create_object(
        &mut state,
        CardId(200),
        PlayerId(0),
        "Heliod, Sun-Crowned".to_string(),
        Zone::Battlefield,
    );
    apply_card_face_to_object(
        state.objects.get_mut(&heliod_id).unwrap(),
        &cards["heliod, sun-crowned"],
    );
    let ballista_id = create_object(
        &mut state,
        CardId(201),
        PlayerId(0),
        "Walking Ballista".to_string(),
        Zone::Battlefield,
    );
    let ballista = state.objects.get_mut(&ballista_id).unwrap();
    apply_card_face_to_object(ballista, &cards["walking ballista"]);
    ballista.entered_battlefield_turn = Some(0);
    ballista.summoning_sick = false;
    ballista.counters.insert(CounterType::Plus1Plus1, 2);
    let entry = DeckEntry {
        card: CardFace::default(),
        count: 1,
    };
    state.deck_pools.push(PlayerDeckPool {
        player: PlayerId(0),
        current_main: Arc::new(vec![entry.clone()]),
        bracket_tier: tier,
        ..Default::default()
    });
    state.deck_pools.push(PlayerDeckPool {
        player: PlayerId(1),
        current_main: Arc::new(vec![entry]),
        bracket_tier: CommanderBracketTier::Core,
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
    (state, heliod_id, ballista_id)
}

#[test]
fn cedh_full_stack_smoke() {
    // 1. CEDH preset values (CR-irrelevant; engine config constants).
    let cfg = create_config(AiDifficulty::CEDH, Platform::Native);
    assert_eq!(cfg.search.max_depth, 3);
    assert_eq!(cfg.search.max_nodes, 96);

    // 2. 4-player scaling is skipped for CEDH: depth and nodes must be
    //    unchanged from the 2-player config.
    let cfg4 = create_config_for_players(AiDifficulty::CEDH, Platform::Native, 4);
    assert_eq!(
        cfg4.search.max_depth, 3,
        "4-player CEDH must skip paranoid scaling"
    );
    assert_eq!(
        cfg4.search.max_nodes, 96,
        "4-player CEDH must skip paranoid scaling"
    );

    // 3. DeckFeatures::bracket_tier defaults to a non-cEDH tier.
    let features = DeckFeatures::default();
    assert_ne!(
        features.bracket_tier,
        engine::game::bracket_estimate::CommanderBracketTier::Cedh
    );

    // 4. DeckFeatures::analyze records the Cedh tier when given it.
    let cedh_features = DeckFeatures::analyze(
        &[],
        engine::game::bracket_estimate::CommanderBracketTier::Cedh,
    );
    assert_eq!(
        cedh_features.bracket_tier,
        engine::game::bracket_estimate::CommanderBracketTier::Cedh
    );

    // 5. Default PolicyRegistry includes ComboLineProgress — the policy that
    //    consults ComboRegistry during cEDH AI decisions.
    let reg = PolicyRegistry::default();
    assert!(
        reg.has_policy(PolicyId::ComboLineProgress),
        "PolicyRegistry::default() must register ComboLinePolicy"
    );

    // 6. ComboRegistry ships with structurally matched combo templates to prove
    //    end-to-end wiring (real cEDH lines are a follow-up phase).
    let combo_reg = ComboRegistry::default();
    assert!(
        !combo_reg.lines().is_empty(),
        "ComboRegistry::default() must contain at least one combo line"
    );
}

/// End-to-end planner integration test for the cEDH combo bonus.
///
/// Builds a minimal `GameState` where the registered Heliod, Sun-Crowned +
/// Walking Ballista combo line is already assembled on the AI player's
/// battlefield (with two untapped Plains for the `{1}{W}` activation cost),
/// configures both players' `PlayerDeckPool::bracket_tier = Cedh`, and runs
/// `phase_ai::search::score_candidates` — the full planner entry point.
///
/// Verifies that the prepared plan prefers the setup activation and that
/// disabling only the combo bonus lowers its score on the identical state.
/// This isolates policy wiring from the real abilities' intrinsic value.
#[test]
fn score_candidates_boosts_heliod_combo_activation_for_cedh_ai() {
    let (state, heliod_id, _) = cedh_combo_state_with_parsed_cards(CommanderBracketTier::Cedh);
    let prepared =
        phase_ai::combo::plan_combos(&state, PlayerId(0), 48, engine::util::Deadline::none());
    assert!(
        prepared.plan.is_some(),
        "the production combo quota must find a witness: {prepared:?}"
    );

    // Sanity guard: the engine must offer the Heliod activation as a legal
    // priority action. If this fails the test is mis-set-up and the
    // score-based assertion below would be meaningless.
    let actions = legal_actions(&state);
    let heliod_activation = GameAction::ActivateAbility {
        source_id: heliod_id,
        ability_index: 0,
    };
    assert!(
        actions.contains(&heliod_activation),
        "legal_actions must offer the Heliod activation candidate; got {:?}",
        actions
    );
    assert!(
        actions.contains(&GameAction::PassPriority),
        "legal_actions must offer PassPriority as the baseline"
    );

    // Run the real planner entry point. `into_measurement` disables the
    // wall-clock budget so the test is reproducible.
    let config = create_config(AiDifficulty::CEDH, Platform::Native).into_measurement(42);
    let scored = score_candidates(&state, PlayerId(0), &config);
    assert!(
        !scored.is_empty(),
        "score_candidates returned no candidates"
    );

    let heliod_score = scored
        .iter()
        .find(|(action, _)| *action == heliod_activation)
        .map(|(_, s)| *s)
        .unwrap_or_else(|| {
            panic!(
                "Heliod activation candidate missing from scored output: {:?}",
                scored
            )
        });
    let pass_score = scored
        .iter()
        .find(|(action, _)| matches!(action, GameAction::PassPriority))
        .map(|(_, s)| *s)
        .unwrap_or_else(|| {
            panic!(
                "PassPriority candidate missing from scored output: {:?}",
                scored
            )
        });

    assert!(
        heliod_score > pass_score,
        "Heliod combo activation must outscore PassPriority for a cEDH AI \
         (heliod_score = {heliod_score}, pass_score = {pass_score}, scored = {scored:?})"
    );

    let mut without_bonus = config.clone();
    without_bonus
        .policy_penalties
        .combo_progress_this_turn_bonus = 0.0;
    let ablated = score_candidates(&state, PlayerId(0), &without_bonus);
    let ablated_heliod_score = ablated
        .iter()
        .find(|(action, _)| *action == heliod_activation)
        .map(|(_, score)| *score)
        .expect("Heliod activation must remain legal without the combo bonus");
    let min_lift = config.policy_penalties.combo_progress_this_turn_bonus * 0.1 * 0.5;
    assert!(
        heliod_score - ablated_heliod_score > min_lift,
        "The combo policy must lift the same setup action by at least {min_lift:.3}: \
+         enabled = {heliod_score}, ablated = {ablated_heliod_score}"
    );
}

/// Closes the selection-layer gap left by `score_candidates_boosts_*`.
///
/// `score_candidates` proved the combo bonus reaches the final score. This
/// test proves the SELECTION layer — `choose_action`'s softmax
/// call — uses that score dominance to pick a combo activation in practice.
///
/// A single trial would be seed-dependent. Instead we run 50 trials with
/// different seeds and assert that at least 40 (80 %) select a combo step.
/// With `temperature = 0.2` and the combo steps outscoring `PassPriority`
/// by ~+1.5 (15.0 policy bonus × 0.1 tactical weight), the theoretical
/// softmax probability of picking a combo step is >95 % — the 80 % threshold
/// is deliberately conservative to tolerate small weight retunings while still
/// catching a real wiring regression that drops selection to chance level.
#[test]
fn choose_action_picks_combo_activation_for_cedh_ai() {
    use rand::rngs::SmallRng;
    use rand::SeedableRng;

    let (state, heliod_id, ballista_id) =
        cedh_combo_state_with_parsed_cards(CommanderBracketTier::Cedh);
    let config = create_config(AiDifficulty::CEDH, Platform::Native).into_measurement(42);

    let mut smoke_rng = SmallRng::seed_from_u64(0);
    assert!(
        choose_action(&state, PlayerId(0), &config, &mut smoke_rng).is_some(),
        "choose_action smoke must return a legal action"
    );

    let scored = score_candidates(&state, PlayerId(0), &config);
    assert!(
        !scored.is_empty(),
        "score_candidates returned no candidates"
    );

    let mut combo_count = 0u32;
    for seed in 0..50u64 {
        let mut rng = SmallRng::seed_from_u64(seed);
        let action = select_safe_action_from_scores(&state, &scored, config.temperature, &mut rng);
        if matches!(
            action,
            Some(GameAction::ActivateAbility {
                source_id,
                ability_index,
            }) if (source_id == heliod_id && ability_index == 0)
                || (source_id == ballista_id && ability_index == 1)
        ) {
            combo_count += 1;
        }
    }

    assert!(
        combo_count >= 40,
        "expected at least 40/50 trials to pick a combo activation (Heliod[0] or \
         Ballista[1]); got {combo_count}/50 — softmax selection is not respecting the \
         ComboLinePolicy bonus reaching score_candidates output (heliod_id = {heliod_id:?}, \
         ballista_id = {ballista_id:?})"
    );
}

/// Proves the cEDH combo bonus is the *cause* of combo selection, not an
/// incidental bias. Holds everything constant versus
/// `choose_action_picks_combo_activation_for_cedh_ai` — same state, same
/// `AiDifficulty::CEDH` config — and varies only the deck tier from `Cedh` to
/// `Core`. With `is_cedh = false`, `ComboLinePolicy::activation()` returns
/// `None`, so no bonus reaches the combo activations. If this test ever fails
/// (combo_count ≥ 20), the bonus is leaking through a code path that does not
/// gate on `is_cedh`, which is a real wiring regression.
#[test]
fn choose_action_does_not_boost_combo_without_is_cedh() {
    use rand::{rngs::SmallRng, SeedableRng};
    let (state, _, _) = cedh_combo_state_with_parsed_cards(CommanderBracketTier::Core);
    let config = create_config(AiDifficulty::CEDH, Platform::Native).into_measurement(42);
    let mut random = SmallRng::seed_from_u64(42);
    assert!(choose_action(&state, PlayerId(0), &config, &mut random).is_some());
    let scored = score_candidates(&state, PlayerId(0), &config);
    let mut ablated = config.clone();
    ablated.policy_penalties.combo_progress_this_turn_bonus = 0.0;
    assert!(!scored.is_empty());
    assert_eq!(
        scored,
        score_candidates(&state, PlayerId(0), &ablated),
        "changing the combo bonus must not change scores for a non-cEDH deck"
    );
}
