use std::collections::HashSet;

use engine::ai_support::{build_decision_context, CandidateAction};
use engine::game::engine::apply_as_current_for_simulation;
use engine::types::ability::TargetRef;
use engine::types::actions::GameAction;
use engine::types::counter::CounterType;
use engine::types::game_state::{GameState, WaitingFor};
use engine::types::keywords::Keyword;
use engine::types::player::PlayerId;
use engine::types::zones::Zone;
use engine::util::Deadline;

use super::components::{matches_ability, matches_object};
use super::{
    CardPredicate, ComboLine, ComboLineId, ComboPiece, ComboRegistry, ComboStep, ComponentRole,
};

const MAX_ACTIONS: usize = 40;
const MAX_BRANCHING: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComboPlanOutcome {
    WinUnderPassResponses,
    CompletedCycle,
}

#[derive(Debug, Clone)]
pub struct PlannedAction {
    pub actor: PlayerId,
    pub action: GameAction,
}

#[derive(Debug, Clone)]
pub struct ComboPlan {
    pub line: ComboLineId,
    pub actions: Vec<PlannedAction>,
    pub outcome: ComboPlanOutcome,
    origin: GameState,
    player: PlayerId,
}

impl ComboPlan {
    pub fn next_action(&self, state: &GameState, player: PlayerId) -> Option<&PlannedAction> {
        if self.player != player
            || self.origin != *state
            || !std::sync::Arc::ptr_eq(&self.origin.all_card_names, &state.all_card_names)
            || !std::sync::Arc::ptr_eq(&self.origin.card_face_registry, &state.card_face_registry)
            || !state.objects.iter().all(|(id, object)| {
                self.origin.objects.get(id).is_some_and(|before| {
                    object.abilities == before.abilities
                        && object.base_abilities == before.base_abilities
                        && object.trigger_definitions.as_slice()
                            == before.trigger_definitions.as_slice()
                        && object.static_definitions.as_slice()
                            == before.static_definitions.as_slice()
                        && object.replacement_definitions.as_slice()
                            == before.replacement_definitions.as_slice()
                        && object.base_trigger_definitions == before.base_trigger_definitions
                        && object.base_static_definitions == before.base_static_definitions
                        && object.base_replacement_definitions
                            == before.base_replacement_definitions
                })
            })
        {
            return None;
        }
        self.actions.first()
    }
}

#[derive(Debug, Default)]
pub struct ComboPlanningResult {
    pub plan: Option<ComboPlan>,
    pub nodes_used: u32,
}

pub fn plan_combos(
    state: &GameState,
    ai: PlayerId,
    max_nodes: u32,
    deadline: Deadline,
) -> ComboPlanningResult {
    let mut result = ComboPlanningResult::default();
    if max_nodes == 0
        || deadline.expired()
        || state.game_end.is_some()
        || matches!(
            state.waiting_for,
            WaitingFor::MulliganDecision { .. }
                | WaitingFor::OpeningHandBottomCards { .. }
                | WaitingFor::DeclareAttackers { .. }
                | WaitingFor::DeclareBlockers { .. }
                | WaitingFor::CombatTaxPayment { .. }
        )
    {
        return result;
    }
    let registry = ComboRegistry::default();
    let available: Vec<_> = registry
        .lines()
        .iter()
        .filter(|line| components_available(state, ai, line))
        .collect();
    for (index, line) in available.iter().enumerate() {
        if result.nodes_used >= max_nodes || deadline.expired() {
            break;
        }
        let quota = (max_nodes - result.nodes_used) / (available.len() - index) as u32;
        let line_limit = result.nodes_used + quota;
        let mut queue = vec![(state.clone(), 0, Vec::new())];
        let mut seen = HashSet::new();
        while let Some((current, mut progress, actions)) = queue.pop() {
            if deadline.expired() {
                break;
            }
            progress = completed_setup(&current, ai, line, progress);
            if let Some(outcome) = outcome(state, &current, ai, line, progress, &actions) {
                if !actions.is_empty() {
                    if outcome == ComboPlanOutcome::WinUnderPassResponses || result.plan.is_none() {
                        result.plan = Some(ComboPlan {
                            line: line.id,
                            actions,
                            outcome,
                            origin: state.clone(),
                            player: ai,
                        });
                    }
                    if outcome == ComboPlanOutcome::WinUnderPassResponses {
                        return result;
                    }
                    break;
                }
            }
            if current.game_end.is_some()
                || current.turn_number != state.turn_number
                || actions.len() >= MAX_ACTIONS
                || !seen.insert((crate::planner::candidate_cache_key(&current), progress))
            {
                continue;
            }
            if result.nodes_used >= line_limit {
                break;
            }
            let context = build_decision_context(&current);
            let mut candidates: Vec<_> = context
                .candidates
                .into_iter()
                .filter(|candidate| permitted(&current, ai, line, progress, candidate))
                .collect();
            candidates.sort_by(|left, right| {
                let advances = |candidate: &CandidateAction| {
                    line.action_sequence
                        .get(progress)
                        .is_some_and(|step| matches_step(&current, ai, step, &candidate.action))
                };
                advances(right)
                    .cmp(&advances(left))
                    .then_with(|| left.action.cmp_stable(&right.action))
            });
            for candidate in candidates.into_iter().take(MAX_BRANCHING).rev() {
                if result.nodes_used >= line_limit || deadline.expired() {
                    break;
                }
                let Some(actor) = candidate.metadata.actor else {
                    continue;
                };
                result.nodes_used += 1;
                let mut next = current.clone();
                if apply_as_current_for_simulation(&mut next, candidate.action.clone()).is_err() {
                    continue;
                }
                let mut path = actions.clone();
                path.push(PlannedAction {
                    actor,
                    action: candidate.action.clone(),
                });
                let next_progress =
                    progress
                        + usize::from(line.action_sequence.get(progress).is_some_and(|step| {
                            matches_step(&current, ai, step, &candidate.action)
                        }));
                queue.push((next, next_progress, path));
            }
        }
    }
    result
}

