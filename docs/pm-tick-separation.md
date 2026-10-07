# Interactive PM and five-minute TICK (#185)

Bounded delivery against Wiki spec revision `6b9027b29bf43d18a24d2cc96ef98d314f396794`
(§ Minimal critical path, #185). Scope: standalone macOS TaskQ, one checkout, fixtures plus read-only
observations. No timer was armed, no worker spawned, no configuration changed. Every live gate below
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

`python3 -m unittest discover -s tests -p test_pm_tick_separation.py` (13 tests, 3 expected failures):

| Acceptance | Existing mechanism | Fixture |
|---|---|---|
| One timer owner per project, explicit handoff | One launchd label per checkout (`taskq.<checkout name>`); each install boots out the old agent first. `[coordinator]` is written only when absent. | A second install from another session keeps the recorded owner and leaves one agent. Editing `[coordinator] session` moves the wake target. |
| TICK carries project identity | In-session: TICK_PROMPT names `cd <main checkout>`. launchd: `WorkingDirectory` is the main checkout; the wake target comes from that checkout's local file. | Both asserted. The wake text itself names no project: Gap 3. |
| Delivery receipt is not a completed pass | `wake` writes the judgement key only after `claude_wake` returns. | A failed delivery writes no key and is retried. A delivered wake leaves only that key: the PM's completed pass is never read back (`unknown`). |
| Duplicate/coalesced TICK, one pass per project | Non-blocking checkout lock; judgement-key dedup; busy PM skipped. | Three manual TICKs during a slow timer pass return in under 1 s, each `Skipped: another tick pass is running.`; `--json` reports the refusal with outcome `ok`. Same items wake once; new items wake again. |
| PM stays responsive | A busy PM gets no turn; the key is not written. | Busy PM: no wake, next tick delivers. |
| Restart, interrupted sender/receiver | Lock file survives a killed holder; claims and take's lock decide. | A killed holder does not block the next pass. A taken task is not started again. Spawn before take: Gap 2. |
| Stale observations, busy/failed projects | Existing tests: `TickBeat` (stale or second armed timer named), `test_tick_codex_unavailable_does_not_stop_other_coordinator_work`, #176 `test_permission_qualification.py`. | Consumed, not duplicated. |

## Gaps (scope extension requested, not implemented here)

1. **Resumed PM under a new session id.** `claude --bg --resume` continues the conversation under a new
   session id with the same name (#182 evidence; `claude agents` here also lists two `T177` jobs with
   different session ids). `wake` checks busy only for the recorded id. A second wake therefore resumes
   the old id beside the busy new one: two active PM turns. Minimal change in `taskq/tick.py` `wake`:
   treat the PM as busy when any `claude agents` job with the recorded session's name is busy, and
   keep the recorded id as the conversation identity.
2. **Spawned but not yet taken.** A TICK between spawn and take (manual TICK after the timer, restart)
   starts a second worker for the same task. take's lock keeps one claim; the second worker runs
   `taskq worker` again and can take another task. Minimal change in `taskq/tick.py` `starts`: skip a
   ready task whose `T<N> ` worker session is alive on this machine.
3. **Wake text without project identity.** The wake turn names no checkout or repository; only its
   target session binds it to the project. Payload identity is #191's report-contract scope; adding
   the main checkout to `WAKE_PROMPT` is the one-line alternative.

## Live gates (unqualified)

Observed read-only on macbook-m2 at 2026-10-07T19:28:49Z: no `taskq.*` launchd agent installed, no
`[coordinator]` in the checkout's `taskq.local.toml`. So nothing below was exercised:

- Claude TICK delivery into a live PM and the PM's applied pass: `unknown`.
- DOT/Codex receive and apply: `unknown`; no route wired.
- User input during a slow pass, before versus after separation: not reproduced live.
- Permission approval route: #176 observations only; Claude pending approvals stay `unknown`.
- Hermes/Linux and multiproject (#186): out of scope.
