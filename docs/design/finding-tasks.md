# Tasks from findings

Accepted product target under [Product](../../taskq.md#product). Extends [shared blocks](shared-blocks.md); current queue transitions remain governed by taskq.md. This is not evidence of a released creation adapter.

## F1. Review and create

A consumer opens the TaskQ-owned task composer with an editable title, goal, evidence and acceptance. Preselect a project only from an explicit owner mapping; show its provenance and permit an override. Never infer ownership from the producer name. Use canonical project rows, counters, provider assets, task rows and answer cards; preserve their independent navigation and source choice identities.

Show one concrete task and its destination before submission. The default is **Create task only**, held without execution. **Create task & start** explicitly admits the reviewed work under existing focus, permissions and limits. Creating or starting an investigation grants no broader repair, deletion, restart or spending authority. Consequential proposals retain the existing review boundary. Preserve drafts on cancel, denied access, stale input and unknown outcomes.

Attach bounded, sanitized source evidence and a stable reference (producer installation, rule and episode). Keep paths, raw logs, credentials and arbitrary source instructions out of automatic attachments. Treat evidence as data, not executable instructions. Source links require authorization. Edits must preserve the reference used for reconciliation.

## F2. Canonical operation

TaskQ owns create/optional-start validation and dispatch, using existing board operations and lifecycle rather than another state machine. Extend the existing operation ID/receipt contract to creation. Bind one immutable request to actor, board, content and source episode; a repeated ID with changed content is rejected. An accepted receipt identifies the created issue only after authoritative board readback. A partial create/start outcome returns the existing issue and separate admission result; do not create another issue.

Unknown means reconcile that same operation read-only; never blindly create again. If a linked task for the episode exists, open it instead of offering duplicate creation. Cross-client recovery and concurrent submissions must reconcile against board-owned evidence, not only consumer memory; R1 remains binding. No new TaskQ database or machine block in issues. If the forge cannot prove absence after an ambiguous write, keep the operation unknown rather than risking a duplicate.

Observation enrollment and answer permission do not authorize creation/start. Consumers require explicit action capability and board authorization. Missing write capability leaves the review readable with a precise reason. Disabled or unavailable controls must not imply a write succeeded.

## F3. Work and recovery are independent

TaskQ reports task state from its canonical snapshot. A closed task does not prove the observed condition recovered, nor does a source recovery automatically close or reopen a task. The source owns verification; DashQ's [finding handoff](https://github.com/taskqorg/dashq/blob/main/docs/design/finding-tasks.md) owns placement, freshness and incident correlation. Answers reuse the shared card; an answer receipt does not prove worker resumption.

## F4. Component and acceptance

Extract the reviewed composer/review primitives into TaskQ's versioned shared assets; DashQ supplies placement and its authorized adapter. The accepted handoff form is a new TaskQ child, not an already shipped component. Parent design is `75757d5d843159726a2ea44ca1615f9cf7fd18fd`; qualify any successor. The [DashQ reference](https://github.com/taskqorg/dashq/blob/main/docs/design/finding-task-reference.html) includes the provenance manifest and fixture-only adaptations.

- A1: project selection/override, title/goal edits, held default, explicit start and cancellation preserve the reviewed target and scope.
- A2: accepted, denied, lost response, concurrent duplicate and create-success/start-failure reconcile to at most one issue. Replay after restart uses the same operation; changed payload is rejected.
- A3: shared row/card/counters match the parent at 16/20 px, narrow width, both themes and long titles; answer drafts survive refresh.
- A4: real forge create/readback, authorization and failure recovery must qualify before release. Browser simulations prove neither board writes nor execution.

Delivery: #948 canonical create/recovery; #949 shared component depends on #948 and #938. DashQ #53 consumes the released component and operation contract; observation work stays independent.