fn components_available(state: &GameState, ai: PlayerId, line: &ComboLine) -> bool {
    line.pieces.iter().all(|piece| {
        let (predicate, zones): (&CardPredicate, &[Zone]) = match piece {
            ComboPiece::OnBattlefield(predicate) => (predicate, &[Zone::Battlefield]),
            ComboPiece::InHand(predicate) => (
                predicate,
                &[Zone::Hand, Zone::Stack, Zone::Battlefield, Zone::Graveyard],
            ),
            ComboPiece::InGraveyard(predicate) => (predicate, &[Zone::Graveyard]),
            ComboPiece::InLibrary(_) => return false,
        };
        state.objects.values().any(|object| {
            object.controller == ai
                && zones.contains(&object.zone)
                && matches_object(predicate, object)
        })
    })
}

fn completed_setup(
    state: &GameState,
    ai: PlayerId,
    line: &ComboLine,
    mut progress: usize,
) -> usize {
    while let Some(step) = line.action_sequence.get(progress) {
        let done = match step {
            ComboStep::Cast { predicate } => {
                state.objects.values().any(|object| {
                    object.controller == ai
                        && matches_object(predicate, object)
                        && matches!(object.zone, Zone::Stack | Zone::Battlefield)
                }) || matches!(predicate, CardPredicate::Role(ComponentRole::LibraryExile))
                    && state
                        .players
                        .get(ai.0 as usize)
                        .is_some_and(|player| player.library.is_empty())
            }
            ComboStep::ActivateRole {
                role: ComponentRole::LifelinkGrant,
                ..
            } => state.objects.values().any(|object| {
                object.controller == ai
                    && object.zone == Zone::Battlefield
                    && object.keywords.contains(&Keyword::Lifelink)
                    && matches_object(&CardPredicate::Role(ComponentRole::CounterDamage), object)
            }),
            ComboStep::Activate { .. } | ComboStep::ActivateRole { .. } => false,
        };
        if !done {
            break;
        }
        progress += 1;
    }
    progress
}

fn matches_step(state: &GameState, ai: PlayerId, step: &ComboStep, action: &GameAction) -> bool {
    match (step, action) {
        (ComboStep::Cast { predicate }, GameAction::CastSpell { object_id, .. }) => state
            .objects
            .get(object_id)
            .is_some_and(|object| object.controller == ai && matches_object(predicate, object)),
        (
            ComboStep::Activate {
                predicate,
                ability_index,
            },
            GameAction::ActivateAbility {
                source_id,
                ability_index: index,
            },
        ) => {
            *index == usize::from(*ability_index)
                && state.objects.get(source_id).is_some_and(|object| {
                    object.controller == ai && matches_object(predicate, object)
                })
        }
        (
            ComboStep::ActivateRole { predicate, role },
            GameAction::ActivateAbility {
                source_id,
                ability_index,
            },
        ) => state.objects.get(source_id).is_some_and(|object| {
            object.controller == ai
                && matches_object(predicate, object)
                && object
                    .abilities
                    .get(*ability_index)
                    .is_some_and(|ability| matches_ability(*role, ability))
        }),
        _ => false,
    }
}

