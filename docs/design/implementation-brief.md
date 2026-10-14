# Implement the TaskQ operator design

Replace the current menu with the approved stable adaptive layout and a proper AI workspace. Preserve every current capability in [the inventory](current-menu-audit.md).
This package specifies the target; it does not authorize deployment or claim backend gaps are resolved.

## Read in order

1. `AGENTS.md`, [taskq.md](../../taskq.md), referenced specq rules.
2. [Decision register](decision-register.md), [system change map](system-change-map.md), [ordered refactoring backlog](refactoring-backlog.md), then [Use cases](../use-cases/README.md), [menu](menu.md), [AI workspace](ai-workspace.md).
3. [Visual style](visual-style.md), [square icon](icon-summary.md), [TaskQ acceptance](operator-ui.md).
4. [Rendering contract](rendering-contract.md), [acceptance ledger/matrix](ui-acceptance.md), [report template](ui-acceptance-report.md).
5. [Remote manager](remote-manager.md) and [FRA audit](remote-manager-audit.md), then [4.1.9 audit](release-4.1.9-ui-audit.md), [Current implementation inventory](current-menu-audit.md), [candidate](workspace-prototype.html), [verification](verification.md).

Use `menu-next-variants.html`, `ai-conversations.html`, `agent-settings.html`, `agent-behavior.html` and `plans-and-decisions.html`, `custom-icon.html` and `icon-summary.config.json` as component sources. Build `workspace-prototype.html` with `build-workspace-preview.py`; `workspace-preview.html` is the standalone preview. Never edit generated output directly. Older menu/icon studies are historical; the square geometry supersedes the wide strip.
The candidate is a simulation: marked placeholder actions and mock receipts are not backend adapters.

[MP1–MP10](menu-product.md) supersede earlier open product choices; [E1–E5](emergency-recovery.md) and I6 remain explicit mechanism/raster candidates.

## First deliverable: an executable implementation plan

Reconcile the current checkout/release with this package before estimating work. Audits are dated evidence; inspect existing implementation and board tasks, retain useful improvements, and avoid duplicate tasks. No accepted product choice needs another vote unless a new conflict is found.

Use the existing backlog as the single work breakdown. For each reviewable slice record requirement/use-case IDs, current behavior, required contract/model/API change, affected system and app components, prerequisites, acceptance proof and migration/rollback where needed. Separate engineering choices from owner-only trade-offs; unresolved owner questions include short options and a recommendation. Do not start dependent effects behind an unresolved gate.

Sequence shared read/operation contracts and safe held plans before wiring live actions; host admission/access before enabling Auto or grants; then recipes/delivery, environments and client/media adapters. Build UI against explicit typed fixtures while backend work is pending, but keep simulated actions labeled. Finish with native and remote end-to-end qualification. Use backlog dependencies rather than creating separate backend and UI plans that disagree.

Create implementation tasks only when authorized, with their specification links and observable acceptance. Planning must not silently start queue work. Do not invoke the historical FRA skill's setup recipes or migrate that host as part of reading this package.

## Approved direction

Apply [product principles](../product-principles.md) to every slice. Preserve external board ownership, source-numbered choices and headless menu operation; proposed capabilities are not runtime guarantees.

- Fixed view/admission/pause/Controls strip, attention-first content, pinned/recent project access.
- Flowing issue/title rows, nonzero symbol counters, board links in headers, verified local session links and remote-host indicators.
- Approved Workspace review: Work mode and emergency recovery open in the existing workspace; menu remains the quick controller.
- One shared Work mode editor; per-project/host/runtime limits, explicit change targets, mixed values preserved.
- Short AI composer with attachments; one main workspace with Conversations and Work sessions. Explicit detached conversations; reopen raises the existing owner window.
- System type, restrained palette, consistent icons; Icon / Summary / Custom. Summary and Custom use one configured renderer; preserve the fixed two-level square summary geometry.

## Plan with the owner before runtime work

Use [D01–D14](decision-register.md) for accepted defaults, accepted D06 recipes and proposed D03 tuning; use G1–G8 for engineering contracts. Do not reopen settled choices. [Scenario review](scenario-review.md) is the coverage/evidence checklist; Spec covered is not Runtime passed.

