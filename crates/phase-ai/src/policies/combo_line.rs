//! Scores only the next action of a root-scoped, engine-simulated combo witness.

use crate::features::DeckFeatures;
use crate::policies::context::PolicyContext;
use crate::policies::registry::{
    DecisionKind, PolicyId, PolicyReason, PolicyVerdict, TacticalPolicy,
};
use engine::game::bracket_estimate::CommanderBracketTier;
use engine::types::game_state::GameState;
use engine::types::player::PlayerId;

pub struct ComboLinePolicy;

impl ComboLinePolicy {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ComboLinePolicy {
    fn default() -> Self {
        Self::new()
    }
}

impl TacticalPolicy for ComboLinePolicy {
    fn id(&self) -> PolicyId {
        PolicyId::ComboLineProgress
    }

    fn decision_kinds(&self) -> &'static [DecisionKind] {
        &[
            DecisionKind::CastSpell,
            DecisionKind::ActivateAbility,
            DecisionKind::SelectTarget,
            DecisionKind::ManaPayment,
            DecisionKind::ChooseX,
            DecisionKind::ActivateManaAbility,
        ]
    }

    fn activation(
        &self,
        features: &DeckFeatures,
        _state: &GameState,
        _player: PlayerId,
    ) -> Option<f32> {
        (features.bracket_tier == CommanderBracketTier::Cedh).then_some(1.0)
    }

