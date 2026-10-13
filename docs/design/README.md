# TaskQ operator design package

Approved menu direction and AI window policy; detailed candidate for implementation planning.
Runtime remains governed by [taskq.md](../../taskq.md). Shared rules live in [specq](https://github.com/taskqorg/specq/); this package holds TaskQ-specific requirements.

Working process: [shared product design workflow](https://github.com/taskqorg/specq/blob/main/design/workflow.md). This package supplies its product decisions, references and evidence.

Feature selection: [TaskQ product principles](../product-principles.md). Preserve board ownership, interchangeable clients and the complete menu-only operator path.

## Start here: implementation handoff

Give the next agent this directory and [implementation brief](implementation-brief.md). Read the root contract first, then plan system and application work using the [ordered backlog](refactoring-backlog.md); the current batch order, development loop and owner decisions are in the [implementation plan](implementation-plan.md). No chat transcript or temporary visualization folder is required.

The current visual reference is [workspace-preview.html](workspace-preview.html); its editable/generated-source relationship is defined in the brief. Keep the entire directory, including `assets/`, `shots/`, `evidence/`, JSON fixtures/configuration and component sources. Historical alternatives are labeled below and are not competing final designs.

The package includes the remote PM scenario (UC-17), not only the desktop menu. The owner accepted all six shared workflow refinements and AI-assisted controls. Compatible shared rules: [specq 9f82cc6](https://github.com/taskqorg/specq/tree/9f82cc6). Use this revision or a compatible successor; an older copy may lack the accepted changes.

[Shared blocks](shared-blocks.md): parent ownership, compact consumer variants and cross-product fidelity.

[Tasks from findings](finding-tasks.md): shared reviewed creation, operation receipts and independent source verification.

## Current package

[Decision register](decision-register.md): remaining choices, numbered alternatives and recommended defaults. [Scenario review](scenario-review.md): complete supplied-case coverage, current evidence and implementation gates.

[System change map](system-change-map.md): required backend slices, full interface routes and remaining decisions.

[Existing-agent entry](agent-entry.md): discoverable bootstrap, canonical reports, capability limits and owner-requested observation.

[Comparative workflow review](comparative-workflow-review.md): VibeCrew, Moshi, Conductor and cmux; evidence, recommendations and implementation gaps.

| Material | Purpose |
|---|---|
| [Remote manager](remote-manager.md) | Universal remote PM flow, project routing, observation, answers/media and RM-A1–A9 |
| [FRA/Hermes audit](remote-manager-audit.md) | Inspected integration baseline and unqualified capabilities |
| [Integrated workspace](workspace-prototype.html) | Current Workspace review: menu, agent settings and AI conversations |
| [AI component](ai-conversations.html) | Conversation list, drafts, files, request states and recovery fixtures |
| [Task decisions and plans](plans-and-decisions.md) | Inline ask-card answers, AI-led plans and explicit multi-repository gaps |
| [Dynamic resources](dynamic-resources.md) | Accepted Auto default, optional caps, proposed tuning and migration |
| [Delivery behavior](agent-behavior.md) | Proposed Local / branch / review PR / auto-merge / direct workflows |
| [Agent settings](agent-settings.md) | Workspace access editor, project overrides, review and readback |
| [Product decisions](menu-product.md) | Approved direction, scenario coverage and candidate boundaries |
| [Emergency recovery](emergency-recovery.md) | Stop/interlock/reconciliation proposal and E checks |
| [Next variants](menu-next-variants.html) | Historical comparison; Workspace review selected |
| [Standalone candidate](final-menu.html) | Inspect menu, AI conversations/sessions, settings, icons and scenario states |
| [Editable fragment](final-prototype.html) | Earlier full candidate; integrated workspace takes precedence |
| [Menu](menu.md) | M1–M12 behavior and M-A1–M-A21 acceptance |
| [AI workspace](ai-workspace.md) | Conversation/session/history separation, windows, attachments, recovery and C checks |
| [Task counters](task-counters.md) | All-state totals, Closed and partial-data semantics |
| [Worker permissions](worker-permissions.md) | Host authority, runtime mapping, settings, #865, migration and acceptance |
| [Visual system](visual-style.md) | Typography, geometry, palette, icon inventory and asset sources |
| [Square summary](icon-summary.md) | I1–I5 precise glyph/graph geometry and unresolved policies |
| [TaskQ UI acceptance](operator-ui.md) | D-A1–D-A8 product checks; general D rules are upstream |
| [Use cases](../use-cases/README.md) | UC-01–UC-17, intersections, open Q/P decisions |
| [Adaptive rationale](adaptive-menu.md) | Stable controls, situational content, prioritization choices |
| [Current implementation inventory](current-menu-audit.md) | Installed/source evidence and preserve/relocate matrix |
| [Implementation brief](implementation-brief.md) | Reading order, unresolved contracts and delivery slices |
| [Rendering contract](rendering-contract.md) | Native size, scroll ownership, physical bar grid and data parity |
| [Implementation acceptance](ui-acceptance.md) | Complete feature ledger, native matrix and review gates |
| [Report template](ui-acceptance-report.md) | Builder/reviewer evidence and owner decisions |
| [4.1.9 audit](release-4.1.9-ui-audit.md) | Historical 4.1.9 source and native fixture findings; refresh against the implementation candidate |
| [Deterministic fixtures](ui-acceptance-fixtures.json) | Expected histogram, height and pixel-grid outcomes |
| [Verification](verification.md) | Candidate evidence, historical evidence and qualification gaps |
| [README page plan](readme-plan.md) | Project page order, screenshot refresh and stale rule, use-case rows, tasks A–C |

The editable candidate contains local simulation, not live service access. Detach models ownership inside the preview; native separate windows require implementation. AI attachments support local image preview and validation; no file bytes are sent. Rebuild the integrated fragment with `python3 docs/design/build-workspace-preview.py`; edit its source components, not the generated fragment.

## Related specifications

- [Board labels](board.md): names/colors and migration constraints.
- [Incident reports](incidents.md): fault catalog, reporting, consent and operator states.
- [Capacity](capacity.md): earlier limits/load-gate detail; current runtime authority is taskq.md.

## Historical references

`menu-prototype.html` and `icon-studies.html` document earlier iterations. Their tests qualify those models only. The wide summary and old phase-0/phase-4 assumptions do not override this package.
`square-summary-reference.html` preserves the earlier accepted geometry experiment; its dynamic multi-digit shrinking is not production policy.
Older board/research material may still need English translation; the current menu/AI handoff is English.

## Change routing

General rule: propose/review in specq. TaskQ decision: taskq.md first, then its linked product specification, candidate and evidence.
Preserve requirement IDs and current capabilities. A file, mockup or green model test is not shipped/native proof.

[Custom icon](custom-icon.md): shared Summary/Custom config renderer, agent editing protocol and CI acceptance. [Final menu review](menu-final-review.md) records wording, hierarchy and icon corrections.

[Refactoring backlog](refactoring-backlog.md) defines ordered workstreams, API boundaries, dependencies and acceptance for implementation tasks.

[Installation and project locations](setup-and-project-locations.md): agent-led and agent-free setup, Projects folder, exact execution roots and S-A1–S-A8.
