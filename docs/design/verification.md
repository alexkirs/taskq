# Operator menu verification

## Scenario closure and defaults · 2026-10-14

- Added [D01–D14/G1–G8](decision-register.md) and [scenario review](scenario-review.md). Expanded supplied-case coverage to UC-01–UC-15; preserved earlier P IDs as aliases. Walkthrough covers entry, outcome, failure/recovery and remaining gates; not live end-to-end verification.
- Proposed editable default recipes and simplest-first implementation: existing tasks/dependencies/Ask, project-owned environments and existing commands. General default/customization principle added to specq; exact TaskQ values remain proposals, not applied policy.
- Read `add --later`: eligible issue creation precedes the `later` entry. Recorded a potential admission race for held-plan creation; source evidence only, no concurrent reproduction or runtime fix.
- Browser suites: plans/numbered decisions, AI conversations, integrated navigation, agent settings and Delivery all pass. These are fixture checks, not proof of the proposed backend semantics.
- `cargo test --locked`: 91 passed; `cargo run -- selftest`: 40 traces/formats passed and read-only board probe succeeded. Existing unused-result warning at `src/operator.rs:563` remains.
- Coverage consistency check: all 15 UC IDs mapped once in review, 14 decision headings, changed-doc local links pass; taskq.md 293 lines. specq checker and both whitespace checks pass.
- Remaining: owner selections, G1–G8, latest-release reconciliation and native/live acceptance. No service configuration, timers, permissions, board tasks, uploads or publication changed.

## Existing-agent entry · 2026-10-14

- Added [A1–A5](agent-entry.md): discoverable PM bootstrap, versioned rules, canonical reports, client capability limits and explicitly requested observation. Linked root Product, principles and UC-02; no timer/integration installed.
- Read `src/main.rs` wait/dispatch: bounded default 10 minutes, one project cursor, state-based detection. Identified initial catch-up, unchanged-state question revisions and multiple observers as qualification gaps; no claim that current wait provides durable delivery.
- Source/spec review only. Cross-agent rendering, bootstrap discovery and scheduled wake-up require client fixtures; no runtime changes or live watcher tests.
- `cargo test --locked`: 91 passed. Changed-document local links and whitespace checks passed; root contract 292 lines. Existing unused-result warning at `src/operator.rs:563` remains unrelated.

## Product foundation and numbered choices · 2026-10-14

- Added owner-defined [product principles](../product-principles.md), linked from the root Product contract. Reassessed competitor features against board portability, replaceable agents, headless operation and existing primitives. Added UC-10–UC-12; no runtime authority/default changes.
- Q1 and the AI workspace require source numbers/order across interfaces and receipts. General choice-identity rule belongs to `specq/design/workflow.md`; TaskQ answer tokens and authority remain project-owned.
- Prototype displays numbered task/plan/conversation choices. Tests verify numbers 1/2, exact source answer text, option-2 receipt, unnumbered custom answer and numbered AI context. Plans, AI and workspace regression suites pass.
- Visually inspected wide 16px and narrow dark 20px numbered choices; wrapping preserves the number column. AI pointer tests center both navigation targets within the standalone iframe before real clicks.
- `cargo test --locked`: 91 passed; `cargo run -- selftest`: 40 traces/formats passed and board read succeeded. Existing unused-result warning at `src/operator.rs:563`. specq checker, changed-spec local links and whitespace checks pass; root contract 291 lines.
- Evidence upload, generic adapters, structured operation schemas, machine-wide budgets and full menu planning remain design/implementation gaps. Native menus and external runtime behavior were not exercised.

## Decisions, plans and comparative research · 2026-10-14

