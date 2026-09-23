//! Combat-damage stack classification and the gated pre-M10 runtime.
//!
//! Admission remains refused until the legacy runtime's wider rules surface is
//! verified. The runtime tests below opt in directly through FormatConfig.
//!
//! The rules premise is the pre-M10 procedure (Classic Sixth Edition 1999
//! through Magic 2010, July 2009): all of a combat damage step's assignments go
//! on the stack as a single object, which is not a spell and not an ability and
//! therefore cannot be countered or targeted. Those historical rule numbers are
//! deliberately named only in prose — the current CR reuses 310 for Battles, so
//! citing them as `CR` annotations would point at the wrong rule.
//!
//! Classification fixtures are hand-built; runtime fixtures below reach the
//! real queue through combat actions. Persisted admission remains closed while
//! the larger legacy-mode compatibility surface is unverified.

use engine::game::derived_views::derive_views;
use engine::game::effects::copy_spell;
use engine::game::scenario::{GameScenario, P0, P1};
use engine::types::ability::{
    CopyRetargetPermission, Effect, ResolvedAbility, TargetFilter, TypedFilter,
};
use engine::types::game_state::{
    AssignedCombatDamage, AssignedDamageRecipient, CombatDamageSubStep, GameState,
    PersistedGameState, PersistedRestoreError, PriorityYield, SpellCastRecord, StackEntry,
    StackEntryKind, StackResolutionEntryFence, WaitingFor, YieldTarget,
};
use engine::types::identifiers::{ObjectId, ObjectIncarnationRef};
use engine::types::phase::Phase;
use engine::types::player::PlayerId;

fn advance_to_queued_damage(
    runner: &mut engine::game::scenario::GameRunner,
    attacker: ObjectId,
    blocker: ObjectId,
) {
    use engine::types::actions::GameAction;
    let mut rules = engine::types::custom_format::old_school_93_94().rules;
    rules.legality.legacy.damage_timing = engine::types::custom_format::CombatDamageTiming::OnStack;
    rules.legality.legacy.mana_burn = Default::default();
    runner.state_mut().format_config =
        engine::types::format::FormatConfig::for_custom_rules(&rules);
    runner.pass_both_players();
    runner
        .declare_attackers(&[(attacker, engine::game::combat::AttackTarget::Player(P1))])
        .unwrap();
    for _ in 0..12 {
        if runner
            .state()
            .stack
            .iter()
            .any(|entry| matches!(entry.kind, StackEntryKind::CombatDamage { .. }))
        {
            assert_eq!(runner.state().phase, Phase::CombatDamage);
            return;
        }
        match runner.state().waiting_for {
            WaitingFor::DeclareBlockers { .. } => {
                runner.declare_blockers(&[(blocker, attacker)]).unwrap();
            }
            WaitingFor::Priority { .. } => {
                runner.act(GameAction::PassPriority).unwrap();
            }
            WaitingFor::AssignCombatDamage {
                total_damage,
                ref blockers,
                ..
            } => {
                assert_eq!(blockers.len(), 1);
                let lethal = blockers[0].lethal_minimum;
                runner
                    .act(GameAction::AssignCombatDamage {
                        mode: Default::default(),
                        assignments: vec![(blocker, lethal)],
                        trample_damage: total_damage - lethal,
                        controller_damage: 0,
                    })
                    .unwrap();
            }
            ref other => panic!("unexpected combat wait: {other:?}"),
        }
    }
    panic!("combat never queued damage");
}

#[test]
fn queued_damage_rejects_a_recipient_that_lost_all_damageable_types() {
    use engine::types::card_type::CoreType;
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let attacker = scenario.add_vanilla(P0, 3, 4);
    let blocker = scenario.add_vanilla(P1, 2, 4);
    let mut runner = scenario.build();
    advance_to_queued_damage(&mut runner, attacker, blocker);
    let recipient = runner.state_mut().objects.get_mut(&blocker).unwrap();
    recipient.card_types.core_types = vec![CoreType::Enchantment];
    recipient.base_card_types.core_types = vec![CoreType::Enchantment];
    runner.pass_both_players();
    assert_eq!(runner.state().objects[&blocker].damage_marked, 0);
    assert_eq!(
        runner.state().objects[&attacker].damage_marked,
        2,
        "a source that stopped being a creature still deals its assignment"
    );
    assert!(!runner
        .state()
        .damage_dealt_this_turn
        .iter()
        .any(|record| record.source_id == attacker));
}

#[test]
fn queued_damage_uses_current_lifelink_but_frozen_power() {
    use engine::types::keywords::Keyword;
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let attacker = scenario.add_vanilla(P0, 3, 4);
    let blocker = scenario.add_vanilla(P1, 2, 8);
    let mut runner = scenario.build();
    advance_to_queued_damage(&mut runner, attacker, blocker);
    let source = runner.state_mut().objects.get_mut(&attacker).unwrap();
    source.keywords.push(Keyword::Lifelink);
    source.base_keywords.push(Keyword::Lifelink);
    source.power = Some(7);
    source.base_power = Some(7);
    runner.pass_both_players();
    assert_eq!(runner.state().objects[&blocker].damage_marked, 3);
    assert_eq!(runner.life(P0), 23);
}

#[test]
fn legacy_zero_and_negative_power_assign_zero_without_dealing_damage() {
    use engine::game::combat::{AttackTarget, AttackerInfo, CombatState};
    use engine::types::custom_format::CombatDamageTiming;
    for timing in [CombatDamageTiming::OnStack, CombatDamageTiming::Modern] {
        let mut scenario = GameScenario::new();
        scenario.at_phase(Phase::CombatDamage);
        let zero = scenario.add_vanilla(P0, 0, 1);
        let negative = scenario.add_vanilla(P0, -2, 1);
        scenario.add_enchantment_from_oracle(
            P0,
            "Coastal Piracy",
            "Whenever a creature you control deals combat damage to a player, you may draw a card.",
        );
        let mut runner = scenario.build();
        let mut rules = engine::types::custom_format::old_school_93_94().rules;
        rules.legality.legacy.damage_timing = timing;
        runner.state_mut().format_config =
            engine::types::format::FormatConfig::for_custom_rules(&rules);
        let mut combat = CombatState::default();
        for id in [zero, negative] {
            combat.attackers.push(AttackerInfo {
                object_id: id,
                defending_player: P1,
                attack_target: AttackTarget::Player(P1),
                blocked: false,
                band_id: None,
            });
        }
        runner.state_mut().combat = Some(combat);
        let mut events = Vec::new();
        engine::game::combat_damage::resolve_combat_damage(runner.state_mut(), &mut events);
        if timing == CombatDamageTiming::OnStack {
            assert_eq!(runner.state().stack.len(), 1);
            let StackEntryKind::CombatDamage { assignments, .. } = &runner.state().stack[0].kind
            else {
                panic!("missing damage object")
            };
            assert_eq!(assignments.len(), 2);
            for id in [zero, negative] {
                assert!(assignments
                    .iter()
                    .any(|a| a.source.object_id == id && a.amount == 0));
            }
            runner.pass_both_players();
            assert!(matches!(
                runner.state().waiting_for,
                WaitingFor::Priority { .. }
            ));
        }
        assert!(runner.state().stack.is_empty());
        assert!(runner.state().damage_dealt_this_turn.is_empty());
        assert_eq!(runner.life(P0), 20);
        assert_eq!(runner.life(P1), 20);
    }
}

#[test]
fn empty_first_strike_still_queues_an_object_and_a_regular_step() {
    use engine::game::combat::{AttackerInfo, CombatState};
    use engine::types::keywords::Keyword;
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::CombatDamage);
    let attacker = scenario
        .add_creature(P0, "First striker", 3, 4)
        .with_keyword(Keyword::FirstStrike)
        .id();
    let mut runner = scenario.build();
    let mut rules = engine::types::custom_format::old_school_93_94().rules;
    rules.legality.legacy.damage_timing = engine::types::custom_format::CombatDamageTiming::OnStack;
    runner.state_mut().format_config =
        engine::types::format::FormatConfig::for_custom_rules(&rules);
    let mut combat = CombatState::default();
    let mut info = AttackerInfo::attacking_player(attacker, P1);
    info.blocked = true;
    combat.attackers.push(info);
    runner.state_mut().combat = Some(combat);
    let mut events = Vec::new();
    engine::game::combat_damage::resolve_combat_damage(runner.state_mut(), &mut events);
    assert!(matches!(&runner.state().stack[0].kind,
        StackEntryKind::CombatDamage { sub_step: CombatDamageSubStep::FirstStrike, assignments } if assignments.is_empty()));
    runner.pass_both_players();
    assert!(runner.state().stack.is_empty());
    assert!(runner.state().combat.as_ref().unwrap().first_strike_done);
    assert!(matches!(
        runner.state().waiting_for,
        WaitingFor::Priority { .. }
    ));
    runner.pass_both_players();
    assert_eq!(runner.state().stack.len(), 1);
    assert!(matches!(&runner.state().stack[0].kind,
        StackEntryKind::CombatDamage { sub_step: CombatDamageSubStep::Regular, assignments } if assignments.is_empty()));
    runner.pass_both_players();
    assert_eq!(runner.life(P1), 20);
    assert!(runner.state().combat.as_ref().unwrap().regular_damage_done);
}

