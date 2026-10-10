# Session contract qualification — isolated #601 continuation

Status: proposal and disposable experiments, not production implementation.
This opening status and slice checkpoints are historical. The current supported boundary is
the release-review/native-receipts section in [taskq.md](../taskq.md): schema1 model workers
remain separate from bounded schema2 artifacts; generic app drain and legacy recovery are
unsupported. A controlled-artifact PASS does not qualify the ordinary model-worker route.
Base: qualified c08678b3e2c0ed80bdcdc008bbcbd41bec6a7c44 / contract18756cc9e5b0.
Owner provenance: Sentinel_c6f51941cf948191a1b85840a6fc59f1, P1 investigation.
No intake, claim, order or result has been written to the blocked TaskQ board.

## One canonical workstream

Extend open #601 (owned by Windows PM) after legitimate board recovery, preserving
ownership. Its PM/ARM scope becomes the end-to-end session obligation/receipt contract;
this enlargement is proposed, not a changed board assignment. Reuse accepted #634
tick-hygiene design; #603 supplies existing shared guard, not permission to replace it.
#636/#637 own hook qualification and #638 owns network-stage diagnosis. Link their
findings; do not create duplicate tickets or edit their active implementation.
#597 supplies question identity; #612 remains the external reporting boundary.
Windows PM was asked to coordinate read-only for this investigation to avoid overlapping
implementation. The main checkout and its modified taskq.json are preserved.

## Roles, obligations and evidence

| Role | Expected action | Required receipt | Failure/question response |
|---|---|---|---|
| PM | Reconcile every owned task's current role obligations, including remote/unknown, and report unresolved problems | Tick completion plus project-qualified problem rows and exact evidence references | Diagnose technical failure within preserved scope; relay genuinely new product/security choices |
| ARM | One targeted wait per selected project, deliver exact event, then continue next wait | Recipient receipt and next verified running wait; send acceptance alone is insufficient | Preserve unacknowledged event and authorization; classify stop, generic abort, loss or rejection; recover only after previous wait absence |
| Supervisor | Decide worker order, review submitted exact result, close/rework/ask | Board order/result/decision receipt joined to its identity and obligation | Explain missing order, failed runtime, pending review/CI or owner choice; never pretend doing means running |
| Worker | Execute one ordered task, preserve artifacts and submit result or blocker | Board result receipt for exact SHA/checks, not completed outer turn | Preserve candidate after failed submission; bounded handoff retry only with known no-effect outcome and current authorization |
| Tick | Execute qualified helper operations and reconcile receipts | Admission, execution/application and board outcome remain separate | Surface unknowns even with exit0; no invented supervisor order or unsafe takeover |

Proposed operation identity joins project/task/current role/obligation/event version and
operation kind. Native continuation may change turn/session IDs, but does not create a new
obligation or retry budget. Board issue state remains the sole durable authority; no local
database or shadow queue. Append-only board event history supplies compacted identity.
Before adopting any fields, resolve the existing event/schema compatibility and migration.

Operation stages: reserved -> admitted -> received -> applied -> result-recorded. Each
stage needs its own attributable evidence. An ack records delivery under the existing
contract; it cannot prove application or result. One responsible ack actor per delivery
boundary is a design option, not an activated change to current sender behavior.
Lost authoritative response becomes UNKNOWN. No second effect until an exact runtime/board
reconciliation proves the first outcome or confirms no effect. Adapter deduplication must
be qualified; an invented operation ID does not make existing send idempotent.

Deadlines expose an unresolved obligation for diagnosis. They never expire authority,
guard tokens, process identity or retry budget. A runtime deadline does not mean a role
died. Foreign/missing identity, retained writer, unreadable controller or in-flight effect
blocks replacement despite owner authorization; the authorization persists while proof
is obtained. A real explicit owner stop blocks continuation; generic tool abortion alone
does not revoke the whole assignment. Actual tool rejection requires exact provenance.

## PM report contract proposal (R6)

Keep task status distinct from runtime state, application progress and approval/dependency
wait. Every unresolved owned-session problem has project/task/role, evidence freshness,
verified execution state or unknown, obligation, blocker, responsible actor and exact next
action. Human choice appears only when a new decision is required, with current question
version and preserved prior authorization. Unchanged problems remain visible without new
comments/questions every tick. Board-unavailable/guard-blocked paths still emit a read-only
diagnostic report; no fabricated fresh task table. Remote roles require owning-host proof,
not omission from local runtime inventory. Private paths/transcripts/credentials stay out
of public reports. Exit0 reports command outcome, never automatic proof of recovered work.

