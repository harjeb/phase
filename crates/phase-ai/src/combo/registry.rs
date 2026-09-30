//! Hand-authored structural templates for damage/counter, library-win,
//! and copy/blink combos. Printed names are explanatory labels only.
//!
//! Role matching provides inexpensive component and tutor hints. The bounded
//! engine planner, not the coarse reachability detector, validates costs,
//! targets, choices, stack resolution, and resource restoration.

use engine::types::game_state::GameState;
use engine::types::mana::{ManaCost, ManaCostShard};
use engine::types::player::PlayerId;

use crate::combo::detection::{piece_present, ComboDetector, StructuralComboDetector};
use crate::combo::line::{
    CardPredicate, ComboLine, ComboLineId, ComboPiece, ComboReachability, ComboStep, ComponentRole,
    WinKind,
};
use engine::types::identifiers::ObjectId;

pub struct ComboRegistry {
    lines: Vec<ComboLine>,
    detector: Box<dyn ComboDetector>,
}

impl Default for ComboRegistry {
    fn default() -> Self {
        Self {
            lines: vec![
                heliod_ballista_line(),
                thoracle_consultation_line(),
                kiki_felidar_line(),
            ],
            detector: Box::new(StructuralComboDetector),
        }
    }
}

impl ComboRegistry {
    /// Returns all combo lines that are reachable (this turn or next turn) for
    /// the given AI player. Lines that are `NotReachable` are filtered out.
    pub fn reachable_lines(
        &self,
        state: &GameState,
        ai: PlayerId,
    ) -> Vec<(ComboLineId, ComboReachability)> {
        self.lines
            .iter()
            .map(|line| (line.id, self.detector.assess(state, line, ai)))
            .filter(|(_, r)| !matches!(r, ComboReachability::NotReachable))
            .collect()
    }

    pub fn lines(&self) -> &[ComboLine] {
        &self.lines
    }

    /// Returns the canonical names of cards that, if added to the AI's hand
    /// or battlefield (depending on the piece's required zone), would complete
    /// a registered combo line. A piece is "missing" when its zone is empty of
    /// any object matching the predicate; the line is "near-reachable" when
    /// all OTHER pieces are already in their required zones.
    ///
    /// Used by the tutor target scorer: cards in this set should receive a
    /// dominant boost so the AI fetches the exact piece that closes a combo,
    /// rather than picking the highest-EV generic creature.
    pub fn missing_pieces_for_near_reachable_lines<'a>(
        &self,
        state: &'a GameState,
        ai: PlayerId,
    ) -> Vec<&'a str> {
        let mut out: Vec<&'a str> = Vec::new();
        for line in &self.lines {
            let (present, missing): (Vec<_>, Vec<_>) = line
                .pieces
                .iter()
                .partition(|piece| piece_present(piece, state, ai));
            // Near-reachable: exactly one piece missing, all others assembled.
            // Lines with zero missing pieces are *already* reachable — the
            // tutor isn't needed. Lines with two-or-more missing pieces are
            // too far away to be worth a tutor cycle.
            if missing.len() != 1 {
                continue;
            }
            // Single-missing pieces with `InLibrary` predicate represent the
            // expected tutor target — capture them. Other zones (InHand,
            // OnBattlefield) also count: e.g., a tutor that grabs into hand
            // closes the gap for an InHand piece too.
            let _ = present;
            let predicate = match missing[0] {
                ComboPiece::InHand(predicate)
                | ComboPiece::OnBattlefield(predicate)
                | ComboPiece::InGraveyard(predicate)
                | ComboPiece::InLibrary(predicate) => predicate,
            };
            let object_names = state
                .objects
                .values()
                .filter(|object| {
                    object.owner == ai
                        && object.zone == engine::types::zones::Zone::Library
                        && super::components::matches_object(predicate, object)
                })
                .map(|object| object.name.as_str());
            let deck_names = state
                .deck_pools
                .iter()
                .filter(|pool| pool.player == ai)
                .flat_map(|pool| pool.current_main.iter())
                .filter(|entry| super::components::matches_face(predicate, &entry.card))
                .map(|entry| entry.card.name.as_str());
            for name in object_names.chain(deck_names) {
                if !out.contains(&name) {
                    out.push(name);
                }
            }
            if let CardPredicate::NameEquals(name) = predicate {
                if !out.contains(name) {
                    out.push(name);
                }
            }
        }
        out
    }

    /// Returns every registered line whose `ComboPiece::InHand` components
    /// are *all* present in the provided hand. Lines with no `InHand` pieces
    /// (i.e., combos that activate entirely on the battlefield) are excluded
    /// because there is nothing to verify against the hand for them.
    ///
    /// This is the right primitive for the mulligan layer: pre-game, the
    /// battlefield is empty and mana hasn't been spent, so the full
    /// `reachable_lines` check would always fail for in-hand combos that
    /// require multiple mana sources. The hand-only check answers the
    /// mulligan-relevant question — "do I have a winning combo in my
    /// opening hand?" — without conflating it with mid-game mana availability.
    pub fn lines_with_pieces_in_hand(
        &self,
        hand: &[ObjectId],
        state: &GameState,
    ) -> Vec<ComboLineId> {
        self.lines
            .iter()
            .filter(|line| {
                let in_hand_predicates: Vec<&CardPredicate> = line
                    .pieces
                    .iter()
                    .filter_map(|p| match p {
                        ComboPiece::InHand(pred) => Some(pred),
                        _ => None,
                    })
                    .collect();
                !in_hand_predicates.is_empty()
                    && in_hand_predicates.iter().all(|pred| {
                        hand.iter().any(|&id| {
                            state
                                .objects
                                .get(&id)
                                .is_some_and(|obj| super::components::matches_object(pred, obj))
                        })
                    })
            })
            .map(|line| line.id)
            .collect()
    }
}

