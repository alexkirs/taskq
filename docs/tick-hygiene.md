# Tick hygiene — #634 design for review

Research and proposed contract only, 2026-10-10, based on `origin/main`
`f3bb2b0` and the assigned release `59d8fbf` (contract `f2d26647c657`).
No recovery, runtime, configuration or board schema is activated by this document.
Independent supervisor acceptance of the design precedes a separately approved implementation
slice. Normative behavior remains in taskq.md; on approval, integrate the accepted text there
before code changes (R13), rather than maintaining a second policy.

## Existing machinery and gaps

All implementation is in `taskq.py`, with regressions in `tests/test_single.py`.

| Existing location | Evidence and gap |
|---|---|
| `one_pass`, `coordination`, `executable` | Board project guard, fresh task reads, ownership/host/assignee and compatibility checks. Sandbox ticks are read-only. Missing another checkout's handle is unknown. Hygiene must use this path, not another scheduler, hook or lock. |
| `process_identity`, `process_state`, `stop_process` | Birth-bearing Windows/Linux/macOS identities distinguish PID reuse and foreign domains. Missing/legacy/unreadable identity is unknown. A different birth means the recorded turn is dead, never permission to kill the replacement process. |
| `Codex.state`, `resumable`, `send` | Matching running process means running. Exited identified turn plus latest `turn.completed` and local rollout means idle; other exited turns are dead. No handle means unknown. Local rollout alone proves neither idle nor ownership. `send` rejects running/unknown, but permits dead; it starts a new turn and is not inherently idempotent. |
| `runtime_state`, `Claude.state`, native Hermes bridge | Shared running/idle/dead/unknown capability. Legacy `alive=False` cannot prove idle. Claude resumes may change ID; native Hermes lifecycle is restart-nondurable. No unverified topology becomes a safe automatic resume by assertion. |
| `follow` | Pending exact-recipient answers/nudges go only to idle workers. Otherwise an idle worker gets a generic nudge after 120 minutes of board silence. Comment activity changes `updated_at`; it can mask an unfinished handoff. Every later 120-minute window can nudge again; this is not a per-incident bound. |
| `supervise`, `replace`, `move` | Ordered worker spawn, worker death notification, idle supervisor event delivery, first-death resume/respawn and second-death ask. A supervisor with no pending event can stay idle indefinitely even when a worker handoff or initial order is missing. Existing death counters reset on result/answer, not on proven recovered progress. |
| `pending`, `event_pending`, `acknowledge`, `seen` | Board exact-recipient event IDs and acknowledgements; successful delivery precedes ack. Lost ack can replay; not exactly-once. Preserve this, including legacy migration boundaries. Observation is never an ack. |
| §3 `action_payloads`, `move`, issue/role briefs | Latest exact ask/answer/requeue/result payload survives event compaction and unrelated writes. Use these existing identities/content for obligations; do not duplicate an answer ledger. A compacted event is not, by itself, proof of application. |
| `GitHub` / `GitLab` acquire, release, get, update | Shared orchestration; GitHub unique `taskq-coordination` label creation / GitLab unique configured board-list label provides the project grant. Release uses the exact label node ID / board-list ID; issue JSON carries task state. Both reuse normal body updates. No new adapter endpoint or external report is needed for hygiene. |

Read-only #608 history confirms owner answer at 03:08 UTC, a worker nudge, a PM recovery
note at 03:51 describing idle roles and unsubmitted result, then a result from the same
worker at 03:52. This supports targeted existing-worker handoff recovery; board history
alone does not retrospectively verify process state. The csgo #350 rejection and running
#597/#632 are owner-supplied examples, not independently observed runtime snapshots here.
No remote Mac process was inspected. No live role was resumed during this research.

## Proposed defaults and authority

One configuration key: `tick_hygiene: "safe"` (default); `"observe"` diagnoses without
additional hygiene recovery; `"off"` disables additional hygiene. Reject other values.
These modes do not disable established event delivery, existing recovery, or guard checks.
They do not bypass a recorded runtime rejection: the approved safety gate applies before
every affected send/admission path, including existing pending-event delivery and recovery.
Setup/update explains the selected mode and its limits before enabling the approved feature.
No separate timeout, scheduler, global hook, permission setting or secret is introduced.

