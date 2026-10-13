# Implement the TaskQ operator design

Replace the current menu with the approved stable adaptive layout and a proper AI workspace. Preserve every current capability in [the inventory](current-menu-audit.md).
This package specifies the target; it does not authorize deployment or claim backend gaps are resolved.

## Read in order

1. `AGENTS.md`, [taskq.md](../../taskq.md), referenced specq rules.
2. [Use cases](../use-cases/README.md), [menu](menu.md), [AI workspace](ai-workspace.md).
3. [Visual style](visual-style.md), [square icon](icon-summary.md), [TaskQ acceptance](operator-ui.md).
4. [Rendering contract](rendering-contract.md), [acceptance ledger/matrix](ui-acceptance.md), [report template](ui-acceptance-report.md).
5. [4.1.9 audit](release-4.1.9-ui-audit.md), [Current implementation inventory](current-menu-audit.md), [candidate](final-menu.html), [verification](verification.md).

Use `final-prototype.html` as editable design source and `final-menu.html` as its standalone preview. Older menu/icon studies are historical; the square geometry supersedes the wide strip.
The candidate is a simulation: marked placeholder actions and mock receipts are not backend adapters.

## Approved direction

- Fixed view/admission/pause/Controls strip, attention-first content, pinned/recent project access.
- Flowing issue/title rows, nonzero symbol counters, board links in headers, verified local session links and remote-host indicators.
- One shared Work mode editor; per-project/host/runtime limits, explicit change targets, mixed values preserved.
- Short AI composer with attachments; one main workspace with Conversations and Work sessions. Explicit detached conversations; reopen raises the existing owner window.
- System type, restrained palette, consistent icons; compact q and fixed two-level square summary.

## Plan with the owner before runtime work

- Resolve remaining C3/C4/C7 choices: attachment support/budgets, retention/deletion, cancellation, operator concurrency and interrupted delivery.
- Resolve UC Q1/Q3–Q6: PM direct actions, precise focus, cross-project priority, multi-repo environment and dependency-plan operations.
- Resolve empty-install AI bootstrap and emergency stop. Preserve current authority until contracts change.
- Resolve I5: overflow/unknown, bins and native Windows legibility. Do not copy exploratory shrinking.
- Confirm the shortlist/recent/attention caps before hardcoding them. Overall layout is approved.

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

Builder fills every acceptance-ledger row and pins reference/artifact/fixture hashes. Reviewer reproduces deviations, then polishes shared causes. Preserve Resources, incidents/consent and navigation recovery from 4.1.9. Owner decides design changes; no repeated approval for matching existing requirements. Fail and Not tested remain open.

## Commands

- Design model: `node docs/design/final-prototype.test.cjs`.
- Rebuild preview: `python3 <visualize-skill>/scripts/render.py <repo>/docs/design/final-prototype.html <repo>/docs/design/final-menu.html --force`.
- Existing tests remain relevant to existing behavior, not proof of the new layout.
- Runtime implementation: required `cargo test --locked` and `cargo run -- selftest`, plus focused adapter/native checks. Follow repository launcher instructions.

Report implemented, tested, published and deployed separately. All project content is English; general rules evolve in specq, TaskQ decisions here.