    fn verdict(&self, ctx: &PolicyContext<'_>) -> PolicyVerdict {
        let next = ctx
            .context
            .combo_plan
            .as_ref()
            .and_then(|plan| plan.next_action(ctx.state, ctx.ai_player));
        if ctx.at_root() && next.is_some_and(|step| step.action == ctx.candidate.action) {
            PolicyVerdict::score(
                ctx.config.policy_penalties.combo_progress_this_turn_bonus,
                PolicyReason::new("combo_line_this_turn"),
            )
        } else {
            PolicyVerdict::neutral(PolicyReason::new("combo_line_no_match"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::ai_support::{ActionMetadata, CandidateAction, TacticalClass};
    use engine::types::actions::GameAction;
    use engine::types::game_state::GameState;
    use engine::types::player::PlayerId;

    use crate::config::{create_config, AiDifficulty, Platform};
    use crate::context::AiContext;
    use crate::features::DeckFeatures;

    fn make_state() -> GameState {
        GameState::new_two_player(0)
    }

    fn make_features(tier: CommanderBracketTier) -> DeckFeatures {
        DeckFeatures {
            bracket_tier: tier,
            ..DeckFeatures::default()
        }
    }

    #[test]
    fn activation_returns_none_when_not_cedh() {
        let policy = ComboLinePolicy::new();
        let state = make_state();
        let features = make_features(CommanderBracketTier::Core);
        let activation = policy.activation(&features, &state, PlayerId(0));
        assert!(activation.is_none());
    }

    #[test]
    fn activation_returns_some_when_is_cedh() {
        let policy = ComboLinePolicy::new();
        let state = make_state();
        let features = make_features(CommanderBracketTier::Cedh);
        let activation = policy.activation(&features, &state, PlayerId(0));
        assert_eq!(activation, Some(1.0));
    }

    #[test]
    fn verdict_returns_zero_score_with_no_reachable_combo() {
        let policy = ComboLinePolicy::new();
        let state = make_state();
        let config = create_config(AiDifficulty::CEDH, Platform::Native);
        let mut context = AiContext::empty(&config.weights);
        context.combo_plan =
            crate::combo::plan_combos(&state, PlayerId(0), 256, engine::util::Deadline::none())
                .plan
                .map(std::sync::Arc::new);

        let candidate = CandidateAction {
            action: GameAction::PassPriority,
            metadata: ActionMetadata::for_actor(Some(PlayerId(0)), TacticalClass::Pass),
        };
        let decision = engine::ai_support::AiDecisionContext {
            waiting_for: state.waiting_for.clone(),
            candidates: vec![candidate.clone()],
        };
        let ctx = PolicyContext {
            state: &state,
            decision: &decision,
            candidate: &candidate,
            ai_player: PlayerId(0),
            config: &config,
            context: &context,
            cast_facts: None,
            search_depth: crate::policies::context::SearchDepth::Root,
        };
        let verdict = policy.verdict(&ctx);
        match verdict {
            PolicyVerdict::Score { delta, .. } => assert_eq!(delta, 0.0),
            _ => panic!("expected Score with zero delta, got {verdict:?}"),
        }
    }

    /// Places Heliod, Sun-Crowned + Walking Ballista on PlayerId(0)'s
    /// battlefield with two untapped Plains (so Heliod's {1}{W} is payable in
    /// the color-aware reachability check), making the Heliod/Ballista line
    /// `ReachableThisTurn { missing_mana: 0, .. }`.
    fn heliod_ballista_state() -> (
        GameState,
        engine::types::identifiers::ObjectId,
        engine::types::identifiers::ObjectId,
    ) {
        crate::combo::tests::heliod_position()
    }

    fn make_context<'a>(
        state: &'a GameState,
        candidate: &'a CandidateAction,
        decision: &'a engine::ai_support::AiDecisionContext,
        config: &'a crate::config::AiConfig,
        context: &'a AiContext,
    ) -> PolicyContext<'a> {
        PolicyContext {
            state,
            decision,
            candidate,
            ai_player: PlayerId(0),
            config,
            context,
            cast_facts: None,
            search_depth: crate::policies::context::SearchDepth::Root,
        }
    }

    #[test]
    fn verdict_boosts_heliod_activation_when_reachable_this_turn() {
        let (state, heliod_id, _ballista_id) = heliod_ballista_state();
        let policy = ComboLinePolicy::new();
        let config = create_config(AiDifficulty::CEDH, Platform::Native);
        let mut context = AiContext::empty(&config.weights);
        context.combo_plan =
            crate::combo::plan_combos(&state, PlayerId(0), 256, engine::util::Deadline::none())
                .plan
                .map(std::sync::Arc::new);

        let candidate = CandidateAction {
            action: GameAction::ActivateAbility {
                source_id: heliod_id,
                ability_index: 0,
            },
            metadata: ActionMetadata::for_actor(Some(PlayerId(0)), TacticalClass::Ability),
        };
        let decision = engine::ai_support::AiDecisionContext {
            waiting_for: state.waiting_for.clone(),
            candidates: vec![candidate.clone()],
        };
        let ctx = make_context(&state, &candidate, &decision, &config, &context);

        let verdict = policy.verdict(&ctx);
        let expected = config.policy_penalties.combo_progress_this_turn_bonus;
        match verdict {
            PolicyVerdict::Score { delta, reason } => {
                assert_eq!(delta, expected, "expected this-turn bonus, got {delta}");
                assert_eq!(reason.kind, "combo_line_this_turn");
            }
            other => panic!("expected Score, got {other:?}"),
        }
    }

    #[test]
    fn verdict_does_not_boost_damage_before_lifelink_setup() {
        let (state, _heliod_id, ballista_id) = heliod_ballista_state();
        let policy = ComboLinePolicy::new();
        let config = create_config(AiDifficulty::CEDH, Platform::Native);
        let mut context = AiContext::empty(&config.weights);
        context.combo_plan =
            crate::combo::plan_combos(&state, PlayerId(0), 256, engine::util::Deadline::none())
                .plan
                .map(std::sync::Arc::new);

        // Ballista's damage ability sits at abilities[1] in card-data.
        let candidate = CandidateAction {
            action: GameAction::ActivateAbility {
                source_id: ballista_id,
                ability_index: 1,
            },
            metadata: ActionMetadata::for_actor(Some(PlayerId(0)), TacticalClass::Ability),
        };
        let decision = engine::ai_support::AiDecisionContext {
            waiting_for: state.waiting_for.clone(),
            candidates: vec![candidate.clone()],
        };
        let ctx = make_context(&state, &candidate, &decision, &config, &context);

        match policy.verdict(&ctx) {
            PolicyVerdict::Score { delta, .. } => {
                let expected = 0.0;
                assert_eq!(delta, expected);
            }
            other => panic!("expected Score, got {other:?}"),
        }
    }

    #[test]
    fn verdict_ignores_unrelated_activation_even_with_combo_on_board() {
        // Combo is on the board, but the candidate is some unrelated land's
        // ability — must not receive the bonus.
        let (state, _heliod_id, _ballista_id) = heliod_ballista_state();
        let policy = ComboLinePolicy::new();
        let config = create_config(AiDifficulty::CEDH, Platform::Native);
        let mut context = AiContext::empty(&config.weights);
        context.combo_plan =
            crate::combo::plan_combos(&state, PlayerId(0), 256, engine::util::Deadline::none())
                .plan
                .map(std::sync::Arc::new);

        // PassPriority is never in any combo line's required_actions.
        let candidate = CandidateAction {
            action: GameAction::PassPriority,
            metadata: ActionMetadata::for_actor(Some(PlayerId(0)), TacticalClass::Pass),
        };
        let decision = engine::ai_support::AiDecisionContext {
            waiting_for: state.waiting_for.clone(),
            candidates: vec![candidate.clone()],
        };
        let ctx = make_context(&state, &candidate, &decision, &config, &context);

        match policy.verdict(&ctx) {
            PolicyVerdict::Score { delta, reason } => {
                assert_eq!(delta, 0.0);
                assert_eq!(reason.kind, "combo_line_no_match");
            }
            other => panic!("expected zero-delta Score, got {other:?}"),
        }
    }
}