#[test]
fn queued_commander_damage_counts_after_a_zone_change() {
    use engine::types::keywords::Keyword;
    use engine::types::zones::Zone;
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let attacker = scenario
        .add_creature(P0, "Commander", 3, 4)
        .with_keyword(Keyword::Trample)
        .id();
    let blocker = scenario.add_vanilla(P1, 1, 1);
    let mut runner = scenario.build();
    runner
        .state_mut()
        .objects
        .get_mut(&attacker)
        .unwrap()
        .is_commander = true;
    advance_to_queued_damage(&mut runner, attacker, blocker);
    let mut events = Vec::new();
    engine::game::zones::move_to_zone(runner.state_mut(), attacker, Zone::Graveyard, &mut events);
    runner.pass_both_players();
    assert_eq!(runner.life(P1), 18);
    assert_eq!(runner.state().commander_damage.len(), 1);
    assert_eq!(runner.state().commander_damage[0].damage, 2);
}

#[test]
fn ending_turn_or_combat_clears_queued_damage_without_reassignment() {
    use engine::types::actions::GameAction;
    for text in ["End the turn.", "End the combat phase."] {
        let mut scenario = GameScenario::new();
        scenario.at_phase(Phase::PreCombatMain);
        let attacker = scenario.add_vanilla(P0, 3, 4);
        let blocker = scenario.add_vanilla(P1, 2, 4);
        let spell = scenario
            .add_spell_to_hand(P0, "End phase", true)
            .with_ability(if text == "End the turn." {
                Effect::EndTheTurn
            } else {
                Effect::EndCombatPhase
            })
            .with_mana_cost(engine::types::mana::ManaCost::zero())
            .id();
        let mut runner = scenario.build();
        advance_to_queued_damage(&mut runner, attacker, blocker);
        runner.cast(spell).commit();
        runner.act(GameAction::PassPriority).unwrap();
        runner.act(GameAction::PassPriority).unwrap();
        assert!(
            runner.state().stack.is_empty(),
            "{text}: stack {:?}, waiting {:?}",
            runner.state().stack,
            runner.state().waiting_for
        );
        assert!(runner.state().combat.is_none());
        assert!(runner.state().pending_combat_lifelink.is_none());
        assert_eq!(runner.state().objects[&attacker].damage_marked, 0);
        assert_eq!(runner.state().objects[&blocker].damage_marked, 0);
        for _ in 0..2 {
            if matches!(runner.state().waiting_for, WaitingFor::Priority { .. }) {
                runner.act(GameAction::PassPriority).unwrap();
            }
        }
        assert!(!runner
            .state()
            .stack
            .iter()
            .any(|e| matches!(e.kind, StackEntryKind::CombatDamage { .. })));
    }
}

#[test]
fn observer_triggers_once_for_a_sacrificed_combat_source() {
    use engine::types::keywords::Keyword;
    use engine::types::zones::Zone;
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    scenario.add_enchantment_from_oracle(
        P0,
        "Coastal Piracy",
        "Whenever a creature you control deals combat damage to a player, you may draw a card.",
    );
    let attacker = scenario
        .add_creature(P0, "Trampler", 3, 4)
        .with_keyword(Keyword::Trample)
        .id();
    let blocker = scenario.add_vanilla(P1, 1, 1);
    let mut runner = scenario.build();
    advance_to_queued_damage(&mut runner, attacker, blocker);
    let mut events = Vec::new();
    engine::game::zones::move_to_zone(runner.state_mut(), attacker, Zone::Graveyard, &mut events);
    // A changed current object must not supply the old damage source's type.
    let object = runner.state_mut().objects.get_mut(&attacker).unwrap();
    object.card_types.core_types.clear();
    object.base_card_types.core_types.clear();
    runner.pass_both_players();
    assert_eq!(runner.life(P1), 18);
    assert_eq!(
        runner.state().stack.len(),
        1,
        "the observer must see the sacrificed creature exactly once"
    );
    assert!(matches!(
        runner.state().stack[0].kind,
        StackEntryKind::TriggeredAbility { .. }
    ));
}

#[test]
fn protection_added_after_assignment_checks_a_sacrificed_sources_color() {
    use engine::types::keywords::{Keyword, ProtectionTarget};
    use engine::types::mana::ManaColor;
    use engine::types::zones::Zone;
    for color in [ManaColor::Red, ManaColor::Green] {
        let mut scenario = GameScenario::new();
        scenario.at_phase(Phase::PreCombatMain);
        let attacker = scenario
            .add_creature(P0, "Colored source", 3, 4)
            .with_color(vec![color])
            .id();
        let blocker = scenario.add_vanilla(P1, 2, 4);
        let mut runner = scenario.build();
        runner
            .state_mut()
            .objects
            .get_mut(&attacker)
            .unwrap()
            .is_token = true;
        advance_to_queued_damage(&mut runner, attacker, blocker);
        let mut events = Vec::new();
        engine::game::zones::move_to_zone(
            runner.state_mut(),
            attacker,
            Zone::Graveyard,
            &mut events,
        );
        let protection = Keyword::Protection(ProtectionTarget::Color(ManaColor::Red));
        let recipient = runner.state_mut().objects.get_mut(&blocker).unwrap();
        recipient.keywords.push(protection.clone());
        recipient.base_keywords.push(protection);
        runner.pass_both_players();
        assert_eq!(
            runner.state().objects[&blocker].damage_marked,
            if color == ManaColor::Red { 0 } else { 3 }
        );
    }
}

#[test]
fn eliminated_active_players_damage_object_survives_and_uses_lki() {
    use engine::game::combat::{AttackerInfo, CombatState};
    use engine::types::actions::GameAction;
    use engine::types::keywords::Keyword;
    let mut scenario = GameScenario::new_n_player(3, 42);
    scenario.at_phase(Phase::CombatDamage);
    let attacker = scenario
        .add_creature(P0, "Departing lifelinker", 3, 4)
        .with_keyword(Keyword::Lifelink)
        .id();
    let mut runner = scenario.build();
    let mut rules = engine::types::custom_format::old_school_93_94().rules;
    rules.legality.legacy.damage_timing = engine::types::custom_format::CombatDamageTiming::OnStack;
    runner.state_mut().format_config =
        engine::types::format::FormatConfig::for_custom_rules(&rules);
    let mut combat = CombatState::default();
    combat
        .attackers
        .push(AttackerInfo::attacking_player(attacker, P1));
    runner.state_mut().combat = Some(combat);
    let mut events = Vec::new();
    engine::game::combat_damage::resolve_combat_damage(runner.state_mut(), &mut events);
    runner.act(GameAction::Concede { player_id: P0 }).unwrap();
    assert_eq!(runner.state().stack.len(), 1);
    assert!(!runner
        .state()
        .objects
        .get(&attacker)
        .is_some_and(|object| object.zone == engine::types::zones::Zone::Battlefield));
    let life_before = runner.life(P0);
    for _ in 0..3 {
        if runner.state().stack.is_empty() {
            break;
        }
        runner.act(GameAction::PassPriority).unwrap();
    }
    assert!(runner.state().stack.is_empty());
    assert_eq!(runner.life(P1), 17);
    assert_eq!(
        runner.life(P0),
        life_before,
        "departed controller gains no life"
    );
    assert!(matches!(
        runner.state().waiting_for,
        WaitingFor::Priority { player: P1 }
    ));
}