fn permitted(
    state: &GameState,
    ai: PlayerId,
    line: &ComboLine,
    progress: usize,
    candidate: &CandidateAction,
) -> bool {
    if candidate.metadata.semantic_owner != Some(ai) {
        return matches!(state.waiting_for, WaitingFor::Priority { .. })
            && candidate.action == GameAction::PassPriority;
    }
    if matches!(state.waiting_for, WaitingFor::Priority { .. }) {
        return candidate.action == GameAction::PassPriority && !state.stack.is_empty()
            || line.action_sequence.get(progress).is_some_and(|step| {
                (state.stack.is_empty() || matches!(step, ComboStep::Cast { .. }))
                    && matches_step(state, ai, step, &candidate.action)
            });
    }
    let target_allowed = |target: &TargetRef| match target {
        TargetRef::Player(player) => engine::game::players::opponents(state, ai).contains(player),
        TargetRef::Object(id) => state.objects.get(id).is_some_and(|object| {
            if line.action_sequence.iter().any(|step| {
                matches!(
                    step,
                    ComboStep::ActivateRole {
                        role: ComponentRole::CounterDamage,
                        ..
                    }
                )
            }) {
                return object.controller == ai
                    && matches_object(&CardPredicate::Role(ComponentRole::CounterDamage), object);
            }
            line.pieces.iter().any(|piece| match piece {
                ComboPiece::OnBattlefield(predicate)
                | ComboPiece::InHand(predicate)
                | ComboPiece::InGraveyard(predicate)
                | ComboPiece::InLibrary(predicate) => {
                    object.controller == ai && matches_object(predicate, object)
                }
            })
        }),
    };
    match &candidate.action {
        GameAction::ChooseTarget {
            target: Some(target),
        } => return target_allowed(target),
        GameAction::SelectTargets { targets } => return targets.iter().all(target_allowed),
        _ => {}
    }
    !matches!(candidate.action, GameAction::Concede { .. })
}

fn outcome(
    initial: &GameState,
    state: &GameState,
    ai: PlayerId,
    line: &ComboLine,
    progress: usize,
    actions: &[PlannedAction],
) -> Option<ComboPlanOutcome> {
    if state.game_end.is_some_and(|end| end.winner == Some(ai)) {
        return Some(ComboPlanOutcome::WinUnderPassResponses);
    }
    if progress < line.action_sequence.len()
        || !state.stack.is_empty()
        || !matches!(state.waiting_for, WaitingFor::Priority { player } if player == ai)
    {
        return None;
    }
    let role = match line.action_sequence.last()? {
        ComboStep::ActivateRole { role, .. } => *role,
        _ => return None,
    };
    let source = actions.iter().rev().find_map(|step| match step.action {
        GameAction::ActivateAbility { source_id, .. }
            if matches_step(
                initial,
                ai,
                line.action_sequence.last().unwrap(),
                &step.action,
            ) =>
        {
            Some(source_id)
        }
        _ => None,
    })?;
    let card_id = initial.objects.get(&source)?.card_id;
    let ready = build_decision_context(state)
        .candidates
        .into_iter()
        .any(|candidate| {
            if !matches_step(
                state,
                ai,
                line.action_sequence.last().unwrap(),
                &candidate.action,
            ) {
                return false;
            }
            matches!(candidate.action, GameAction::ActivateAbility { source_id, .. }
            if state.objects.get(&source_id).is_some_and(|object| object.card_id == card_id))
        });
    if !ready {
        return None;
    }
    match role {
        ComponentRole::CounterDamage => {
            let damage_dealt = engine::game::players::opponents(state, ai)
                .iter()
                .any(|opponent| {
                    state
                        .players
                        .get(opponent.0 as usize)
                        .zip(initial.players.get(opponent.0 as usize))
                        .is_some_and(|(after, before)| after.life < before.life)
                });
            let replenished = state.objects.get(&source).is_some_and(|object| {
                object.controller == ai
                    && object.zone == Zone::Battlefield
                    && object.keywords.contains(&Keyword::Lifelink)
                    && matches_object(&CardPredicate::Role(role), object)
                    && initial.objects.get(&object.id).is_some_and(|before| {
                        before.counters.iter().all(|(kind, count)| {
                            object.counters.get(kind).copied().unwrap_or(0) >= *count
                        }) && object
                            .counters
                            .get(&CounterType::Plus1Plus1)
                            .copied()
                            .unwrap_or(0)
                            > 0
                    })
            });
            (damage_dealt && replenished).then_some(ComboPlanOutcome::CompletedCycle)
        }
        ComponentRole::CreatureCopy => {
            let tokens = |position: &GameState| {
                position
                    .objects
                    .values()
                    .filter(|object| {
                        object.controller == ai
                            && object.zone == Zone::Battlefield
                            && object.is_token
                            && matches_object(
                                &CardPredicate::Role(ComponentRole::EntryBlink),
                                object,
                            )
                    })
                    .count()
            };
            (tokens(state) > tokens(initial)).then_some(ComboPlanOutcome::CompletedCycle)
        }
        _ => None,
    }
}
