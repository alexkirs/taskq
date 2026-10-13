# Operator menu product contract

Accepted direction; implementation target, not shipped behavior. Inherit [M1–M12](menu.md), [C1–C8](ai-workspace.md), [K counters](task-counters.md), [R-UI rendering](rendering-contract.md) and [shared workflow](https://github.com/taskqorg/specq/blob/main/design/workflow.md). This file owns the newly resolved product choices and maps scenarios to surfaces; do not create another menu specification.

## Decisions

- **MP1 Platforms.** Both macOS and Windows offer Icon / Summary / Custom, persist the choice and preview the actual square. One composition and semantic data model; per-platform font calibration and native-size rendering under I6. Windows compact-only behavior is a migration baseline, not the target.
- **MP2 Work mode.** One editor. Distinguish a rule applying to future matches from an explicit task snapshot. Show matching/excluded tasks, dependencies outside scope and old→new effects for each project. Unsupported intent returns clarification; no silent broader match. Projects outside the change list remain unchanged. “Only these projects” must explicitly include the others' admission changes in review.
- **MP3 Conversations.** Main AI workspace with explicit detached windows; reopening raises the owner. Retain history until deletion; Archive is separate. One active request per conversation; other conversations use configured operator capacity. Drafting remains available. Window close never cancels. A cancellation requires backend acknowledgement and never implies rollback.
- **MP4 Attachments.** Menu and AI window support selection/drop; proposed initial types are PNG/JPEG/WebP, text/Markdown and PDF. Runtime capability and byte/page budgets remain engineering decisions; present actual limits before Send. Preserve drafts on rejection. Never silently omit a file.
- **MP5 Agents.** Read only / Workspace / Full plus effective Files, Network, Tools and exceptions. Show requested, saved and applied separately; host owns grants. Detailed review candidate: [AS1–AS4](agent-settings.md). UI direction approved; secure grant channel, OS enforcement, migration and default rollout still require P9 qualification. This approval changes no running permission settings.
- **MP6 Emergency.** Emergency stop is required and separate from Pause. Visible fixed top-strip action opens a compact named-host review; confirmation starts stopping immediately. Recovery is a dedicated route under [E1–E5](emergency-recovery.md), not ordinary Play. Mechanism below is a reviewable candidate; lifecycle implementation requires its R1/R11/§2/§4 amendments and traces.
- **MP7 First project.** With no usable service, show Set up TaskQ and explicit service/runtime availability. Do not display a Send action that cannot work. Existing-service onboarding stays in AI; zero-service bootstrap uses an inspected setup route and exact external-effects review before installation.
- **MP8 Adaptation.** Keep pinned projects in owner order; accepted default shows three unpinned recent projects ranked by explicit interaction. Attention initially shows two representative causes plus All. Limits are tunable defaults, not reasons to drop urgent items or aggregate counts.
- **MP10 Resources.** Auto by default on new qualified hosts, optional strict caps, explicit Fixed alternative; use [dynamic resources](dynamic-resources.md). Existing installations retain policy until reviewed migration.
- **MP9 Scope boundary.** Global machine budgets, cross-project urgency, generic multi-repository execution and transactional plan creation are separate backend work. Show supported routes and explicit unavailable states. Do not advertise missing operations as working.

Inline decisions and AI-led plans: [Q1–Q2 / P1–P6](plans-and-decisions.md), proposed implementation under MP9. Ordinary owner answers need no separate conversation; plan editing lives in the workspace.

## Scenario coverage

| Scenario | Entry → outcome | Required adverse case |
|---|---|---|
| Start a new project | Projects + → setup/AI → reviewed registration | No service, auth missing, existing files preserved |
| Plan or clarify work | Menu composer → conversation → proposal/result | Attach rejection, stale proposal, partial apply |
| Focus several projects | Work mode → rule/snapshot → per-project review | Unsupported intent, external dependency, Mixed |
| Answer / inspect failure | Attention or Human counter → explained inline choices → matching receipt; Failed → linked context | Stale board, inaccessible local session |
| Inspect execution | Project → task → local session; remote host indicator | No valid permalink; no invented remote link |
| Pause intake | Named-host pause review → readback | Existing attempts continue; partial target failure |
| Emergency stop | Fixed stop action → target review → stop progress | Unknown process, failed termination, offline board |
| Recover | Held banner → per-attempt reconciliation → reviewed resume | Dirty work, uncertain side effect, old attempt still live |
| Tune agents | Settings → Agents → effective policy → change review | Unsupported sandbox, old policy still active |
| Find an AI window | AI conversations list → raise selected owner | Restart/interrupted delivery; no duplicate request |
| Change tray view | Icon / Summary / Custom → immediate local preference | Small Windows slot, DPI/theme change, overflow |

## Accepted placement

**Workspace review is accepted.** The menu remains a quick controller; Work mode and recovery open a full-width workspace page beside conversation/session navigation. Preserve targets, context and drafts; reopen the existing workspace, never a duplicate.

[Earlier comparison](menu-next-variants.html) is historical. Menu review is not an active alternative. [Integrated prototype](workspace-prototype.html) shows only Workspace review, with menu, conversations, work sessions, Work mode and agent settings reachable. Placement approval does not approve unresolved safety/backend mechanisms.

## Next implementation gates

- Reconcile latest released source, not just the 4.1.9 audit. The 4.1.10 source still hides Summary on Windows and uses the wide segmented renderer.
- Execute the feature ledger and native matrix; every surface/state must have a route. Prototype actions are simulations.
- Resolve I6 overflow/rebin and E2/E3 safety mechanics through the reviewed candidate and executable lifecycle model before runtime changes.