#[test]
fn beginning_step_triggers_are_above_damage_and_ordering_does_not_requeue() {
    use engine::types::ability::{AbilityDefinition, AbilityKind, QuantityExpr, TriggerDefinition};
    use engine::types::actions::GameAction;
    use engine::types::triggers::TriggerMode;
    for (count, inert) in [(1, false), (2, false), (2, true)] {
        let mut scenario = GameScenario::new();
        scenario.at_phase(Phase::PreCombatMain);
        let attacker = scenario.add_vanilla(P0, 3, 4);
        let blocker = scenario.add_vanilla(P1, 2, 4);
        for i in 0..count {
            let trigger = TriggerDefinition::new(TriggerMode::Phase)
                .phase(Phase::CombatDamage)
                .execute(AbilityDefinition::new(
                    AbilityKind::Spell,
                    Effect::GainLife {
                        amount: QuantityExpr::Fixed {
                            value: i as i32 + 1,
                        },
                        player: TargetFilter::Controller,
                    },
                ));
            scenario
                .add_creature(P0, &format!("Step observer {i}"), 1, 4)
                .with_trigger_definition(trigger);
        }
        let mut runner = scenario.build();
        if count == 1 {
            let object = runner.state_mut().objects.get_mut(&attacker).unwrap();
            object
                .keywords
                .push(engine::types::keywords::Keyword::DoubleStrike);
            object
                .base_keywords
                .push(engine::types::keywords::Keyword::DoubleStrike);
        }
        if inert {
            // A step with no combatants is covered separately. Here effects
            // explicitly prohibit assignment from both remaining combatants.
            for id in [attacker, blocker] {
                runner
                    .state_mut()
                    .objects
                    .get_mut(&id)
                    .unwrap()
                    .assigns_no_combat_damage = true;
            }
        }
        advance_to_queued_damage(&mut runner, attacker, blocker);
        if count == 2 {
            assert!(matches!(
                runner.state().waiting_for,
                WaitingFor::OrderTriggers { .. }
            ));
            runner
                .act(GameAction::OrderTriggers { order: vec![0, 1] })
                .unwrap();
        }
        assert_eq!(runner.state().stack.len(), count + 1);
        assert!(matches!(
            runner.state().stack[0].kind,
            StackEntryKind::CombatDamage { .. }
        ));
        for _ in 0..count {
            runner.pass_both_players();
            assert_eq!(runner.state().objects[&blocker].damage_marked, 0);
        }
        assert_eq!(runner.life(P0), 20 + (count * (count + 1) / 2) as i32);
        assert_eq!(runner.state().stack.len(), 1);
        runner.pass_both_players();
        assert!(
            runner.state().stack.is_empty(),
            "marker must not fire again at resolution"
        );
        assert_eq!(runner.life(P0), 20 + (count * (count + 1) / 2) as i32);
        if count == 1 {
            runner.pass_both_players();
            assert_eq!(
                runner.state().stack.len(),
                1,
                "the regular sub-step must not duplicate the beginning-step marker"
            );
            assert!(matches!(
                runner.state().stack[0].kind,
                StackEntryKind::CombatDamage {
                    sub_step: CombatDamageSubStep::Regular,
                    ..
                }
            ));
            runner.pass_both_players();
            assert_eq!(runner.life(P0), 21);
        }
    }
}

#[test]
fn queued_damage_allows_a_real_bounce_response_and_still_deals_from_lki() {
    use engine::types::zones::Zone;
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let attacker = scenario.add_vanilla(P0, 3, 4);
    let blocker = scenario.add_vanilla(P1, 2, 4);
    let bounce = scenario
        .add_spell_to_hand_from_oracle(
            P0,
            "Test bounce",
            true,
            "Return target creature to its owner's hand.",
        )
        .with_mana_cost(engine::types::mana::ManaCost::zero())
        .id();
    let mut runner = scenario.build();
    advance_to_queued_damage(&mut runner, attacker, blocker);
    assert_eq!(runner.state().objects[&attacker].damage_marked, 0);
    assert_eq!(runner.state().objects[&blocker].damage_marked, 0);
    assert_eq!(runner.state().stack.len(), 1);
    assert!(matches!(
        runner.state().waiting_for,
        WaitingFor::Priority { player: P0 }
    ));
    runner.cast(bounce).target_object(attacker).commit();
    assert_eq!(runner.state().stack.len(), 2);
    runner.pass_both_players();
    assert_eq!(runner.state().objects[&attacker].zone, Zone::Hand);
    assert_eq!(runner.state().objects[&blocker].damage_marked, 0);
    assert_eq!(runner.state().stack.len(), 1);
    runner.pass_both_players();
    assert_eq!(runner.state().objects[&blocker].damage_marked, 3);
    assert_eq!(runner.state().objects[&attacker].damage_marked, 0);
    assert!(runner.state().stack.is_empty());
}

#[test]
fn queued_damage_uses_original_source_lki_and_skips_returned_recipient() {
    use engine::types::keywords::Keyword;
    use engine::types::zones::Zone;
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let attacker = scenario
        .add_creature(P0, "Lifelink attacker", 3, 4)
        .with_keyword(Keyword::Lifelink)
        .id();
    let blocker = scenario.add_vanilla(P1, 2, 4);
    let mut runner = scenario.build();
    advance_to_queued_damage(&mut runner, attacker, blocker);
    let original = runner.state().objects[&attacker].incarnation;
    let mut events = Vec::new();
    engine::game::zones::move_to_zone(runner.state_mut(), attacker, Zone::Hand, &mut events);
    engine::game::zones::move_to_zone(runner.state_mut(), attacker, Zone::Battlefield, &mut events);
    let returned = runner.state_mut().objects.get_mut(&attacker).unwrap();
    assert_ne!(returned.incarnation, original);
    returned.keywords.clear();
    returned.base_keywords.clear();
    returned.controller = P1;
    returned.power = Some(9);
    runner.pass_both_players();
    assert_eq!(
        runner.state().objects[&blocker].damage_marked,
        3,
        "amount is frozen"
    );
    assert_eq!(
        runner.state().objects[&attacker].damage_marked,
        0,
        "old recipient has left"
    );
    assert_eq!(
        runner.life(P0),
        23,
        "old source's lifelink controller is read from LKI"
    );
    assert_eq!(runner.life(P1), 20);
    let record = runner
        .state()
        .damage_dealt_this_turn
        .iter()
        .find(|record| record.source_id == attacker)
        .unwrap();
    assert_eq!(record.source_incarnation, Some(original));
    assert_eq!(record.source_power, Some(3));
    assert_eq!(record.source_controller_snapshot, P0);
    assert!(record.source_keywords.contains(&Keyword::Lifelink));
    assert!(
        !runner.state().objects_that_dealt_damage.contains(&attacker),
        "the returned creature did not deal the old incarnation's damage"
    );
}

#[test]
fn queued_player_damage_credits_old_incarnation_for_casting_permissions_and_monarch() {
    use engine::types::keywords::Keyword;
    use engine::types::zones::Zone;
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let attacker = scenario
        .add_creature(P0, "Assassin attacker", 3, 4)
        .with_subtypes(vec!["Assassin"])
        .with_keyword(Keyword::Trample)
        .id();
    let blocker = scenario.add_vanilla(P1, 1, 1);
    let mut runner = scenario.build();
    runner.state_mut().monarch = Some(P1);
    advance_to_queued_damage(&mut runner, attacker, blocker);
    let original = runner.state().objects[&attacker].incarnation;
    let mut events = Vec::new();
    engine::game::zones::move_to_zone(runner.state_mut(), attacker, Zone::Hand, &mut events);
    engine::game::zones::move_to_zone(runner.state_mut(), attacker, Zone::Battlefield, &mut events);
    let returned = runner.state_mut().objects.get_mut(&attacker).unwrap();
    assert_ne!(returned.incarnation, original);
    returned.controller = P1;
    returned.card_types.subtypes = vec!["Goblin".into()];
    returned.base_card_types.subtypes = vec!["Goblin".into()];
    runner.pass_both_players();
    assert_eq!(
        runner.life(P1),
        18,
        "queued trample damage reached the monarch"
    );
    assert!(runner
        .state()
        .assassin_or_commander_dealt_combat_damage_this_turn
        .contains(&P0));
    assert!(!runner
        .state()
        .assassin_or_commander_dealt_combat_damage_this_turn
        .contains(&P1));
    assert!(runner
        .state()
        .creature_types_dealt_combat_damage_this_turn
        .contains(&(P0, "Assassin".into())));
    assert!(!runner
        .state()
        .creature_types_dealt_combat_damage_this_turn
        .contains(&(P1, "Goblin".into())));
    assert_eq!(
        runner.state().stack.len(),
        1,
        "the old controller's monarch trigger was collected"
    );
    runner.pass_both_players();
    assert_eq!(runner.state().monarch, Some(P0));
}

#[test]
fn queued_player_damage_credits_old_incarnation_for_initiative() {
    use engine::types::keywords::Keyword;
    use engine::types::zones::Zone;
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let attacker = scenario
        .add_creature(P0, "Trampler", 3, 4)
        .with_keyword(Keyword::Trample)
        .id();
    let blocker = scenario.add_vanilla(P1, 1, 1);
    let mut runner = scenario.build();
    runner.state_mut().initiative = Some(P1);
    advance_to_queued_damage(&mut runner, attacker, blocker);
    let mut events = Vec::new();
    engine::game::zones::move_to_zone(runner.state_mut(), attacker, Zone::Hand, &mut events);
    engine::game::zones::move_to_zone(runner.state_mut(), attacker, Zone::Battlefield, &mut events);
    runner
        .state_mut()
        .objects
        .get_mut(&attacker)
        .unwrap()
        .controller = P1;
    runner.pass_both_players();
    assert_eq!(runner.life(P1), 18);
    assert_eq!(
        runner.state().stack.len(),
        1,
        "initiative is collected exactly once"
    );
    assert_eq!(runner.state().stack.back().unwrap().controller, P0);
    runner.pass_both_players();
    assert_eq!(runner.state().initiative, Some(P0));
}

#[test]
fn queued_lifelink_replacement_resumes_once_and_finishes_damage() {
    check_queued_lifelink_replacement(false);
}

#[test]
fn queued_lifelink_keeps_owed_gain_when_source_leaves_during_choice() {
    check_queued_lifelink_replacement(true);
}

