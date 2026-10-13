# Decisions and recommended defaults

One register for accepted defaults and unresolved product choices. Detailed contracts remain in their linked specifications.
Status: D01, D02, D04, D05, D07–D14 option 1 accepted. D08 includes the milestone reminder below. D03 Auto direction accepted; tuning remains an engineering proposal. D06 option 2 accepted: plans with editable recipes over existing board/service primitives.
Approval selects product targets, not implementation proof. No runtime default, grant, timer or publication policy changes here. Unsupported alternatives remain explicit limitations.

## Already settled

External board authority; GitHub/GitLab first; replaceable clients/runtimes; menu and existing-agent entry; numbered source choices; Human = Ask; attention first; pinned/recent projects; nonzero counters; quiet Closed; one main AI workspace with explicit detach; square Icon/Summary on both OSes; full usable-height menu; shared Work mode editor; required emergency stop; English concise specs and spec-first iteration.
Do not ask the owner to choose these again. Mechanism and native proof remain separate.

## D01 Manager scope

1. **Recommend:** PM reads, plans and calls allowed queue/service operations. Small maintenance uses an existing command; code/content changes become tasks. No new “quick fix” execution role.
2. PM also performs small project changes directly. Faster sometimes; needs an explicit R3 exception and a precise task-work boundary.

