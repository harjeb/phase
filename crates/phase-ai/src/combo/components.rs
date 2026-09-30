use engine::game::game_object::GameObject;
use engine::types::ability::{
    AbilityCondition, AbilityDefinition, AbilityKind, ChoiceType, Comparator,
    ContinuousModification, CostCategory, Effect, QuantityExpr, QuantityRef, TargetFilter,
    TriggerDefinition, ZoneRef,
};
use engine::types::card::CardFace;
use engine::types::counter::CounterType;
use engine::types::keywords::Keyword;
use engine::types::triggers::TriggerMode;
use engine::types::zones::Zone;

use crate::ability_chain::collect_chain_effects;

use super::{CardPredicate, ComponentRole};

pub fn matches_object(predicate: &CardPredicate, object: &GameObject) -> bool {
    if object.face_down {
        return false;
    }
    match predicate {
        CardPredicate::NameEquals(name) => object.name == *name,
        CardPredicate::All(predicates) => {
            !predicates.is_empty()
                && predicates
                    .iter()
                    .all(|predicate| matches_object(predicate, object))
        }
        CardPredicate::Role(role) => matches_parts(
            *role,
            &object.abilities,
            object
                .trigger_definitions
                .iter_unchecked()
                .map(|entry| &entry.definition),
        ),
    }
}

pub fn matches_face(predicate: &CardPredicate, face: &CardFace) -> bool {
    match predicate {
        CardPredicate::NameEquals(name) => face.name == *name,
        CardPredicate::All(predicates) => {
            !predicates.is_empty()
                && predicates
                    .iter()
                    .all(|predicate| matches_face(predicate, face))
        }
        CardPredicate::Role(role) => matches_parts(*role, &face.abilities, face.triggers.iter()),
    }
}

fn matches_parts<'a>(
    role: ComponentRole,
    abilities: &[AbilityDefinition],
    triggers: impl Iterator<Item = &'a TriggerDefinition>,
) -> bool {
    match role {
        ComponentRole::LifelinkGrant
        | ComponentRole::CounterDamage
        | ComponentRole::LibraryExile
        | ComponentRole::CreatureCopy => abilities
            .iter()
            .any(|ability| matches_ability(role, ability)),
        ComponentRole::LifeCounter | ComponentRole::LibraryWin | ComponentRole::EntryBlink => {
            triggers.into_iter().any(|trigger| {
                let Some(ability) = trigger.execute.as_deref() else {
                    return false;
                };
                let effects = collect_chain_effects(ability);
                match role {
                    ComponentRole::LifeCounter => {
                        trigger.mode == TriggerMode::LifeGained
                            && effects.iter().any(|effect| {
                                matches!(
                                    effect,
                                    Effect::PutCounter {
                                        counter_type: CounterType::Plus1Plus1,
                                        ..
                                    }
                                )
                            })
                    }
                    ComponentRole::EntryBlink => {
                        trigger.mode == TriggerMode::ChangesZone
                            && trigger.destination == Some(Zone::Battlefield)
                            && effects.iter().any(|effect| {
                                matches!(
                                    effect,
                                    Effect::ChangeZone {
                                        destination: Zone::Exile,
                                        ..
                                    }
                                )
                            })
                            && effects.iter().any(|effect| {
                                matches!(
                                    effect,
                                    Effect::ChangeZone {
                                        destination: Zone::Battlefield,
                                        ..
                                    }
                                )
                            })
                    }
                    ComponentRole::LibraryWin => {
                        trigger.mode == TriggerMode::ChangesZone
                            && trigger.destination == Some(Zone::Battlefield)
                            && std::iter::successors(Some(ability), |ability| {
                                ability.sub_ability.as_deref()
                            })
                            .any(|ability| {
                                matches!(*ability.effect, Effect::WinTheGame { .. })
                                    && matches!(
                                        &ability.condition,
                                        Some(AbilityCondition::QuantityCheck {
                                            comparator: Comparator::GE,
                                            rhs: QuantityExpr::Ref {
                                                qty: QuantityRef::ZoneCardCount {
                                                    zone: ZoneRef::Library,
                                                    ..
                                                }
                                            },
                                            ..
                                        })
                                    )
                            })
                    }
                    _ => false,
                }
            })
        }
    }
}

pub fn matches_ability(role: ComponentRole, ability: &AbilityDefinition) -> bool {
    let effects = collect_chain_effects(ability);
    match role {
        ComponentRole::LifelinkGrant => ability.kind == AbilityKind::Activated
            && effects.iter().any(|effect| match effect {
                Effect::GenericEffect { static_abilities, .. } => static_abilities.iter()
                    .flat_map(|definition| &definition.modifications)
                    .any(|modification| matches!(modification,
                        ContinuousModification::AddKeyword { keyword: Keyword::Lifelink })),
                _ => false,
            }),
        ComponentRole::CounterDamage => ability.kind == AbilityKind::Activated
            && ability.cost_categories().contains(&CostCategory::RemovesCounters)
            && effects.iter().any(|effect| matches!(effect, Effect::DealDamage { .. })),
        ComponentRole::CreatureCopy => ability.kind == AbilityKind::Activated
            && effects.iter().any(|effect| matches!(effect,
                Effect::CopyTokenOf { extra_keywords, .. } if extra_keywords.contains(&Keyword::Haste))),
        ComponentRole::LibraryExile => ability.kind == AbilityKind::Spell
            && effects.iter().any(|effect| matches!(effect,
                Effect::Choose { choice_type: ChoiceType::CardName, .. }))
            && effects.iter().any(|effect| matches!(effect,
                Effect::RevealUntil { player: TargetFilter::Controller,
                    filter: TargetFilter::HasChosenName, rest_destination: Zone::Exile, .. })),
        ComponentRole::LifeCounter | ComponentRole::LibraryWin | ComponentRole::EntryBlink => false,
    }
}
