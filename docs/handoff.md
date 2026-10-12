# Handoff
Operational state only. Rules are in [taskq.md](../taskq.md); the work order is in [refactor-plan.md](refactor-plan.md).
Refresh every line with a fresh read before acting on it (R12).

## Hosts
- Mac: v3 stopped (`com.taskq.mac-dispatch` not loaded). v4 installed 2026-10-12 as `com.taskq.taskq`, focus `all`;
  it exits until the board token is in `~/.local/share/taskq/token` (mode 0600).
- Windows: v3 stopped by the owner 2026-10-12. v4 not installed yet.
- No other host is registered. Delivery to a host is not proof it ran.

## Board (v4 since 2026-10-12)
- PR 720 merged, release `v4.0.0` published (macOS, Linux, Windows binaries).
- `CHANGELOG.md` v4.0.0 applied to 23 open issues: label `taskq`, v3 blocks removed (`deps` became `Depends on #N`),
  `later`/`failed` entries. The 4 v3 `ask` tasks (#612, #601, #597, #588) are parked `later` with their question.
- #643 (v3 capacity) and #638 (v3 PID recovery) closed by owner decision.
- `taskq status` after the cutover: Ready 0, waiting 1 (#613), later 16, failed 6.
- PR 719 (test contracts) is on hold, unaccepted.

## Waits on the owner
1. Mac: write the board token file; the service then starts on the next launchd retry.
2. Windows: install v4 from release `v4.0.0`, `taskq service install`, `taskq service focus all`.
3. A week clean on v4: delete `taskq.py`, `host_service.py`, `runtimes/`, `docs/legacy/` (§ 9).
