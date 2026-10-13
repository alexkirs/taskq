# Implement the TaskQ operator design

Replace the current menu with the approved stable adaptive layout and a proper AI workspace. Preserve every current capability in [the inventory](current-menu-audit.md).
This package specifies the target; it does not authorize deployment or claim backend gaps are resolved.

## Read in order

1. `AGENTS.md`, [taskq.md](../../taskq.md), referenced specq rules.
2. [Use cases](../use-cases/README.md), [menu](menu.md), [AI workspace](ai-workspace.md).
3. [Visual style](visual-style.md), [square icon](icon-summary.md), [TaskQ acceptance](operator-ui.md).
4. [Rendering contract](rendering-contract.md), [acceptance ledger/matrix](ui-acceptance.md), [report template](ui-acceptance-report.md).
5. [4.1.9 audit](release-4.1.9-ui-audit.md), [Current implementation inventory](current-menu-audit.md), [candidate](workspace-prototype.html), [verification](verification.md).

Use `menu-next-variants.html`, `ai-conversations.html`, `agent-settings.html`, `agent-behavior.html` and `plans-and-decisions.html`, `custom-icon.html` and `icon-summary.config.json` as component sources. Build `workspace-prototype.html` with `build-workspace-preview.py`; `workspace-preview.html` is the standalone preview. Never edit generated output directly. Older menu/icon studies are historical; the square geometry supersedes the wide strip.
The candidate is a simulation: marked placeholder actions and mock receipts are not backend adapters.

[MP1–MP10](menu-product.md) supersede earlier open product choices; [E1–E5](emergency-recovery.md) and I6 remain explicit mechanism/raster candidates.

## Approved direction

Apply [product principles](../product-principles.md) to every slice. Preserve external board ownership, source-numbered choices and headless menu operation; proposed capabilities are not runtime guarantees.

- Fixed view/admission/pause/Controls strip, attention-first content, pinned/recent project access.
- Flowing issue/title rows, nonzero symbol counters, board links in headers, verified local session links and remote-host indicators.
- Approved Workspace review: Work mode and emergency recovery open in the existing workspace; menu remains the quick controller.
- One shared Work mode editor; per-project/host/runtime limits, explicit change targets, mixed values preserved.
- Short AI composer with attachments; one main workspace with Conversations and Work sessions. Explicit detached conversations; reopen raises the existing owner window.
- System type, restrained palette, consistent icons; compact q and fixed two-level square summary.

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

Builder fills every acceptance-ledger row and pins reference/artifact/fixture hashes. Reviewer reproduces deviations, then polishes shared causes. Preserve Resources, incidents/consent and navigation recovery from 4.1.9. Owner decides design changes; no repeated approval for matching existing requirements. Fail and Not tested remain open.

## Commands

- Integrated design: `workspace-prototype.test.cjs`, `ai-conversations.test.cjs`, `agent-settings.test.cjs`, `agent-behavior.test.cjs`, `plans-and-decisions.test.cjs`, `resources-recipes.test.cjs`, `custom-icon.test.cjs` under `docs/design/`.
- Rebuild: `python3 docs/design/build-workspace-preview.py`, then `python3 <visualize-skill>/scripts/render.py <repo>/docs/design/workspace-prototype.html <repo>/docs/design/workspace-preview.html --force`.
- Existing tests remain relevant to existing behavior, not proof of the new layout.
- Runtime implementation: required `cargo test --locked` and `cargo run -- selftest`, plus focused adapter/native checks. Follow repository launcher instructions.

Report implemented, tested, published and deployed separately. All project content is English; general rules evolve in specq, TaskQ decisions here.

[System change map](system-change-map.md) owns backend slices, acceptance and the remaining product questions. No design approval enables runtime policy.

[Custom icon](custom-icon.md): shared Summary/Custom config renderer, agent editing protocol and CI acceptance. [Final menu review](menu-final-review.md) records wording, hierarchy and icon corrections.

[Refactoring backlog](refactoring-backlog.md) defines ordered workstreams, API boundaries, dependencies and acceptance for implementation tasks.
