# Handoff
Operational state only. Rules are in [taskq.md](../taskq.md); the work order is in [refactor-plan.md](refactor-plan.md).
Refresh every line with a fresh read before acting on it (R12).

## Hosts (v3, paused)
- Mac: service paused and stopped, LaunchAgent unloaded, host leases released.
- Windows: service paused, scheduled task present; reserved and waiting grants are still held.
- No other host is registered. Delivery to a host is not proof it ran.

## Queue
- 706–715 closed as not planned; PR 716–718 closed unmerged.
- Held grants stay held until the § 9 migration. Do not force-clear grants, restore live DBs, resume the queue or
  infer fencing from process absence.
- PR 719 (test contracts) is on hold, unaccepted.

## v4 status
- Spec: `taskq.md`. Code: `src/` (Rust), `tests/golden/fold.json`. `cargo test` and `taskq selftest` pass on macOS.
- Live board read (`selftest`, `status`) works; nothing was written to the board.
- Windows and Linux: only CI runs them; no host run yet (unknown, R12).

## Waits on the owner
1. Review and merge PR 720 (`refactor/plan`).
2. Two-host claim test on the live board (§ 3) before any v4 `service run`.
3. Before cutover: decide the 4 v3 `ask` tasks #612, #601, #597, #588 (answer in v3 or park) and whether #643, #638
   (no v3 block) are tasks. 8 `later` tasks have no recorded reason (#664, #654, #652, #651, #650, #647, #642, #640).
4. Cutover (§ 9): an agent applies `CHANGELOG.md` v4.0.0 with v3 stopped on every host; then v4 on Mac, then Windows.