fn check_queued_lifelink_replacement(source_leaves_during_choice: bool) {
    use engine::types::Zone;
    use engine::types::ability::{
        AbilityDefinition, AbilityKind, QuantityExpr, QuantityRef, ReplacementDefinition,
    };
    use engine::types::actions::GameAction;
    use engine::types::counter::CounterType;
    use engine::types::keywords::Keyword;
    use engine::types::replacements::ReplacementEvent;
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let attacker = scenario
        .add_creature(P0, "Lifelinker", 3, 4)
        .with_keyword(Keyword::Lifelink)
        .id();
    let blocker = scenario.add_vanilla(P1, 1, 4);
    let observer = scenario
        .add_creature_from_oracle(
            P0,
            "Ajani's Pridemate",
            2,
            2,
            "Whenever you gain life, put a +1/+1 counter on this creature.",
        )
        .id();
    for (name, amount) in [
        (
            "Double gain",
            QuantityExpr::Multiply {
                factor: 2,
                inner: Box::new(QuantityExpr::Ref {
                    qty: QuantityRef::EventContextAmount,
                }),
            },
        ),
        (
            "Extra gain",
            QuantityExpr::Offset {
                offset: 1,
                inner: Box::new(QuantityExpr::Ref {
                    qty: QuantityRef::EventContextAmount,
                }),
            },
        ),
    ] {
        scenario
            .add_creature(P0, name, 1, 5)
            .with_replacement_definition(
                ReplacementDefinition::new(ReplacementEvent::GainLife).execute(
                    AbilityDefinition::new(
                        AbilityKind::Spell,
                        Effect::GainLife {
                            amount,
                            player: TargetFilter::Controller,
                        },
                    ),
                ),
            );
    }
    let mut runner = scenario.build();
    advance_to_queued_damage(&mut runner, attacker, blocker);
    runner.pass_both_players();
    assert!(matches!(
        runner.state().waiting_for,
        WaitingFor::ReplacementChoice { .. }
    ));
    assert!(runner.state().pending_combat_lifelink.is_some());
    assert!(!runner.state().combat.as_ref().unwrap().regular_damage_done);
    assert_eq!(runner.life(P0), 20);
    assert_eq!(runner.state().objects[&blocker].damage_marked, 3);
    assert_eq!(runner.state().objects[&attacker].damage_marked, 1);
    assert!(runner.state().stack.is_empty());
    if source_leaves_during_choice {
        // The forced replacement window has no priority; simulate a zone-change
        // effect at this continuation boundary to test the parked obligation.
        let mut events = Vec::new();
        engine::game::zones::move_to_zone(runner.state_mut(), attacker, Zone::Hand, &mut events);
    }
    for _ in 0..4 {
        if !matches!(
            runner.state().waiting_for,
            WaitingFor::ReplacementChoice { .. }
        ) {
            break;
        }
        runner
            .act(GameAction::ChooseReplacement { index: 0 })
            .unwrap();
    }
    assert!(runner.state().pending_combat_lifelink.is_none());
    assert!(runner.state().combat.as_ref().unwrap().regular_damage_done);
    assert!(
        [27, 28].contains(&runner.life(P0)),
        "both legal replacement orders apply"
    );
    assert_eq!(
        runner.state().stack.len(),
        1,
        "one life gain produces one observer trigger"
    );
    runner.pass_both_players();
    assert_eq!(
        runner.state().objects[&observer]
            .counters
            .get(&CounterType::Plus1Plus1),
        Some(&1)
    );
    assert_eq!(runner.state().objects[&blocker].damage_marked, 3);
    if !source_leaves_during_choice {
        assert_eq!(runner.state().objects[&attacker].damage_marked, 1);
    }
    assert_eq!(
        runner.state().objects[&attacker].zone,
        if source_leaves_during_choice {
            Zone::Hand
        } else {
            Zone::Battlefield
        }
    );
    assert!(runner.state().stack.is_empty());
    assert!(runner
        .act(GameAction::ChooseReplacement { index: 0 })
        .is_err());
}

#[test]
fn stacked_double_strike_has_separate_assignment_and_priority_windows() {
    use engine::types::keywords::Keyword;
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let attacker = scenario
        .add_creature(P0, "Double strike attacker", 2, 6)
        .with_keyword(Keyword::DoubleStrike)
        .id();
    let blocker = scenario.add_vanilla(P1, 1, 6);
    let mut runner = scenario.build();
    advance_to_queued_damage(&mut runner, attacker, blocker);
    assert!(matches!(
        runner.state().stack.back().unwrap().kind,
        StackEntryKind::CombatDamage {
            sub_step: CombatDamageSubStep::FirstStrike,
            ..
        }
    ));
    runner.pass_both_players();
    assert_eq!(runner.state().objects[&blocker].damage_marked, 2);
    assert_eq!(runner.state().objects[&attacker].damage_marked, 0);
    assert!(runner.state().stack.is_empty());
    assert!(runner.state().combat.as_ref().unwrap().first_strike_done);
    assert!(!runner.state().combat.as_ref().unwrap().regular_damage_done);
    runner.pass_both_players();
    assert!(matches!(
        runner.state().stack.back().unwrap().kind,
        StackEntryKind::CombatDamage {
            sub_step: CombatDamageSubStep::Regular,
            ..
        }
    ));
    assert_eq!(runner.state().objects[&blocker].damage_marked, 2);
    runner.pass_both_players();
    assert_eq!(runner.state().objects[&blocker].damage_marked, 4);
    assert_eq!(runner.state().objects[&attacker].damage_marked, 1);
    assert!(runner.state().combat.as_ref().unwrap().regular_damage_done);
}

#[test]
fn first_strike_lifelink_choice_resumes_to_priority_before_regular_object() {
    use engine::types::ability::{
        AbilityDefinition, AbilityKind, QuantityExpr, QuantityRef, ReplacementDefinition,
    };
    use engine::types::actions::GameAction;
    use engine::types::keywords::Keyword;
    use engine::types::replacements::ReplacementEvent;
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let attacker = scenario
        .add_creature(P0, "First strike lifelinker", 3, 6)
        .with_keyword(Keyword::FirstStrike)
        .with_keyword(Keyword::Lifelink)
        .id();
    let blocker = scenario.add_vanilla(P1, 2, 6);
    for (name, amount) in [
        (
            "Double",
            QuantityExpr::Multiply {
                factor: 2,
                inner: Box::new(QuantityExpr::Ref {
                    qty: QuantityRef::EventContextAmount,
                }),
            },
        ),
        (
            "Extra",
            QuantityExpr::Offset {
                offset: 1,
                inner: Box::new(QuantityExpr::Ref {
                    qty: QuantityRef::EventContextAmount,
                }),
            },
        ),
    ] {
        scenario
            .add_creature(P0, name, 1, 5)
            .with_replacement_definition(
                ReplacementDefinition::new(ReplacementEvent::GainLife).execute(
                    AbilityDefinition::new(
                        AbilityKind::Spell,
                        Effect::GainLife {
                            amount,
                            player: TargetFilter::Controller,
                        },
                    ),
                ),
            );
    }
    let mut runner = scenario.build();
    advance_to_queued_damage(&mut runner, attacker, blocker);
    for _ in 0..2 {
        runner.act(GameAction::PassPriority).unwrap();
    }
    assert!(matches!(
        runner.state().waiting_for,
        WaitingFor::ReplacementChoice { .. }
    ));
    assert!(runner.state().pending_combat_lifelink.is_some());
    for _ in 0..4 {
        if !matches!(
            runner.state().waiting_for,
            WaitingFor::ReplacementChoice { .. }
        ) {
            break;
        }
        runner
            .act(GameAction::ChooseReplacement { index: 0 })
            .unwrap();
    }
    assert!(runner.state().pending_combat_lifelink.is_none());
    assert!(matches!(
        runner.state().waiting_for,
        WaitingFor::Priority { player: P0 }
    ));
    assert!(runner.state().stack.is_empty());
    assert!([27, 28].contains(&runner.life(P0)));
    assert_eq!(runner.state().objects[&attacker].damage_marked, 0);
    assert_eq!(runner.state().objects[&blocker].damage_marked, 3);
    for _ in 0..2 {
        runner.act(GameAction::PassPriority).unwrap();
    }
    assert!(matches!(
        runner.state().stack.back().unwrap().kind,
        StackEntryKind::CombatDamage {
            sub_step: CombatDamageSubStep::Regular,
            ..
        }
    ));
    for _ in 0..2 {
        runner.act(GameAction::PassPriority).unwrap();
    }
    assert_eq!(runner.state().objects[&attacker].damage_marked, 2);
    assert_eq!(runner.state().objects[&blocker].damage_marked, 3);
}