- [Comparison](comparative-workflow-review.md): official VibeCrew/Moshi/Conductor/cmux docs; VibeCrew Bridge/Fleet screenshots visually inspected. No installed-product interaction or platform-parity proof.
- Q1/Q2 and P1–P6 separate inline decisions, dependency plans, task stages, environment finalization and backend gates. Shared `specq/design/workflow.md` adds AI-assisted controls; TaskQ-specific authority stays here.
- `plans-and-decisions.test.cjs`: ordinary/custom answers, sending lock, stale/unknown guards, draft-only AI handoff, reviewed plan, creation/start without duplicate cards, pause/retry, separate question identities and narrow themes pass. Board receipts and transitions are simulated; concurrent writers require backend evidence.
- Workspace navigation and Delivery regression checks pass. Inspected `plans-wide.png`, `plans-light.png`, `plans-dark.png`; fixed stage title/reason collisions and re-inspected wide/dark captures.
- AI regression passes, including restored drafts and narrow conversation navigation. Pointer diagnostics found the standalone iframe edge receiving the click; the test centers the target before a real pointer click. No application workaround or forced click added.
- `cargo test --locked`: 91 passed. `cargo run -- selftest`: 40 golden traces and formats passed; board read succeeded. Existing unused-result warning in `src/operator.rs:563`. Runtime untouched.
- specq checker passed. Native menu sizing, runtime reply adapters, real planner, multi-repository ownership/cleanup and stage gates remain unqualified.

## Full available menu height · 2026-10-14

- Owner-approved sizing clarification: natural height up to 100% of the usable monitor work area. Reposition before scrolling; anchor-side space is not a height cap. Short menus stay short.
- Shared F2 updated in specq; TaskQ R-UI2 owns native allowances and placement. Acceptance adds 95%/100% height and middle-anchor cases.
- Integrated HTML reference already has natural panel height and no primary internal scroll cap; no visual code change required. Input/image caps are independent controls, not menu-height limits.
- Native Windows/macOS sizing remains unqualified; no runtime renderer was changed in this specification pass.

## Delivery behavior proposal · 2026-10-14

- B1–B5 propose shared runtime behavior with project overrides. §7 remains unchanged: current `pr` publishes during close; manual handoff is not implemented.
- Representative workflow catalogue, not usage-ranked research. Local, branch, review PR, auto-merge and direct presets; dependent options reduce effects, never silently broaden them.
- `agent-behavior.test.cjs`: preset sequences, commit/push/PR guards, draft-PR merge block, retained project drafts, inherit review, single confirmation, stale review refusal and unknown readback pass. Integrated navigation regression passes.
- Inspected wide/light and narrow/dark 20px; corrected preview-state synchronization and added active Access/Delivery tab indication. Fixture merge capabilities require forge validation before implementation.
- `cargo test --locked`: 91 passed. Runtime unchanged; publication lifecycle, native platform behavior and cross-host policy storage remain unqualified.
- Shared-spec review: density, dependent controls and truthful readback already covered. Only project publication policy added.

## AI conversation review · 2026-10-14

- C9 extends the selected workspace. AI source: `ai-conversations.html`; builder composes menu, settings and AI. General specq rules unchanged.
- Browser checks: isolated drafts, navigation, confirmation guard, partial-result retry refusal, cancellation acknowledgement, archive/delete, attachment preview/removal/type/size errors, drag/drop, restart metadata and explicit retry, menu handoff, simulated window ownership, transcript retention.
- `ai-conversations.test.cjs` and `workspace-prototype.test.cjs` pass. Inspected integrated light and narrow light/dark 20px screenshots under `evidence/menu-next/ai-*.png`; fixed unreadable primary-button hover.
- `cargo test --locked`: 91 passed. `cargo run -- selftest`: 40 golden traces and formats passed. Existing unused-result warning at `src/operator.rs:563`; runtime unchanged.
- Preview states are synthetic, attachment reads are local, requests never leave the preview. Widget persistence stores metadata, not attachment bytes. Native windows, actual upload/cancellation/reconciliation, transcript scroll-follow and OS accessibility remain implementation gates.

## Integrated preview correction · 2026-10-14

- Restored menu and workspace navigation around agent settings; no discarded Menu review alternative. Compose with `python3 docs/design/build-workspace-preview.py`; the menu and settings files remain editable sources.
- Replaced indirect Readback selector with immediate Preview state fixtures. Production Refresh status stays distinct from preview controls.
- Browser tests cover all preview states, menu→Agents, conversations/sessions navigation, preserved draft and narrow 20px layout. Light and dark integrated screenshots inspected; fixed host label to match the selected platform.
- These checks establish prototype navigation only, not real service effects or native OS behavior.

