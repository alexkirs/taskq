# Release 4.1.9 UI audit

Compared with the approved direction and current design package. This is a revision report, not runtime acceptance.

Publication note: main advanced to 4.1.10 (`71bc0bf`) before this package was published. Findings below qualify 4.1.9 only; recheck newer rendering changes before implementation.

## Evidence

- Local checkout remains `cfae06708776d252d52f31510afed6b47918cee6`; dirty design work preserved.
- Installed Mac pointer: 4.1.9. Inspected released source at `2d1ba851822c2506db7d9345998170e41a00f5fe`; no checkout reset or merge.
- Binary SHA256: `944244b01d720954f3b02c7e3a4207d46c8cf4ab87b9acd4eabcaa717bc3ff5a`.
- macOS 27.0.1 arm64, native WKWebView snapshot path, isolated fixture registry. Default fixture reports historical 4.1.0 service versions; this is input data, not renderer version.
- Captured both themes and 16/20 px; inspected dark home 16/20, light project 16, mixed icon and Ask-only icon/home. [Files and hashes](evidence/4.1.9/manifest.json).
- Snapshot mode expands groups and uses a different height cap than normal opening. These are diagnostic images, not proof of actual menu opening height, native installed icon placement, keyboard behavior or Windows fit.
- No Windows native session was available in this review. Its scroll complaint is user-reported; the shared sizing code supplies a plausible mechanism, not a Windows reproduction.

Sources: [tray.rs](https://github.com/taskqorg/taskq/blob/2d1ba851822c2506db7d9345998170e41a00f5fe/src/tray.rs), [tray.html](https://github.com/taskqorg/taskq/blob/2d1ba851822c2506db7d9345998170e41a00f5fe/src/tray.html).

## Findings

| ID | Priority / requirement | Observed | Required correction |
|---|---|---|---|
| V1 | Blocker · I1/I2 | `wide()` lines 907–936 renders width from numbers; `segments()` supplies seven-segment digits | Use approved fixed square/system glyph geometry; test actual OS slot |
| V2 | Blocker · M6/R-UI4 | Histogram holds four series; icon destructures `[close, failed, ..]`, peak excludes Ask/Blocked | Share four-series projection and scale; preserve all totals |
| V3 | Major · R-UI2/U-A2 | Size handler 369–370 caps at 80% total monitor height; snapshot uses 95%. Placement 558–571 uses monitor bounds, not work area | Fit natural content to anchor's available usable area before enabling scroll |
| V4 | Major · R-UI5/U-A6 | Icon pitch `(w+1)/12`, floating rectangles and supersampling; no common integer pixel lattice | Quantize one width/gap/baseline; preserve outer alignment; resolve tiny-bin feasibility |
| V5 | Major · D-A1/M-A17 | Fixed metadata columns, ellipsis at 14/16; Board below groups | Flowing titles and header Board; independently clickable session/PR links |
| V6 | Major · K/M-A18 | Project counters only work/ask/ready, include zero; no expanded aggregate/Closed | Implement all-state projections and authoritative quiet Closed total |
| V7 | Major · C/M7 | AI before attention, newest request only, full answer occupies menu; no workspace/attachment/session contract implemented here | Preserve existing revision review while introducing specified workspace |
| V8 | Major · M9/U-A1 | Services repeated on home; existing useful controls absent from some design demos | Relocate through complete feature ledger, never delete to match a mock |

V2 reproduction: replace isolated alpha events with three unique Ask events in the last bin. Native history reports Ask 3; [summary-ask-only.png](evidence/4.1.9/summary-ask-only.png) has no cadence bars. [Mixed icon](evidence/4.1.9/summary-mixed.png) retains Close/Failed. Current graph is event-based, not merely active-task count; the confirmed loss is two series in the icon.
V3: the 80% cap is a verified source deviation. Actual Windows premature scrolling remains unverified until native capture.
V4: fractional rasterization explains varying edge coverage; temporal drift was not recorded. Test a frame sequence before claiming its exact cause.

## Keep useful implementation work

- Resources: separate external monitor (`btop` launcher in the inspected source); preserve access under Controls/Services.
- Incident status, opt-in disclosure and enable/disable controls; preserve consent and unknown state when reorganizing.
- Agent probes include version/login freshness and codex-app; retain all supported runtimes.
- Capacity validation/readback, unknown load gate and explicit target host.
- Per-project Back scroll restoration, custom-range validation/timezone and incomplete-history explanation.
- Mixed-pause confirmation and per-project lifecycle targets.

Existing function does not imply approved placement or full native qualification. These additions are valuable; retain their behavior and improve presentation.

## Fix order and acceptance

1. Reconcile newer released source with design branch; preserve release fixes and local unrelated work.
2. Correct data parity and feature ledger before cosmetic changes.
3. Implement native sizing/scroll ownership; reproduce the Windows report.
4. Replace icon renderer and pin integer bar grid; settle I5 overflow/bin choices.
5. Complete AI/session/navigation contracts, then visual polish.
6. Freeze artifact; apply [native matrix and U-A1–U-A10](ui-acceptance.md). Current verdict: Fail against target; Windows Not tested.

No production service restart, permission change, deployment or runtime patch performed by this audit.
