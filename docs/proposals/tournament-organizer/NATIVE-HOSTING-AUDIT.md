# Native tournament hosting audit (working tree)

This audit covers P6 in `phase-mana/docs/special-play-modes-development-plan.zh-CN.md` and the R1–R10 acceptance requirements in [HOSTED-MATCHES.md](HOSTED-MATCHES.md). It describes the local implementation, not the historical upstream revision in STATUS.md.

**Automatic verified tournament hosting is not implemented.** The existing organizer remains an explicitly unverified, seated-player self-report workflow. Draft auto-hosting is a reference implementation, not tournament hosting.

## Implemented in this change

Native `phase-server` now stores the serialized `TournamentManager` in its existing SQLite game database. All native tournament actions, including credential rotation, persist before any broker success reply or broadcast is sent. A failed write restores the prior in-memory authority and connection bookkeeping and returns a refusal. Expiry also commits before publication, restoring the prior authority for the next sweep if persistence fails.

Startup restores tournament authority before Full game-session recovery and runs the existing expiry policy. A corrupt or unreadable saved authority fails startup instead of replacing tournaments with an empty registry. The snapshot retains credential expiry and recoverable-rotation nonce records, pairings, results, and standings inputs. It contains private credentials and is part of the private server database, not a client export.

This completes the tournament-registry portion of R7. It does **not** implement a hosted receipt, transactional terminal ingestion, a hosting generation fence, or any new verified result authority. The native Worker-independent storage boundary now exists; the Cloudflare broker's own persistence behavior is unchanged.

Focused check:

```sh
cargo test -p phase-server --bin phase-server tournament_persistence_tests -- --nocapture
```

Validation in this working tree: **3 passed, 0 failed** (262 unrelated tests filtered out).

Checks cover actual broker creation/join/round state surviving database reopen, credential-rotation replay after reopen, write failure rollback without success outbounds, and corrupt saved state rejection. No engine `--lib` test build is required.

## Remaining acceptance requirements

| Requirement | Current native tournament status |
| --- | --- |
| R1: system-only results and broker publication | Missing. `ReportGate` has no hosted arm, and `ReportMatchResult` remains seated-player-authorized. |
| R2: durable terminal handoff | Missing. `FullTerminalArtifact` has no tournament identity or complete authoritative Bo3 score. |
| R3: rehost generation fencing | Missing. Pairings carry no hosted-game lifetime/receipt. |
| R4: capability negotiation/manual fallback | Missing. No participant hosted-match capability or hosted tournament selection exists. |
| R5: reserved-seat binding | Missing for tournaments. Existing native game/draft seat APIs can be reused, but are not bound to tournament credentials. |
| R6: trusted disconnect result for all classes | No tournament path. Bo3, Bo1, and multiplayer expiry must reach an authoritative outcome policy; never infer a verified draw merely from transport teardown. |
| R7: durable authority and receipt | Registry persistence implemented; receipt and atomic result ingestion remain missing. |
| R8: reconnect/expiry claim plus tournament ingestion | Existing Full lifetime protections do not by themselves publish a tournament result. A tournament receipt must participate before removal. |
| R9: single-elimination draw adjudication | Missing. Existing core deliberately rejects a draw in single elimination. An automatic replay must advance the hosting generation without inventing a winner. |
| R10: deck submission/readiness/lock | Missing. Tournament entrants have no deck submissions or locked snapshots. |

Byes and existing drop/forfeit handling remain available in the manual organizer. No-show handling, all-seats expiry, pod continuation after a dropout, and restart while hosting are not covered by a native tournament hosting workflow.

## Exact frontend/protocol integration still needed

These are required new interfaces, **not messages currently accepted by the server**:

1. A hosted-mode selection/capability on tournament creation and views, with a lobby protocol bump and participant support tracking. Reject unsupported organizers and apply the proposal's explicit per-pairing manual fallback for unsupported participants. Native Full hosting must be distinguished from a lobby-only deployment.
2. An authenticated per-pairing deck-submission request carrying tournament code, pairing id, tournament player credential, and the existing structured deck choice. Server-side resolution and format validation precede durable acceptance. The reply/view must distinguish submitted, waiting-for-other-seats, locked, and failed; retries must not replace a locked deck.
3. A private match-start/lookup reply carrying tournament code, pairing id, hosting generation, `FullSessionKey`, game code, and the recipient's seat/attach capability. Do not broadcast credentials in `TournamentView`. Reconnect lookup must reauthorize the current tournament credential, including rotation and expiry, and return only that entrant's assigned seat.
4. A tournament-aware attach request bound to the current pairing generation and Full session key. Reuse native authenticated session attachment and the existing seat-private snapshot path; generic `JoinGame` must not take a reserved tournament seat or replace its locked deck.
5. A hosted report gate in pairing views. The frontend must hide manual self-report for hosted pairings, while the broker independently rejects that RPC. There must be **no** client message that supplies a supposedly verified winner, score, or system-report authority.
6. Tournament updates from a server-private, receipt-backed broker action after an authoritative completed match. Bo3 reads the real completed match score; Bo1/pods use empty `game_wins`. Durable terminals must contain the exact mapped outcome and generation so recovery does not depend on a removed live session.

Reusing the draft spawner requires all tournament seats' validated decks, a format configuration, and a durable binding before announcements. Reusing only the draft winner callback is insufficient: it omits terminal receipt recovery, Bo3 match scoring, stale-generation rejection, capability negotiation, and deck/credential ownership.
