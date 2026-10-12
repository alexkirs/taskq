# Handoff
Operational state only. Rules are in [taskq.md](../taskq.md); the work order is in [refactor-plan.md](refactor-plan.md).
Refresh every line with a fresh read before acting on it (R12).

## Hosts
- Mac: v3 stopped (`com.taskq.mac-dispatch` not loaded). v4.0.5 runs as `com.taskq.taskq`, focus `all`,
  limits claude 4 / codex 0, board token from the `gh` login.
- Windows: v3 stopped (`TaskQ-Windows-Dispatch` disabled, files kept). v4.0.5 runs as Scheduled Task `TaskQ-taskq`
  (launcher supervises the service, no console window, output in `%LOCALAPPDATA%\taskq\taskq.log`), focus `all`,
  on the board since 2026-10-13. v4.0.2 died 2026-10-12 within 2 min (console window closed, `0xC000013A`); the task
  did not restart it. Install from a process outside Claude Desktop (MSIX virtualizes AppData); ssh works
  (`alexk@desktop`, key `csgo_desktop_ed25519`) but has no `gh` token, so test the service through the task.
  `taskq` on PATH is still the v3 shim; use `%LOCALAPPDATA%\taskq\taskq.exe`.
- No other host is registered. Delivery to a host is not proof it ran.

## Board (v4 since 2026-10-12)
- PR 720, 721, 723, 724, 746, 747 and 748 merged; releases `v4.0.0`–`v4.0.5` (macOS, Linux, Windows binaries).
  v4.0.3 fixed the frozen heartbeat (same-body edit keeps `updated_at`), v4.0.4/v4.0.5 the Windows launcher.
- `CHANGELOG.md` v4.0.0 applied to 23 open issues: label `taskq`, v3 blocks removed (`deps` became `Depends on #N`),
  `later`/`failed` entries. The 4 v3 `ask` tasks (#612, #601, #597, #588) are parked `later` with their question.
- #643 (v3 capacity) and #638 (v3 PID recovery) closed by owner decision.
- `taskq status` after the cutover: Ready 0, waiting 1 (#613), later 16, failed 6.
- Two-host claim test 2026-10-13 (#749–#754, 6 tasks added at once): every task had one attempt at a time; win's
  codex attempts ended `gone | start failed: codex: program not found` (no `codex` on the Windows host, limits say
  codex 4 / claude 0) and mac took them afterwards.
- Cleanup 2026-10-12 before the repo transfer: the 5 open v3 PRs (594, 599, 628, 661, 719) closed unmerged, all
  branches but `main` deleted, 17 worktrees removed; uncommitted v3 files are in `~/Documents/taskq-archive-2026-10-12.tar.gz`.

## Waits on the owner
1. A week clean on v4: delete `taskq.py`, `host_service.py`, `runtimes/`, `docs/legacy/` (§ 9).