- Apply MP3 retention/archive and one-request-per-conversation decisions; qualify attachment budgets, cancellation acknowledgement, operator capacity and interrupted delivery.
- Apply MP2 rule/snapshot direction; resolve its formal filter contract and remaining UC Q1/Q3–Q6 details: PM direct actions, precise focus, cross-project priority, multi-repo environment and dependency-plan operations.
- Implement explicit empty-install setup and required emergency stop/recovery after E4 amendments. Preserve current authority until contracts change.
- Resolve I5: overflow/unknown, bins and native Windows legibility. Do not copy exploratory shrinking.
- Use accepted shortlist defaults: three recent projects and two representative attention causes, with All and full totals. Verify 20-project behavior. Overall layout is approved.

Counters: implement [K-A1–K-A6](task-counters.md), including authoritative Closed totals.
Permissions: discuss [P1–P9](worker-permissions.md) before changing R9; reconcile newer #865/#866 upstream changes. The settings candidate grants nothing.

## Delivery slices

| Slice | Deliverable | Proof |
|---|---|---|
| Contracts/data | Conversation/request/attachment identity, operation receipts, attempt metadata and URL allowlist | Executable state traces, retry/stale/partial outcomes |
| Navigation | Stable menu, attention, flowing rows, settings categories and current-function parity | M-A1–M-A21 plus inventory reconciliation |
| AI workspace | Conversation routing, drafts, windows, attachments, history and work-session browser | C-A1–C-A10; no cross-conversation context leaks |
| Work mode | One editor plus natural-language review under approved filter semantics | UC-04/UC-05; untouched fields preserved |
| Tray assets | Calibrated q/summary rendering and pause states | I5 native sheets; overflow decisions |
| Qualification | Both OSes, accessibility, large type, short windows, live service round trips | Evidence per platform/artifact SHA, limitations explicit |

Keep runtime changes and tests with their contract amendment. Lifecycle changes follow the executable behavioral model. Do not weaken checks to match a mock.
Readback, window reopening and service updates must not duplicate writes or agent launches. Preserve dirty work and unrelated project registrations.

## Review handoff

Qualification adds UC-10–UC-12: switch PM/client without copying tasks; complete planning/answer/result flow with external desktops closed using a headless runtime; distinguish per-project limits from machine totals. Preserve Resources → btop/btop4win and explain unsupported remote hosts. Keep visual evidence on the ticket through an authorized durable destination. Inventory machine-readable command results and planning operations before inventing new interfaces.