/// Lifelink plus a life-gain counter trigger replenishes a counter-damage source.
/// A surviving source needs at least two counters unless another effect protects it.
fn heliod_ballista_line() -> ComboLine {
    ComboLine {
        id: ComboLineId(0),
        name: "Heliod, Sun-Crowned + Walking Ballista",
        pieces: vec![
            ComboPiece::OnBattlefield(lifelink_counter_source()),
            ComboPiece::OnBattlefield(CardPredicate::Role(ComponentRole::CounterDamage)),
        ],
        // Cost to start the loop: activate Heliod's {1}{W} once. Ballista's
        // damage ability pays via counter removal, not mana, so the per-loop
        // marginal mana cost is zero.
        mana_cost: ManaCost::Cost {
            shards: vec![ManaCostShard::White],
            generic: 1,
        },
        action_sequence: vec![
            ComboStep::ActivateRole {
                predicate: lifelink_counter_source(),
                role: ComponentRole::LifelinkGrant,
            },
            ComboStep::ActivateRole {
                predicate: CardPredicate::Role(ComponentRole::CounterDamage),
                role: ComponentRole::CounterDamage,
            },
        ],
        win_kind: WinKind::InfiniteLoop,
    }
}

/// Library-win and named-card exile effects, requiring exactly `{U}{U}{B}`.
/// The planner validates casts, naming, targets, and terminal engine resolution.
fn thoracle_consultation_line() -> ComboLine {
    ComboLine {
        id: ComboLineId(1),
        name: "Thassa's Oracle + Demonic Consultation",
        pieces: vec![
            ComboPiece::InHand(CardPredicate::Role(ComponentRole::LibraryWin)),
            ComboPiece::InHand(CardPredicate::Role(ComponentRole::LibraryExile)),
        ],
        // {U}{U}{B} — both spells must be castable in the same turn so
        // Consultation can resolve before Thoracle's ETB.
        mana_cost: ManaCost::Cost {
            shards: vec![
                ManaCostShard::Blue,
                ManaCostShard::Blue,
                ManaCostShard::Black,
            ],
            generic: 0,
        },
        action_sequence: vec![
            ComboStep::Cast {
                predicate: CardPredicate::Role(ComponentRole::LibraryWin),
            },
            ComboStep::Cast {
                predicate: CardPredicate::Role(ComponentRole::LibraryExile),
            },
        ],
        win_kind: WinKind::ImmediateLoss,
    }
}

/// A haste-granting creature copy source and an entry-blink trigger.
/// One completed cycle must create a token and restore the actual copy source.
fn kiki_felidar_line() -> ComboLine {
    ComboLine {
        id: ComboLineId(2),
        name: "Kiki-Jiki, Mirror Breaker + Felidar Guardian",
        pieces: vec![
            ComboPiece::OnBattlefield(CardPredicate::Role(ComponentRole::CreatureCopy)),
            ComboPiece::OnBattlefield(CardPredicate::Role(ComponentRole::EntryBlink)),
        ],
        // The activation cost is `{T}` only — Kiki's mana cost is irrelevant
        // because it is already on the battlefield. No mana shortfall is
        // possible for the loop itself; the engine's legal-actions layer
        // enforces summoning-sickness / tapped-state constraints.
        mana_cost: ManaCost::NoCost,
        action_sequence: vec![ComboStep::ActivateRole {
            predicate: CardPredicate::Role(ComponentRole::CreatureCopy),
            role: ComponentRole::CreatureCopy,
        }],
        win_kind: WinKind::InfiniteLoop,
    }
}

