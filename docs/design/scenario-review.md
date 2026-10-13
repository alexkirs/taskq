# Scenario closure review

Spec walkthrough of all supplied use cases; baseline design worktree `d7fd593` plus local proposals.
Not a fresh installed-release audit, live execution test or runtime implementation proof. Accepted product defaults and the accepted recipe approach are recorded in D01–D14.
[Cases](../use-cases/README.md) · [Decisions/defaults](decision-register.md) · [Native acceptance](ui-acceptance.md) · [Evidence](verification.md).

## Method and result

Walk entry → intended action → observable outcome → failure/recovery for each case.
Prefer existing commands/roles before a new subsystem. Keep unsupported alternatives visible as limitations.
All 16 cases now have a specified route and named gaps. None is marked end-to-end verified merely from specification coverage or a browser fixture.

## Case matrix

| Case / starting condition | Simplest complete route | Adverse case → required response | Remaining choice/gate |
|---|---|---|---|
| UC-01 No project, or existing files | Inspected local bootstrap → reviewed effects → register → status; choose admission separately | No service/runtime/auth → usable setup route, no dead Send; preserve files/account/visibility | D03/D05; G1/G6; actual first-install proof |
| UC-02 Existing agent session | Thin entry loads contract → canonical status → queue operations; observation by agreement | Unknown keyword/context loss/client cannot wake → reload or explicit on-demand fallback | D01/D09; G6; two-client bootstrap/report tests |
| UC-03 Idea, specs not ready | Discuss → spec/research tasks → scenario review → explicit implementation start | Duplicate/conflicting requirements → amend existing task or owner question; no automatic coding | D06/D08/D14; G2; current planning-role limits |
| UC-04 Urgent task, slots occupied | p0 → next eligible slot → usual checks/review | Pause, dependency, host mismatch/load hold → explain waiting, no capacity bypass | D03; G3; multi-project fairness |
| UC-05 Free-text scope | AI translates → exact rule or selected-task snapshot → review → matching readback | Unsupported exclusion/outside dependency/stale proposal → clarification; preserve untouched projects | D02; G2; rule semantics and races |
| UC-06 Five repositories | Project-owned environment → pinned setup → task work → compatibility → preserve → cleanup | Failed prep/build/cancel/cleanup → terminality and owned-resource inventory; keep shared/unpublished work | D07; G5; concrete repo topology/commands |
| UC-07 Large plan | Existing task reconciliation → dependency groups → create held or reviewed start | Cycle/missing dependency/partial write → reject or per-item reconciliation, no duplicate issue | D06/D08; G2; closed-vs-success dependency semantics |
| UC-08 Different permission needs | Host Access preset → exact grant review → new-attempt readback | Unsupported sandbox/worker-written grant/stale approval → block; Full never masquerades as containment | D05; G1; OS/provider negative tests |
| UC-09 Question in menu/task | Canonical numbered card → choice/custom answer → exact receipt → observed resume | Replaced card/double click/timeout/other client answered → reconcile; no answer retargeting | D08; G2; prototype exists, board race proof missing |
| UC-10 Change PM/client | Same board/task → compatible contract/status → permitted command | Unsupported runtime or missing session link → explain; never migrate a live attempt | D09; G6; capability matrix and portability proof |
| UC-11 External desktops closed | Service + headless runtime → menu planning/answers/results | No service, app-dependent runtime, interrupted application → capability reason and recovery | D01/D08/D09; G2/G6; complete operator planning backend |
| UC-12 Different computers | Named-host Auto + optional strict caps/access → review changes → status/Resources | Remote offline/stale load/unknown slots → no false zero or success; no invented remote btop | D03/D05; G1/G3; host totals and concurrency |
| UC-13 Interruption/switch | Pause or emergency → held state → liveness/output reconciliation → selected recovery | Unknown process/board offline/old binary/uncertain push → stay held, preserve work | D12; G1/G2/G3; native termination/fencing |
| UC-14 Result ready | Ticket evidence → candidate checks → configured handoff/publication → readback | Changed SHA/CI/permissions/media upload/unknown merge → pending or failure, no false close | D04/D10; G4/G7; source-independent evidence |
| UC-15 Daily oversight | Attention → task/project; settings → named service; exact detail behind small icon | 20 projects, large type, overflow, stale counts, hidden window, bad update → preserved routes and truthful state | D11/D13; G8; full native matrix |

