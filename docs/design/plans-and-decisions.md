# Task decisions and plans

Design proposal under R13; no runtime amendments. Inherit [shared AI-assisted controls](https://github.com/taskqorg/specq/blob/main/design/workflow.md#ai-assisted-controls), [density](https://github.com/taskqorg/specq/blob/main/design/operator-ui.md) and [native sizing](rendering-contract.md).

## Q1 Answer where the question appears

- Attention, Human filter, task detail and plan detail use one decision component. Same task/card identity and receipt across surfaces; no duplicate prompts.
- Show project + linked issue, agent/host, decision in plain language, why blocked and 2–4 labelled choices. Preserve original option text/trade-offs; mark the supplied recommendation. Never preselect or auto-submit it.
- Prefix each choice with its source number: `1.`, `2.`, etc. Keep numbers, order and meaning identical to the board and CLI; never sort the recommended choice first or renumber a filtered list. In a task card, issue/project context is already visible; CLI references remain `N.K`. A bare number is valid only in one unambiguous current question context. Custom answer is not an invented numbered option.
- Keep the selected number in the receipt and AI discussion context. Display formatting never changes the submitted source text or choice identity. Non-board conversation questions use stable local numbers, without invented issue IDs.
- Keep Done/Evidence behind Details unless needed to understand the decision. AI may explain, never invent evidence or replace the source question.
- Ordinary choice submits once, without another confirmation dialog. Other answer opens a compact field; Discuss with AI opens a conversation carrying task, attempt, question and choices, without posting a board answer.
- A choice that closes/publishes, broadens scope/permissions or performs another consequential operation shows exact effects before confirmation. §2 `close` options are publication, not ordinary text answers.
- Submit → Sending → Recorded only after matching board readback. Recorded is not proof the agent resumed. Keep task in Human until fresh task state changes it.
- Failed: preserve answer, explicit retry only when absence of write is verified. Unknown: Check status, no blind resend. Stale: show updated card and require a new selection; never retarget the old choice.

## Q2 Command/API readiness

One service operation shared by tray, workspace and clients; rendering never writes the board directly.

- Read: canonical forge/project/issue, attempt identity, immutable question/card identity or fingerprint, revision, Done/Problem/Evidence, stable choice IDs and exact text, recommendation, choice effects, capabilities and freshness.
- Submit: authenticated actor, exact target/card/attempt revision, choice ID OR free text, unique operation ID. Bind confirmation to exact effects when required.
- Atomically validate card/attempt eligibility at the authoritative write boundary; reject replaced/answered cards. A client refresh alone cannot fence a race. Repeated operation ID returns the same receipt, never another answer.
- Response: accepted with matching readback; stale with current card; rejected with reason; unknown with reconciliation identity. Transport timeout is not rejection.
- Current §2 supports answer options/text, including special close options. Conditional card binding, duplicate receipts and cross-client concurrency need explicit §2/§4 changes and model traces before shipping this guarantee.

## P1 What a pipeline means here

Option 2 accepted: [D06](decision-register.md#d06-pipelines-and-task-stages). UI term: Plan. The owner describes an outcome, sees what will happen, then starts it explicitly. Board tasks remain portable; no second task queue.

| Alternative | What the owner does | Benefit / cost |
|---|---|---|
| 1. Task plan | “Split this into tasks; run independent work together” | Fewest mechanics. Working method must already be in project instructions; recurring setup/check/gate details are less discoverable |
| 2. Plan with recipes — accepted | “Use our usual implementation flow, but show me the spec before coding” | Built-in editable sequences, relevant owner checkpoints and project commands. Needs explicit gate/environment contracts, but no graph editor |
| 3. Pipeline constructor | Draw custom stages; assign agents, retry rules and branches | Maximum stage control. More configuration and persisted lifecycle; justified only by concrete recurring needs beyond recipes |

The previous tasks-plus-ask recommendation covered ordering but underspecified repeatability, preparation and failure cleanup. Option 2 fills those gaps while preserving the same board/service architecture.

### Three concrete workflows

- **Spec before code:** research/spec task → owner reviews the result → implementation tasks → project checks/reviewer → configured delivery. Show “Waiting for your spec approval”. Only downstream work waits; unrelated eligible work may continue. Approval does not imply a permission increase or merge approval.
- **Large plan:** reconcile existing cards → draft small outcomes → show sequential groups and independent siblings → create held → explicit start. Auto admits eligible siblings according to host load/caps. A failed prerequisite holds dependants; independent valid branches can continue. Dependency edges come from inspected inputs, not AI guesses alone.
- **Five repositories:** pin the two Rust libraries, main project, Huff project and frontend → prepare an owned isolated environment → build independent members together where verified → integrate/check → review/publish per repository → preserve results and clean owned temporary resources. Failure/cancellation still reaches finalization after attempts stop. Never assume languages reveal build order.

### Recipes without a new engine

Ship the accepted D14 intentions: Plan first, Implement and review, Urgent fix, Multi-repo work. Selection creates an editable proposal, not a running job. Project instructions supply relevant checks and commands. Save a reviewed project recipe for reuse; the next use is a fresh draft, not replayed authorization.

- Make a separate board task when a step produces an independently reviewable outcome or needs independent scheduling/retry. Ordinary inspect/edit/test steps stay inside one task. Avoid a card for every command.
- Display the compact route with current evidence and blocking reason. A stage label is a projection, not a new board state. Turn completion or a checked AI todo never proves completion.
- Put enforceable boundaries in service/board contracts: prerequisites, planned owner gates, publication, environment ownership and finalization. An instruction saying “ask first” alone cannot enforce a gate.
- Gate answers bind the exact plan/result revision; revised inputs require renewed approval. Planned approval, unexpected question and runtime permission may share UI but retain distinct authority and submission paths.
- Keep Access and Delivery separate. A recipe cannot raise grants, bypass required checks, silently change model/effort or authorize publication.
- Persistent arbitrary stages, per-stage agent assignment and custom retry graphs remain option 3, not hidden prerequisites of option 2. Required new gate/group metadata must satisfy R1 and version compatibility; never parse free-form numbered prose as execution state.
- Unsupported runtime prompts return to their original session. No invented reply controls or remote-session capabilities.

[Competitor evidence](comparative-workflow-review.md): adopt VibeCrew's readable sequence/checkpoints, Conductor's explicit setup/checks and Moshi/cmux's unified questions. Avoid a private board, always-running AI dispatcher and mandatory pipeline configuration. These products were researched, not installed/tested.

## P2 AI-led authoring

- One input: goal, repository set, constraints and desired result. Existing project/task context accompanies it. No phase wizard by default.
- AI reads existing tasks/specs, resolves duplicates, proposes small tasks, acceptance, dependencies, owner gates and exact environment membership. Ask only material unresolved questions.
- Preview: short goal, repository set, sequential groups and parallel siblings, task counts, waits, publication behavior and external effects. Expand a row for issue/evidence, not another status dashboard.
- Detect cycles, missing references and inaccessible repositories before creation. Unsupported semantics stay explicit; no guessed cross-board dependency or shared-capacity guarantee.
- Default to Create tasks only. An explicit request to start can select Create tasks & start in the same review; start affects only the reviewed task set within existing admission gates. Never broaden focus implicitly. A paused project remains paused unless separately reviewed.
- After a meaningful completed planning milestone, if the held plan is actionable, say once: “Ready to start when you say.” Bind reminder deduplication to plan revision/milestone; do not repeat per message, after dismissal, or when blocked. If blocked, name the blocker instead. This is a response at a milestone, not a timer or automatic start.
- Bind plan revision and task mapping to confirmation. Partial creation returns per-task receipts and remaining writes. Reconcile before retry; do not duplicate existing issues.
- Prefer an existing hold primitive to a new task state, but qualify creation before admission. Current `add --later` creates the eligible issue before appending the hold; it cannot alone prove Create tasks only under concurrency. See [D08](decision-register.md#d08-plan-creation-and-owner-answers).
- For arbitrary prototype text, explain that a live planner is unavailable; never present the example plan as its interpretation.

## P3 Operator view and controls

- Menu: attention first, then one active-plan summary with Open. All plans stays inside Projects/workspace; do not add another always-visible top-level tab.
- Workspace plan: name + status; compact stage list; parallel members grouped under one stage. Show what runs, what waits and the blocking reason. No invented progress percentage or ETA.
- Normal controls: Pause new tasks / Resume, Discuss changes, task/decision links. Pause preserves active attempts and unrelated projects. Resume respects current focus, project pause and limits.
- Failed stage: show cause; Repair with AI drafts remediation. Retry only eligible failed work after liveness/output reconciliation. Preserve verified outputs; changed inputs invalidate dependent results.
- Changed plan: show task additions/removals/dependencies and new effects. Running attempts retain identity; steer/requeue/cancel through their existing explicit lifecycle. A plan edit never silently changes running work.
- Stop plan is separate, reviewed, and needs supported cancellation/cleanup; never label admission pause Stop. Emergency stop remains the fixed host control.
- Questions appear inline using Q1. User choice, not AI recommendation, resolves owner-only decisions.

## P4 Five-repository environment

- Example only: two Rust libraries + Huff may build in parallel after preparation; main/frontend compatibility waits for required outputs. Real build edges must be inspected, not inferred from language names.
- Each repository has explicit path, revision, write authority and owner. Shared mutable checkouts/build dirs require exclusive ownership or isolated workspaces; dependency order alone is not isolation.
- Preparation records owned resources and pinned inputs. Outputs carry revision/attempt identity; retries reuse only verified compatible outputs.
- Finalization runs after relevant attempts are terminal on success, failure or acknowledged cancellation. It is not an ordinary success-only `Depends on` task.
- Preserve changes, results and diagnostics before cleanup. Remove only owned disposable resources; never shared caches, unrelated files or unpublished work. Failed cleanup stays visible with resource inventory and explicit retry.
- Cross-repository publication is not atomic. Show per-repository outcomes and reconcile partial writes. Delivery policy remains [B1–B5](agent-behavior.md).

## P5 Smallest delivery and gates

1. Inline current ask-card UI + service-backed conditional answer/receipt model.
2. AI draft → review → existing same-board tasks/dependencies; plan projection; creation receipts. No second scheduler.
3. Multi-repository environment lifetime, resource ownership/finalization and cross-board dependencies only after Q5/Q6 model decisions. Independent branches can continue after one fails only when their inputs/resources remain valid.

Task-set admission hold (`add --hold`), idempotent bulk writes (op ids), recipes and revision-bound human gates distinct from publication are decided in [taskq.md](../../taskq.md) § 2 and § 7. Plan grouping storage, conditional-answer authority, environment leases and finalization remain unimplemented. Root R1/R2/R3/R11 and §§2/4/7/9 must resolve affected lifecycle decisions before runtime work.

## P6 Simplicity pass across the menu

| Surface | Keep direct | Reveal only when needed |
|---|---|---|
| Home | State, attention/answer, active-plan row, quick request | All tasks/plans and full diagnostics |
| Project/task | Board/session links, question/choices, flowing title | Attempt evidence, full history |
| AI | Goal, attachments, current conversation | Archive, detached-window recovery |
| Plan | Stage/reason, owner question, pause/resume | Dependencies, repo pins, receipts, cleanup inventory |
| Work mode | Current short scope and limits | Exact filters and changed targets |
| Agents | Access or Delivery; one effective summary | Overrides, capabilities, individual delivery stages |
| Services | Health and a named next action | Logs, resource details and maintenance |

No extra Plans top-level tab, pipeline wizard, graph editor, duplicated counters or autonomous-answer switch. Preserve earlier accepted routes. Chat/source instructions can drive AI work; service contracts still own allowed effects and proof.

## Acceptance

- Q-A1: source numbers, order, text, recommendation and task/card identity preserved across menu/task/plan/AI views; one ordinary click sends one exact answer. Receipt retains its number; custom text has no synthetic number.
- Q-A2: stale/replaced card, double-click, concurrent client and timeout cannot answer another card or duplicate a write; readback precedes Recorded.
- Q-A3: custom answer stays on failure; AI discussion does not submit; consequential close option reviews publication.
- P-A1: sequential/parallel rows match verified dependencies; no start beyond capacity/focus or human gate.
- P-A2: arbitrary input never receives an unrelated fixture plan; partial creation retries only missing writes.
- P-A3: pause affects new plan admissions only; failed branch cannot falsely unblock dependants; retry preserves successful compatible work.
- P-A4: cleanup waits for terminal attempts, preserves uncommitted work and reports leftover resources.
- P-A5: question/plan drafts survive navigation; full available height before scrolling; readable 16/20px and light/dark narrow layout.
- P-A6: task stages never masquerade as board states; planned gates, owner questions and permission requests retain distinct authority and identity.
- P-A8: held tasks cannot launch during creation; one milestone reminder never starts work or repeats on unchanged discussion.
- P-A7: stage titles and reasons have visible separation at 16/20px; compactness never joins words or hides a blocking reason.

[Interactive reference](plans-and-decisions.html) is a local fixture, not a planner, scheduler or board client. Backend acceptance needs model, adapter and native evidence beyond browser tests.
