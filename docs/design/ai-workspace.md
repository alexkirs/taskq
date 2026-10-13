# AI workspace

Design target under [R3/R9](../../taskq.md). Not a claim of runtime support.
One main window plus explicitly detached conversations is owner-approved. Retention, attachment budgets and cancellation semantics below are proposals pending implementation review.
[Menu M7](menu.md#m7-ai-entry) · [Use cases](../use-cases/README.md) · [Current gaps](current-menu-audit.md).

## C1. Three distinct collections

| Collection | Contains | Opening does |
|---|---|---|
| Conversations | Owner messages, operator replies, proposals, attachments and operation outcomes | Raises the conversation; never resends |
| Work sessions | Worker/reviewer attempts with project, issue, runtime, host and observed state | Opens a verified local agent permalink; remote sessions remain informational |
| Command history | Last 20 distinct submitted request texts | Fills the composer; never executes |

A conversation is not an agent process. A worker session is not a PM conversation. A completed answer is not a completed board task.
No background scan of unrelated Codex/Claude chats; list only TaskQ-known conversations and recorded attempts.

## C2. Windows and recovery

- Menu Send opens the main AI window with the submitted conversation selected. Existing workspace is raised.
- Main window: Conversations / Work sessions, search, New conversation, selected content and persistent composer.
- Conversation list: needs attention, active, recent; Archive is a separate view. Status, project and unread state accompany the title.
- Conversation picker in the menu returns to any active/recent conversation, including detached windows. Search reaches older items.
- Detach is explicit. One window owns a conversation's editable view; its list entry remains reachable. Reopen raises that window, not a duplicate.
- Return to main reattaches the same conversation. Closing a window hides it; it neither cancels requests nor archives work.
- Restart restores drafts and a recoverable list; do not reopen every detached window automatically. Restore the selected conversation when the workspace is next opened.
- Confirmed operations are identified by request ID/revision independently of window identity. Two views cannot apply the same revision twice.
- Narrow window: switch between list and conversation with Back; preserve selection, drafts and scroll position.

## C3. Composer and context

- Show explicit project set, host and selected conversation before Send. Menu scope and conversation scope are not silently interchangeable.
- Changing context affects the next message, not previous messages or applied work; show the change in the conversation.
- New conversation starts a separate context and draft. Continue replies in the selected conversation, never whichever request finished last.
- Enter sends; Shift+Enter inserts a newline; IME composition never sends. Attach-only requests require an explicit action or clarification.
- Attach button and drag/drop accept files and images in both menu and window. Chips show name, size, transfer state, Remove and image preview.
- Proposal: PNG/JPEG/WebP, UTF-8 text/Markdown and PDF; reject folders/executables. No silent truncation. Confirm actual runtime support and size budgets before shipping.
- Before Send, report unsupported type, unreadable file, unavailable vision/PDF handling and over-limit input. Keep the draft and other attachments.
- Freeze sent content by attachment ID/hash, not a mutable local path. Show what will be shared and with which agent. Do not put file content in command history or logs.
- Removing a draft attachment does not alter a sent message. Storage/cleanup follows C7; no automatic deletion of the source file.
- Show agent/runtime once known. Respect configured model, effort and permissions; do not silently switch providers on failure.

## C4. Request states and actions

| State | Visible content | Actions / transition |
|---|---|---|
| Draft | Text, context, attachments | Send; keep if rejected |
| Queued | Accepted request ID and target service | Waiting reason; cancel only if backend can acknowledge it |
| Running | Agent/runtime, elapsed time, honest progress | Follow-up draft; no invented token stream or percent |
| Needs answer | One question, options and relevant evidence | Answer in this conversation; preserve the full chain |
| Proposal | Exact targets, old/new values and effects | Confirm, revise or decline; no implicit apply |
| Applying | Operations being checked/applied | No second confirm or automatic retry |
| Applied | Per-operation readback and links | Continue or open affected tasks |
| Partial / apply failed | Verified successes, failures and unknown outcomes | Refresh state; propose remaining operations only |
| Result | Answer with source task/status references and freshness | Follow-up, copy, open references |
| Refused | Unsupported operation/data/authority and reason | Revise or use an allowed route |
| Failed / interrupted | Known failure and uncertain delivery boundary | Retry after reconciliation; keep draft/context |
| Cancelled | Backend-acknowledged cancellation | Continue with a new request; no claim of rollback |

While a request runs, allow drafting the next message. Proposal: one in-flight request per conversation; Send waits until the active request reaches a compatible terminal/clarification state. Different conversations may proceed within confirmed operator capacity.
Closing a window is never Cancel. Do not ship an enabled Stop button without backend cancellation and readback.
A newer message or changed target state invalidates a conflicting proposal. Reconcile before offering Confirm again.

## C5. Scenario contracts

| Use case | Conversation treatment |
|---|---|
| UC-01 Setup | Folder/account/repository/visibility review; exact effects and registration readback; empty installation must have a supported bootstrap path |
| UC-02 Manage | Linked task references; clarify goal/acceptance; compare duplicates/conflicts; direct-action authority follows Q1 |
| UC-03 Plan | Specification links and proposed tasks; planning does not authorize development; show a separate implementation decision |
| UC-04 Urgent | Review p0 plus project/runtime; show dependency, capacity and gate reasons; no preemption or shared-pool promise |
| UC-05 Focus | Original request, matched/excluded/uncertain tasks, current/proposed scope, future-arrival semantics and readback |
| UC-06 Multi-repo | Explicit repository membership and stages: prepare, build, work, verify, cleanup; require Q5 before claiming support |
| UC-07 Plan to cards | Preview tasks/dependencies, independent branches, owner questions and existing matches; creation receipt per card; retry only missing writes |
| Task question | Link exact issue, question and attempt; stale answer refuses or refreshes, never answers a replacement question |
| Failed task | Show failure evidence; diagnosis is separate from retry; proposed requeue identifies the attempt and consequences |
| AI/service outage | Keep history/draft; distinguish agent failure from failed task; opening never relaunches work |

The current tool-less operator cannot perform arbitrary project research or filesystem work. UI may propose board tasks or hand off to a known PM session; broader agent authority needs an explicit R3/R9 contract, not a disguised tool call.

## C6. Work sessions

- Active view groups by project; each row has status, linked issue/title, worker/reviewer role, Codex/Claude identity, executing host and age/freshness.
- `codex-app` uses Codex identity. Remote: network icon + host, no fabricated local permalink or tmux action.
- Local permalink opens the recorded session; missing identity disables the action with a reason. Issue access remains available.
- Attempt history shows ended/replaced sessions separately; old state is labelled historical. A session ending does not prove task completion.
- Session search matches project, issue, title and host. No token-cost/progress metrics unless the service supplies them.
- A live task may receive steering only through a separately reviewed backend operation; the UI must not send text directly to an unrelated session.

## C7. Persistence and history proposal

- Persist conversation ID, context, messages, attachment references, request/revision links, operation receipts, draft and window ownership locally.
- Board task state stays on the board (R1). Conversation storage does not become a second scheduler or task database.
- Suggested first version: retain conversations until explicit deletion; Archive hides terminal conversations but preserves access. Active/unresolved work cannot be silently purged.
- Before storage is full, stop accepting new attachments with a clear explanation; do not silently drop history. Budgets and retention UI require owner review.
- Delete conversation explains local messages/attachments removed and board tasks preserved. Never delete source files or running work. Unresolved application must be reconciled first.
- Clear command history affects only command text. Archive, window close, request cancellation and deletion are distinct actions.
- On restart, mark interrupted requests truthfully and reconcile operations before retry. Do not reuse stale Confirm controls.

## C8. Acceptance

| ID | Evidence |
|---|---|
| C-A1 | Interleaved replies in two conversations keep their context, draft and proposals separate. |
| C-A2 | Reopen/detach/reattach/close never resends or creates duplicate editable ownership. |
| C-A3 | History selection fills only; conversation selection restores; session selection opens only a verified target. |
| C-A4 | Attach/remove/preview/error works in both composers; failed upload preserves the draft; sent bytes are immutable. |
| C-A5 | Confirmation includes exact targets/effects; double-click, stale revision and second window cannot apply twice. |
| C-A6 | Partial apply exposes each result; retry cannot duplicate successful task creation. |
| C-A7 | Restart/offline preserves context and displays unknown/interrupted state; cancellation needs acknowledgement. |
| C-A8 | Remote/missing permalinks cannot launch local sessions; local attempts use the correct agent identity. |
| C-A9 | UC-01–UC-07 have an explicit result, supported route or honest unresolved contract boundary. |
| C-A10 | 16/20 px, narrow/short windows, keyboard, IME, screen reader, both themes; composer and Back remain reachable. |
