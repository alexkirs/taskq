# Use-case-driven menu proposal

Status: overall layout approved. Exact shortlist limits, recent ranking and emergency-stop mechanism remain review candidates; the capability is required. Runtime evidence is recorded in current-menu-audit.md.
Basis: [UC-01–UC-07](../use-cases/README.md), approved [M1–M12](menu.md), [shared D1–D14](https://github.com/taskqorg/specq/blob/main/design/operator-ui.md).
The prototype uses invented fixtures. It demonstrates navigation and prioritization, not service operations.

## A1. Stable controls, situational content

- Keep view selection, host admission and pause in a fixed top strip (M1). Add a fixed Controls entry beside pause.
- Keep Work mode directly below: a short applied-scope phrase; open the single M12 editor.
- Follow with Attention, AI input, pinned/recent projects and Settings. Do not reorder these sections by frequency.
- Adapt Attention to unresolved decisions, failed tasks and admission problems. Empty Attention collapses without placeholder cards.
- Pinned projects stay in owner order. Show up to three unpinned recent projects as an initial proposal; All opens the searchable list.
- Define recent by explicit interaction: opening a project/task or sending a project-scoped request. Background activity does not make a project personally frequent.
- Do not collect gaze or infer importance from dwell time. Frequent use may suggest pinning later; no automatic control rearrangement in this version.
- Freeze row order while the menu is open. Update values in place; signal new items without moving the pointer or keyboard target. Reorder on the next opening or explicit refresh.
- Project shortlist affects navigation only; attention and totals still cover all observed projects (M2).

## A2. Explain the queue

Show the effect, cause and direct next action together.

| Situation | Home treatment | Detail/action |
|---|---|---|
| Healthy work | Host admission state and nonzero project counters | Project shows active and next eligible tasks |
| Owner decision | Linked task and short question | Open the answer context |
| Failed task | Linked task and failure reason | Inspect result/log before retry |
| Capacity full | Neutral waiting reason, not failure | Per-project/runtime used and limit |
| Dependency wait | Neutral blocked count in project | Link the unmet prerequisites |
| Scope mismatch | Show when work was requested but nothing qualifies | Matching/excluded preview in Work mode |
| Load gate | Admission held; current work continues | Measurements, freshness and reopen condition |
| Stale/unreachable service | Unknown state, affected project/host and age | Service details and known log path |
| AI confirmation | Pending change and target scope | Raise its conversation window |

Proposed Attention order: service failures that prevent reliable control, owner questions/confirmations, failed tasks, then actionable admission blockers. Ordinary capacity/dependency waits stay in project detail unless they explain stalled requested work.
Show one representative item per cause group, up to two items plus All N initially; counts include hidden items. A critical service fault cannot disappear because its project is not recent.
Multiple blockers remain available in detail; do not pretend one cause explains every task.

## A3. Control without ambiguity

- Pause means stop new admission on named projects on this host; current attempts continue (M5). Show targets/effects before confirmation.
- Mixed admission opens scoped controls; it is not a blind global resume toggle.
- Controls remains reachable from every menu level. It contains scoped admission, service diagnostics and the existing lifecycle commands.
- Host-wide pause must not claim to stop execution on remote hosts. Readback and partial failure remain per target.
- Emergency stop is required; use [E1–E5](emergency-recovery.md) for named targets, preserved work, fencing and explicit recovery. Do not relabel Pause as Stop or imply the prototype terminates processes.
- Pending/unreachable pause never displays successful pause. Keep the last known state and application result distinct.
- Restart, stop until next login and uninstall stay under the service target; they are not interchangeable recovery shortcuts.

## A4. Use-case gaps and next design slices

| Case | Existing design entry | Missing design work |
|---|---|---|
| UC-01 Setup | Projects +, M10 | Connect/setup review, explicit switch from planning to admission |
| UC-02 PM | AI input, attachments, conversation window | Task references, decision context, direct-action boundary Q1 |
| UC-03 Plan to build | AI conversation | Specification/plan review and explicit implementation authorization |
| UC-04 Urgent | AI request, task/project detail | Visible p0, next eligible slot, truthful wait reason; no implied shared pool |
| UC-05 Natural-language focus | Work mode, M12 | Matched/excluded tasks, unsupported intent, future arrivals, confirmation/readback |
| UC-06 Five repositories | Project detail | Environment preparation/build/cleanup status; contract Q5 first |
| UC-07 Plan and parallel branches | AI plan review, project detail | Created/remaining tasks, dependency links, ready/blocked reasons; contract Q6 first |

The menu is the control and attention surface. Long plans, conversations and multi-repository diagnostics open detail views; they do not turn the tray into a full board.
Prioritize UC-05 next, then questions/failures and service controls; resolve UC-06/UC-07 contracts before designing controls that promise unsupported behavior.

## A5. Remaining decisions

- Stable controls plus adaptive Attention are approved; exact ordering and initial two-item limit remain proposed.
- Confirm pinned plus three recent projects and the definition of recent.
- Define emergency stop separately from pause; keep current behavior until approved.
- AI windows: one main workspace plus explicit detached conversations approved; see ai-workspace.md for attachments, retention and recovery.
- Finish scope semantics (Q3), per-project limits, plan review and cross-repository workflow (Q5/Q6).
- Settings categories were selected in the earlier design review; exact contents and service drill-down still need design.
- Icon overflow, graph bin count and Windows native legibility remain open in I5.

## Review fixtures

- Normal work: no fake alert; fixed controls and project shortcuts stay available.
- Question/failure: direct decision access without hiding projects behind full chat output.
- Urgent capacity wait: no false failure, limit bypass or estimated start time.
- Stale project outside the shortlist: global Attention still exposes it; admission is unknown, not paused.
- Large font/long title: full-width wrapping, reachable controls, no horizontal clipping.
- Live update: a new critical item is signalled without moving an existing click target.

## Shared-rule candidate

Propose in specq: adaptive operator interfaces keep control locations stable; adapt information by current effect and explicit user preference, not opaque frequency ranking. Preserve active pointer/keyboard targets during updates.
This extends D2/D5/D13. The owner approved stable adaptive navigation for this design; the product-specific ordering, limits and recent definition stay here.

## Prototype evidence

Browser review: dark-theme home, attention, urgent-capacity wait and stale-service fixtures inspected at the 360 px menu width. Pause review names projects/host and distinguishes admission from active work; confirmation changes only simulated state. Settings navigation checked.
At 20 px, the standalone preview frame clipped the bottom of the menu; large-font viewport sizing is not qualified. Light theme, narrow windows, native OS behavior, live reordering, real attachments and service actions remain unverified.
