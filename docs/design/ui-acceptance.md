# UI implementation acceptance

[Shared process](https://github.com/taskqorg/specq/blob/main/design/implementation-fidelity.md) · [rendering rules](rendering-contract.md).
Historical release review: [4.1.9 audit](release-4.1.9-ui-audit.md). No current release is accepted against this target yet.

## Workflow

1. Builder reads taskq.md and package index, follows [F1](https://github.com/taskqorg/specq/blob/main/design/implementation-fidelity.md#f1-source-and-change-control) and lists unsupported decisions before implementation.
2. Builder completes the ledger and supplies the [F5](https://github.com/taskqorg/specq/blob/main/design/implementation-fidelity.md#f5-acceptance-package) package; each row names its fixture and navigation route.
3. Reviewer checks functionality/data first, sizing/navigation second, natural-size rendering third; findings use the F5 fields.
4. Polish, owner review, freeze and acceptance follow [F6](https://github.com/taskqorg/specq/blob/main/design/implementation-fidelity.md#f6-build-polish-accept); native evidence qualifies the implementation.

Use [synthetic expected fixtures](ui-acceptance-fixtures.json) for histogram, sizing-boundary and lattice checks; these are inputs, not native proof.
Use [report template](ui-acceptance-report.md). Keep reports per candidate; do not overwrite failing evidence. Fixed items link replacement evidence.

## Feature ledger

Builder completes the ledger per [F6](https://github.com/taskqorg/specq/blob/main/design/implementation-fidelity.md#f6-build-polish-accept). Each TaskQ row records initial, success, failure/unknown and recovery evidence where applicable.

| Surface / route | Must preserve | Requirements |
|---|---|---|
| Tray Icon/Summary/Custom | q, square type/grid, all-series cadence, gray pause overlay, partial/unknown | I1–I5, R-UI4/5 |
| Home top / Controls | named host, admission, pause review; urgent controls stable | M1/M5, M-A4/16 |
| Work mode | originals, interpretation, applied state; Mixed; explicit targets; CPU/memory and all runtimes | M4/M12, M-A9/20 |
| Attention / task | questions, failures, permission requests, service faults distinct | M-A7/10/21, P5 |
| Aggregate / projects | all states, Human=Ask, quiet Closed; dedup/partial | K-A1–K-A7 |
| Pinned/recent / All | stable order, search, complete registered list, add beside header | M-A2/18 |
| Project / tasks | Board at top, full flowing titles, issue/PR/local-session targets, remote host no-link | D-A1/2, M-A3/17 |
| Cadence detail | four series, totals, period/custom timezone, unavailable history | M-A5/6, R-UI4/5 |
| Menu composer | target context, attach/drop, history fill-only, Send opens conversation | M-A13/19 |
| AI workspace | selected conversation, drafts, active/archived lists, search, all C4 states | C-A1–C-A10 |
| AI windows | main/explicit detach, raise owner, recovery, no duplicated launch | C2/C-A2 |
| Work sessions | worker/reviewer, host/runtime/attempt, live/ended, valid local link | C6/C-A8 |
| Setup | picker cancel, new/existing/old/nonrepo, exact external effects, empty-install gap | M-A14 |
| Appearance | theme, 14/16/18/20, Icon/Summary/Custom, persistence; no icon scaling from text | D-A3–D-A8 |
| Plans | four editable recipes, held creation, explicit start, dependencies and partial retry | UC-03/06/07, D06/D08/D14, G2/G5 |
| Auto resources | shared host admission, optional caps, fresh load, wait reasons, effective readback | UC-04/12, D03, G3 |
| Delivery | separate commit/push/PR/merge policy, candidate revision and checks, publication receipt | UC-14, D04, G4 |
| Remote PM | project routing, opt-in watch, source-numbered answers, media and gateway recovery | UC-17, RM-A1–RM-A9 |
| Agents | probes, accounts, versions, disabled runtimes, effective policy/source | M9, P-A1–P-A10 |
| Services | freshness, logs/folder, incidents + disclosure/consent, Resources | M-A10–M-A12 |
| Emergency / recovery | named host, owned-process stop, durable hold, preserved work, selected fenced restart | E-A1–E-A9 |
| Lifecycle | pause/restart/stop/uninstall differences, named targets, confirmation/readback | M-A4/11/12 |
| About/update | tray/service/host versions, availability/progress/failure/rollback | M9 |

Permission, AI and remote extensions remain gated by their unimplemented or unqualified backend contracts. Accepted product direction does not resolve those engineering gates. Do not mark their overall row Pass from a mock.

## Native matrix

The full matrix runs only for release qualification and README shots; a task's check captures only the routes its diff changes (`taskq tray --snapshot DIR --route R,..`, [taskq.md R12](../../taskq.md#r12-unverified-means-unknown)).

- macOS WKWebView + installed menu-bar icon; Windows WebView2 + installed notification-area icon. Same fixture revision.
- Both themes; 14/16/18/20 px. Windows scale 100/125/150/200%; macOS actual 1×/2× displays where supported. Unsupported/unavailable cells explicitly Not tested.
- Windows 1366×768 and 1920×1080 work areas; bottom/top/side taskbar; secondary monitor with different DPI. macOS ordinary/notched menu bar, Dock reservation and external monitor.
- Empty, one project, 20 projects; 0/1/12/100/1000 counts; 180-character titles and long unbroken identifiers; stale/mixed/fully paused.
- Fixtures: content needs 95% and 100% of usable height; content exceeds anchor-side space but fits after repositioning; content fits by 1 pixel; equals available height; exceeds by 1 pixel; becomes smaller after collapse; async content grows; font changes while a field is focused.
- Counter/graph cases: closed-only, failed-only, ask-only, blocked-only, equal mixed stack, zero middle bins, nonzero edges, duplicate events, period boundaries, incomplete history.
- Graph time sequence: two refreshes within one bin, exact next boundary, late event and range change. Verify a frame sequence; a still image cannot prove motion.

## Hard gates

| ID | Pass condition |
|---|---|
| U-A1 | Every ledger route reachable; no unapproved omission; real effects verified separately |
| U-A2 | Natural content fits: no primary scrollbar; overflow: one intended scrollbar and reachable last control |
| U-A3 | Usable-area clamp, anchor/DPI conversion and resize/collapse correct on both OSes |
| U-A4 | Header/footer/composer never overlap content; focus/draft/Back position preserved |
| U-A5 | Histogram totals and series agree across adapter, tray and detail; no active-count substitution |
| U-A6 | Integer bar bounds, equal widths/gaps, shared baseline; approved temporal shift and rebin semantics |
| U-A7 | Icon shape/font/grid/pause match I1–I5 at native size; unknown/overflow policy explicitly accepted |
| U-A8 | Keyboard, accessible names, contrast and large text verified in native renderer |
| U-A9 | Existing 4.1.9 additions retained or explicitly superseded; proposed improvements resolved |
| U-A10 | Frozen artifact evidence complete; changed reference requires owner acceptance, not screenshot regeneration alone |

Blocking: wrong data/action target, duplicate side effects, lost series/features, clipped essential controls, unjustified scrolling, unreadable icon or unapproved semantics. Minor platform font antialias differences may be documented; geometry and behavior still pass.

Custom icon: CI-A1–CI-A7 in [custom-icon.md](custom-icon.md), including identical Summary/Custom rendering, approval persistence and native size/state matrix.
