# TaskQ operator design package

Approved menu direction and AI window policy; detailed candidate for implementation planning.
Runtime remains governed by [taskq.md](../../taskq.md). Shared rules live in [specq](https://github.com/taskqorg/specq/); this package holds TaskQ-specific requirements.

Working process: [shared product design workflow](https://github.com/taskqorg/specq/blob/main/design/workflow.md). This package supplies its product decisions, references and evidence.

## Current package

| Material | Purpose |
|---|---|
| [Standalone candidate](final-menu.html) | Inspect menu, AI conversations/sessions, settings, icons and scenario states |
| [Editable fragment](final-prototype.html) | Canonical source for the current interactive candidate |
| [Menu](menu.md) | M1–M12 behavior and M-A1–M-A21 acceptance |
| [AI workspace](ai-workspace.md) | Conversation/session/history separation, windows, attachments, recovery and C checks |
| [Task counters](task-counters.md) | All-state totals, Closed and partial-data semantics |
| [Worker permissions](worker-permissions.md) | Host authority, runtime mapping, settings, #865, migration and acceptance |
| [Visual system](visual-style.md) | Typography, geometry, palette, icon inventory and asset sources |
| [Square summary](icon-summary.md) | I1–I5 precise glyph/graph geometry and unresolved policies |
| [TaskQ UI acceptance](operator-ui.md) | D-A1–D-A8 product checks; general D rules are upstream |
| [Use cases](../use-cases/README.md) | UC-01–UC-07, intersections, open Q/P decisions |
| [Adaptive rationale](adaptive-menu.md) | Stable controls, situational content, prioritization choices |
| [Current implementation inventory](current-menu-audit.md) | Installed/source evidence and preserve/relocate matrix |
| [Implementation brief](implementation-brief.md) | Reading order, unresolved contracts and delivery slices |
| [Rendering contract](rendering-contract.md) | Native size, scroll ownership, physical bar grid and data parity |
| [Implementation acceptance](ui-acceptance.md) | Complete feature ledger, native matrix and review gates |
| [Report template](ui-acceptance-report.md) | Builder/reviewer evidence and owner decisions |
| [4.1.9 audit](release-4.1.9-ui-audit.md) | Current released source and native fixture findings |
| [Deterministic fixtures](ui-acceptance-fixtures.json) | Expected histogram, height and pixel-grid outcomes |
| [Verification](verification.md) | Candidate evidence, historical evidence and qualification gaps |

The editable candidate contains local simulation, not live service access. Detach models ownership inside the preview; native separate windows require implementation. Uploaded-file chips model selection only; no file bytes are sent.

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
