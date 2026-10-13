# Agent workflow comparison

Research snapshot: 2026-10-14. Recommendation, not a runtime amendment.
Official docs and public product screenshots inspected; applications not installed or exercised.
TaskQ baseline: design worktree `d7fd593` plus local proposals; not a fresh audit of the installed release.
Compare against [TaskQ contract](../../taskq.md), [use cases](../use-cases/README.md) and [proposed decisions/plans](plans-and-decisions.md).

## Findings

| Product | Documented approach | Useful for TaskQ | Boundary |
|---|---|---|---|
| VibeCrew | Card stages, board dispatch waves, multi-repository workspaces, orchestrator, approval inbox | Visible task stages; planned owner checkpoints; one question inbox; shared UI/API planning | Handbook distinguishes production pipelines from the unfinished crews engine. Many named screens/settings; too much for a tray |
| Moshi | Existing live sessions become chat, terminal and attention views; one activity row per session | Resume the same session; needs-human first; explain capability readiness | Structured replies require a supported live integration. Unsupported prompts return to terminal. Turn finished does not mean task delivered |
| Conductor | Isolated workspaces, setup/run/archive scripts, changes/checks/PR workflow | Explicit environment setup; result review; publication as a visible step | Archive script is not guaranteed failure cleanup. Process-group stop can miss escaped descendants |
| cmux | Terminal workspaces; unified permission/question/plan-approval Feed with reply routing | One decision component; stable request identity; jump to the exact waiting surface | Agent integrations have different decision capabilities. Timeout falls through to native UI; it does not resolve the request |

## Scenarios

| Owner need | Best reference | TaskQ recommendation |
|---|---|---|
| Know what needs me now | VibeCrew inbox; Moshi session activity; cmux Feed | One attention list. Unresolved questions persist; reading a notification never answers it |
| Answer without finding a terminal | Moshi mapped cards; cmux request routing | Exact question + reason + choices + custom answer. Receipt and agent resumption shown separately |
| Explain a large goal | VibeCrew board ship plan | AI proposes existing/new tasks, dependencies and effects. Review once; service enforces capacity and focus |
| Approve a plan before coding | VibeCrew stage gates | Planned checkpoint inside a task, visibly distinct from an unexpected question |
| Build across five repositories | VibeCrew per-repo worktrees; Conductor scripts | Named revisions, isolated/owned resources, explicit build edges, cleanup after terminal attempts |
| Continue the agent conversation | Moshi same-session chat/terminal; VibeCrew live terminal | Reopen the recorded session. No duplicate agent. Unsupported remote control: explicit unavailable state |
| Check what actually shipped | Conductor review/merge; VibeCrew pipeline review | Result receipt: diff, checks, preview/evidence, branch/PR and publication readback |
| Configure behavior without learning internals | Moshi scoped hook settings/readiness | Effective Access + Delivery summaries, source/scope, pending changes and named fixes. Advanced details on demand |

## Four separate concepts

- **Plan:** task relationships; sequence, parallel work, shared environment lifetime.
- **Task workflow:** stages within one task; inspect, plan, implement, verify, review. Planned owner gates when needed.
- **Delivery:** local result, branch push, PR, merge; governed by publication policy.
- **Access:** tools, filesystem, network and escalation; governed by host authority.

AI chooses a proposed route from the goal and project contract. Show one short route and any owner checkpoint; expand only to edit.
Do not make every task pass a research/spec wizard. Do not let a short route skip required checks or review.
Keep board task state canonical; a stage badge or terminal event cannot close a task.

## Fit to TaskQ principles

Owner clarification: [product principles](../product-principles.md) are the selection test. Competitor mechanisms are inputs, not a target feature checklist.