## Agent settings detail · 2026-10-14

- AS1–AS4 detail the accepted workspace route; host permission architecture remains a proposal under P9.
- Browser checks: draft/effective separation, saved/readback, stale review refusal and explicit reload, failed application preserving prior effective policy, separate runtime drafts, narrow 20px overflow.
- Inspected light settings and dark narrow failure receipt; captured under `evidence/menu-next/agents-*.png`. Corrected active/effective revision labels; existing attempts retain their original revision.
- `agent-settings.html` is an isolated settings view for review, not a replacement for the complete menu reference. Network/preset and fixture exceptions are editable; files/tools explain capability limits. No actual policy change, broker, filesystem enforcement or native-platform qualification.
- General principles unchanged: canonical F/P rules already cover this; only TaskQ settings details added.

## Product decisions and next variants · 2026-10-13

- MP1–MP9 record accepted direction; I6 specifies common square composition with native Windows/macOS metrics. E1–E5 propose required emergency stop and explicit recovery; runtime authority is unchanged pending model amendments.
- Reviewed 4.1.10 source (`71bc0bf`): `src/tray.rs` still gates full view with `!cfg!(windows)` and keeps the wide renderer. Earlier 4.1.9 audit remains version-specific.
- Microsoft NOTIFYICONDATAW guidance inspected: incorrect-size notification icons may be scaled; supply native sizes and qualify Shell output. Browser simulations do not qualify Windows.
- `menu-next.test.cjs`: stop/recovery guard, selected-task restart review, scope snapshot, page errors and narrow 20px overflow checks. Chrome headless via Playwright; bundled browser absent, installed Chrome used.
- Inspected light recovery, dark workspace review and narrow dark 20px captures in `evidence/menu-next/`. Fixed stacked icon labels, theme repaint and partial-recovery status after inspection.
- Two placement alternatives extend the existing package; they do not replace the full feature ledger. File chips hold metadata only; scope uses an explicit fixture; dialogs/windows, access grants, native stop/restart and service readback are simulated.
- Shared-rule review: F1–F6 already cover source ownership, cross-platform rendering, truthful states and acceptance. No new universal emergency lifecycle rule added; fencing/interlocks belong to TaskQ until a reusable contract is demonstrated.

## Rendering fidelity and released UI audit · 2026-10-13

- Added shared F1–F6 in specq; TaskQ [R-UI1–R-UI6](rendering-contract.md), [acceptance ledger/matrix](ui-acceptance.md), report template and deterministic expected fixtures. Shared changes are local, not published.
- Human confirmed by owner: Ask, waiting for a human answer. Updated counter contract.
- Audited released 4.1.9 source and isolated native Mac snapshots; [findings and evidence](release-4.1.9-ui-audit.md). Ask-only fixture proves missing Ask bars in summary. Preserved useful Resources, incident consent and Back recovery in the target ledger.
- Snapshot sizing differs from live opening. No native Windows, live-popup sizing, keyboard or temporal-motion qualification. Fractional geometry is source evidence, not a recorded animation diagnosis.
- Fixture JSON/arithmetic, local file links, both repository whitespace checks and specq lint passed. taskq.md remains 300 lines.
- Documentation/evidence only in this revision; no runtime patch, production restart, permission change, commit or push. Cargo suites not rerun. Pending I5 and rolling-bin proposals remain explicit.

## Counters and permissions candidate · 2026-10-13

- Added [counter semantics](task-counters.md), [permission proposal](worker-permissions.md), UC-08 and linked contract-amendment targets. Existing R9 runtime defaults unchanged.
- Read #865 comments and upstream Folders text; findings are reported evidence, not a fresh TCC reproduction. Inspected local runtime launch and CLI help (Codex 0.159.3, Claude 2.1.296); official documentation linked in the proposal.
- Node model PASS: prior conversation checks plus provider/server/repository/issue deduplication, disjoint lifecycle counts and reopened Closed removal. Coherent fixtures only; freshness/conflicting snapshots and board pagination require backend tests.
- Browser inspected: dark 16 px compact all-state strip and quiet Closed; light 320 px/20 px Closed navigation and permissions review. Fixed flex-shrinking header overlap revealed by long lists. Root client/scroll width both 288 px.
- Permission UI: Full sets unrestricted runtime network, identifies absent isolation; review names host/runtime and new-attempt effect. Confirmation explicitly reports simulation. No policy files, services or permission grants changed.
- Node checks, local links, 300-line contract cap, both repo whitespace checks and specq lint passed. No runtime/Cargo/native-security qualification claimed.

