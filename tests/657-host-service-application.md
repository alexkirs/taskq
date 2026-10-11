# Unified host service and onboarding candidate

Task: https://github.com/alexkirs/taskq/issues/657
Task: https://github.com/alexkirs/taskq/issues/600

The owner authorized one implementation session, one unified review candidate and one
qualified deployment. This branch adds the current-user host service and retains #600's
project bootstrap in the same source. The original #600 worker, result b3117422, PR661,
question and held live qualification are preserved; integration is not acceptance or an
application receipt for that worker. The twelve bootstrap functions are byte-for-byte
equal source segments, recorded in `657-onboarding-source-map.json`. Main CLI and shared
contract integration require the final common gates.

The service registers one exact existing project without adoption, uses OS-authenticated
JSON IPC, atomically caches control responses, and owns each controller's exact native
PID/birth under a dispatch epoch. Its process lock remains held through child settlement,
including observation/journal errors. Unknown operations cannot admit another child.
Only an exact terminal identity and output receipt can reconcile an interrupted owned
controller; model recovery and capacity ledgers are never cleared by this service.

The pre-integration service gate passed 492 tests with 18 platform skips on Mac. This is
development evidence, not the final unified commit qualification. Real isolated cadence,
claim, native dispatch refusal, IPC control, terminal receipt and release were exercised
with a local file board, zero model limits and a failing local fake provider CLI. That
fixture proves the lifecycle boundary; it does not prove remote provider behavior or a
published automatic update. Targeted service/bootstrap gates also passed before freeze.

Windows native IPC/ACL/cadence, the final common full gate, independent review, exact-head
CI, published release metadata and deployment receipts remain separate required gates.
No new #600 private repository/folder has been created. If necessary, one concrete
combined live qualification request must name the final common SHA, native account/IDs,
exact resources, effects and stop rules. Earlier #579 fixtures authorize no new resource.

Migration requires a fresh reviewed old-source settlement checkpoint, preserves the
existing single OS scheduler and fixed finite invocation, and refuses unknown writers.
Rollback releases only a paused, settled service epoch before restoring that same old
source. It never restores live ledgers. Existing #601/#612 recovery blockers, other holds,
Mac4/Windows4/shared8/Claude0 and production configuration remain unchanged.