#[test]
fn combat_push_and_resolution_action_replay_reproduces_state_and_journal() {
    use engine::types::actions::GameAction;
    use engine::types::resolved_commands::ResolvedRulesCommand;
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let attacker = scenario.add_vanilla(P0, 3, 4);
    let blocker = scenario.add_vanilla(P1, 2, 4);
    let mut original = scenario.build();
    let mut replay = engine::game::scenario::GameRunner::from_state(original.state().clone());
    advance_to_queued_damage(&mut original, attacker, blocker);
    advance_to_queued_damage(&mut replay, attacker, blocker);
    let push = original
        .state()
        .resolved_rules_journal
        .entries()
        .iter()
        .find_map(|entry| match &entry.command {
            Some(ResolvedRulesCommand::StackPush(command))
                if matches!(command.entry.kind, StackEntryKind::CombatDamage { .. }) =>
            {
                Some(command)
            }
            _ => None,
        })
        .expect("combat push must be journaled");
    let encoded = serde_json::to_string(push).unwrap();
    let push = serde_json::from_str(&encoded).unwrap();
    replay.state_mut().stack.pop_back();
    engine::game::stack::apply_resolved_stack_push(replay.state_mut(), &push).unwrap();
    assert_eq!(
        serde_json::to_value(original.state()).unwrap(),
        serde_json::to_value(replay.state()).unwrap()
    );
    for _ in 0..2 {
        let recorded = serde_json::to_string(&GameAction::PassPriority).unwrap();
        original.act(GameAction::PassPriority).unwrap();
        replay
            .act(serde_json::from_str(&recorded).unwrap())
            .unwrap();
    }
    assert!(original.state().stack.is_empty());
    assert_eq!(original.state().objects[&blocker].damage_marked, 3);
    assert_eq!(
        serde_json::to_value(original.state()).unwrap(),
        serde_json::to_value(replay.state()).unwrap()
    );
}

#[test]
fn queued_recipient_removed_from_combat_still_receives_damage() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let attacker = scenario.add_vanilla(P0, 3, 4);
    let blocker = scenario.add_vanilla(P1, 2, 4);
    let mut runner = scenario.build();
    advance_to_queued_damage(&mut runner, attacker, blocker);
    engine::game::effects::remove_from_combat::remove_object_from_combat(
        runner.state_mut(),
        blocker,
    );
    runner.pass_both_players();
    assert_eq!(runner.state().objects[&blocker].damage_marked, 3);
    assert_eq!(runner.state().objects[&attacker].damage_marked, 2);
}

#[test]
fn eliminated_recipient_drops_only_their_assignments_and_priority_is_apnap() {
    use engine::game::combat::{AttackerInfo, CombatState};
    use engine::types::actions::GameAction;
    let p2 = PlayerId(2);
    let mut scenario = GameScenario::new_n_player(3, 42);
    scenario.at_phase(Phase::CombatDamage);
    let first = scenario.add_vanilla(P0, 3, 4);
    let second = scenario.add_vanilla(P0, 2, 4);
    let mut runner = scenario.build();
    let mut rules = engine::types::custom_format::old_school_93_94().rules;
    rules.legality.legacy.damage_timing = engine::types::custom_format::CombatDamageTiming::OnStack;
    runner.state_mut().format_config =
        engine::types::format::FormatConfig::for_custom_rules(&rules);
    let mut combat = CombatState::default();
    combat
        .attackers
        .push(AttackerInfo::attacking_player(first, P1));
    combat
        .attackers
        .push(AttackerInfo::attacking_player(second, p2));
    runner.state_mut().combat = Some(combat);
    engine::game::combat_damage::resolve_combat_damage(runner.state_mut(), &mut Vec::new());
    for player in [P0, P1, p2] {
        assert!(
            matches!(runner.state().waiting_for, WaitingFor::Priority { player: actual } if actual == player)
        );
        if player != p2 {
            runner.act(GameAction::PassPriority).unwrap();
        }
    }
    runner.act(GameAction::Concede { player_id: P1 }).unwrap();
    assert_eq!(runner.state().stack.len(), 1);
    let departed_life = runner.life(P1);
    for _ in 0..3 {
        if runner.state().stack.is_empty() {
            break;
        }
        runner.act(GameAction::PassPriority).unwrap();
    }
    assert!(runner.state().stack.is_empty());
    assert_eq!(runner.life(P1), departed_life);
    assert_eq!(runner.life(p2), 18);
    assert!(!runner
        .state()
        .damage_dealt_this_turn
        .iter()
        .any(|record| record.source_id == first));
}

#[test]
fn eliminated_controller_returns_stolen_combatant_for_live_damage() {
    use engine::types::ability::{ContinuousModification, Duration};
    use engine::types::actions::GameAction;
    use engine::types::keywords::Keyword;
    let mut scenario = GameScenario::new_n_player(3, 42);
    scenario.at_phase(Phase::PreCombatMain);
    let attacker = scenario
        .add_creature(PlayerId(2), "Stolen lifelinker", 3, 6)
        .with_keyword(Keyword::Lifelink)
        .id();
    let blocker = scenario.add_vanilla(P1, 2, 6);
    let thief = scenario.add_vanilla(P0, 1, 1);
    let mut runner = scenario.build();
    runner.state_mut().add_transient_continuous_effect(
        thief,
        P0,
        Duration::Permanent,
        TargetFilter::SpecificObject { id: attacker },
        vec![ContinuousModification::ChangeController],
        None,
    );
    engine::game::layers::evaluate_layers(runner.state_mut());
    assert_eq!(runner.state().objects[&attacker].controller, P0);
    let mut combat = engine::game::combat::CombatState::default();
    let mut attacking = engine::game::combat::AttackerInfo::attacking_player(attacker, P1);
    attacking.blocked = true;
    combat.attackers.push(attacking);
    combat.blocker_assignments.insert(attacker, vec![blocker]);
    combat.blocker_to_attacker.insert(blocker, vec![attacker]);
    runner.state_mut().combat = Some(combat);
    runner.state_mut().phase = Phase::CombatDamage;
    let mut rules = engine::types::custom_format::old_school_93_94().rules;
    rules.legality.legacy.damage_timing = engine::types::custom_format::CombatDamageTiming::OnStack;
    runner.state_mut().format_config =
        engine::types::format::FormatConfig::for_custom_rules(&rules);
    let source = ObjectIncarnationRef::from_object(&runner.state().objects[&attacker]);
    engine::game::combat_damage::resolve_combat_damage(runner.state_mut(), &mut Vec::new());
    assert_eq!(runner.state().stack.len(), 1);
    runner.act(GameAction::Concede { player_id: P0 }).unwrap();
    assert_eq!(runner.state().objects[&attacker].controller, PlayerId(2));
    assert_eq!(
        runner.state().objects[&attacker].incarnation,
        source.incarnation
    );
    for _ in 0..3 {
        if runner.state().stack.is_empty() {
            break;
        }
        runner.act(GameAction::PassPriority).unwrap();
    }
    assert!(runner.state().stack.is_empty());
    assert_eq!(runner.state().objects[&blocker].damage_marked, 3);
    assert_eq!(runner.state().objects[&attacker].damage_marked, 2);
    assert_eq!(runner.life(PlayerId(2)), 23);
    assert!(matches!(
        runner.state().waiting_for,
        WaitingFor::Priority { player: P1 }
    ));
}

#[test]
fn queued_damage_uses_first_incarnation_after_two_departures() {
    use engine::types::keywords::Keyword;
    use engine::types::zones::Zone;
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let attacker = scenario
        .add_creature(P0, "Original lifelinker", 3, 4)
        .with_keyword(Keyword::Lifelink)
        .with_keyword(Keyword::Deathtouch)
        .id();
    let blocker = scenario.add_vanilla(P1, 1, 8);
    let mut runner = scenario.build();
    advance_to_queued_damage(&mut runner, attacker, blocker);
    let original = runner.state().objects[&attacker].incarnation;
    let mut events = Vec::new();
    for zone in [Zone::Hand, Zone::Battlefield] {
        engine::game::zones::move_to_zone(runner.state_mut(), attacker, zone, &mut events);
    }
    let returned = runner.state_mut().objects.get_mut(&attacker).unwrap();
    returned.keywords.clear();
    returned.base_keywords.clear();
    returned.controller = P1;
    engine::game::zones::move_to_zone(runner.state_mut(), attacker, Zone::Graveyard, &mut events);
    runner.pass_both_players();
    assert_eq!(runner.life(P0), 23);
    assert_eq!(runner.life(P1), 20);
    assert_eq!(runner.state().objects[&blocker].zone, Zone::Graveyard);
    let record = runner
        .state()
        .damage_dealt_this_turn
        .iter()
        .find(|record| record.source_id == attacker)
        .unwrap();
    assert_eq!(record.source_incarnation, Some(original));
}

#[test]
fn stacked_strike_eligibility_uses_initial_participants_and_current_double_strike() {
    use engine::types::actions::GameAction;
    use engine::types::keywords::Keyword;
    for (initial, current, expected) in [
        (Some(Keyword::FirstStrike), None, 2),
        (Some(Keyword::DoubleStrike), None, 2),
        (Some(Keyword::FirstStrike), Some(Keyword::DoubleStrike), 4),
        (None, Some(Keyword::FirstStrike), 2),
    ] {
        let mut scenario = GameScenario::new();
        scenario.at_phase(Phase::PreCombatMain);
        let attacker = scenario.add_vanilla(P0, 2, 8);
        let blocker = scenario
            .add_creature(P1, "First striker", 1, 8)
            .with_keyword(Keyword::FirstStrike)
            .id();
        let mut runner = scenario.build();
        if let Some(keyword) = initial {
            let object = runner.state_mut().objects.get_mut(&attacker).unwrap();
            object.keywords.push(keyword.clone());
            object.base_keywords.push(keyword);
        }
        advance_to_queued_damage(&mut runner, attacker, blocker);
        for _ in 0..2 {
            runner.act(GameAction::PassPriority).unwrap();
        }
        let object = runner.state_mut().objects.get_mut(&attacker).unwrap();
        object.keywords.clear();
        object.base_keywords.clear();
        if let Some(keyword) = current {
            object.keywords.push(keyword.clone());
            object.base_keywords.push(keyword);
        }
        for _ in 0..2 {
            runner.act(GameAction::PassPriority).unwrap();
        }
        assert!(matches!(
            runner.state().stack.back().unwrap().kind,
            StackEntryKind::CombatDamage {
                sub_step: CombatDamageSubStep::Regular,
                ..
            }
        ));
        for _ in 0..2 {
            runner.act(GameAction::PassPriority).unwrap();
        }
        assert_eq!(runner.state().objects[&blocker].damage_marked, expected);
        assert_eq!(runner.state().objects[&attacker].damage_marked, 1);
    }
}