Default: option 1. “Quick” never grants broader authority. [UC Q1](../use-cases/README.md#q1-direct-pm-actions).

## D02 Natural-language focus

Both rule and snapshot modes are already in the accepted UI direction. The pending choice is the supported first implementation and exact predicate contract, not whether to remove a mode.

1. **Recommend:** formal rule when the request maps exactly; otherwise a reviewed task snapshot. Label Future matches or Selected tasks. Exclusions visible; same fields editable as chips.
2. Snapshot only. Simpler first release; new matching tasks need another selection.
3. AI evaluates each admission. Most flexible language matching; recurring cost, latency and less predictable decisions.

Accepted default: option 1; ship option 2 first if formal predicates are not ready, explicitly labelled. No silent broader match. [Q3](../use-cases/README.md#q3-accurate-natural-language-focus).
Initial predicate proposal: registered project + issue IDs, labels, type and priority with AND/OR/NOT. Evaluate fresh task metadata at admission; task edits can change rule membership, never snapshot IDs or running work. Preserve current p0/pause semantics unless amended explicitly. Unmapped semantic meaning needs a snapshot or clarification.

## D03 Machine resources and urgency

1. **Accepted:** Auto on new hosts. Before each launch, assess recent host load and current pressure; admit gradually while healthy. No required session-count setup or hidden one-slot ceiling.
2. Auto with explicit host/project/runtime ceilings. Same protection, with a predictable concurrency maximum.
3. Fixed ceilings, without automatic load gating. Explicit advanced choice; higher overload risk. Show load even when it does not control admission.

[Dynamic resources](dynamic-resources.md) owns admission, UI, migration and qualification. Optional ceilings only narrow admission. Urgent eligible work takes the next available launch; ordinary work rotates across eligible projects. Running work is preserved. Load cannot predict future build spikes or provider spend.

## D04 Delivery default

1. **Recommend:** Review PR for new projects. Commit/check/push/create PR; owner approves exact candidate; service verifies publication before close.
2. Auto-merge after checks/reviewer/protections. Less intervention; deliberate project choice.
3. Local or Push branch. No automatic publication; define handoff/dependency completion before implementation.

Keep Direct and custom stages available in advanced settings. Initial merge method Squash where supported; expose verified alternatives. Existing `publish: pr` means automatic publication, not option 1: never migrate by matching its name. [B1–B5](agent-behavior.md).

## D05 Access default and migration

1. **Recommend:** Workspace on qualified hosts; explicit Full preset for operator-trusted runners; Read only for restricted work. Unsupported containment blocks, never falls back to Full.
2. Mandatory isolated identity/VM for every worker. Stronger boundary; more setup.
3. Preserve legacy behavior without claiming a new enforced ceiling. Simplest compatibility route; weaker control.

For the owner's isolated runner, selecting Full once is supported; no repeated prompts for unchanged grants. Recommend reviewed per-host migration with new admission held until policy is confirmed; current attempts are preserved. Same-user Full cannot protect its own grant store from arbitrary code. Local approval broker/isolation and compatibility floor remain security engineering gates. [P1–P9](worker-permissions.md).

## D06 Pipelines and task stages

1. **Task plan:** AI drafts board tasks and dependencies. Good for one-off plans; project instructions carry the working method.
2. **Accepted: plan with recipes.** Built-in editable work sequences reuse tasks, checks, owner gates and environment commands. Describe the goal; review a compact plan; explicitly start. No separate scheduler or mandatory graph editor.
3. **Pipeline constructor:** arbitrary persistent stages, per-stage agents/retries and graph editing. Useful when custom repeated orchestration is itself the job; more state, recovery and configuration.

Option 2 selected. Alternatives remain reference trade-offs, not pending choices. [P1](plans-and-decisions.md#p1-what-a-pipeline-means-here) compares concrete outcomes. D14's accepted recipe presets do not approve arbitrary internal stage persistence. Recipes must identify enforceable effects; instructions alone cannot enforce a gate or guarantee cleanup.

## D07 Five-repository work

1. **Recommend:** one project-owned external environment with explicit repo revisions and prepare/check/cleanup commands. One task for one reviewable outcome; split only meaningful independent outcomes. Isolate concurrent environments.
2. Generic shared-environment manager and cross-board graph. Reusable, but adds leases, locks, invalidation and finalization.

Default: option 1 where the acceptance fits. Preparation/cleanup ownership must still be proven; a finalizer runs after terminal attempts, not only successful dependencies. Preserve unpublished work. Neither option promises atomic multi-repo publication. [P4](plans-and-decisions.md#p4-five-repository-environment).

## D08 Plan creation and owner answers

Ordinary single-click numbered answers and consequential-effect review are preserved requirements. The pending choice here is plan creation/start default, not reopened answer UI approval.

1. **Recommend:** one reviewed proposal, exact target/revision, per-item receipts and reconcile-before-retry. Ordinary numbered answers submit once; consequential effects retain explicit review. Plan default is Create tasks only; explicit “start” can select Create tasks & start in the reviewed proposal.
2. Always create and start. One fewer choice; unsuitable while specifications and scope are still being discussed.
3. Always create held, then use a separate Start review. Clear separation; adds an interaction even when the owner already requested execution.

After a meaningful planning milestone, remind once when the held plan can start: “Ready to start when you say.” Track the plan revision/milestone; do not repeat on every reply, after dismissal, or while blocked. A reminder never starts work. An updated plan may earn a new reminder only after another completed milestone.

Option 1 needs authoritative card/attempt binding and duplicate-operation handling in the board model. No claim that forge APIs offer a multi-write transaction. Created-only tasks must remain visibly held; current ready issue creation cannot implement that promise alone. [Q2/P2](plans-and-decisions.md).
Prefer existing `later` for held tasks if safe creation/start can be qualified. Source `add --later` currently creates a `taskq`/ready issue before appending `later` (`src/main.rs`); that ordering leaves a potential admission window. Qualify creation before eligibility (for example, establish the hold before exposing the task label), partial-write recovery and all writers. No race reproduction or fix is claimed here.

## D09 Bootstrap, standard reports and observation

1. **Recommend:** thin client entry → installed compatible contract → canonical report. Observation off until requested; event/cursor-based delivery, bounded wait while active, native scheduling only where supported.
2. Periodic AI wake-up on every check. Easy on supported clients; higher cost and still no universal closed-app guarantee.
3. On-demand only. Smallest supported baseline; no proactive questions outside active use.

Ship option 3 as truthful fallback, preserve option 1 as target. Notifications do not auto-answer; pending questions never age out. Each observer needs its own cursor; changing a question while still `ask` must notify. No new agent-to-agent bus until board/inbox operations demonstrably fail a use case. [A1–A5](agent-entry.md).

## D10 Attachments, history and visual evidence

1. **Recommend:** local retained conversation history; explicit archive/delete. Advertised per-runtime limits; request budget is the smaller of client and runtime budgets. Start candidate: 5 MiB/file, 20 MiB/request, 1 GiB local attachment budget; PDF pages also bounded by adapter capability before sending.
2. External links only. Less local storage; less convenient and requires accessible durable hosting.
3. Cloud-synchronized conversations/files. Cross-device convenience; requires a separate storage/privacy/identity design.

Option 1: reject visibly, preserve drafts, never silently truncate or auto-purge unresolved content. These initial budgets are accepted product defaults, subject to lower adapter limits and adapter qualification. Visual evidence goes to the board's supported upload path; fallback is an approved durable link. No public upload merely to obtain a preview. [C3/C7](ai-workspace.md).

## D11 Icon overflow and history

1. **Recommend:** fixed square/type; calibrated threshold such as `9+` only beyond that slot's readable capacity, exact values in menu; unknown as dash. Rebin all series to the largest fitting divisor of 12; exact totals/detail retained.
2. Exact counts always, even when space is insufficient. Conflicts with square/readable type unless another composition is approved.
3. Switch automatically to compact Q when crowded. Readable but unexpectedly hides selected Summary content.

Option 1 needs native visual approval for threshold per size; do not hardcode one threshold for all OSes. Use fixed time bins; bars shift only at boundaries, not smooth floating motion. [I5/I6](icon-summary.md), [rendering](rendering-contract.md).

## D12 Emergency recovery

1. **Recommend:** durable local admission hold, stop owned attempts, reconcile, restart only reviewed eligible tasks. Default recovery is Keep held. Remote/global stop is a separate operation with explicit reachability limits.
2. Stop then automatically restart everything. Faster; unsafe with unknown processes or external writes.

No force-kill by process name, automatic cleanup or global-stop guarantee offline. Old incompatible binaries must not clear/ignore the hold. Approved emergency requirement remains; persistence/fencing representation needs model work. [E1–E5](emergency-recovery.md).

## D13 Adaptive defaults

1. **Recommend:** pinned projects plus three recent; two representative attention causes plus All and full counts. Fixed controls; no disappearing unresolved items. History/counters remain accessible.
2. All projects/tasks expanded. Predictable but crowded at 20 projects.
3. Fully adaptive reordered controls. Short path sometimes; unstable targets.

Use existing approved typography/view defaults. Appearance prefs change locally; global counts/freshness are never silently reduced by shortlist limits. MP8 values are accepted; 20-project/native review remains required. [MP8](menu-product.md).

## D14 Presets and customization

1. **Recommend:** ship editable recipes composed from existing controls: Plan first, Implement and review, Urgent fix, Multi-repo work. Each shows scope, limits, Access and Delivery separately; selecting a recipe drafts changes, never applies them.
2. Custom setup from scratch. Flexible but repeats avoidable configuration.
3. Opaque one-click “autopilot”. Short UI; hides consequential effects.

Defaults belong to their authoritative domain: host grants/resources, project workflow/delivery, local appearance, conversation context. One global preset must not overwrite all four. Existing installs keep explicit settings; updates show default diffs before adoption. No model/effort changes implied.

## Recommended recipes

| Recipe | Built-in sequence | Owner can change |
|---|---|---|
| Plan first | Read specs/board → reconcile → propose tasks → create held → review start | Tasks, dependencies, scope; creation/start separately |
| Implement and review | Confirm goal → eligible task → code/checks → reviewer → PR → owner publication decision | Delivery preset and project checks within contract |
| Urgent fix | Inspect duplicate → p0 → next eligible slot → normal checks/review | Priority, project, host constraint; never bypass pause/capacity |
| Multi-repo work | Pin members → owned setup → work → compatibility checks → preserve output → finalize | Repo revisions/commands; explicit independent work |

These are accepted default recipe intentions, not new task states or a workflow DSL. Reuse the same primitives for custom flows. A small task can skip unnecessary planning artifacts, never required verification.

## Engineering contracts, not owner preference questions

| Gate | Required result | Related decisions |
|---|---|---|
| G1 Authority/compatibility | Qualified host grants; migration/rollback floor; explicit trusted vs isolated Full | D05, D12 |
| G2 Board identities/receipts | Versioned card/attempt binding, first valid answer, idempotency, partial creation reconciliation, held-plan representation | D06, D08 |
| G3 Admission | Shared budget ownership, fair ordering, concurrent services, resume/reviewer/operator liveness | D03 |
| G4 Delivery | Manual handoff/close/dependency meaning; exact-SHA approval and cross-host policy authority | D04 |
| G5 Environment | Owned resources, cancellation/cleanup, invalidation, per-repo publication results | D07 |
| G6 Client operations | Working zero-service setup route; versioned bootstrap/reports/capabilities, independent watch cursors, supported session/notification adapters | D01, D09 |
| G7 Storage/media | Real byte/page limits, authorized durable artifacts, no silent loss, restart recovery | D10 |
| G8 Native rendering | Actual-size icon and full-height menu on both OSes; accessibility and temporal-bin tests | D11, D13 |

Resolve gates in implementation specs/models. Escalate only a discovered product trade-off; do not ask the owner to design synchronization primitives.

## Decision protocol

Record selection as `Dxx.option` with any changed constraint; preserve IDs. Accepting this document's structure does not approve every recommendation.
After selection, update the owning spec/default and this status; retain alternatives under advanced routes where supported. Unsupported alternatives show a limitation instead of fake controls.
Re-run [scenario review](scenario-review.md) for affected cases. Runtime release still needs contract/model, adapter and platform evidence.

## Accepted follow-up decisions

- Custom is the third tray mode; built-in Summary is configuration of the same renderer. Agent proposals stay drafts until owner approval; re-edit/revert preserves a known-good icon. [CI1–CI5](custom-icon.md).
- Continue independent valid branches after a failure; hold dependants. Monetary budgets are deferred until reliable usage attribution. Real five-repository inputs remain required for qualification.