## Final candidate · 2026-10-13

Sources: [candidate](final-prototype.html), [AI contract](ai-workspace.md), [implementation inventory](current-menu-audit.md).

- Installed 4.1.8 native fixture snapshots inspected: dark home/16 px, light project/16 px. Isolated TASKQ_INSTALL; no live service mutation. Source inventory at `cfae06708776d252d52f31510afed6b47918cee6`; installed/source mismatch recorded.
- `node docs/design/final-prototype.test.cjs`: PASS. Conversation/revision isolation, duplicate confirmation refusal, local-session eligibility, detach/reopen/reattach ownership.
- Browser candidate inspected at 1024 px in light/dark. Narrow 320 px with 20 px type: root client/scroll width both 288 px; no horizontal overflow. Fixed responsive grid minimum and theme controls after visual review.
- Browser interactions: narrow conversation list opens/selects; draft survives hide/reopen; detached ownership retained; supported-change confirmation produces a receipt and removes Confirm. Work sessions distinguish local links, remote host without link, historical attempts.
- Rebuilt standalone from fragment. TaskQ contract remains 300 lines. Local documentation links and whitespace checked.

Limits: candidate only. Files select metadata; uploads, native windows, service writes and agent launches are simulated. Browser layout/model checks do not qualify OS tray pixels, Windows DPR1, screen readers, crash recovery, attachment security or live backend contracts. No runtime code changed; Cargo regression suites were not rerun for this documentation/prototype package.

## Historical package evidence

The records below qualify their named artifacts/releases only. They do not override the current candidate or establish its native acceptance.

Sources: [menu-prototype.html](menu-prototype.html), [icon-studies.html](icon-studies.html).
Requirements: [operator-ui.md](operator-ui.md), [menu.md](menu.md).
Phase 0 records the approved design target; it does not implement a service adapter, native popover or AI channel.
The HTML uses English and a small state/action bridge target instead of OpenAI widget state.

## Checks for this package

Run `node docs/design/menu-prototype.test.cjs`, `node docs/design/icon-studies.test.cjs`,
`cargo test --locked`, `cargo run -- selftest`, and `git diff --check` before handoff.
Cargo checks establish current-product regression evidence, not implementation of the target menu.

Observed for this package: both Node commands pass (19 menu fixture checks; icon model, render and pause checks).
`git diff --check` passes; all 21 existing acceptance IDs remain and M-A14 is appended.

Rust verification: `cargo test --locked` in Ubuntu WSL passes 15 tests; `cargo run -- selftest`
passes 31 golden traces and the GitHub board read. Native Windows cargo stopped before tests because the
GNU toolchain could not find `dlltool.exe`; this is not a native Windows PASS.
No existing D-A or M-A ID is removed or renumbered; Add project appends M-A14.

## Coverage and limits

