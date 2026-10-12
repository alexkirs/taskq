# Operator menu verification

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
both hosts on 4.1.0, the Windows popover of this phase, and the menu bar icon in the OS.

## Remaining acceptance

Beyond the phase 2 snapshots: native placement, renderer opening/closing, actions against a live service,
OS icon rendering in the menu bar, keyboard/screen reader, scale/theme, multi-project safety, actual events-cache completeness,
AI onboarding and external-effect confirmation remain unqualified. A prototype/model PASS is not an OS or service PASS (R12).
The earlier local file preview was rejected by browser URL policy; this package makes no visual PASS claim.
Canonical package and source priority: [README](README.md); phased work: [implementation brief](implementation-brief.md).
