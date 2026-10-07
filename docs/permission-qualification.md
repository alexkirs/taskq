# Permission observation qualification receipt (#176)

Scope: Wiki spec revision `6b9027b29bf43d18a24d2cc96ef98d314f396794`, standalone macOS routes on one machine
(`macbook-m2`). This is fixture and read-only probe qualification. It is not a live multi-project rollout.

## Live read-only readbacks (2026-10-07, machine clock UTC)

Both ran through `taskq runtime-status --json`. Neither sent a turn, replied to an approval, or changed a policy.

| Field | Codex thread of #191 | Claude worker of #176 (this session) |
|---|---|---|
| status | `unknown` | `unknown` |
| observed_at | `2026-10-07T18:45:59.584995+00:00` | `2026-10-07T18:45:59Z` |
| event_at | `1791398740.499` (last item event, epoch s) | `null` (CLI exposes no event time) |
| metadata_updated_at | `1791398740` (thread `updatedAt`) | `2026-10-07T18:45:34.064Z` (job `updatedAt`) |
| runtime state | `active`, `activeFlags: []`, last turn `inProgress`, no running typed item | `status: busy`, `state: working` |
| permission_requests | `[]` (none seen on this connection) | `[]` |
| effective launch policy | `unknown` in thread/read; `taskq codex-read` shows the recorded last-turn policy: `workspace-write`, `approvalPolicy: never` | `permission_mode: dontAsk` from job `respawnFlags` |
| session_link | `open.html#codex://threads/<thread>` | `https://claude.ai/code/session_<id>` |

Reading: an active Codex thread with empty `activeFlags` and no running typed item stays `unknown`. It is not
reported as active or as "no approval pending".

## Supported fields per runtime

- Codex app-server: `item/commandExecution/requestApproval`, `item/fileChange/requestApproval` and
  `item/permissions/requestApproval` server requests carry the request id, `threadId`, `turnId`, `itemId` and
  `startedAtMs`. `serverRequest/resolved` removes the request. The decision (approved or denied) is not in that
  event, so taskq does not report it. Delivery of these requests to other connections is not qualified: a short
  status connection can miss a request sent before it opened. `activeFlags: ['waitingOnApproval']` is the
  connection-independent signal, without an id.
- Claude `claude agents --json`: `status` (`busy`/`idle`), `state` (`working`/`blocked`/`done`/`stopped`/`failed`)
  and `pid`. No approval event, request id or event time. The job record gives `respawnFlags` and `updatedAt`.
  `blocked` means the session waits on the owner. Seen live, it waited on a taskq answer, not on a permission.
- DOT and other `[runtimes.*]` executors: no qualified status source. They stay `unknown`.

## Fixture checks

`tests/test_permission_qualification.py` (8 tests) and `tests/test_runtime_observation.py`:

- waiting: a request makes `waiting_permission`, worker `busy`, dedup key `permission codex <thread> <ids>`.
- dedup: the same key wakes the PM once. A new request id changes the key and wakes again.
- missing id: a `waitingOnApproval` flag only gives key `... flag` and an empty request list. No id is invented.
- approved/denied: after `serverRequest/resolved` the state is `unknown` until typed terminal evidence. The
  decision is not reported.
- stale: an old `event_at` stays apart from `observed_at`. A pending request keeps the worker `busy`, so the
  120-minute stale release and the idle nudge do not touch it.
- unreachable: no app-server socket gives `None` liveness and `unknown` status with the exact error.
- metadata time: Codex `updatedAt` is `metadata_updated_at`, never `event_at`.
- launch policy: Codex requests `approvalPolicy: never` with `workspaceWrite`. Claude pins `dontAsk`. No bypass flag.
- Claude: `blocked` is `busy` with or without pid: no nudge, no dead or stale release. A terminal status needs
  no pid plus a terminal `state` (`done`/`failed`/`stopped`). No pid with any other or a missing `state` is
  `unknown`: only the 120-minute stale release applies. An unlisted session is `unknown` too.
  `tests/test_taskq.py` `test_tick_keeps_a_blocked_claude_without_pid` checks this through a full tick.

## Changes in this delivery

- `taskq/worker.py` `runtime_status`: adds `metadata_updated_at` and `effective_launch_policy`. Claude readback
  reports the CLI `status`/`state` and the `--permission-mode` from job `respawnFlags`.
- `taskq/tick.py` `liveness`: a Claude job in `state: blocked` is `busy`, with or without pid. A job without pid
  is `dead` only in a terminal `state`, else unknown. Before, a blocked job with a live pid could be nudged, and
  one without pid was released as dead (review of `796f90c7`).
- `tests/test_taskq.py`: the existing dead-worker fixture gets `state: stopped`, the shape the CLI lists; a new
  tick-level control for a blocked Claude job without pid.

## Not qualified (live limitations)

- No real permission prompt was forced. The owner approval in the UI and the same-worker continuation are not
  demonstrated live. Fixtures prove fail-closed handling: a pending request keeps the claim, gets no nudge,
  no requeue and no approval reply.
- Claude workers run with `dontAsk`: unlisted tools are denied without a prompt, so taskq-launched Claude workers
  do not wait on permission. A Claude session opened in the app without `dontAsk` can prompt. The CLI does not
  show that.
- Codex requests sent before the status connection opened are seen only through the `waitingOnApproval` flag.
  That flag has no request id or time.
- Unreachable Codex for more than 120 minutes without issue activity still falls under the existing stale release.
- Scoped launch policies other than the current pins, and a cross-host launch gate, are not covered here.