Observation happens during the existing tick, including its ordinary safety-window pass;
this is not a promise of monitoring when no tick runs. Age may prioritize examination,
never authorize death, takeover or a send. Read-only status can display a suspicion but
cannot record an attempt, create a question, consume an event or recover anything.

To avoid bypass by an older recovery branch, approved implementation must route affected
generic nudges and supervisor-death recovery through the same incident bound and rejection
check. That changes §7 behavior and must be explicitly approved; no silent replacement of
R2/R3/R4/R11/R12 or the unsupervised migration path. Initial safe slice adds observation and
bounds only; **additional automatic recovery stays disabled for every resume topology
without independently demonstrated receipt and duplicate suppression**.

Only the task's recorded supervisor chooses worker work/requeue; tick remains its hands.
An owner choice uses the existing answer mechanism and the explicit authority below. Tick
must not invent a worker order, replace a role without that authorization, or execute an
inferred owner decision.

## Classification and decision table

Classify runtime state separately from obligation: a healthy idle session is not a stalled
task. Join exact role/runtime/full session/host, birth-bearing handle, latest turn boundary
and terminal event, local rollout, authoritative board claim/order/result, pending event
IDs and acknowledged recipient, and trusted owner/supervisor decisions. Contradictory,
unreadable or incompletely joined evidence yields unknown, not a guessed classification.
An obligation must come from a current trusted order, pending event, or explicit task/answer
handoff requirement joined to that role; idle/no-order alone does not prove one. Read full
original relevant result/answer and terminal-tool evidence before consequential action.
Rejection detection needs exact tool-result provenance; `Codex.state` currently reads turn
termination only, and a tail string or successful outer turn is insufficient. Until that
input is supported and verified, classify uncertainty and leave the case observation-only.