| Keep / adapt / defer | Decision | Why |
|---|---|---|
| Keep TaskQ | External board and visible task log | Core advantage: another manager/client can continue without copying tasks. Verify with UC-10 |
| Keep TaskQ | Deterministic admission, attempt fencing, result/review contract | Reliable execution already has a foundation; no AI supervisor needed just to poll |
| Keep TaskQ | Shared concise task/question/result formats | Make source numbering and evidence equally usable across clients; do not create UI-only answer semantics |
| Adapt VibeCrew | AI-decomposed plans and owner checkpoints | Project-governed workflow over existing board tasks. Stage persistence must satisfy R1; avoid a private second board |
| Adapt Moshi/cmux | Unified decisions and exact-session return | Owner controls work from menu/workspace. Show each runtime's actual reply/link capability |
| Adapt Conductor | Environment preparation and result review | Reuse project commands and worktrees; qualify ownership/finalization before generic multi-repo orchestration |
| Add for TaskQ | Host resources, effective access and delivery | Per-computer operation is central. Current per-project limits are not a total machine budget |
| Add for TaskQ | Complete menu planning path and command contract | A settings operator alone cannot provide project management. Define planning operations/structured output with the same role boundaries |
| Defer | Universal pipeline builder, template marketplace, per-stage model selectors | No demonstrated need in the ordinary describe/review/start path |
| Reject as default | Private canonical queue, required desktop agent, autonomous owner answers | Conflicts with portability, independent operation or R3. Optional runtime capabilities must be explicit |

### Where our design needs correction

- **Too many apparent controls:** keep one effective Work mode summary; show parameters/chips and exceptions inside its existing editor. AI and direct edits produce the same review.
- **Menu-only is incomplete:** current R3 operator is tool-less and allowlisted. Planning needs board context, task proposals, reviewed bulk apply and receipts; adding chat chrome does not implement it.
- **Host budgets are incomplete:** per-project limits can cumulatively exceed the operator's intended machine total. Specify shared capacity and fairness before promising one host-wide limit.
- **Agent interface is uneven:** reuse current commands, but qualify stable machine schemas, capabilities and error outcomes rather than asking agents to parse human prose or duplicate adapters.
- **Portability needs proof:** a new board adapter must implement required identity, log, authorization and readback semantics. Unsupported semantics are reported; a connector name alone is insufficient.
- **Evidence is not automatic:** current ask/result links provide a foundation. Inline image/video publication needs a provider-qualified upload/hosting path, visibility review and durable references.
- **Sessions differ:** headless runtimes can work with desktops closed; `codex-app` currently depends on the app. Do not mask this with one generic Running indicator.
- **Resources already have a tool:** source includes Resources → btop/btop4win (`src/tray.rs`). Preserve the shortcut and host label; qualify installation/access and remote behavior. Do not rebuild a full process monitor in the menu.

## Priorities and implementation gaps

| Priority | Next slice | Exists now / gate before delivery |
|---|---|---|
| First | Numbered owner decisions + shared inbox | Ask cards/options exist. Preserve numbers across surfaces; conditional submission, stale-card handling, idempotent receipts and client concurrency need contract/model work |
| First | Same-session return with capability status | Recorded sessions and links exist. Remote interaction and structured runtime replies need adapter-specific support |
| First | AI plan draft → reviewed board tasks | Same-board dependencies/admission exist. Bulk receipts, grouping and create-without-start semantics need definition |
| First | Agent-facing capability and operation contracts | Existing CLI/board/runtime primitives remain; inventory structured output and missing planning operations before adding APIs |
| Next | Host-wide resource control | Per-project/host/runtime limits and load gate exist; shared machine capacity/fairness remain an explicit contract gap |
| Next | Task stages + planned owner gate | Worker/reviewer/result lifecycle exists. Arbitrary stage persistence, gate transitions and restart recovery are proposals |
| Next | Result review in the workspace | Result/check/publication contracts exist. UI must expose evidence rather than infer completion from chat |
| Next | Feature readiness + settings readback | Expand proposed settings UI with capability, effective value, source, restart requirement and one next action |
| Later | Shared multi-repository environment | Ownership, isolation, leases, invalidation, failure finalization and partial publication remain backend gates |
| Later | Notifications and remote clients | Build on the same decision IDs/receipts; do not create another authority or lose unresolved questions |

