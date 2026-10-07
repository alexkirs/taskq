# Interactive PM and five-minute TICK (#185)

Bounded delivery against Wiki spec revision `6b9027b29bf43d18a24d2cc96ef98d314f396794`
(§ Minimal critical path, #185). Scope: standalone macOS TaskQ, one checkout, fixtures plus read-only
observations, and three narrow fixes in `taskq/tick.py` (`wake`, `starts`)
approved on review. No timer was armed, no worker spawned, no configuration changed. Every live gate below
stays `unknown`/unqualified until a real supported send and receive is shown.

## Separation with existing mechanisms

- **Claude.** The interactive PM is the session named in `[coordinator] session` of the checkout's
  `taskq.local.toml`. The TICK sender is the launchd agent from `taskq tick --install-timer`: every
  5 minutes it runs `taskq tick --act --wake` in its own process from the main checkout. It does the
  mechanical steps itself and gives the PM one turn (`claude --bg --resume`) only when judgement is
  needed. The in-session `CronCreate` timer (TICK_PROMPT) is the non-separated mode: the pass runs inside
  the PM's turn.
- **Codex/DOT.** A local scheduled task runs the same bounded pass:
  `taskq tick --act --json` from the main checkout (docs/external-pm-tick.md). `--wake` reaches a
  Claude coordinator only. Delivery of the TICK into a DOT thread (`codex-send` is the supported turn
  route) is not wired and not demonstrated: DOT receive and apply stay `unknown`.

The user's observation (better responsiveness with separation) is a hypothesis to reproduce, not a
proven cause.

## What the fixtures show

`python3 -m unittest discover -s tests -p test_pm_tick_separation.py` (16 tests). Inventory rows have the
real `claude agents --json --all` shape (`id`, `cwd`, `kind`, `startedAt`, `sessionId`, `name`, `state`,
plus `pid`/`status` while running).

| Acceptance | Existing mechanism | Fixture |
|---|---|---|
| One timer owner per project, explicit handoff | One launchd label per checkout (`taskq.<checkout name>`); each install boots out the old agent first. `[coordinator]` is written only when absent. | A second install from another session keeps the recorded owner and leaves one agent. Editing `[coordinator] session` moves the wake target. |
| TICK carries project identity | In-session: TICK_PROMPT names `cd <main checkout>`. launchd: `WorkingDirectory` is the main checkout. The wake turn starts with `Project <repo>, main checkout <path>.` | All three asserted. |
| Delivery receipt is not a completed pass | `wake` writes the judgement key only after `claude_wake` returns. | A failed delivery writes no key and is retried. A delivered wake leaves only that key: the PM's completed pass is never read back (`unknown`). |
| Duplicate/coalesced TICK, one pass per project | Non-blocking checkout lock; judgement-key dedup; busy PM skipped. | Three manual TICKs during a slow timer pass return in under 1 s, each `Skipped: another tick pass is running.`; `--json` reports the refusal with outcome `ok`. Same items wake once; new items wake again. |
| PM stays responsive, also after a resume | A busy PM gets no turn. Busy means the recorded session busy, or a `working`/`blocked` job with the recorded session's name whose `cwd` is this checkout (`claude --bg --resume` runs under a new session id, #182). | Resumed PM busy or blocked under a new id: not woken. Same name in another checkout, a terminal job, an idle job, another name: woken, and always the recorded session (conversation identity). Unreachable inventory (empty list): delivery attempted, receipt only on success. |
| Restart, interrupted sender/receiver | Lock file survives a killed holder; claims and take's lock decide. `starts` skips a ready task whose `T<N> ` Claude job of this checkout is alive (`working`, or `blocked` without pid). | A killed holder does not block the next pass. A taken task is not started again. A spawned, not yet taken worker (working or blocked) is not spawned again and no claim is invented. A terminal or stopped job, another checkout, another task number, or an unreachable inventory does not hold the start. |
| Stale observations, busy/failed projects | Existing tests: `TickBeat` (stale or second armed timer named), `test_tick_codex_unavailable_does_not_stop_other_coordinator_work`, #176 `test_permission_qualification.py`. | Consumed, not duplicated. |

Limits of these fixes: a Codex worker spawned but not taken is not detected (a `thread/list` read; Claude is
the default worker runtime). An interactive (non-background) PM session is not in the background inventory
`claude_agents` reads, so its busy state is not seen. With an unreachable inventory busy is unknown and the
wake is attempted, as before.

## Live gates (unqualified)

Observed read-only on macbook-m2 at 2026-10-07T19:28:49Z: no `taskq.*` launchd agent installed, no
`[coordinator]` in the checkout's `taskq.local.toml`. So nothing below was exercised:

- Claude TICK delivery into a live PM and the PM's applied pass: `unknown`.
- DOT/Codex receive and apply: `unknown`; no route wired.
- User input during a slow pass, before versus after separation: not reproduced live.
- Permission approval route: #176 observations only; Claude pending approvals stay `unknown`.
- Hermes/Linux and multiproject (#186): out of scope.
