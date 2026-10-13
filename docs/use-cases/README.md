# Operator use cases

Validate TaskQ project setup, planning, focus and execution against user intent.
[Contract](../../taskq.md) · [Menu](../design/menu.md) · [Shared documentation rules](https://github.com/taskqorg/specq/blob/main/caveman/rules/documentation.md).

Follow the [shared use-case method](https://github.com/taskqorg/specq/blob/main/engineering/use-cases.md). Keep TaskQ cases in this file. Needs are distinct from implementation guarantees. [Decision register](../design/decision-register.md) owns pending selections; [scenario review](../design/scenario-review.md) records coverage/evidence. Q2 is resolved; Q1/Q3–Q6 explain remaining contract constraints.

## UC-01. Start work on a project

Source: owner. Variants: new or existing project; agent request or agent-free install command. [Setup and project locations](../design/setup-and-project-locations.md) defines shared setup, runtime detection and S-A1–S-A8.

- Select the host Projects folder for new checkouts, or an exact existing project path. Do not scan home or register sibling folders; workers use their exact project/worktree root.
- New: describe the goal, name, repository provider and destination. Clarify missing details.
- Existing: name the project; inspect its folder, board and registration.
- Show setup effects. After confirmation, initialize or connect, register and verify access (§6).
- Select the host and all tasks in this project. Show the applied mode after readback.
- Registration and permission to admit tasks are separate actions.
- Leave other projects unchanged unless explicitly requested. Clarify ambiguous names.

Result: the intended project is registered, its board is accessible, and admission scope is clear.
Open: initial capacity limits.

## UC-02. Manage a project through an agent

Source: owner.

- Say `task` and name the project in a supported agent session.
- Activation, canonical reports and opt-in observation follow [A1–A5](../design/agent-entry.md). A discoverable integration must exist; the keyword alone supplies no tools or permissions.
- The agent loads TaskQ rules, the PM role and the current board. The chat is not a second queue.
- The owner describes work. PM clarifies the goal, boundaries and acceptance before filing a task.
- Do not repeat questions already answered by the request.
- Compare with the board: file new work, extend an existing task, or ask the owner to resolve a conflict.
- Link dependencies, set priority and return task links. Route changes to active work through the existing steering mechanism.
- For a quick service request, propose direct action when a task adds no value. The authority boundary remains open (Q1).

Result: each request is accounted for once through a task, task update or direct response.
Open: how `task` activates each supported agent; which service actions PM may perform directly.

## UC-03. Move from idea to implementation

Source: owner. Shared rules: [specq](https://github.com/taskqorg/specq/).

- Describe the goal and problems. Read the applicable shared rules; link them from project specifications.
- Discuss alternatives. Queue research, marketing material and project specifications when needed.
- Integrate work into the existing plan through UC-02 and UC-07.
- Check specifications against scenarios: covered, missing or conflicting. Amend project-specific requirements and recheck affected cases.
- Propose reusable gaps upstream through [shared-rule improvement](https://github.com/taskqorg/specq/blob/main/engineering/spec-first.md#improve-shared-rules).
- During planning, run agreed planning tasks; do not start implementation before authorization.
- After “start development”, use the requested stack or propose one suited to the specification and applicable rules. Resolve material technical questions.
- Create reviewable implementation tasks, dependencies and independent branches. Run them in the confirmed mode.

Result: requirements, tasks and execution order agree. Completed planning is not a completed product.

## UC-04. Admit urgent work without exceeding limits

Source: owner.

- Name the project, problem and urgency. Check duplicates, clarify acceptance, then create or update the task.
- Assign highest priority. Admit it before ordinary eligible tasks when a suitable slot becomes available.
- Keep current attempts running. Preserve capacity, dependencies, host capabilities and the load gate.
- Contract §4: `p0` passes an active focus; `off` or pause still prevents admission.
- Show why it waits: capacity, dependency, pause, host or gate. Do not promise immediate execution.

Result: urgent work receives priority without bypassing execution constraints.
Open: priority across projects without a shared capacity pool (Q4).

## UC-05. Select work in natural language

Source: owner. Example: “Today, projects A and B; in A, only this admin section. Leave the rest alone.”

- Accept the full request. Clarify ambiguous projects, areas, inclusions and exclusions.
- Resolve change targets, host, tasks per project and limits separately. Clarify what happens to other running projects.
- Translate intent into verifiable conditions. Show matching tasks, exclusions and unsupported parts of the request.
- No exact filter: explain the gap and propose clarification or an explicitly narrower/wider alternative. Never silently broaden scope.
- Before applying, show a short accurate phrase and changes per project. The owner edits or confirms.
- Apply only confirmed changes. Read back actual state; report partial success per project.
- In the menu, summarize actual parameters. Details show the original request, confirmed interpretation and applied state (M4).
- Expose mismatches and stale data. Desired mode is not applied mode.
- Current attempts continue. New admission follows applied conditions. Preserve untouched fields under M12.

Result: execution matches the confirmed intent; the owner can inspect the translation.
Open: filter language, future tasks, dependencies outside focus, and task edits that change a match (Q3).

## UC-06. Complete work across five repositories

Source: another user's question, forwarded by the owner. Solution not approved.
Composition: two Rust libraries, the main project, a Huff project and a frontend.

- Define the task, repositories, required revisions and build relationships.
- Prepare the environment: obtain sources, install dependencies and build required components.
- Perform the task and verify compatibility across affected components.
- Preserve changes, results and diagnostics. Remove owned temporary resources.
- On preparation, execution or cleanup failure, report the stage and remaining resources. Preserve unrelated files, changes and shared environments.

Result: reproducible work across the required repositories; preserved output and verifiable cleanup.
Open: boards, task boundaries, environment lifetime, platforms, caches, shared resources and publication (Q5).

## UC-07. Turn a large plan into dependent and parallel tasks

Source: owner and forwarded user question.

- Read the plan and existing tasks. Reconcile duplicates; ask about conflicts.
- Propose reviewable tasks, dependencies and owner decision points.
- Show what can start, what waits for results and what waits for answers. Detect cycles and missing dependencies.
- After agreement, create or update tasks. Return links and waiting reasons.
- Run independent eligible tasks in parallel within limits. Dependent work waits for its prerequisites.
- After partial creation, show completed and remaining writes. Retrying must not duplicate tasks.
- Show failed branches. Do not treat a missing result as completed work; cancelled dependencies need a defined policy (Q6).

Result: the plan becomes an inspectable queue. Dependencies determine order; capacity and capabilities bound parallelism.

## UC-08. Control worker access

Source: owner. Implementation proposal: [worker permissions](../design/worker-permissions.md).

- Isolated personal runner: choose Full once for the named host/runtime; inspect actual isolation.
- Daily development: Workspace; approve explicit build/cache/network exceptions.
- Research/review: Read only; retain private runtime/temp access.
- Five repositories: approve exact workspace members; no implicit sibling-folder access.
- Permission denial: see the blocked task, reason and exact grant request; decide once or retain the limit.
- Different agents/projects: host defaults plus visible project exceptions; inspect effective source.
- Urgent/parallel work never raises permissions. New policy and existing attempts remain distinguishable.

Result: operator intent matches effective access; project/task content cannot grant authority.
Open: authenticated grant channel, OS qualification, migration and breach lifecycle in P9.

## UC-09. Answer without leaving the task

Source: owner. [Decision UI and API proposal](../design/plans-and-decisions.md#q1-answer-where-the-question-appears).

- Open Attention or a task with a question. Read the reason and choices.
- Select one ordinary answer; the service posts that exact choice to that card. Show matching receipt before success.
- Use Other answer or discuss with AI when choices do not fit. AI does not answer for the owner.
- Replaced question: refresh and choose again. Unknown delivery: reconcile before retry.

Result: the intended task receives one owner answer, without opening an external agent.
Open: conditional-write/receipt contract; current option parsing alone does not prove race safety.

## UC-10. Continue through another agent or client

Source: owner. [P1/P2](../product-principles.md).

- Open the same registered board through another capable PM or directly in the forge.
- Read task, dependencies, question, answer and evidence without importing a proprietary task database.
- Keep task IDs and source option numbers. Route actions through existing authority and attempt rules.
- Unsupported execution adapter: explain the missing capability. Do not start a duplicate or migrate a live attempt.

Result: management can change while board work remains intact. Provider migration is a separate operation.

## UC-11. Work with agent desktops closed

Source: owner. [P10](../product-principles.md).

- With a service and headless runtime available, close external agent desktops.
- Use the menu/workspace to plan, attach evidence, confirm task/configuration proposals, answer numbered questions and inspect results.
- Reopen the same conversation/session when supported; never launch another attempt just to display it.
- Missing runtime or app-dependent adapter: show the exact limitation and supported alternative before starting work.

Result: no external agent desktop is needed for the supported headless path. Planning and runtime adapters require end-to-end qualification.

## UC-12. Control computers with different resources and access

Source: owner. [P8](../product-principles.md).

- Name each host's limits and access through text/dictation or direct controls.
- Review targets, effective values and exceptions; apply only those confirmed. Existing attempts retain their defined policy.
- Inspect occupied slots, freshness and waiting reasons. Open Resources for the named supported host using btop/btop4win.
- Do not confuse a per-project limit with the total machine budget. Installation, unavailable hosts and unsupported remote tools stay explicit.

Result: the owner can explain where work runs and why it waits. Shared budgets/fairness remain Q4 work.

## UC-13. Interrupt, recover and switch work

Source: owner emergency requirement plus P-01–P-03 proposals.

- Pause new admission while current attempts continue; preserve the current focus before switching projects.
- For emergency, name the host/targets, stop owned work and retain a durable hold. Offline hosts and uncertain effects stay unknown.
- After crash, quota loss or cancellation, inspect liveness, leases, preserved work and operation outcomes before retry.
- Show choices: keep held, inspect or restart eligible selected work. Never launch a second live attempt or silently broaden scope.
- Restore a previous mode only through current eligibility/policy review. No saved-mode library is required for the basic return path.

Result: visible state and recoverable work; no duplicate launch/publication. Policy choices D12; runtime gates G1/G2/G3 remain.

## UC-14. Review evidence and deliver

Source: owner. [P3/P4/P9](../product-principles.md), [Delivery](../design/agent-behavior.md).

- Open the task/result from menu or another PM. Inspect acceptance checks, exact candidate, visual evidence and publication target.
- For graphical/motion work, reach image/video evidence from the ticket outside the original chat.
- Apply configured Local/branch/PR/merge behavior; resolve an owner-only question using unchanged source numbers.
- Changed SHA, missing CI, protection failure or uncertain merge invalidates success; preserve work and reconcile.
- Show final remote readback or pending handoff. Task closure and downstream eligibility follow the reviewed lifecycle, not merely a created PR.

Result: the owner knows what is verified and delivered. D04/D10 and G4/G7 govern outstanding behavior.

## UC-15. Inspect the system in daily use

Source: owner menu requirements.

- At zero/one/20 projects, locate attention, pinned/recent projects, all-state counters and exact task details.
- Inspect active/paused/unknown/overflow tray states on both OSes; open full history when small pixels cannot show detail.
- Enlarge text; let the menu use available work area before scrolling. Essential controls remain reachable.
- Open Settings → named service/runtime for health, accounts, versions, logs, resources, incident consent and lifecycle actions. Never infer zero from stale data.
- Reopen a conversation/window without another request. Failed update, missing runtime, unavailable remote link or resource tool shows a next action and preserves context.

Result: common control remains compact and inspectable; native matrix and G8 qualify real platform behavior.

## Coverage matrix

UC-02 observation extension: agree to watch named projects in the existing session. Show current questions first; surface fresh numbered questions and separate suggestions. Stop watching without stopping tasks. Test unsupported scheduling, context loss, duplicate observers and an answer arriving through the menu while this PM is offline (A-A1–A-A7).

| Capability | Primary case | Overlap |
|---|---|---|
| Create or connect a project | UC-01 | UC-06: multiple repositories |
| Conversation, clarification, duplicates, conflicts | UC-02 | All task changes |
| Shared rules, research, specification, stack | UC-03 | UC-02 + UC-07 |
| Urgency | UC-04 | UC-05: focus exception |
| Project/task selection and mode confirmation | UC-05 | UC-01, UC-04, UC-07 |
| Environment and cleanup | UC-06 | UC-07: shared resources |
| Decomposition, dependencies, parallelism | UC-07 | UC-03, UC-06 |
| Host permissions | UC-08 | UC-12 |
| Numbered cross-interface answers | UC-09 | UC-10, UC-11 |
| Board/client independence | UC-10 | UC-01, UC-02 |
| Complete menu operation | UC-11 | UC-03, UC-05, UC-09 |
| Host resources and effective limits | UC-12 | UC-04, UC-08 |
| Pause, emergency and recovery | UC-13 | UC-04, UC-05, UC-12 |
| Evidence, handoff and publication | UC-14 | UC-03, UC-06, UC-08 |
| Tray, settings, diagnostics and accessibility | UC-15 | UC-01, UC-09, UC-11 |

Apply the shared cross-case checks to TaskQ hosts, boards, admission changes and active attempts. This set covers the supplied TaskQ scenarios.

## Open decisions

### Q1. Direct PM actions

R3 prohibits PM from doing task work. UC-02 requests an exception for quick service actions.
Proposal: PM reads, diagnoses and manages the queue within its authority; product changes remain tasks. Speed alone is not the criterion.
Changing authority requires an R3 decision. This document does not grant it.

### Q2. Shared specifications

Resolved: use [specq](https://github.com/taskqorg/specq/). Project content is English under R7.
Project-specific requirements stay here; shared rules and their improvement process remain upstream.

### Q3. Accurate natural-language focus

Current §4 supports `all|off|labels`, matching any selected label; `p0` passes active focus.
It does not define general AND/NOT expressions, semantic area matching or an explicit issue-list filter.

| Option | Benefit | Limit |
|---|---|---|
| Structured filter: labels, type, priority, IDs; AND/OR/NOT | Fast, inspectable, includes future matches | Needs accurate task metadata |
| Confirmed set of matching tasks | Exact snapshot selected from a free-form request | Future tasks do not enter automatically |
| AI decision before each admission | Can interpret new task content | Latency, cost and inconsistent decisions |

Proposal: AI translates intent; the service evaluates formal conditions. Unsupported meaning requires clarification or a confirmed task set, not AI on every launch.
Both extensions need implementation. Show whether confirmation creates a snapshot or a rule for future arrivals.
The short phrase must retain material exclusions; expose the full filter nearby.
For dependencies outside focus, propose inclusion; never include them silently.
“Only A/B” needs an explicit decision about other projects' admission. Unchecking a project in M12 leaves it unchanged.

### Q4. Priority and capacity

Contract §3 sets capacity per project, host and runtime; no shared machine pool exists.
“The next free slot” means an eligible project slot. Capacity from another project is not automatically transferred.
A machine-wide budget and cross-project priority are separate proposals, not UC-04 guarantees.

### Q5. Multiple repositories

§7 defines an ordinary attempt in one Git worktree. `workspace: external` delegates environment ownership to the project; it does not prove a complete multi-repository lifecycle.
Proposal: explicit repository membership, pinned revisions, prepare/check/cleanup commands and ownership of temporary resources.
First evaluate a project-owned external environment. Add a generic mechanism only for a demonstrated gap.
Multi-repository publication and partial success need a separate contract; do not promise atomicity.

### Q6. Dependencies and bulk creation

`Depends on #N` currently waits for a closed issue on the project's board; closure does not distinguish every completion reason.
Qualify cross-board dependencies, cancelled predecessors and recovery after partial plan creation separately.
UC-07 does not prove a bulk transaction or cross-repository dependency graph exists.

## Earlier proposed cases

| ID | Intent | Coverage |
|---|---|---|
| P-01 | Answer a question or recover a failed task | Covered by UC-09/UC-13; recovery mechanisms remain gated |
| P-02 | Temporarily switch work, then return | Covered by UC-05/UC-13; no saved-mode feature required |
| P-03 | Recover after host loss or agent quota exhaustion | Covered by UC-13; runtime/host qualification remains open |

UC-06/UC-07 design continuation: [plans](../design/plans-and-decisions.md#p1-what-a-pipeline-means-here). UC-09 covers the answer portion of P-01; failed-task recovery remains proposed.

Next: select pending defaults in the decision register, then qualify the implementation against the scenario review. UC-06 still needs concrete repository/build ownership inputs before execution.

Resource refinement for UC-01/04/12: [Auto admission](../design/dynamic-resources.md) is the accepted new-host default; optional strict caps remain available. Pipeline alternatives for UC-03/06/07 are compared in [P1](../design/plans-and-decisions.md#p1-what-a-pipeline-means-here). Plan creation stays held by default, with a brief milestone reminder under P2.

## UC-16 Describe and revise a tray icon

Choose Custom → agent inspects the icon tools/current config → compare supported drafts at real size → refine by voice/text → approve exact draft → save local appearance. Reopen to edit or revert. Summary is a built-in config of the same renderer; copying it never changes the original. No agent available: direct editing remains usable. Invalid draft/save failure keeps the current icon. [CI1–CI5](../design/custom-icon.md).

## UC-17. Plan and supervise through a remote chat agent

Source: owner; FRA/Hermes with Codex workers. [Remote manager RM1–RM7](../design/remote-manager.md).

- Open a project chat; discuss work and create an agreed held plan. Explicit start authorizes service execution.
- Let the host admit workers under qualified Auto; inspect actual limits, pressure and waiting reasons.
- Say “watch this work”; receive meaningful progress, numbered questions and final evidence. Answer in the same chat.
- Name another registered project for a one-off request or explicitly switch the conversation default. Keep execution and watch scope unchanged unless requested.
- Send images/files; receive supported materials and durable board references.
- After gateway loss, reconcile pending questions and results. Stop watching independently from pausing work.

Result: a replaceable remote PM provides planning and owner control while TaskQ executes independently. Existing integration and remaining qualification are recorded in the [FRA audit](../design/remote-manager-audit.md). Covers UC-02/03/07/09/10/12/14 without a second queue.

## Proposals

Improvement proposals the PM makes in chat, one line each, oldest first: `- YYYY-MM-DD <proposal> (from <request>)`. `taskq retro` reads them as `p<k>` beside the request log ([§ 4](../../taskq.md#4-service)); a filed or rejected proposal stays as history.

- 2026-10-14 Pin the tray menu into a movable window that snaps to screen edges where the OS supports it; filed as WS11b (from owner request in chat)
- 2026-10-14 Rebuild the project README: setup phrase first, dark menu screenshots refreshed by a manifest-checked pipeline, shipped use-case rows; plan docs/design/readme-plan.md, tasks A–C (from owner request in chat)
