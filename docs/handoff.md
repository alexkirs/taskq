# Handoff
Operational state only. Rules are in [taskq.md](../taskq.md); the work order is in [refactor-plan.md](refactor-plan.md).
Refresh every line with a fresh read before acting on it (R12).

## Hosts
- Mac: v3 stopped (`com.taskq.mac-dispatch` not loaded). v4.0.1 runs as `com.taskq.taskq`, focus `all`, board token
  from the `gh` login.
- Windows: v3 stopped by the owner 2026-10-12. v4 not installed yet: the owner runs setup there (§ 6) with `gh` logged in.
- No other host is registered. Delivery to a host is not proof it ran.

## Board (v4 since 2026-10-12)
- PR 720, 721 and 723 merged; releases `v4.0.0` and `v4.0.1` (macOS, Linux, Windows binaries).
- `CHANGELOG.md` v4.0.0 applied to 23 open issues: label `taskq`, v3 blocks removed (`deps` became `Depends on #N`),
  `later`/`failed` entries. The 4 v3 `ask` tasks (#612, #601, #597, #588) are parked `later` with their question.
- #643 (v3 capacity) and #638 (v3 PID recovery) closed by owner decision.
- `taskq status` after the cutover: Ready 0, waiting 1 (#613), later 16, failed 6.
- PR 719 (test contracts) is on hold, unaccepted.

## Waits on the owner
1. Windows: install v4 from release `v4.0.1`, `taskq service install`, `taskq service focus all`
   (sent to the WIN_CLD session 2026-10-12, no reply yet).
2. A week clean on v4: delete `taskq.py`, `host_service.py`, `runtimes/`, `docs/legacy/` (§ 9).