## Do not copy

- Prose parsed as workflow state. VibeCrew requires stable numbered `## Pipeline` lines; paraphrasing can break tracking. TaskQ needs a versioned contract consistent with R1, not a second hidden store.
- A permanently running AI dispatcher merely to poll state. Existing deterministic service owns admission; agents interpret goals and produce reviewed proposals.
- Automatic owner answers. VibeCrew describes delegation from specs; TaskQ R3 forbids PM answering for the owner. First offer a cited suggestion. Delegated authority needs a separate owner-approved rule.
- A settings chatbot with imaginary reach. VibeCrew's documented assistant config writes are narrow; TaskQ must show actual supported operations and exact diffs.
- Notification age-out as resolution. Moshi activity expiry is not suitable for unresolved TaskQ decisions.
- Dropping nested-agent events to reduce noise. Moshi documents that suppression can discard approval events too. Mute delivery without losing required decisions.
- Mandatory TOML pipeline editing, graph builders, many branded screens or per-stage model pickers in the default path.

## Visual observations

- VibeCrew's official Bridge/Fleet screenshots show a persistent bottom agent strip plus right telemetry. Useful session continuity; duplicate presence and small type are poor fits for our menu.
- Retain TaskQ's attention-first menu and one workspace. Put active sessions in their existing view; no second cockpit.
- Screenshots establish layout only. They do not prove interaction quality, accessibility or native platform behavior.

## Sources

VibeCrew public repository is documentation, not the application source. Handbook pinned at `57a16f6d27d2f1198465fdab40648bcae1fd757a`:

- [Pipelines and crews](https://github.com/dexloom/vibecrew_sh/blob/57a16f6d27d2f1198465fdab40648bcae1fd757a/handbook/05-pipelines-and-crews.md)
- [Cards/workspaces](https://github.com/dexloom/vibecrew_sh/blob/57a16f6d27d2f1198465fdab40648bcae1fd757a/handbook/03-cards-and-workspaces.md)
- [Orchestrator](https://github.com/dexloom/vibecrew_sh/blob/57a16f6d27d2f1198465fdab40648bcae1fd757a/handbook/06-orchestrator.md), [approvals](https://github.com/dexloom/vibecrew_sh/blob/57a16f6d27d2f1198465fdab40648bcae1fd757a/handbook/07-approvals-and-escalation.md), [configuration](https://github.com/dexloom/vibecrew_sh/blob/57a16f6d27d2f1198465fdab40648bcae1fd757a/handbook/09-configuration.md)
- [Product screenshots](https://vibecrew.sh/features): `evidence/comparative-review/vibe-bridge.png`, `vibe-fleet.png`.

Moshi: [chat view](https://getmoshi.app/docs/chat-view), [hooks](https://getmoshi.app/docs/hooks), [hook settings](https://getmoshi.app/docs/hook-settings), [agent usage/readiness](https://getmoshi.app/docs/agents-usages), [Jump To](https://getmoshi.app/docs/jump-to). Desktop landing inspected; no Windows/macOS parity claim. Local-first transport still sends some notification metadata/summaries through Moshi infrastructure.

Conductor: [scripts](https://www.conductor.build/docs/reference/scripts), [review/merge](https://www.conductor.build/docs/guides/review-and-merge), [workspaces](https://www.conductor.build/docs/concepts/workspaces-and-branches).

cmux: [Feed](https://github.com/manaflow-ai/cmux/blob/main/docs/feed.md), [notifications](https://github.com/manaflow-ai/cmux/blob/main/docs/notifications.md). Moving branch; findings describe the inspected snapshot. Hook-only Codex events do not grant reply control; managed app-server integrations have separate capabilities.