fn lifelink_counter_source() -> CardPredicate {
    CardPredicate::All(vec![
        CardPredicate::Role(ComponentRole::LifelinkGrant),
        CardPredicate::Role(ComponentRole::LifeCounter),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_state_returns_no_reachable_lines() {
        let state = GameState::new_two_player(0);
        let reg = ComboRegistry::default();
        assert_eq!(reg.reachable_lines(&state, PlayerId(0)).len(), 0);
    }

    #[test]
    fn registry_exposes_expected_lines() {
        let reg = ComboRegistry::default();
        assert_eq!(reg.lines().len(), 3);
        assert_eq!(reg.lines()[0].id, ComboLineId(0));
        assert_eq!(
            reg.lines()[0].name,
            "Heliod, Sun-Crowned + Walking Ballista"
        );
        assert_eq!(reg.lines()[1].id, ComboLineId(1));
        assert_eq!(
            reg.lines()[1].name,
            "Thassa's Oracle + Demonic Consultation"
        );
        assert_eq!(reg.lines()[2].id, ComboLineId(2));
        assert_eq!(
            reg.lines()[2].name,
            "Kiki-Jiki, Mirror Breaker + Felidar Guardian"
        );
    }

    #[test]
    fn thoracle_combo_reachable_with_both_cards_in_hand_and_mana() {
        use engine::game::zones::create_object;
        use engine::types::card_type::CoreType;
        use engine::types::identifiers::CardId;
        use engine::types::zones::Zone;

        let mut state = GameState::new_two_player(0);
        // Four lands producing UUU+B → pays {U}{U}{B} (two U pips + B pip +
        // one additional Island remains unused).
        let subtypes = ["Island", "Island", "Island", "Swamp"];
        for (i, subtype) in subtypes.iter().enumerate() {
            let land_id = create_object(
                &mut state,
                CardId(10 + i as u64),
                PlayerId(0),
                subtype.to_string(),
                Zone::Battlefield,
            );
            let obj = state.objects.get_mut(&land_id).unwrap();
            obj.card_types.core_types.push(CoreType::Land);
            obj.card_types.subtypes.push(subtype.to_string());
        }
        crate::combo::tests::place(
            &mut state,
            &crate::combo::tests::card("thassa's oracle"),
            Zone::Hand,
        );
        crate::combo::tests::place(
            &mut state,
            &crate::combo::tests::card("demonic consultation"),
            Zone::Hand,
        );

        let reg = ComboRegistry::default();
        let reachable = reg.reachable_lines(&state, PlayerId(0));
        let thoracle = reachable
            .iter()
            .find(|(id, _)| *id == ComboLineId(1))
            .expect("Thoracle/Consult line must be reachable");
        match &thoracle.1 {
            ComboReachability::ReachableThisTurn {
                missing_mana,
                required_actions,
            } => {
                assert_eq!(*missing_mana, 0);
                assert_eq!(required_actions.len(), 2);
            }
            other => panic!("expected ReachableThisTurn, got {other:?}"),
        }
    }

    /// Discriminating regression: both Thoracle pieces are in hand and the AI
    /// controls four untapped lands — but they produce only W and G, never the
    /// U/U/B that {U}{U}{B} requires. With the color-accurate affordability
    /// primitive the line collapses to NotReachable and is filtered out.
    ///
    /// This MUST fail on pre-fix code: the old count-based check saw 4 mana
    /// sources >= 4 pips and reported `missing_mana: 0` → ReachableThisTurn.
    /// It passes only after delegating to `can_pay_cost_after_auto_tap`.
    #[test]
    fn thoracle_line_not_reachable_with_wrong_color_mana() {
        use engine::game::zones::create_object;
        use engine::types::card_type::CoreType;
        use engine::types::identifiers::CardId;
        use engine::types::zones::Zone;

        let mut state = GameState::new_two_player(0);
        // Four untapped lands, WRONG colors only: W, W, G, G — no U, no B.
        let subtypes = ["Plains", "Plains", "Forest", "Forest"];
        for (i, subtype) in subtypes.iter().enumerate() {
            let land_id = create_object(
                &mut state,
                CardId(10 + i as u64),
                PlayerId(0),
                subtype.to_string(),
                Zone::Battlefield,
            );
            let obj = state.objects.get_mut(&land_id).unwrap();
            obj.card_types.core_types.push(CoreType::Land);
            obj.card_types.subtypes.push(subtype.to_string());
        }
        crate::combo::tests::place(
            &mut state,
            &crate::combo::tests::card("thassa's oracle"),
            Zone::Hand,
        );
        crate::combo::tests::place(
            &mut state,
            &crate::combo::tests::card("demonic consultation"),
            Zone::Hand,
        );

        let reg = ComboRegistry::default();
        let reachable = reg.reachable_lines(&state, PlayerId(0));
        // The Thoracle line (ComboLineId(1)) must not appear as reachable this
        // turn — under the collapse semantics it is NotReachable and filtered.
        let thoracle = reachable.iter().find(|(id, _)| *id == ComboLineId(1));
        assert!(
            !matches!(
                thoracle,
                Some((
                    _,
                    ComboReachability::ReachableThisTurn {
                        missing_mana: 0,
                        ..
                    }
                ))
            ),
            "Thoracle line must not be reachable-this-turn with wrong-color mana, got {thoracle:?}"
        );
    }
}