/// Verbatim Oracle text (Scryfall, 2026-09-16).
const COUNTERSPELL: &str = "Counter target spell.";
const STIFLE: &str =
    "Counter target activated or triggered ability. (Mana abilities can't be targeted.)";

/// A combat-damage entry carrying one assignment, as the pushing phase will
/// build it: source and object recipient pinned to a CR 400.7 incarnation.
fn combat_damage_entry(
    entry_id: ObjectId,
    sub_step: CombatDamageSubStep,
    source: ObjectId,
    recipient: ObjectId,
    amount: u32,
) -> StackEntry {
    StackEntry {
        id: entry_id,
        source_id: entry_id,
        controller: P0,
        kind: StackEntryKind::CombatDamage {
            sub_step,
            assignments: vec![AssignedCombatDamage {
                source: ObjectIncarnationRef::of(source, 0),
                target: AssignedDamageRecipient::Object(ObjectIncarnationRef::of(recipient, 0)),
                amount,
            }],
        },
    }
}

/// A triggered-ability entry, used throughout as the paired positive control:
/// it is an ability, so every assertion that refuses a combat-damage entry has
/// something in the same run that it must still accept.
fn triggered_entry(entry_id: ObjectId, source: ObjectId, controller: PlayerId) -> StackEntry {
    let ability = ResolvedAbility::new(
        Effect::Destroy {
            target: TargetFilter::Typed(TypedFilter::creature()),
            cant_regenerate: false,
        },
        vec![],
        source,
        controller,
    );
    StackEntry {
        id: entry_id,
        source_id: source,
        controller,
        kind: StackEntryKind::TriggeredAbility {
            source_id: source,
            ability: Box::new(ability),
            condition: None,
            trigger_event: None,
            description: Some("Whenever this creature attacks, destroy target creature.".into()),
            source_name: "Control Trigger".into(),
            subject_match_count: None,
            die_result: None,
            provenance: None,
        },
    }
}

/// An activated-ability entry — the second ability kind, so the ability row's
/// reach guard covers both halves of what a kindless counter accepts.
fn activated_entry(entry_id: ObjectId, source: ObjectId, controller: PlayerId) -> StackEntry {
    let ability = ResolvedAbility::new(
        Effect::Destroy {
            target: TargetFilter::Typed(TypedFilter::creature()),
            cant_regenerate: false,
        },
        vec![],
        source,
        controller,
    );
    StackEntry {
        id: entry_id,
        source_id: source,
        controller,
        kind: StackEntryKind::ActivatedAbility {
            source_id: source,
            ability: Box::new(ability),
        },
    }
}

/// Row 1a — no ABILITY-filter effect can target it.
///
/// This row is decided by the classification: `add_stack_abilities` pushes an
/// entry as a candidate with no `state.objects` lookup at all, so
/// `matches_stack_ability_kind` — i.e. `class()` — is the only gate. Reverting
/// `class()` to report an ability makes Squelch list the entry and this test
/// fails.
#[test]
fn ability_filters_cannot_target_a_combat_damage_entry() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let attacker = scenario.add_vanilla(P1, 2, 2);
    let blocker = scenario.add_vanilla(P0, 2, 2);
    let stifle = scenario
        .add_spell_to_hand_from_oracle(P0, "Stifle", true, STIFLE)
        .with_mana_cost(engine::types::mana::ManaCost::zero())
        .id();

    let mut runner = scenario.build();
    let damage_id = ObjectId(9_001);
    let triggered_id = ObjectId(9_002);
    let activated_id = ObjectId(9_003);
    runner.state_mut().stack.push_back(combat_damage_entry(
        damage_id,
        CombatDamageSubStep::Regular,
        attacker,
        blocker,
        2,
    ));
    // TWO ability controls, deliberately: a kindless counter accepts both, so
    // the legal set is guaranteed to hold more than one candidate and target
    // selection must be raised rather than auto-assigned.
    runner
        .state_mut()
        .stack
        .push_back(triggered_entry(triggered_id, blocker, P1));
    runner
        .state_mut()
        .stack
        .push_back(activated_entry(activated_id, blocker, P1));

    let card_id = runner.state().objects[&stifle].card_id;
    let result = runner.act(engine::types::actions::GameAction::CastSpell {
        object_id: stifle,
        card_id,
        targets: vec![],
        payment_mode: engine::types::game_state::CastPaymentMode::Auto,
    });
    assert!(result.is_ok(), "Stifle must reach target selection");

    let legal = legal_targets_of(runner.state());
    // Reach guards: the ability enumeration ran and offered BOTH ability kinds.
    assert!(
        legal.contains(&engine::types::ability::TargetRef::Object(triggered_id)),
        "reach guard: the triggered-ability control must be offered"
    );
    assert!(
        legal.contains(&engine::types::ability::TargetRef::Object(activated_id)),
        "reach guard: the activated-ability control must be offered"
    );
    assert!(
        !legal.contains(&engine::types::ability::TargetRef::Object(damage_id)),
        "combat damage on the stack is not an ability and must not be targetable"
    );
}

/// Row 1b — no SPELL-filter effect can target it.
///
/// Deliberately claims **no** mutation. The exclusion here is structural rather
/// than class-decided: `stack_spell_entry_matches_filter` opens with a
/// `matches!(entry.kind, Spell { .. })` guard, and below it both spell paths
/// drop any entry with no `GameObject` — which a combat-damage entry never has.
/// Widening the kind guard therefore would not make the entry appear, so this
/// row proves the instrument fired instead: the control spell is offered, and
/// the entry was in `state.stack` for the enumeration that produced that offer.
#[test]
fn spell_filters_cannot_target_a_combat_damage_entry() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let attacker = scenario.add_vanilla(P1, 2, 2);
    let blocker = scenario.add_vanilla(P0, 2, 2);
    let counterspell = scenario
        .add_spell_to_hand_from_oracle(P0, "Counterspell", true, COUNTERSPELL)
        .with_mana_cost(engine::types::mana::ManaCost::zero())
        .id();
    let victim = scenario
        .add_spell_to_hand_from_oracle(P1, "Counterspell", true, COUNTERSPELL)
        .with_mana_cost(engine::types::mana::ManaCost::zero())
        .id();
    // A second victim, for the same reason as the ability row: two candidates
    // force target selection instead of a lone auto-assigned target.
    let victim2 = scenario
        .add_spell_to_hand_from_oracle(P1, "Counterspell", true, COUNTERSPELL)
        .with_mana_cost(engine::types::mana::ManaCost::zero())
        .id();

    let mut runner = scenario.build();
    let damage_id = ObjectId(9_101);
    runner.state_mut().stack.push_back(combat_damage_entry(
        damage_id,
        CombatDamageSubStep::Regular,
        attacker,
        blocker,
        2,
    ));
    // A real spell on the stack, as the control this row's negative is paired
    // with. Built through the object map so it has the `GameObject` a spell
    // entry is expected to have.
    for spell in [victim, victim2] {
        let spell_card = runner.state().objects[&spell].card_id;
        runner.state_mut().objects.get_mut(&spell).unwrap().zone =
            engine::types::zones::Zone::Stack;
        runner.state_mut().stack.push_back(StackEntry {
            id: spell,
            source_id: spell,
            controller: P1,
            kind: StackEntryKind::Spell {
                card_id: spell_card,
                ability: None,
                casting_variant: Default::default(),
                actual_mana_spent: 0,
            },
        });
    }

    let card_id = runner.state().objects[&counterspell].card_id;
    let result = runner.act(engine::types::actions::GameAction::CastSpell {
        object_id: counterspell,
        card_id,
        targets: vec![],
        payment_mode: engine::types::game_state::CastPaymentMode::Auto,
    });
    assert!(result.is_ok(), "Counterspell must reach target selection");

    // Reach guard, read off the state the enumeration consumed: the entry was
    // on the stack when the legal set was produced.
    assert!(
        runner
            .state()
            .stack
            .iter()
            .any(|entry| entry.id == damage_id),
        "reach guard: the combat-damage entry was on the stack for this enumeration"
    );
    let legal = legal_targets_of(runner.state());
    assert!(
        legal.contains(&engine::types::ability::TargetRef::Object(victim)),
        "reach guard: the spell enumeration ran and offered the control spell"
    );
    assert!(
        !legal.contains(&engine::types::ability::TargetRef::Object(damage_id)),
        "combat damage on the stack is not a spell and must not be targetable"
    );
}