| State / obligation | Default action and owner route |
|---|---|
| Running with matching birth, including #597/#632 | Leave role and claims alone. Pending events stay pending until established delivery can run. Silence/restart/old board activity does not override running proof. A separately proven rejection can still be reported without interruption. |
| Idle, no unresolved obligation | Legitimate wait: supervisor awaiting worker/event, result awaiting review/CI, explicit owner ask, parked task. No nudge solely because the supervisor is idle. Respect paused monitoring and owner stop. |
| Idle with pending unacknowledged event | If no rejection/explicit stop blocks that role, use existing exact-recipient delivery; no second hygiene send. A blocked role's events remain pending, with no ack, until verified repair and scoped authorization. Ack only successful delivery; receiver uses existing event identity. Unknown send/ack outcome retains the guard and blocks automatic retry. |
| Idle worker completed a turn, answer acknowledged, still doing with no new result (#608) | Suspect completed unsubmitted handoff, not completion. Inspect exact turn and trusted outcome/answer. If a narrowly scoped handoff resume is qualified, one same-worker reminder to submit the existing result; no repeated research or implementation. Ambiguous result/answer applicability requires choices. |
| Missing worker, valid unconsumed supervisor order | Existing ordered spawn only after fresh read and verified predecessor retirement. Limits/ineligibility may legitimately hold the order; report that reason. Hygiene creates no extra worker. |
| Missing worker and no order, idle supervisor | Inspect result/review/ask and supervisor terminal turn. Legitimate wait stays silent; doing with an unresolved initial-order obligation becomes suspicion. One qualified same-supervisor continuation may ask it to read and decide; tick cannot synthesize `run`. |
| Dead/interrupted identified turn, local rollout | Preserve handle, branch, results/assets and role identity. Existing bounded recovery is inventoried above, not newly authorized. Apply the approved common bound before any resume; additional auto recovery requires qualified same-role idempotent continuation. Otherwise owner choices. |
| PID reused / foreign domain / missing handle / unreadable log | Never touch the PID now occupying the number. Reused birth proves only the recorded process ended; derive idle/dead from its own terminal evidence. Foreign domain, missing handle or contradictory evidence is unknown. Preserve, report uncertainty, verify on owning host before action. |
| Remote, including Mac from Windows | Remote runtime is unverified here regardless of recent board comments. No remote send/stop/takeover. Owning controller must verify locally; choice cannot turn local unknown into remote running or dead. |
| Tool/automatic-approval/runtime rejection (#350 `helper_unknown_error`) | Runtime blocker even if the outer turn completed and is idle. Preserve exact sanitized rejection and provenance. No automatic retry, respawn, alternate shell, permissions change or bypass. Offer original role after legitimate runtime repair, verified-stop restart, or keep blocked. Generic user/tool abort alone is not proof of rejection or withdrawn owner authority (#601). |
| Second failure / unchanged attempt with no receipt / ambiguous handoff | Keep blocked and offer choices once; no repeated automatic recovery. A successful spawn/send or process exit alone does not establish progress. |
| Guard contention / retained unknown grant | Read-only diagnostics available. No incident mutation, send or takeover without grant. Bounded contention failure remains visible; recovery is existing explicit stop/drain/reconcile/exact-token procedure, never stale-lock stealing. |

## Small board-owned incident record

Use one bounded `hygiene` object per task with entries only for its current worker and
supervisor, in the existing issue JSON. No local incident/receipt store. Proposed entry:
`{key, reason, evidence, attempt, outcome, question_id}`. Keep evidence sanitized and bounded:
exact role identity and relevant event IDs, terminal turn identity/type, process birth
identity when available, obligation and rejection code; never raw transcripts or private paths.
Future schema/unknown values fail closed through existing compatibility policy.

`key` identifies role/runtime/full session/host + triggering obligation identity (specific
event/answer/order/result) + blocker class. Runtime snapshots and terminal boundaries are
evidence, **not a new incident every turn**: the attempted recovery's new process/turn,
generic comments, age, controller restart, observation writes or an unrelated answer do
not reset the bound. At most one attempt for an unchanged unresolved obligation.
Resolution requires actual obligation progress; only a new distinct obligation after
resolution, an independently owner-authorized new role/work obligation, or an explicit
owner-approved scoped retry creates new budget.
Keep the last attempted record until resolved/replaced; never compact away an unresolved bound.
Transfer the unresolved record when native resume continues the same role under a new ID
(Claude), preserving the original obligation and attempt. The same applies to any
recovery-induced replacement, including existing automatic first-death supervisor respawn;
its new session ID is not fresh budget. A changed blocker classification
updates evidence, not budget. Do not overwrite unresolved obligation A with B; preserve A's
bound and leave B in the existing board event/order state until A resolves or an explicit
current owner decision reconciles both. B then A must not regain A's budget. A recorded
authorized replacement is distinct from a native continuation; retain the replaced attempt
in existing action/history evidence when reconciling, not a new unbounded incident ledger.

Under the existing grant: re-read issue and authority, re-probe exact role, check record and
pending delivery, reserve `attempt=1` in an acknowledged board update **before** invoking a
qualified recovery effect. Re-check runtime immediately before send. Unknown reservation
write => no send, retain grant. Unknown send => retain reservation and grant; no rollback or
retry. Known refusal => record blocked outcome without bypass. A controller crash after
reservation but before send leaves a blocked attempted incident; safety beats guessed retry.
After successful send, record admission evidence, not `recovered`; await proof on a later
ordinary tick. A persistence gap does not authorize another send. No nested/child controller
inherits the grant. Fresh readers see the reservation before any additional action.

Recovery proof is the expected new board result/order transition joined to the intended
role/obligation, or confirmed intended-recipient event receipt/application with its exact ID.
Ack proves delivery under the existing contract, not successful task completion. Generic
board edits, `turn.completed`, PID activity and send return are not obligation progress.
When a joined resumed turn ends without expected proof, record blocked outcome and choices;
if no new turn/receipt can be verified, report unknown on the next pass, never reattempt.
No repeated unchanged comments/questions; status may continue showing the unresolved row.
Observation-only/off periods retain attempt records; toggling configuration resets no budget.

Ordinary event replay remains at-least-once; this design does not assert exactly-once
runtime execution. Qualification must demonstrate idempotent handling of the narrow recovery
instruction and exact-event replay before enabling it. Otherwise safe mode only observes
that case. Guard-retained unknown effects require explicit quiescent reconciliation.

## Owner choices and coordination boundaries

Illustrative choice card, using existing versioned question identity from #597:

> #350: runtime blocked. Supervisor idle; no worker/order. Its last relevant tool returned
> `helper_unknown_error`; remote/runtime repair is unverified. Work and claims preserved.
> 350.1 Continue existing supervisor after verified runtime repair.
> 350.2 Restart supervisor after verifying all prior role turns stopped; retain branch/results.
> 350.3 Keep blocked (recommended until repair/stop evidence exists).

For #608 the recommendation is continue the existing worker for result handoff only, with
the same three choices and the reason that the answer is acknowledged but no subsequent
result exists. Present actual evidence/uncertainty and one recommendation, never a fabricated
running label. Link existing verified artifacts when useful; do not manufacture assets.
Public text contains no secrets or paths outside the repository.

Choice is conditional authorization, not evidence of stop/repair. Re-read current task,
question/incident identity and ownership under grant before acting; stale, ambiguous or
duplicate tokens cannot authorize changed actions. If verification remains unavailable,
keep blocked. Restart must be ordered by the supervisor after verified predecessor stop/
retirement for a worker. For a blocked/dead supervisor, its exact current owner answer
authorizes tick, as helper, to perform the existing supervisor replacement path after
verified stop/retirement; a dead supervisor cannot authorize its own replacement. The new
supervisor adopts the recorded worker without restarting it or inventing a worker order.
Worker requeue follows the existing controller path,
retains work and does not retire a running/unknown predecessor to free a slot.

#601 owns PM/ARM authorization, sender/wait continuity and tool-interruption provenance:
consume its proven scope/evidence, add no sender or recovery hook. #612 owns external incident
reporting/redaction/deduplication: hygiene writes only the task's board state and emits no
public tracker report; any integration is separately approved there. #597 owns compact output
and question/version identity: consume that identity, add no competing counter or renderer.
Their bodies were read for these boundaries; their claimed code is untouched. Supervisor/PM
should link this design in their normal review/coordination, rather than altering those tasks.

## Acceptance scenarios and implementation gate

These are design invariants and planned checks, not executed recovery proofs.

| Scenario | Required observable assertion / cheapest level |
|---|---|
| #608 acknowledged answer, completed idle worker | One qualified handoff to same role, exact result resolves; ambiguous evidence asks, preserved branch/assets; Tick fake-board test. Real idle receipt/result proof required before automatic enablement. |
| #350 rejection and setup errors | Classify runtime blocker; zero sends/spawns/permission changes, sanitized rejection retained; Tick table test. Exact terminal-tool provenance needed, not a tail-string guess. |
| Running / idle legitimate wait / interrupted | Running never sent; legitimate wait silent; interrupted owned turn follows bound without clearing work; extend existing Tick state/death cases. |
| PID reuse / legacy / remote / unknown | No unrelated kill/send/spawn or invented runtime status; extend RuntimeProcessBoundary cases. macOS native qualification is still unknown. |
| Pending answer/result, lost ack, role replacement | Original payload/recipient preserved, ack only after delivery, no competing hygiene send; EventDelivery regressions retained. Lost authoritative response retains grant. |
| Repeated ticks / resume then no progress / unrelated answer / changed blocker / Claude continuation ID / automatic supervisor replacement / A→B→A | Exactly one reserved attempt; recovery process/session changes, time and classification do not reset or overwrite A; one question, no repeated noise; Tick controlled-clock table. |
| Guard contention / crash before or after effect / two controllers | No effects without grant; durable reservation precedes send; second checkout cannot duplicate; Coordination isolated fault injection plus RealChild two-checkout boundary. No timeout stealing. |
| Owner continue/restart/block, changed question | Revalidate exact current choice, authority and verified stop; no lost result/assets or live predecessor replacement; Commands/Tick after #597 interface settles. |
| GitHub and GitLab | Same shared classification/reservation decisions; adapter mocks assert issue update under grant. Mocks do not qualify real service atomicity/receipt. Approved isolated adapter/runtime proof before changed live boundary publication. |

Before implementation: independent review of this design, approved R-number/§7 amendments
and default/config slice, resolved #597 identity interface and #601 rejection evidence input.
Implement shared orchestration first, preserving adapter guard semantics. Extend existing
checks rather than a new framework. Documentation-only delivery checks contract sentinels;
future code follows §10 focused/full and changed-boundary qualification. No paid lifecycle
run, active launcher/global hook change, remote takeover or permission change is authorized
by this research. Process state is not application progress; unsupported runtime evidence
and idempotence remain explicit blockers, not PASS.