## Incident fixtures and planned experiments

| Incident | Deterministic oracle | Real boundary still needed |
|---|---|---|
| Guard retained / unreadable video PM | No effect; explicit controller drain/reconcile action, never PID/age deletion | Addressed durable-controller stop/drain and exact-token recovery |
| Idle CLI / loaded app writer | Authorized replacement still blocked by writer; artifact preserved | Supported targeted unload on owning app-server and release readback |
| App-steered auth missing / CLI succeeds | Surface-specific context problem; no credential copy or broader policy | Same-role effective launch context and harmless auth metadata |
| Stuck after-hook | Exited worker is not evidence that follow-up pass ran | Hook termination plus actual pass receipt |
| pong_timeout / lost wait | Unknown delivery retained; absent-wait proof precedes recovery | Supported transport, interrupted/repeated delivery and next wait |
| Supervisor doing without worker/order | Visible initial-order obligation, no synthetic run order | Same supervisor makes and records native order |
| Candidate produced, no board result | Handoff incomplete despite successful turn/CI | Same-role exact result receipt |
| Sender/PM competing ack | Known guard refusal is visible; no lost event or false delivery failure | Chosen ack actor or qualified duplicate receipt reconciliation |
| Stale question number | Version mismatch refuses changed action | #597 stable identity integration |
| Native Windows / WSL mismatch | Tooling blocker distinct from game runtime; no inferred WSL approval | Native Windows fixture and supported project tool repair |

Run crash injection at reservation, effect admission and receipt persistence. Repeat the
same event, continue with new turn IDs, interrupt/restart observation and change blocker
classification: zero duplicated effects or fresh retry budget. Mutate safety boundaries
to verify that the tests reject broken behavior. This is model sensitivity, not production
regression proof. All fixture effects remain memory-only, using no real runtime/API.

## Bounded real qualification gate

After reviewed spec/implementation slice, use one disposable task and supported transport,
no paid calls/security expansion. Preserve operational waits and all existing work.
Prove idle wake -> actual PM pass -> visible diagnosis -> already-authorized bounded repair
or current owner question -> exact result receipt -> next running targeted wait. Repeat
event and interrupted flow; record each gap, actual commands, current IDs and outcomes.
At present #353 and video PM cannot be safely stopped through available tools; no positive
real recovery claim can be made. Historical #350 close is a positive research-handoff
receipt, not qualification of this candidate's new lifecycle mechanism.

## Consequential choices for review

1. Observation/report-first with no new automatic recovery (recommended first slice),
   versus enabling only independently qualified idempotent same-role handoff repair.
2. Extend #601 acceptance coherently versus a linked successor if its current Windows
   implementation is already active; do not split overlapping fixes or rebind claims.
3. Board receipt/schema expansion requires explicit compatibility/migration planning and
   all-host stop/drain; retain current guard semantics pending qualification.
4. Ack responsibility must be singular or duplicate-safe with qualified reconciliation;
   no silent unilateral sender contract change.

The initial model changed no operational code. The subsequently authorized candidate observation slice changes taskq.py only in this isolated branch; launcher, installed runtime policy, production board and global settings remain unchanged.

## First executed evidence

Command: `python3 experiments/session_contract_probe.py` — 11 tests passed.
The runner repeated this same suite with five controlled `PROBE_MUTATION` values:
guard bypass, ignored app writer, retry after unknown admission, eager application
receipt and stale-question admission. Every broken variant failed its relevant oracle
(1, 1, 5, 1 and 1 failures). Baseline wall time 0.066s; variants 0.041–0.061s.
Machine-readable outcomes: `experiments/results.json`; `git diff --check` passed.

The report-routing table covers ten recorded incident categories but is a declared
fixture oracle, not a production classifier test. Executed behavioral experiments cover
crash/reservation boundaries, authorized replacement gating, repeated/interrupted waits,
exact result receipts, guard/ack contention and stale questions. No actual app-server
unload, network failure, hooks, Windows tooling or paid operation is simulated as PASS.
These are synthetic sensitivity results; historical red-before-fix and production fix
proof remain unavailable. Next slice should plug the reviewed observation classifier
into existing tests/test_single.py and qualify the real supported boundary separately.

## Implemented isolated observation slice (2026-10-10)