/// Every legal target the engine is currently offering, flattened across slots.
fn legal_targets_of(state: &GameState) -> Vec<engine::types::ability::TargetRef> {
    match &state.waiting_for {
        WaitingFor::TargetSelection { target_slots, .. } => target_slots
            .iter()
            .flat_map(|slot| slot.legal_targets.iter().cloned())
            .collect(),
        other => panic!("expected target selection, got {other:?}"),
    }
}

/// Row 2 — the new state serializes, and a legacy incarnation payload still
/// loads through the existing compat shim.
#[test]
fn combat_damage_entry_round_trips_and_accepts_legacy_incarnations() {
    let entry = StackEntry {
        id: ObjectId(11),
        source_id: ObjectId(11),
        controller: P0,
        kind: StackEntryKind::CombatDamage {
            sub_step: CombatDamageSubStep::FirstStrike,
            assignments: vec![
                AssignedCombatDamage {
                    source: ObjectIncarnationRef::of(ObjectId(3), 7),
                    target: AssignedDamageRecipient::Object(ObjectIncarnationRef::of(
                        ObjectId(4),
                        2,
                    )),
                    amount: 3,
                },
                AssignedCombatDamage {
                    source: ObjectIncarnationRef::of(ObjectId(5), 1),
                    target: AssignedDamageRecipient::Player(P1),
                    amount: 2,
                },
            ],
        },
    };

    let json = serde_json::to_string(&entry).expect("serializes");
    let back: StackEntry = serde_json::from_str(&json).expect("round-trips");
    assert_eq!(back, entry, "both recipient arms must survive a round trip");

    // A legacy payload stored a bare `ObjectId` where the incarnation pair now
    // lives; `ObjectIncarnationRefCompat` is what keeps it loading.
    let legacy = json.replace(r#"{"object_id":3,"incarnation":7}"#, "3");
    assert_ne!(
        legacy, json,
        "the legacy rewrite must actually change the payload"
    );
    let from_legacy: StackEntry =
        serde_json::from_str(&legacy).expect("a legacy bare-ObjectId source still deserializes");
    assert!(matches!(
        from_legacy.kind,
        StackEntryKind::CombatDamage { .. }
    ));
}

/// Row 3 — the engine owns the label; the client renders `kind_label` and
/// derives nothing.
#[test]
fn engine_supplies_the_combat_damage_label_for_both_sub_steps() {
    let mut runner = GameScenario::new().build();
    let first = ObjectId(9_201);
    let regular = ObjectId(9_202);
    let control = ObjectId(9_203);
    let source = ObjectId(1);
    runner.state_mut().stack.push_back(combat_damage_entry(
        first,
        CombatDamageSubStep::FirstStrike,
        source,
        source,
        1,
    ));
    runner.state_mut().stack.push_back(combat_damage_entry(
        regular,
        CombatDamageSubStep::Regular,
        source,
        source,
        1,
    ));
    runner
        .state_mut()
        .stack
        .push_back(triggered_entry(control, source, P0));

    let views = derive_views(runner.state(), Some(P0));
    let label = |id: ObjectId| views.stack_entry_details[&id].kind_label.clone();
    assert_eq!(label(first), "Combat damage — first strike");
    assert_eq!(label(regular), "Combat damage");
    // Sibling control: the existing labels are untouched.
    assert_eq!(label(control), "Triggered ability");

    let detail = &views.stack_entry_details[&regular];
    assert!(detail.provenance.is_none(), "not a synthesized trigger");
    assert!(
        detail.targets.is_empty(),
        "assignment lines land with the pushing phase"
    );
}

/// Row 4 — combat-damage entries never coalesce in the stack display, while
/// genuinely identical triggers still do.
#[test]
fn combat_damage_entries_never_coalesce_but_identical_triggers_still_do() {
    let mut runner = GameScenario::new().build();
    let source = ObjectId(1);
    let a = ObjectId(9_301);
    let b = ObjectId(9_302);
    for id in [a, b] {
        runner.state_mut().stack.push_back(combat_damage_entry(
            id,
            CombatDamageSubStep::Regular,
            source,
            source,
            1,
        ));
    }
    let t1 = ObjectId(9_303);
    let t2 = ObjectId(9_304);
    for id in [t1, t2] {
        runner
            .state_mut()
            .stack
            .push_back(triggered_entry(id, source, P0));
    }

    let views = derive_views(runner.state(), Some(P0));
    let damage_groups = views
        .stack_display_groups
        .iter()
        .filter(|group| group.member_ids.iter().any(|id| *id == a || *id == b))
        .count();
    assert_eq!(
        damage_groups, 2,
        "each combat damage step is its own object and must not be coalesced"
    );
    // Positive control: grouping still works for the entries it is meant for.
    let trigger_group = views
        .stack_display_groups
        .iter()
        .find(|group| group.member_ids.contains(&t1))
        .expect("the identical triggers form a group");
    assert_eq!(
        trigger_group.count, 2,
        "identical triggers must still coalesce, or this test proves nothing"
    );
}

/// Row 5 — the resolution fence captures the new kind rather than losing it.
#[test]
fn the_resolution_fence_captures_a_combat_damage_entry() {
    let entry = combat_damage_entry(
        ObjectId(21),
        CombatDamageSubStep::FirstStrike,
        ObjectId(3),
        ObjectId(4),
        2,
    );
    let fence = StackResolutionEntryFence::capture(&entry);
    let json = serde_json::to_string(&fence).expect("the fence serializes");
    assert!(
        json.contains("CombatDamage"),
        "the fence must record the kind, not erase it: {json}"
    );

    // Sibling control: a keyword-action-free ability entry still captures too.
    let control = triggered_entry(ObjectId(22), ObjectId(3), P0);
    let control_fence = StackResolutionEntryFence::capture(&control);
    assert_eq!(control_fence.entry_id, ObjectId(22));
}

/// Row 6 — a player can never pre-yield priority to combat damage (CR 117.3d).
///
/// Routed through the auto-pass recommendation, which consults
/// `is_priority_yielded` with **no** controller conjunct — unlike the session
/// gate, which short-circuits on `top.controller != player` and would make this
/// row decided by the entry's seat. The two boards below differ only in the top
/// stack entry.
#[test]
fn combat_damage_is_never_priority_yielded() {
    // NOTE: `combat_damage_entry` sets `source_id` to the ENTRY id (a
    // combat-damage object has no single source permanent), so the yield must
    // be keyed on THAT id for the two entries to be compared on equal footing.
    // An earlier revision claimed the two "shared a source" while keying the
    // yield on the control's source — which made the negative pass for the
    // wrong reason.
    let entry = combat_damage_entry(
        ObjectId(31),
        CombatDamageSubStep::Regular,
        ObjectId(3),
        ObjectId(4),
        2,
    );
    // The control's source is the entry's OWN source id, so one stored yield
    // genuinely covers both.
    let control = triggered_entry(ObjectId(32), ObjectId(31), P0);

    let mut runner = GameScenario::new().build();
    let state = runner.state_mut();
    // Store a real yield keyed to the shared source, so the control below can
    // actually be yielded. Without this the negative passes vacuously — an
    // empty yield list refuses everything.
    state.priority_yields.push(PriorityYield {
        player: P0,
        target: YieldTarget::ThisObject {
            source_id: ObjectId(31),
            incarnation: None,
            trigger_description: None,
        },
    });

    // Positive control FIRST: the instrument fires — an ability from that
    // source is yielded under exactly this stored yield.
    assert!(
        state.is_priority_yielded(P0, &control),
        "reach guard: the stored yield must match the control ability"
    );
    // The combat-damage entry shares that source and that seat, and is still
    // never yielded, because it is not an ability (CR 117.3d).
    assert!(
        !state.is_priority_yielded(P0, &entry),
        "combat damage is not an ability and can never be yielded"
    );
}

/// An untargeted copy effect must not duplicate a combat-damage entry.
///
/// CR 707.10 copies a spell or ability, and combat damage is neither. The
/// dangerous route is the UNTARGETED one: with no target in the ability,
/// `copy_source_entry` falls through to `state.stack.last()` without ever
/// consulting a target filter — so rows 1a/1b's targeting-legality argument
/// does not cover it. `stack_entry_cant_be_copied` is what refuses it, and
/// `copy_spell::resolve` returns `Ok` having copied nothing.
///
/// REVERT PROBE: delete the `CombatDamage` arm at the head of
/// `stack_entry_cant_be_copied` and this test reds — the fallback picks the
/// top-of-stack combat-damage entry and pushes a duplicate.
#[test]
fn an_untargeted_copy_cannot_duplicate_a_combat_damage_entry() {
    let mut scenario = GameScenario::new();
    scenario.at_phase(Phase::PreCombatMain);
    let source = scenario.add_vanilla(P0, 2, 2);
    let mut runner = scenario.build();

    let damage_id = ObjectId(9_501);
    runner.state_mut().stack.push_back(combat_damage_entry(
        damage_id,
        CombatDamageSubStep::Regular,
        source,
        source,
        2,
    ));

    // Reach guard: the untargeted fallback reads `state.stack.last()`, so the
    // combat-damage entry must genuinely be the top of the stack.
    assert!(
        matches!(
            runner.state().stack.back().map(|entry| &entry.kind),
            Some(StackEntryKind::CombatDamage { .. })
        ),
        "reach guard: the fallback's `state.stack.last()` is the combat-damage entry"
    );
    let before = runner.state().stack.len();

    // No targets — this is the untargeted form the gate has to catch.
    let copy_ability = ResolvedAbility::new(
        Effect::CopySpell {
            target: TargetFilter::Any,
            retarget: CopyRetargetPermission::KeepOriginalTargets,
            copier: None,
            additional_modifications: Vec::new(),
            starting_loyalty_from_casualty_sacrifice: false,
        },
        vec![],
        ObjectId(9_502),
        P0,
    );
    let mut events = Vec::new();
    copy_spell::resolve(runner.state_mut(), &copy_ability, &mut events)
        .expect("CR 707.10: the copy effect resolves, having copied nothing");

    assert_eq!(
        runner.state().stack.len(),
        before,
        "no duplicate combat-damage entry may be pushed"
    );

    // Positive control, same call shape on the same board: an ordinary ability
    // entry on top IS copied, so the refusal above is about this kind and not
    // about `resolve` refusing everything.
    runner
        .state_mut()
        .stack
        .push_back(triggered_entry(ObjectId(9_503), source, P0));
    let before_control = runner.state().stack.len();
    let mut control_events = Vec::new();
    copy_spell::resolve(runner.state_mut(), &copy_ability, &mut control_events)
        .expect("copying an ordinary ability entry must succeed");
    assert_eq!(
        runner.state().stack.len(),
        before_control + 1,
        "reach guard: an ordinary ability entry is still copyable"
    );
}

/// A persisted state carrying combat damage must be REFUSED at admission.
///
/// Decoding the type and admitting it as a playable game are separate
/// contracts. The variant is serde-decodable so later phases can round-trip
/// it, but no phase before the pushing one can deal its assignments — so a
/// restored state holding one has pending damage this build cannot resolve.
/// Admitting it would either crash resolution or silently drop that damage.
/// `prepare_for_restore` is the single chokepoint every production restore
/// funnels through (WASM `prepare_restored_game_state`, `server-core`'s
/// `from_persisted`, offline tooling), so the refusal lives there.
///
/// Covers BOTH persisted forms with a populated payload, and pairs each with a
/// supported-entry positive control proving ordinary states still restore.
///
/// REVERT PROBE: delete the `UnsupportedStackObject` guard in
/// `prepare_for_restore` and both rejection arms below fail.
#[test]
fn persisted_admission_refuses_combat_damage_and_still_admits_ordinary_states() {
    // A populated payload: a real assignment, not an empty entry.
    let seated = |runner: &mut engine::game::scenario::GameRunner| {
        runner.state_mut().stack.push_back(combat_damage_entry(
            ObjectId(9_601),
            CombatDamageSubStep::Regular,
            ObjectId(1),
            ObjectId(2),
            3,
        ));
    };

    for raw in [true, false] {
        let form = if raw { "Raw" } else { "Trusted" };

        // Positive control FIRST: the same form, same board, carrying an
        // ORDINARY stack entry, must still restore. Without this the rejection
        // below could pass because the boundary refuses everything.
        //
        // An ACTIVATED ability deliberately, not a triggered one. Decoding runs
        // `validate_trigger_firing_coherence`, which demands that every
        // `TriggeredAbility` stack entry carry a matching row in
        // `stack_trigger_firings` — a `pub(crate)` field an integration test
        // cannot seat. A triggered control therefore fails to DECODE and never
        // reaches the admission boundary this test exists to guard. That loop
        // skips every non-triggered kind, so an activated entry is an ordinary
        // supported object with one fewer precondition.
        let mut ok_runner = GameScenario::new().build();
        ok_runner
            .state_mut()
            .stack
            .push_back(activated_entry(ObjectId(9_602), ObjectId(1), P0));
        let ok_state = ok_runner.state().clone();
        let ok_persisted = if raw {
            PersistedGameState::Raw(Box::new(ok_state))
        } else {
            PersistedGameState::capture(ok_state)
        };
        let ok_json = serde_json::to_string(&ok_persisted).expect("control serializes");
        let ok_decoded: PersistedGameState =
            serde_json::from_str(&ok_json).expect("control decodes");
        assert!(
            ok_decoded.into_game_state().is_ok(),
            "reach guard ({form}): an ordinary stack entry must still be admitted"
        );

        // The combat-damage payload must be refused by the same boundary.
        let mut bad_runner = GameScenario::new().build();
        seated(&mut bad_runner);
        let bad_state = bad_runner.state().clone();
        let bad_persisted = if raw {
            PersistedGameState::Raw(Box::new(bad_state))
        } else {
            PersistedGameState::capture(bad_state)
        };
        let bad_json = serde_json::to_string(&bad_persisted).expect("payload serializes");
        let bad_decoded: PersistedGameState =
            serde_json::from_str(&bad_json).expect("payload still DECODES — that contract is kept");
        match bad_decoded.into_game_state() {
            Err(PersistedRestoreError::UnsupportedStackObject(_)) => {}
            Err(other) => panic!("({form}) refused for the wrong reason: {other:?}"),
            Ok(_) => panic!("({form}) a state carrying unresolvable combat damage was admitted"),
        }
    }
}

/// Persisted admission also owns the entry already popped for resolution.
/// A non-Priority prompt keeps terminal-rest recovery from masking this gate.
#[test]
fn persisted_admission_refuses_paused_combat_damage_carriers() {
    for raw in [true, false] {
        for unsupported in [false, true] {
            let mut runner = GameScenario::new().build();
            let entry = if unsupported {
                combat_damage_entry(
                    ObjectId(9_603),
                    CombatDamageSubStep::Regular,
                    ObjectId(1),
                    ObjectId(2),
                    3,
                )
            } else {
                activated_entry(ObjectId(9_604), ObjectId(1), P0)
            };
            runner.state_mut().resolving_stack_entry = Some(entry);
            // Synthetic persisted boundary fixture: both payload kinds use the
            // same paused prompt; this does not claim a Phase 3c damage resolver.
            runner.state_mut().waiting_for = WaitingFor::ScryChoice {
                player: P0,
                cards: vec![],
            };
            let state = runner.state().clone();
            let persisted = if raw {
                PersistedGameState::Raw(Box::new(state))
            } else {
                PersistedGameState::capture(state)
            };
            let json = serde_json::to_string(&persisted).expect("carrier serializes");
            let decoded: PersistedGameState = serde_json::from_str(&json).expect("carrier decodes");
            let restored = decoded.into_game_state();
            if unsupported {
                assert!(
                    matches!(
                        restored,
                        Err(PersistedRestoreError::UnsupportedStackObject(_))
                    ),
                    "raw={raw}: reject the unsupported resolving carrier at admission"
                );
            } else {
                let restored = restored.expect("ordinary paused carrier still restores");
                assert!(restored.stack.is_empty());
                assert!(matches!(
                    restored.waiting_for,
                    WaitingFor::ScryChoice { .. }
                ));
                assert!(matches!(
                    restored
                        .resolving_stack_entry
                        .as_ref()
                        .map(|entry| &entry.kind),
                    Some(StackEntryKind::ActivatedAbility { .. })
                ));
            }
        }
    }
}

/// Row 7 — the storm count ignores a combat-damage entry rather than treating
/// it as a shadowing stack object.
#[test]
fn storm_count_ignores_a_combat_damage_entry() {
    let mut runner = GameScenario::new().build();
    let source = ObjectId(1);
    let damage = ObjectId(9_401);
    runner.state_mut().stack.push_back(combat_damage_entry(
        damage,
        CombatDamageSubStep::Regular,
        source,
        source,
        1,
    ));

    // A NONZERO baseline is the point: with an all-zero ledger, an arm that
    // wrongly returned `Some(0)` would be indistinguishable from the correct
    // fall-through. Seed the ledger `storm_count` folds so the expected value
    // is nonzero and a zero override is visible.
    let cast_records = runner
        .state()
        .spells_cast_this_turn_by_player
        .get(&P0)
        .map_or(0, |records| records.len());
    assert_eq!(
        cast_records, 0,
        "fixture assumption: the ledger starts empty before seeding"
    );
    runner.state_mut().spells_cast_this_turn_by_player.insert(
        P0,
        vec![SpellCastRecord::default(); 2].into_iter().collect(),
    );

    let with_entry = derive_views(runner.state(), Some(P0)).storm_count;
    assert_eq!(
        with_entry, 2,
        "the combat-damage entry must not shadow the seeded cast count"
    );

    // Differential control: removing the entry must not change the answer.
    runner.state_mut().stack.clear();
    let without_entry = derive_views(runner.state(), Some(P0)).storm_count;
    assert_eq!(
        with_entry, without_entry,
        "a combat-damage entry must not shadow or alter the storm count"
    );
}
