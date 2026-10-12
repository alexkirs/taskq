# Handoff
Operational state only. Rules are in [taskq.md](../taskq.md); the work order is in [refactor-plan.md](refactor-plan.md).
Refresh every line with a fresh read before acting on it (R12).

## Hosts
- Mac: v3 stopped (`com.taskq.mac-dispatch` not loaded). v4.0.2 runs as `com.taskq.taskq`, focus `all`,
  limits claude 4 / codex 0, board token from the `gh` login.
- Windows: v3 stopped (`TaskQ-Windows-Dispatch` disabled, files kept). v4.0.2 runs as Scheduled Task `TaskQ-taskq`,
  focus `all`, on the board since 2026-10-12. Install from a process outside Claude Desktop (MSIX virtualizes AppData).
  `taskq` on PATH is still the v3 shim; use `%LOCALAPPDATA%\taskq\taskq.exe`.
- No other host is registered. Delivery to a host is not proof it ran.

## Board (v4 since 2026-10-12)
- PR 720, 721, 723 and 724 merged; releases `v4.0.0`, `v4.0.1` and `v4.0.2` (macOS, Linux, Windows binaries).
- `CHANGELOG.md` v4.0.0 applied to 23 open issues: label `taskq`, v3 blocks removed (`deps` became `Depends on #N`),
  `later`/`failed` entries. The 4 v3 `ask` tasks (#612, #601, #597, #588) are parked `later` with their question.
- #643 (v3 capacity) and #638 (v3 PID recovery) closed by owner decision.
- `taskq status` after the cutover: Ready 0, waiting 1 (#613), later 16, failed 6.
- Cleanup 2026-10-12 before the repo transfer: the 5 open v3 PRs (594, 599, 628, 661, 719) closed unmerged, all
  branches but `main` deleted, 17 worktrees removed; uncommitted v3 files are in `~/Documents/taskq-archive-2026-10-12.tar.gz`.

## Waits on the owner
1. A week clean on v4: delete `taskq.py`, `host_service.py`, `runtimes/`, `docs/legacy/` (§ 9).