| Criteria | Evidence to inspect | Qualification still required |
|---|---|---|
| D-A1, D-A2, M-A3 | Generated issue rows contain status, number, title and the correct project URL; text escaping | Native click-through and navigation |
| D-A3, M-A1 | Font choices, compact default, platform-specific view gating; 22 pt macOS summary study | Settings restoration, physical size and OS scaling |
| D-A4, D-A5, D-A8 | Compact grid and count labels; shared visible digit/graph edges; proportional stacked bars | 320/360 px, long names, 0/1/12/100 counts, both themes, 100/200% scaling |
| D-A6, D-A7 | Accessible labels, keyboard targets, hidden empty groups | Contrast, screen reader, touch targets and color-independent meaning |
| M-A2 | Fixture task deduplication; installed registry target; observation focus off distinct from ARM | One host tray over real registry, actual focus plus host filtering; no separate PM filter |
| M-A4, M-A11, M-A12 | Scoped maintenance presentation and saved-focus requirement | Mixed pause, label prompt when focus missing, neighbor-project safety, restart, stop, uninstall, update readback |
| M-A5, M-A6 | Event-ID and [start,end) model; local events-cache contract and incomplete start-date presentation | Service delivery of time/task/close or failed/attempt ID as event ID, cache recovery; no board replay and no false zero |
| M-A7, M-A8, M-A9, M-A10 | AI proposal, stale/retry and request-state fixtures; original request target via focus `--request` | Phase 4 R3/R9 contract, real transport, confirmation, readback, menu close/reopen, agent failure |
| M-A13 | Request-history model: 20 latest, deduplication, empty requests, separate recall/send, persistence/clear | Native persistence and actual interaction |
| M-A14 | Add project section, four folder classifications and phase 4 placement | Native OS folder picker, §6 conversation inside menu AI, confirmed external effects, register/install or §9 cutover |

Windows supports only compact q and hides summary/M1 chooser. macOS summary uses about 12 pt digits and 7 pt graph within 22 pt;
the approved 47×29-study deviation retains proportional bars and centered compact pause overlay.
Geometry assertions are model/static evidence, not raster or native qualification.

## Runtime boundary

Phases 1–3 exclude M7 AI and Add project. Phase 4 includes both; Add project has no terminal handoff or clipboard phrase.
Folder classification: no config plus empty → new onboarding; no config plus files → existing onboarding;
config plus passing selftest → register and offer service install; config plus failing selftest/old format → §9 cutover.
The exact old-project marker remains the sole owner question (proposal: config present plus failing selftest).
Service target history is `<root>/.taskq/events`; incomplete history reports its start date. Original request is stored by
`taskq service focus ... --request` and displayed as “You asked”. A verified newer version in status.json exposes
“Update now”, acting through `<root>/.taskq/update`. These are implementation targets, not shipped features.
`taskq.md` and Rust remain unchanged; §4 is still the runtime contract.

## Phase 2 renderer

`src/tray.rs` and `src/tray.html` ship the wry popover, the page bridge, icons, actions and `<install>/tray.json` (§ 4).
Fixture: `sh docs/design/tray-fixture.sh DIR`, then `TASKQ_INSTALL=DIR/install taskq tray --snapshot DIR/shots`.
It covers a long title, HTML in a title, the same #12 in two repos, a paused project with a saved focus, a service
error, a stale status.json, an observation-only project without a log, and a duplicated close event.

Observed: `cargo test --locked` covers icon geometry (run/pause, wide numbers, gray paused summary), cross-board
deduplication, settings validation and base64. `taskq tray --snapshot` wrote 8 icon PNGs and 6 popover snapshots
(dark and light; home at 16 and 20; project page) natively on macOS (WKWebView) and on Windows (WebView2), for the
fixture and for the live install. Inspected: one row per board and issue, "Paused 1 of 2", error and stale rows apart
from Failed, escaped titles, ellipsis at 14–16 and wrapping at 18–20.
Windows: a GNU toolchain build links `WebView2Loader.dll` dynamically and does not start without it next to the exe;
the MSVC build of the release workflow links it statically.

Not qualified here (follow-up task): menu bar and notification-area placement, 200% scale, OS screenshots of the open
popover, click-through, keyboard and screen reader, and pause, resume, restart, stop and uninstall run from the popover.

## Phase 3: history, update, mixed pause

The fixture adds a day of history in alpha (3 closes, 3 failed, first and last of 12 bins not empty), no history in
beta and gamma, and `update: 4.1.1` in alpha. `--snapshot` adds a `home-actions` page per theme: a custom range with
start >= end, then `Pause all…`. Screenshots: [shots/](shots/).

Observed on macOS (WKWebView): "Refused: start must be before end." with the period unchanged; the confirm lists
alpha: pause, beta: paused, unchanged, gamma: observation, unchanged; "Paused 1 of 2: beta"; "No history in beta,
gamma." and "incomplete" in the total; "Update now to 4.1.1"; the summary icon draws the same bins in both themes.
`cargo test --locked` checks `[start, end)`, equal oldest-left bins, equal green and red heights for 3 and 3 on one
peak scale, custom range refusal and `since` without an events cache.

