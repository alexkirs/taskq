# Emergency stop and recovery

Required capability approved. E1–E5 describe the mechanism; [taskq.md](../../taskq.md) § 3 and § 4 implement the host hold, liveness reconciliation and fenced recovery (`taskq service hold|release|recover`). The tray control, graceful/force stop, per-project held entries and selective resume remain proposals. Product entry: MP6. Pause only blocks new admission; service restart is not recovery.

## E1 Scope and review

- Fixed stop icon is reachable on every menu page; accessible name Emergency stop. Distinct shape from pause; red only for this consequential action/status.
- Review names this host, projects, worker/reviewer/operator attempts and in-flight service operations. Default scope is all local managed work; remote hosts are explicitly unaffected.
- Confirm once. Stop new launches and state-changing operator applications immediately; keep status, diagnostics and stop/recovery control available. Report operations already past a cancellable boundary as uncertain until reconciled.
- Do not kill by executable name. Address recorded process/session identity and owned descendants; unrelated terminals and agents remain untouched.

## E2 Durable stop

- Persist a host-owned emergency admission interlock with explicit per-project held entries before terminating work. It survives service restart, logout and app reopening; ordinary Play cannot clear it.
- Proposed R1 exception: local interlock stores host admission safety state and operation identity, not a second task queue. Board remains task-state authority.
- Request graceful cancellation where supported; expose unresolved attempts and a separate Force stop action. Force requires verified ownership and warns that unsaved editor buffers may be lost. Grace timeout is adapter-specific, measured and shown; no hidden universal timeout.
- State per attempt: requested → stopping → stopped / failed / unknown. Only observed process/session termination proves stopped. Unreachable runtime stays unknown and blocks relaunch.
- Preserve worktrees, diffs, untracked files, logs and session references. No cleanup, reset or stash as an implicit side effect. Disk writes survive; memory-only edits and interrupted external effects are not guaranteed recoverable.
- Offline board: stop local execution and keep the interlock. Reconcile task events when reachable; do not report board retirement or resume early.
- Multi-host safety requires board-visible attempt fencing/hold before lease expiry can permit another host to claim. If board communication is unavailable, local stop cannot promise a global stop. Show that limit explicitly.

## E3 Recovery

1. Keep the host held. Read current board, service, process/session identities, leases, workspace status and last operation receipts. Evidence: [specq verification](https://github.com/taskqorg/specq/blob/main/engineering/verification.md).
2. Show one row per interrupted attempt: task, runtime/host, confirmed stopped or unknown, preserved work, last known action and blocker. Unknown remains unselected and disabled for relaunch.
3. Retire/fence the previous attempt through the board model. Reject late output; prove no live old process before creating a new attempt. Do not replay external operations with uncertain outcomes.
4. Offer Inspect, Keep held, or Restart from preserved work for eligible rows. Default is Keep held. Same-session continuation is only available when the adapter proves it can bind the new fenced attempt safely; otherwise use a new session with preserved artifacts.
5. Review selected tasks, current permissions/limits/focus and intended admission state. Preserve the saved scope; explain tasks no longer eligible. Resume only named projects and explicitly requeued tasks. Previously paused projects stay paused.
6. Clear only reviewed project entries after checked transitions/readback; other project entries and remaining interrupted tasks stay held. Display partial recovery as Mixed, never all-running. Repeated confirmation/recovery requests use one operation identity and cannot duplicate launches.

Recovery is not rollback. Successful remote writes remain successful; uncertain writes need inspection before retry.

## E4 Contract and compatibility work

- Amend R1 (host interlock), R11 (explicit emergency termination), §2 (retirement/held transitions and late-output fencing), §4 (service admission/recovery) in the implementation deliverable.
- Check worker, reviewer and operator paths, including codex-app handoffs. Termination support differs by runtime; unsupported control must remain visible.
- Define interlock authorization, atomic persistence, startup ordering and minimum compatible service version. Older services must not ignore a held host after rollback/update.
- Choose board event/projection representation without adding an invented task state in the prototype. Emergency held is an execution condition; task counts retain their defined lifecycle semantics.

## E5 Acceptance

- E-A1: pause leaves current work running; emergency stops only owned targeted work.
- E-A2: crash/restart during stopping keeps admission held; duplicate commands do not relaunch.
- E-A3: partial termination and offline board remain explicit; no global-stop claim.
- E-A4: dirty worktree/untracked files preserved; no promised recovery of unsaved buffers.
- E-A5: late old-attempt output ignored; new attempt blocked while old liveness is unknown.
- E-A6: review precedes recovery; only selected eligible tasks resume under current limits/permissions.
- E-A7: previously paused projects and unselected tasks remain held; partial recovery is inspectable.
- E-A8: uncertain external effect is reconciled before retry; no duplicate publication/application.
- E-A9: non-cooperative runtime, old binary, app-session handoff and remote lease expiry have tested limits.
