# #646 — approved ARM/PM split, stage 1

Status: owner approved design; local implementation preparation. This document
is not installed behavior or permission to replace existing schedulers.

## Existing c25 behavior and gaps

`tick --headless --quiet` executes a guarded queue pass without the human table.
Model grants account for TaskQ worker/supervisor turns, not conversational PM or
external Codex sessions. Explicit host/project providers enforce finite capacity;
roles/claims and heavy-resource leases outlive model turns. `wait --pm` selects
manager-addressed outcomes; native manager ACK is application acknowledgement.
The present ARM prompt describes a sender, not an independent execution role.
Conversation-idle delivery and the old immediate CLI wait/send loop are not a
qualified event transport. The lease/ACK patch settles closed-task model grants
from bounded ledgers and withholds that unsafe shortcut. It does not create a
cursor store, timer or new transport.

## Approved target and first implementation boundary

ARM owns execution scope (projects, task filters, interval, invocation ceilings).
Its named activation is idempotent by host + arm_id. Repeating a name shows its
state or an explicit scope/limit diff; there is no second timer. `stop` stops
admissions, not workers. Addressed cancellation is a separate command.
All ARM instances on a host share the same finite model budget; per-ARM/project
ceilings only restrict it. Shared claims and project reservations cannot be
postponed. Reuse the existing scheduler/runtime; no new service or second board.

PM owns subscription interests, independently of execution scope or write
permission. Initial snapshot includes unresolved questions and a cursor/freshness
boundary. Only meaningful started/completed/question/error/decision deltas follow;
unchanged is quiet. Status on demand includes last verified synchronization time.
Offline PM does not block independent tasks; unanswered questions stay pending.

Delivery ACK/cursor, owner decision and worker applied receipt are three distinct
facts. Per-subscriber cursors cannot consume another PM's events. Decisions bind
questionID + version + commandID; repeat returns the original result, stale or
conflicting answer is refused. Viewing never grants write authority. Dot child
executes locally and returns typed facts; parent renders them and forwards only
validated commands, not arbitrary task-output instructions.

## Minimal rollout and honest failure

1. Contract/model first: define typed observations and deterministic transition
   tests, including stop/repeat/scope change, duplicate delivery, offline PM,
   stale answer, two PM interests and two ARM competing for one host budget.
2. Implement one existing runtime route and board writer boundary. Providers do
   not offer universal CAS/fencing: specify and test the enforceable coordination
   guard; never call a read/PATCH pair atomic. Any missing cursor interval is an
   explicit reconnect gap, not recovered-by-assertion. Bound replay/backpressure.
3. Snapshot + cursor must have no silent skip window. Re-read/replay overlap can
   be deduplicated; unavailable observations preserve pending work and show stale
   status. Critical failures stop affected admissions and report once.
4. Disposable integration: standalone Codex and dot typed-event/render roundtrip;
   existing scheduler repeat/stop/reconnect with no overlapping new timer.
5. Exact candidate tests and separately approved staged rollout replace one
   existing activation at a time, retaining rollback and pending receipts.

Fleet overview UI is Later. Arbitrary external sessions, brokers and universal
resource drain are not implied by native model-budget proof. Existing Mac4 +
Win4 / project8 / Claude0 and held legacy tasks remain unchanged. #597 risk
approval is independent. Related #601 continuation, #597 UI, #642 plugins retain
their scopes; this stage is not duplicate implementation of them.