Candidate branch `codex/session-contract-qualification`, base c08678b3e2c0ed80bdcdc008bbcbd41bec6a7c44. Spec amendment precedes implementation. Optional `status --diagnose` rereads selected task metadata before probing current identities; `tick --diagnose` preserves a guard refusal and its nonzero outcome while printing read-only diagnostics. Default commands and installed release remain unchanged. GitHub and GitLab adapter fixtures verify GET-only list/metadata reads. No guard release, queue change, sender, runtime launch, policy change, merge or publication occurred.

Actual focused verification: `python3 -m unittest tests.test_single.ObligationDiagnostics tests.test_single.Coordination tests.test_single.EventDelivery tests.test_single.Tick`: **117 tests passed, 25.018s**. This includes 18 diagnostic tests, fresh reassignment/closure/unavailable reads, distinct pending recipients, exact-command retry, bounded structured CLI evidence, writer observation and both native adapters. Independent rereview accepted this opt-in read-only slice after correcting collapsed failures, unstable evidence IDs and hidden writer-verification gaps.

Russian fixture report: `experiments/diagnostic-report-ru.md`; explicitly synthetic, not a live board snapshot. Prototype experiments remain separate evidence and do not qualify actual runtime behavior. The preimplementation unsupported-flag failures establish a feature gap, not reproduction of historical production failures.

**Qualification limits at this first checkpoint:** latest CLI turn only; superseded below by existing-log retention. App-only commands and native Windows observation remain unqualified. Recognized output markers describe symptoms, not proven root causes. Runtime/board unknown remains unknown; no diagnosis authorizes retirement, recovery or application-writer unload. Durable unsettled command obligations require a further specified receipt/persistence design before claiming complete reporting. Production activation remains held.

## Extended qualification checkpoint

Read-only candidate `status --diagnose` calls on two consuming projects exited0 without stderr.
This qualifies report execution, not an operational pass. Project-specific snapshots and full
operational handoffs are retained in a private provenance archive rather than distributed here.

Observed symptoms include a held project guard, a writer descriptor with failed native result
submission and absent board receipt, legacy two-field handles, pending questions/events, remote
roles unknown and blocked prerequisites. A descriptor is not active-turn or unload proof; an
inaccessible controller is not proven safely orphaned.

Retention uses the existing append-only log, at most16MiB, scoped by structured thread.started/thread_id to the current observed SID. Unknown attribution/truncation/unreadability is itself an unresolved row. Clean turns and process restarts do not settle earlier commands. Exact structured successful-command retry clears only its own session's command failure; matching board result receipt suppresses failed-result reporting. No new persistent store, board schema, migration or state write was introduced. Missing/app-only logs remain a completeness gap; this is reconstruction of available evidence, not guaranteed durable retention after deletion/rotation.

26 diagnostic regressions cover GET-only GitHub/GitLab guard/metadata routes, fresh role/assignee/closure/failure reads, remoteunknown, distinct event recipients, question survival and report references across module restart, multi-command failures and retry settlement, cross-session replacement attribution, writer holder and verification failure, legacy completed handleunknown and non-task dependencies. Independent rereview found no remaining blocker for this opt-in observation slice after identifying and fixing predecessor-history attribution.

Baseline vs candidate `cmd_status` used identical hermetic board/runtime inputs (fixtureSHA25664fbc0e99fc2172bcbd0216d9c65ecf0b3c2bc7404064636c9a1be66ff38429f): baseline preserves question but omits guard, pending delivery, auth symptom and non-task prerequisite diagnostics; candidate reports all five. See experiments/baseline-comparison.json and compare_reports.py. This is a controlled feature-gap comparison, not historical production-failure replay.

Applicable full command: `python3 -m unittest tests.test_single`. Real baseline Git worktree:235 tests44.087s, OK/skipped17. Candidate:261 tests44.003s, OK/skipped17. First broad run with /var temp paths had existing path-canonicalization test failures; rerun with physical /private/tmp avoided that environment mismatch. Initial archive-only baseline failed its Git-provenance test because there was no Git metadata; rerun in a proper detached worktree passed. No application/network/model workload was used to obtain green tests. Skipped live/platform tests are unqualified, not PASS.

Maintenance observations remain historical evidence, not a current operational checkpoint.
The reusable preservation and qualification checklist is [maintenance-handoff.md](maintenance-handoff.md).
Detailed per-project/host records were archived with their original bytes and Git provenance.

Final checkpoint Mac full suite (physical TMPDIR=/private/tmp):261 tests43.891s, OK/skipped17.
Windows pause reports did not prove all application processes' ownership or all-app quiescence.
No canary, recovery, installation or queue-resume success is inferred from those observations.
