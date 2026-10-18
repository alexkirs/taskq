# System change map

Accepted target: describe → review → start → answer → inspect result. The external board owns tasks; the service enforces effects; any supported agent/client can continue. D01–D14 product choices are accepted, including Auto and editable recipes. This is an implementation handoff, not a shipped-feature report.

## One menu and workspace

| Surface | Required route and behavior | Canonical contract |
|---|---|---|
| Menu | Icon/Summary, admission, pause, emergency, attention, numbered answers, quick request/files, aggregate counts, pinned/recent projects | menu.md, menu-product.md |
| Projects/tasks | Board above tasks, flowing title, host/runtime/session identity, all counters, complete lists | menu.md, task-counters.md |
| Remote PM | Existing agent chat, explicit project context, opt-in observation, numbered replies, media and reconnect | [remote-manager.md](remote-manager.md), [remote-manager-audit.md](remote-manager-audit.md) |
| Conversations | Existing main owner, explicit detach/raise, files/drafts/history, interrupted requests | ai-workspace.md |
| Work sessions | Same live local session, agent identity; remote capability explained | agent-entry.md, ai-workspace.md |
| Work mode | Exact rule or snapshot plus the local host admission policy and optional caps; review effective changes | dynamic-resources.md, menu-product.md |
| Plans | Four editable recipes; held creation, explicit start, dependencies, owner gates, retry, environment finalization | plans-and-decisions.md |
| Agents | Access and Delivery separately; source, requested/saved/applied and capabilities | worker-permissions.md, agent-behavior.md |
| Settings/services | Appearance/history, hosts, logs, folder, Resources, incidents, lifecycle and setup | current-menu-audit.md, ui-acceptance.md |
| Recovery | Persistent hold, preserved work, unknown attempts blocked, selected reviewed restart | emergency-recovery.md |

The integrated browser candidate demonstrates navigation and synthetic states. Native pickers/windows, live planning, board writes, service lifecycle and diagnostics are explicit handoff points, not simulated proof. Earlier standalone candidates are historical.

## What the system must change

| Slice | Existing base → required change | Completion evidence |
|---|---|---|
| Shared operation contract | CLI commands → typed capabilities, project/task/attempt/card identities, revision-bound mutation and receipts shared by UI/agents | Two clients answer once; stale answer rejected; timeout reconciled |
| Menu PM | Tool-less settings operator → bounded planning/read/proposal/apply operations under R3 | Menu-only bootstrap, plan, answer and result with headless runtime; no external desktop required |
| Host admission | Per-project runtime ceilings + opt-in load gate → one host admission authority, Auto default, optional caps, explicit enablement and compatibility migration | Concurrent projects, pressure/spikes, unknown metrics, crash reconciliation, review/interactive progress |
| Held plans and recipes | Task creation/dependencies → safe held creation, versioned grouping/gates and recipe drafts with per-item receipts | No creation admission race; partial retry creates no duplicate; restart retains gate |
| Recipe enforcement | Instructions → named checks, dependencies, owner gate and configured publication effects | Spec approval blocks coding; changed candidate invalidates approval; unknown result cannot unblock dependants |
| Environment | External worktrees → pinned multi-repo membership, owned resources, compatible outputs, finalizer after terminal attempts | Setup/build/cancel/cleanup failures preserve work; per-repo publication receipts |
| Delivery | Existing direct/pr close publication → Review PR default and explicit advanced presets with exact-candidate authorization | Review/push/merge changes do not silently inherit old publish semantics |
| Access/recovery | Runtime flags + current lifecycle → operator-owned grants, qualified OS boundaries, emergency hold and fenced restart | Negative permission tests and offline/restart/old-version hold tests |
| Sessions/media | Stored links and local requests → qualified agent adapters, retained conversations/files, owned windows, upload receipts | Same-session reopening; no duplicate launch; attachment failure preserves draft |
| Rendering | Current tray/menu → square per-OS assets, full available height, dense rows, every state/counter, keyboard/accessibility | Actual macOS/Windows sizes/themes/DPI, 16/20px and 20-project review |

Implementation order: identities/receipts and held creation; host admission/access; planning/recipes/delivery; environments/media; integrated native qualification. Each slice includes its affected R/§ contract, executable model and adapter tests. Do not defer safety to a final polish pass. Do not create an unrestricted workflow engine or a separate hidden task database.

## Decisions still needed

No accepted layout, Auto default or recipe choice needs another vote. Engineering gates G1–G8 are implementation work, not questions for the owner to design locks or APIs.

Accepted follow-up recommendations: continue independent branches with valid inputs/resources after another branch fails; hold dependants. Defer monetary budgets until reliable provider usage/pricing attribution exists. Session limits never promise a spending cap.

Remaining project input: actual five-repository membership, revisions, build dependencies and prepare/check/cleanup commands. This is needed for a real integration fixture, not another global preference vote.

Calibration remains: load thresholds/windows/pacing, native icon overflow, runtime attachment capabilities. Use the selected starting values as candidates; measure, then amend the owning spec with evidence. Escalate only a resulting user-visible trade-off.

## Prototype coverage and limits

The browser candidate still demonstrates legacy Auto/Fixed controls alongside optional caps, workload-pressure states, policy review, recipe drafts, held-plan reminder, explicit start, questions, failed builds and cleanup projection. Fixed is a prototype compatibility gap against the host-service target, not an accepted current host policy. The host target permits optional ceilings without a required count ceiling. Editing settings or selecting a recipe applies nothing until review. Existing conversations and Access/Delivery controls remain available.

No prototype can certify scheduling, security, remote session access, atomic board effects or real cleanup. Report each slice as Specified / Prototyped / Implemented / Qualified; never replace these with one Done flag.

Custom icon implementation: versioned shared preset/schema, capability catalogue, one renderer for Summary/Custom, agent draft/preview tools, validated atomic local preference save and re-edit/revert. See [custom-icon.md](custom-icon.md).

[Refactoring backlog](refactoring-backlog.md) defines ordered workstreams, API boundaries, dependencies and acceptance for implementation tasks.
