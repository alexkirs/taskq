# Operator use cases

Validate TaskQ project setup, planning, focus and execution against user intent.
[Contract](../../taskq.md) · [Menu](../design/menu.md) · [Shared documentation rules](https://github.com/taskqorg/specq/blob/main/caveman/rules/documentation.md).

Follow the [shared use-case method](https://github.com/taskqorg/specq/blob/main/engineering/use-cases.md). Keep TaskQ cases in this file. UC-01–UC-07 record supplied needs; Q1/Q3–Q6 and P-01–P-03 remain open. Q2 is resolved.

## UC-01. Start work on a project

Source: owner. Variants: new or existing project.

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

## Coverage

| Capability | Primary case | Overlap |
|---|---|---|
| Create or connect a project | UC-01 | UC-06: multiple repositories |
| Conversation, clarification, duplicates, conflicts | UC-02 | All task changes |
| Shared rules, research, specification, stack | UC-03 | UC-02 + UC-07 |
| Urgency | UC-04 | UC-05: focus exception |
| Project/task selection and mode confirmation | UC-05 | UC-01, UC-04, UC-07 |
| Environment and cleanup | UC-06 | UC-07: shared resources |
| Decomposition, dependencies, parallelism | UC-07 | UC-03, UC-06 |

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

## Additional cases proposed for approval

| ID | Intent | Coverage |
|---|---|---|
| P-01 | Answer a question or recover a failed task | Decision context, answer target, safe retry |
| P-02 | Temporarily switch work, then return | Pause/resume, previous focus, unaffected projects; no saved-mode feature required |
| P-03 | Recover after host loss or agent quota exhaustion | Waiting reason, recovery, no duplicate admission |

Next: walk UC-05 through a real board, then clarify UC-06 with its requester. Open proposals are not approved behavior.
