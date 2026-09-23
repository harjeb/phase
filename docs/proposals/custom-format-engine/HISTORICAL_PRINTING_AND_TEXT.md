# Historical printing and card-text admission

This implementation note supersedes the earlier Swedish reprint uncertainty in
`CONTEXT.md` Open item 6 and the corresponding `RESEARCH.md` wording. It does not
release a preset or certify historical gameplay acceptance.

## Swedish Old School

Primary source: <https://oldschool-mtg.blogspot.com/p/banrestriction.html>.
The complete page lists Alpha, Beta, Unlimited, Arabian Nights, Antiquities,
Legends, The Dark and Summer Magic, and says “Only English versions are allowed
in Oldschool”. Its separate “Local variations of Oldschool” section explains:
“Most commonly a few extra reprint sets are legal”. Those additions belong to
regional variants, not the Swedish baseline. Summer Magic uses set code `SUM`.
The 25 restricted names, empty banned list and ante exclusion remain unchanged.

`LegacyRuleSet.printing_policy = SwedishOriginal` enforces the eight-edition,
English-only selection rule. The preset declares `OriginalPrintingsOnly` and
`PrintingFidelity::SelectedPrinting`. Name-pool eligibility is still evaluated
separately: owning a legal card name does not make its Revised, Collector's
Edition or modern printing legal. Basic lands receive no printing exemption.

`database::historical_printing::validate_deck_with_printings` takes a host-owned
catalog and one selected printing ID per submitted card copy. It rejects absent,
extra, unknown, wrong-card, wrong-edition and wrong-language selections, including
sideboard copies. It also requires the card database to confirm the card's edition.
After proving printing legality it invokes the existing authoritative deck gate,
retaining deck-size, sideboard, restricted/banned, ante and legacy-axis checks.
Only a local validation clone discharges the printing requirement; the caller's
format is untouched. Existing name-only full and summary paths fail closed.

Remaining release gates: trusted catalog loading and selected-ID transport at
host/game creation, followed by the separate gameplay acceptance audit. Those
entry points are outside this worker's scope. Swedish is still absent from
`bundled_presets()` and `custom_format_registry()`.

## Classic Magic

Source text fetched from the Lords of the Pit format document:
<https://raw.githubusercontent.com/northern-information/lordsofthepit.com/main/src/pages/formats.md>.
The source's Classic Magic section explicitly requires:

- Time Vault enters tapped and does not untap normally. **Skipping the next
  turn is an activation cost**, followed by untapping and adding a time counter.
  Tapping and removing a time counter pays for the extra-turn ability.
- Illusionary Mask puts a creature with mana value at most X from hand onto the
  battlefield face down as **0/1**, with X mask counters, at sorcery timing.
  Turning it face up removes all mask counters and is available at instant
  timing. This is not the current Oracle wording or ordinary morph.

`LegacyRuleSet.card_text = ClassicMagic` selects these authored strings through
`database::historical_card_text::oracle_text_for_format`. All other cards and
Oracle-policy formats retain their database text; no shared database mutation
occurs. Text resolution is explicitly separate from compiled-ability resolution.
`resolve_card_face_for_format` rejects Classic rather than returning historical
text paired with modern compiled abilities.

**Executable Classic overrides are not implemented.** `AbilityCost` has no
skip-next-turn cost, and Mask needs dedicated face-down characteristics and a
mask-counter face-up action. Adding an effect that skips a turn would not pay
Vault's activation cost and would be incorrect when the activation is countered.
These runtime changes require coordination outside the assigned format/printing/
card-text ownership. `LegacyAxis::ClassicCardText` is deliberately absent from
`IMPLEMENTED_LEGACY_AXES`: registration, config deserialization and deck admission
reject it independently of combat timing. No Classic constructor is registered,
and `CombatDamageTiming::OnStack` was not enabled by this change.

## Focused verification

`crates/engine/tests/historical_format_policy.rs` covers original editions,
reprints, language, basic lands, identity/catalog misses, per-copy coverage,
sideboard selection, restricted copies, ante, deck size, full/summary name-only
rejection, scoped text, modern-text preservation, old-payload defaults and the
independent Classic capability gate.

The existing custom-format schema tests now assert the sourced Swedish printing
metadata. Existing ante-only unit fixtures explicitly use the Swedish name pool
without printing requirements so their positive controls still isolate ante
behavior. Full gameplay tests for Vault/Mask remain a prerequisite for admitting
`ClassicCardText`.
