# Interactive PM and five-minute TICK (#185)

Bounded delivery against Wiki spec revision `6b9027b29bf43d18a24d2cc96ef98d314f396794`
(§ Minimal critical path, #185). Scope: standalone macOS TaskQ, one checkout, fixtures plus read-only
observations, and narrow fixes in `taskq/tick.py` (`wake`, `starts`) approved on review. No timer was armed, no worker spawned, no configuration changed. Every live gate below
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

`python3 -m unittest discover -s tests -p test_pm_tick_separation.py` (25 tests). Inventory rows have the
real `claude agents --json --all` shape (`id`, `cwd`, `kind`, `startedAt`, `sessionId`, `name`, `state`,
plus `pid`/`status` while running).

| Acceptance | Existing mechanism | Fixture |
|---|---|---|
| One timer owner per project, explicit handoff | One launchd label per checkout (`taskq.<checkout name>`); each install boots out the old agent first. `[coordinator]` is written only when absent. | A second install from another session keeps the recorded owner and leaves one agent. Editing `[coordinator] session` moves the wake target. |
| TICK carries project identity | In-session: TICK_PROMPT names `cd <main checkout>`. launchd: `WorkingDirectory` is the main checkout. The wake turn starts with `Project <repo>, main checkout <path>.` | All three asserted. |
| Delivery receipt is not a completed pass | `wake` writes the judgement key only after `claude_wake` returns. | A failed delivery writes no key and is retried. A delivered wake leaves only that key: the PM's completed pass is never read back (`unknown`). |
| Duplicate/coalesced TICK, one pass per project | Non-blocking checkout lock; judgement-key dedup; busy PM skipped. | Three manual TICKs during a slow timer pass return in under 1 s, each `Skipped: another tick pass is running.`; `--json` reports the refusal with outcome `ok`. Same items wake once; new items wake again. |
| PM stays responsive, also after a resume | Only a coordinator listed in `claude agents` is woken. Busy means state `working` or `blocked`, or `status: busy` (`status` is optional in the CLI's rows), for the recorded session or a job with its name whose `cwd` is this checkout (`claude --bg --resume` runs under a new session id, #182). | Resumed PM working (also without `status`) or blocked under a new id: not woken. Same name in another checkout, a terminal or idle job, another name: woken, always the recorded session (conversation identity). Recorded PM done/failed/stopped: woken. Unknown owner (empty or unreachable inventory, or the recorded session not listed): not woken, no receipt, `[coordinator]` unchanged, printed; the next tick tries again. The receipt names its owner: after a handoff PM a -> PM b the same unresolved items reach PM b once. |
| Restart, interrupted sender/receiver | Lock file survives a killed holder; claims and take's lock decide. `starts` treats a live `T<N> ` worker of this checkout as started: a Claude job `working`/`blocked` or running in a non-terminal state, or a Codex thread from the read-only `thread/list` (cleanup_codex) that is `active`, `idle` or `notLoaded`. An unclaimed one also holds its runtime's place (room counts claimed ones). `claude agents` is read strictly: no CLI is an empty machine, a failed or unreadable list is unknown. | A killed holder does not block the next pass. A taken task is not started again. A spawned, not yet taken worker (Claude working/blocked; Codex active/idle/notLoaded) is not spawned again and no claim is invented. Claude limit 1 with a live unclaimed T1: T2 is not started; limit 2: it is (Codex alike). A claimed live worker is counted once. A terminal or stopped Claude job, a `systemError` or archived (unlisted) Codex thread, another checkout, another task number: the start proceeds. Unknown Claude inventory holds Claude and `any` tasks; unknown Codex inventory holds Codex and `any` tasks; each named; the other runtime's pinned tasks start. |
| Stale observations, busy/failed projects | Existing tests: `TickBeat` (stale or second armed timer named), `test_tick_codex_unavailable_does_not_stop_other_coordinator_work`, #176 `test_permission_qualification.py`. | Consumed, not duplicated. |

Limits of these fixes: an interactive (non-background) PM session is not in the background inventory
`claude_agents` reads, so it is never woken by `--wake` (unknown owner). Codex thread
status alone does not tell a worker before its take from one whose turn ended without a take; the existing
archive pass archives such an unclaimed thread after 10 minutes, which ends the hold.

## Live gates (unqualified)

Observed read-only on macbook-m2 at 2026-10-07T19:28:49Z: no `taskq.*` launchd agent installed, no
`[coordinator]` in the checkout's `taskq.local.toml`. So nothing below was exercised:

- Claude TICK delivery into a live PM and the PM's applied pass: `unknown`.
- DOT/Codex receive and apply: `unknown`; no route wired.
- User input during a slow pass, before versus after separation: not reproduced live.
- Permission approval route: #176 observations only; Claude pending approvals stay `unknown`.
- Hermes/Linux and multiproject (#186): out of scope.