UC-16 Custom icon: describe → catalogue/validated draft → actual-size comparison → approval → saved preference → re-edit/revert. Agent unavailability, unsupported primitives, corrupt config and font/DPI changes retain a valid active icon. CI-A1–CI-A7 and native persistence remain qualification gates.

## Cross-case invariants

Source finding for UC-07: `src/main.rs` creates an eligible issue before posting `later` for `add --later`. Concurrent admission is therefore a risk even though ordinary sequential tests pass. G2 must close this creation window before Create tasks only is advertised; reuse of `later` alone does not prove safety.

| Trigger | Required result | Proof needed |
|---|---|---|
| Ambiguous project/answer | Resolve exact target before mutation; keep source numbers | Adapter/client test |
| Empty match or missing data | Distinguish No tasks, Unknown and Partial; do not broaden focus | Board/status fixtures + live readback |
| Two clients confirm/retry | Same operation receipt or stale rejection; never duplicate a write/launch | Executable model + board race qualification |
| Timeout after external write | Unknown and reconciliation before retry | Fault injection/readback |
| Policy changes during work | Existing-attempt semantics explicit; final publication gate checks current authority | G1/G4 model traces |
| Cancel/close/archive | Cancellation acknowledged; closing UI hides; archive/delete preserves board work | Conversation model + native client tests |
| Host loss or quota exhaustion | Explain why waiting; fenced recovery only after liveness proof | Runtime/lease qualification |
| Dependency parked/cancelled/closed | Never equate arbitrary closure with verified prerequisite success | G2 lifecycle decision/traces |
| Default changes after upgrade | Existing overrides retained; broader effects reviewed; version displayed | Config migration tests |
| Same question via multiple surfaces | Same identity/options; reading does not resolve; authoritative answer clears all projections after refresh | Cross-client receipt tests |

## Complete UI inventory

Use the existing [feature ledger](ui-acceptance.md#feature-ledger), not another layout.

- Home/project/task routes, all-state counts, cadence and shortlist: UC-09/UC-15; no dropped series or projects from totals.
- Icon/Summary, pause, overflow and scrolling: UC-13/UC-15; native geometry remains unqualified.
- Composer/attachments/history, conversations/windows, work sessions: UC-02/UC-10/UC-11/UC-14; no duplicate agent on reopen.
- Work mode/limits/Access/Delivery: UC-04/UC-05/UC-08/UC-12/UC-14; same review path for AI and controls.
- Setup, services, accounts, logs, incident consent, Resources, update/rollback/uninstall: UC-01/UC-13/UC-15; preserve current-function inventory and unrelated registrations.
- Existing-agent bootstrap, reports and observation: UC-02/UC-10; plain text fallback, independent observer cursors, no unsupported wake-up promise.

## Implementation order

1. Preserve accepted D01–D14 selections; implement accepted D06 recipes using the common task/service model. Calibrate D03 Auto mechanics through G3.
2. G2/G6: shared identities, canonical reports and safe command contracts; prove one numbered answer through menu, CLI and board. Complete first-install bootstrap before advertising menu-only onboarding.
3. G3/G1: host resources and qualified grants; preserve current settings until reviewed migration.
4. G2/G4: plan creation/hold/start plus delivery; reuse dependencies/Ask before persistent stages.
5. G5/G7: project-owned multi-repo recipe, evidence upload and attachment recovery; introduce generic environment machinery only if needed.
6. G8 plus all cross-case checks: actual macOS/Windows menus, icon sizes, large text, keyboard, screen reader, events and live service round trips.

Do not defer safety prerequisites to the final visual pass. Each slice ships only with its model/adapter proof; the native pass checks the integrated product.

## Coverage limits

Current browser fixtures cover selected navigation, draft/attachment states, settings review, numbering and simulated plan/recovery states. They do not execute the 16 real workflows.
No new binary, host grant, scheduler, board mutation, upload or publication was performed by this review.
The remaining work is explicit: D03 calibration, implementation contracts G1–G8, current-release reconciliation and native/live qualification. The design inventory is complete for supplied requirements; implementation is not complete.
