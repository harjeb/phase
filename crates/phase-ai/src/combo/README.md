# Combo planning

## Component recognition

The default registry contains three structural families: lifelink/counter damage,
library exile/library win, and creature copy/entry blink. Card names label the
templates; effects and triggers identify their components. `ActivateRole` resolves
the current ability slot instead of assuming a printed index. Tutor and mulligan
hints reuse the same predicates.

## Engine-verified action chains

`plan_combos` explores engine-issued candidates and applies every action through
the simulation reducer. Its replayable trace includes actors, announcements,
costs, targets, naming choices, and priority passes. Invalid payments, missing
targets, dead components, or failure to restore the template's required counters
or copy-source readiness cannot produce a witness.

`WinUnderPassResponses` records an actual terminal engine win while opponents
pass priority. `CompletedCycle` records one template cycle with counter or
copy-source restoration tied to the acting source. Neither result proves safety against opponent
interaction or an arbitrary infinite loop. Unregistered combo families are not
discovered automatically.

## Decision integration and budgets

The public AI pipeline prepares a plan once per root decision, only for a player
whose deck tier is cEDH. It uses at most half the configured node budget, capped
at 96 nodes, and shares the decision deadline. Continuation search reserves those
spent nodes. Templates share the remaining planning quota; traces are capped at
40 actions and branches at eight candidates.

A completed cycle remains a fallback while the remaining templates are searched
within their quota. A terminal win takes precedence over that fallback.

Only the next action of a plan receives the combo policy bonus. Non-priority
prompts follow that action only if the engine still offers it. Each new decision
replans; a stored plan rejects changes to its origin state or ability definitions.
Lookahead evaluations do not run another combo search or reuse the root bonus.

## Regression verification

Fixtures contain the actual parsed definitions of the six template cards.
Positive cases replay complete traces. Recognition cases cover renamed components
and reordered ability slots. Negative cases cover name-only impostors,
insufficient counters or mana, protected targets, tapped sources, stale plans,
and multiple similar sources.
End-to-end cases verify the public AI loop and same-state policy ablation.

```powershell
cargo test -p phase-ai --lib combo -- --test-threads=2
cargo test -p phase-ai --test cedh_integration -- --test-threads=2
cargo test -p phase-ai --lib -- --test-threads=2
cargo clippy -p phase-ai --lib --tests -- -D warnings
cargo ai-gate --data-root data
cargo ai-perf-gate --data-root data
```

Tests and Clippy must pass. Compare the gates against the existing baselines
without refreshing them. Ordinary mirror-suite gates complement, but do not
replace, the dedicated combo positions.