Builder fills every acceptance-ledger row and pins reference/artifact/fixture hashes; build, review and polish follow [F6](https://github.com/taskqorg/specq/blob/main/design/implementation-fidelity.md#f6-build-polish-accept). Preserve Resources, incidents/consent and navigation recovery from 4.1.9.

## Commands

- Integrated design: `workspace-prototype.test.cjs`, `ai-conversations.test.cjs`, `agent-settings.test.cjs`, `agent-behavior.test.cjs`, `plans-and-decisions.test.cjs`, `resources-recipes.test.cjs`, `custom-icon.test.cjs`, `project-counters.test.cjs` under `docs/design/`.
- Rebuild: `python3 docs/design/build-workspace-preview.py`, then `python3 <visualize-skill>/scripts/render.py <repo>/docs/design/workspace-prototype.html <repo>/docs/design/workspace-preview.html --force`.
- Existing tests remain relevant to existing behavior, not proof of the new layout.
- Runtime implementation: required `cargo test --locked` and `cargo run -- selftest`, plus focused adapter/native checks. Follow repository launcher instructions.

Status reports follow [specq Core](https://github.com/taskqorg/specq/blob/main/caveman/rules/core.md); general rules follow [shared-rule improvement](https://github.com/taskqorg/specq/blob/main/engineering/spec-first.md#improve-shared-rules), TaskQ decisions live here.

[System change map](system-change-map.md) owns backend slices, acceptance and the remaining product questions. No design approval enables runtime policy.

[Custom icon](custom-icon.md): shared Summary/Custom config renderer, agent editing protocol and CI acceptance. [Final menu review](menu-final-review.md) records wording, hierarchy and icon corrections.

[Refactoring backlog](refactoring-backlog.md) defines ordered workstreams, API boundaries, dependencies and acceptance for implementation tasks.

## Copyable assignment for the next agent

> Read AGENTS.md, taskq.md and docs/design/README.md. Follow the implementation brief and linked shared rules. Reconcile the design package with current code and existing tasks. Prepare a dependency-ordered implementation plan covering the service/CLI, native application and remote PM adapters; map each slice to existing requirements and acceptance. Preserve accepted decisions, ask only unresolved product questions with recommendations, and identify unsupported capabilities honestly. After implementation is authorized, amend affected contracts/models first, implement each slice, and verify actual behavior against the approved design and scenarios. Do not deploy, migrate FRA or enable observation merely because the design describes it.

## Final system-to-interface contract map

This is the development entrypoint for the accepted menu. Use the existing numbered backlog, not a second scheduler or task store. Public operation names and transport are engineering choices; the following are semantic contracts, not claims of existing CLI commands.

| Menu capability | System contract required | Workstreams / acceptance |
|---|---|---|
| Status, all-state counters, projects and task rows | Canonical snapshot with board/project/issue identity, source/freshness/completeness, runtime/host/session capability; filtered and aggregate counts from one projection | 1, 11; K-A1–A7, M/D checks |
| Ask cards and numbered replies | Authenticated actor and exact task/attempt/card revision; one accepted answer, stale rejection, durable board readback and uncertain-outcome reconciliation | 2, 9; UC-09, RM-A4 |
| Scope, pause and reviewed changes | Formal rule or explicit task set, exact selected hosts/projects, expected policy revision, partial receipts, preserved untouched fields | 2, 5; UC-04/05/12 |
| Auto/Fixed and optional limits | Host-wide reservation authority, runtime enablement, fresh telemetry, intersecting caps and explicit admission reasons | 5; G3, RM-A7 |
| Held plans and editable recipes | Safe creation before eligibility, task mapping/dependencies, explicit start, revision-bound gates and repeat-safe partial creation | 3–4; UC-03/06/07 |
| Access and Delivery | Separate host grant and project publication policies; requested/saved/applied values; exact candidate approval and supported isolation | 6–7; P checks, G1/G4 |
| AI conversations, files and separate windows | Project-bound conversation/request IDs, one active request, retained drafts/history, owner-window routing, attachment validation/storage/delivery receipts | 9, 11; C-A1–A10, G6/G7 |
| Worker session links | Recorded runtime/host/attempt, actual return capability, no duplicate attempt or fabricated remote permalink | 1, 9; C-A8, RM-A9 |
| Remote manager and monitoring | Compatible bootstrap, explicit project context, authorized actor/chat/topic, independent cursors, pending/delivered distinction and restart reconciliation | 9 with 1–3/5; RM-A1–A9 |
| Icon/Summary/Custom and history | Shared versioned icon schema/renderer, immutable Summary preset, validated approved Custom, canonical series/time bins, native DPI/theme calibration | 1, 10–11; CI/I/R-UI checks |
| Settings, setup, Resources and incidents | Named service/runtime capability and lifecycle operations; authorized diagnostics, consent, exact setup effects, progress/failure/readback | 6, 9, 11; M-A10–A14 |
| Emergency and recovery | Durable admission hold, owned attempt identities, stop outcomes, fencing and preserved work, reviewed selected restart | 6; E-A1–A9 |

For each row: reconcile current code and board work, amend the owning contract/model, implement service/CLI semantics, connect app/agent adapters, then capture real acceptance. Fixture success cannot close the system requirement. Use the [complete UI ledger](ui-acceptance.md) for route-by-route evidence, including failure and recovery.

## Package readiness

Accepted: product direction, six shared workflow refinements, and AI-assisted controls. Planning can proceed without reopening those decisions. Native/service implementation is not complete. Remote PM is specified and audited, not prototyped as a separate chat product. Concrete multi-repository inputs, resource/font calibration and current adapter capability qualification remain implementation inputs.

The current final browser reference is workspace-preview.html, assembled from the component sources above. It remains an interactive simulation. Publication must include the matching specq revision and all TaskQ source components, assets, configuration, fixtures and evidence; transfer the package through version control rather than temporary chat paths.

Installation handoff: [S1–S5](setup-and-project-locations.md) requires a real documented command per supported shell, shared setup without a running AI agent, host-owned Projects folder, exact launch/resume roots and no home discovery. Include S-A1–S-A8 in streams 1/6/9/11; audit the reported macOS prompts before attributing their cause.