Not qualified here (release task after #765): Update now against a real release with the status.json readback,
both hosts on 4.1.0, the Windows popover of this phase, and the menu bar icon in the OS. See Release 4.1.0.

## Release 4.1.0

`v4.1.0` tags 995ff65 (#780, the #765 merge); the release workflow published the macOS, Windows and Linux binaries,
`SHA256SUMS` and `latest.json` 4.1.0.

Update on both hosts went through `<root>/.taskq/update`, the file `Update now` writes, because the running 4.0.11
trays have no button. Both logs read `update requested: 4.1.0 found` and `updated to 4.1.0; restarting`.
macOS re-executed, and `taskq service status` reads `"version": "4.1.0"` on mac; its tray restarted from
`releases/4.1.0`. Windows wrote `current` 4.1.0 but kept running 4.0.11: the re-exec exits only if no child is live at
that moment, never later, and the service went on admitting. `taskq service restart` waited for the live children,
then `taskq service status` on win read `"version": "4.1.0"`. The Windows re-exec needs a fix in a later release.

`Update now` is only half observed. With `update` null the live macOS popover (`taskq tray --snapshot`, WKWebView,
real registry) shows no button and the footer `taskq 4.1.0 · mac`. An installed service updates itself as soon as it
finds a newer version, so the button lives at most `update_every` (900 s) after a release. The click and its readback
need a release newer than 4.1.0 while a host runs 4.1.0; until then the fixture `Update now to 4.1.1` (phase 3) is the
only rendering.

| Criteria | Observed natively | Gap |
|---|---|---|
| D-A1, D-A7 | macOS live popover: one line of status, number, title per task; empty groups hidden | Windows popover of 4.1.0 |
| D-A2, M-A3 | Project page in place with `All projects` back | Click-through to the issue on both OSes |
| D-A3, M-A1 | Text size 16 and 20 px applied to the whole popover; compact and summary icons rendered | Persistence across a tray restart, OS scaling |
| D-A4, D-A5, D-A8 | 320 px width, long titles end in an ellipsis; dark and light; counts on one right edge | 360 px, 200 % scale, counts 12 and 100 on a live board |
| D-A6 | Text labels next to colors (`Waiting for answer`, `In work`, `Later`) | Keyboard, screen reader, touch targets, contrast |
| M-A2 | One tray per host over the real registry (one project per host) | Several projects on one host |
| M-A4 | Phase 3 fixture: mixed pause confirm names each project | Mixed pause on a live host |
| M-A5, M-A6 | Live history: hour period, closes in bins, `Incomplete: history since …; earlier is unknown, not zero.`; custom start >= end refused | Failed in live history, events-cache recovery |
| M-A10 | Service error and stale rows apart from Failed (phase 2 fixture) | AI error: phase 4 |
| M-A11, M-A12 | `service restart` on Windows waited for live children and left the other host alone | Pause, resume, restart, stop and uninstall clicked in the popover; resume label prompt; update readback |

## Release 4.1.1

`v4.1.1` tags a1f70ac (main as is, #820); the release workflow published the macOS, Windows and Linux binaries,
`SHA256SUMS` and `latest.json` 4.1.1 at 00:27 UTC on 2026-10-13. macOS logged `updated to 4.1.1; restarting` and
re-executed with a live child; heartbeat #722 read `version 4.1.1` at 00:37. Windows logged the same line and wrote
`current` 4.1.1 but kept running 4.1.0 with no live child: the running 4.1.0 still has the re-exec bug that #804
fixes in 4.1.1. `taskq service restart` finished it; heartbeat #745 read `version 4.1.1` at 00:48. The #804 fix
itself is first exercised by the update after 4.1.1.

`Update now` was not clicked: an installed service installs a version as soon as its check finds it and writes
`update` null (`src/main.rs`, service loop), so `status.json` never reports a newer version there and the button
never shows. A bare `1` folds as `answer 1` only on 4.1.1 hosts; no task was in `ask` with a fresh bare reply.

## Release 4.1.3

`v4.1.3` tags ce03891 (main as is, the first build on coreq, #836) after `tests` passed on it; the release workflow
published the macOS, Windows and Linux binaries, `SHA256SUMS` and `latest.json` 4.1.3 at 07:13 UTC on 2026-10-13.
macOS: the first `<root>/.taskq/update` check raced the publish (`up to date`), the second logged
`updated to 4.1.3; restarting` and re-executed with live children #836 and #837; heartbeat #722 read `version 4.1.3`
at 07:14, `previous` 4.1.2, and `releases/4.1.3/healthy` appeared after `rollback_after`. The tray of 4.1.2 exited and
the service started `releases/4.1.3/taskq tray` 30 s later (fresh `<install>/tray`). Windows updated on its own
`update_every` check without a restart: heartbeat #745 read `version 4.1.3` at 07:27, `status.json` 4.1.3 with no
error, and the service started `taskq-tray.exe` (4.1.3) about 70 s later with a fresh heartbeat.

Rollback (coreq.md § Launcher) on macOS with the released 4.1.3 binary as launcher in a temporary `TASKQ_INSTALL`:
`current` 4.1.3 whose release exits 1, `previous` 4.1.2. Starts 1 and 2 ran 4.1.3 (`starts` 1, 2); start 3 printed
`taskq: 4.1.3 failed to become healthy 3 times; back to 4.1.2`, wrote `current` 4.1.2 and ran it; start 4 ran 4.1.2.
The real install was not rolled back; Windows rollback was not tested.

## Remaining acceptance

Beyond the phase 2 snapshots: native placement, renderer opening/closing, actions against a live service,
OS icon rendering in the menu bar, keyboard/screen reader, scale/theme, multi-project safety, actual events-cache completeness,
AI onboarding and external-effect confirmation remain unqualified. A prototype/model PASS is not an OS or service PASS (R12).
The earlier local file preview was rejected by browser URL policy; this package makes no visual PASS claim.
Canonical package and source priority: [README](README.md); phased work: [implementation brief](implementation-brief.md).

## Auto resources and pipeline reconsideration

Spec-only iteration: accepted D01/D02/D04/D05/D07–D14 recommendations; D08 milestone reminder; D03 Auto target with unqualified numerical tuning; D06 reopened with three concrete alternatives. Owning specs and scenario routes updated. No live policy or prototype changed.

Validation: `cargo test --locked` passed 91 tests; `cargo run -- selftest` passed 40 golden traces and formats, with successful board read. Existing unused-Result warning remains at `src/operator.rs:563`. Local file links checked in 15 affected documents; URL examples excluded. Root contract remains 293 lines. `git diff --check` passed.

These checks do not prove dynamic admission, safe held creation, recipe gates or environment finalization. G1–G8 qualification and native/live scenario execution remain open. Competitor conclusions use the saved documented research, not a fresh installation test.

## Accepted recipes and integrated menu refresh

Supersedes the preceding D06-open status: D06 option 2 accepted. Root/register/plan/scenario contracts and implementation handoff updated; system-change-map.md separates backend requirements, engineering calibration and remaining project input.

Integrated candidate now demonstrates Auto/Fixed, optional host/project/runtime caps, load-window selection, pressure/stale states, policy review and four recipe drafts. Creation remains held by default; explicit start reuses existing fixture cards. Top admission status and telemetry agree with pressure. No backend policy changes.

Validation: six browser suites passed (workspace, AI conversations, agent settings, agent behavior, plans/decisions, resources/recipes). The new suite checks review-before-apply, invalid Fixed configuration, pressure/readback, all four held-first recipes and wide/narrow layouts. Cargo: 91 tests passed; selftest: 40 traces plus formats and board read passed. Existing unused Result warning in src/operator.rs:563 remains.

Visual inspection: resources-wide, resources-narrow, recipe-review-wide and recipe-held-narrow, including light/dark and 16/20px. Corrected the contradictory Admitting/pressure state and reinspected final resources-wide and recipe-held-narrow. Text/control wrapping is readable; native work-area sizing, OS icons and actual separate windows still need platform qualification. Browser page height is not proof of native menu sizing.

Source components generate workspace-prototype.html and workspace-preview.html. Simulated effects and backend handoff points remain labelled. No commit, push, deploy, timer or runtime grant change.

## Custom icon and final wording review

Accepted Custom mode and the shared Summary/Custom configuration architecture. Summary now comes from icon-summary.config.json; the integrated build uses the same paint function for built-in and saved custom configurations. Added UC-16, CI protocol/acceptance, final menu review and implementation handoff links. Accepted independent-branch continuation and deferral of monetary budgets recorded.

Prototype changes: three view modes, Custom editor/copy/edit shortcut, actual-size and enlarged previews, three sample compositions, local font/gap/counter/alignment controls, draft/save/discard/revert and contextual AI handoff. Persistence here is preview widget state, not native application storage. Arbitrary AI generation, native write failure/stale revision fencing and platform font qualification remain unimplemented.

Removed duplicate appearance view controls; shortened Chats/Sessions labels; moved fixture selectors behind Test states; removed ordinary selection-card borders; retained exact source-numbered choices and meaningful action labels.

Checks: seven browser suites passed (custom-icon, workspace, plans, resources/recipes, AI conversations, agent settings, agent behavior). Custom checks identical Summary/Custom pixels across five sizes and zero/large/unknown counts, draft isolation, apply/discard/revert, state restoration, invalid/overlapping configs and AI handoff. Cargo: 91 passed; selftest: 40 traces and formats passed, board read succeeded. Existing src/operator.rs:563 unused-Result warning remains.

Visual inspection: final-home, final-work, custom-light and custom-dark. Checked navigation, readable actual-size samples versus labelled enlargement, narrow 20px wrapping and direct edit access. Native icons/windows and real agent rendering are not qualified by these browser images. Root contract: 294 lines; local links/config/build embedding and diff whitespace checks passed.

## Project counters and publication qualification

Restored project-scoped nonzero counters in pinned/recent and All projects; known all-zero rows contain only project identity. Unknown values remain explicit. Counter navigation binds both project and state. Added K-A7 and the ordered refactoring/API backlog.

Integrated current origin/main without reverting its document cleanup or runtime fixes. Fresh validation: 93 Cargo tests and selftest (40 traces, formats and board read) passed. All eight browser suites passed, including project-counters, Custom, workspace, plans, resources, AI, Access and Delivery. Existing unused-Result warning remains. Project counter light/dark screenshots inspected at 16px and narrow 20px; counters wrap without losing the project name. Local documentation links and diff whitespace checked.

This publishes specifications, prototype and implementation backlog. No native menu/backend rollout is claimed.

## Accepted final package, 2026-10-14

All six shared workflow refinements and AI-assisted controls are accepted and saved in the owning specq files. The handoff now maps each menu capability to service/API semantics, backlog workstreams and observable acceptance. UC-17 covers remote PM behavior; no separate chat transport or live watcher is implemented by this prototype.

Rebuilt workspace-prototype.html and workspace-preview.html from canonical components. The approved layout is unchanged. Visual review removed the redundant Plan label in the plan shortcut; the name and state remain visible. The exact source/reference/shared-rule hashes are in [final-package.json](evidence/final-package.json). Sources are local changes, not proof of remote publication.

Validation: all eight browser suites passed. After the one-label correction, plans/decisions and resources/recipes passed again. 93 Cargo tests passed; selftest passed 40 golden traces, formats and a board read. Shared style checker, changed-document links and diff whitespace checks passed; taskq.md remains within 300 lines.

Fresh visual inspection: ai-narrow-dark, workspace-integrated, project-counters-light, custom-dark, resources-wide and recipe-held-narrow. Reviewed 16px wide/light and 20px narrow/dark wrapping, counters, actionable numbered questions, drafts, policy review and held-plan start. Reinspected recipe-held-narrow after the label correction. Existing model checks are not proof of native sizing, platform accessibility, delivery or scheduling.

Still Not tested: actual macOS/Windows native menu/icon/window behavior for this design, live UI-to-service mutations, host-wide Auto, remote human-answer/watch/media round trip, gateway recovery and multi-repository execution. No deployment, remote migration, policy change, commit or push in this iteration.
