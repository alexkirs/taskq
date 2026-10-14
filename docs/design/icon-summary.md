# Square summary icon

Product selection: D11 option 1 is accepted in the [decision register](decision-register.md). Remaining candidate language below refers to implementation/calibration, not a request to reapprove that direction. No running policy changes.

Summary is the built-in configuration of the [shared icon renderer](custom-icon.md), not a separately coded drawing path. Its immutable preset and Custom use the same primitives, font metrics, state transforms and validation. Edit Summary copies its configuration to a Custom draft.

Approved two-level composition: ordinary numerals, square visible bounds.
[Contract](../../taskq.md#product) · [Menu M1](menu.md#m1-appearance) · [Geometry reference](square-summary-reference.html).
Replaces the wide strip as a design target. Runtime changes with implementation; Windows remains unqualified.

## I1 Composition

```text
work       answers
ready      cadence
```

- Work = doing + review; answers = ask; ready = ready. Fixed positions; zero remains visible in the icon.
- Four elements only. Active: no background, frame, grid, labels, q or play.
- N×N container; value changes never widen it.
- macOS: 22×22 logical points. Windows: actual slot/DPI; qualify 16/20/24/32 physical pixels, not one repeatedly resized PNG.
- Reference grid and light/dark backing surfaces are inspection aids; export transparent assets.

## I2 Type

- System sans serif: SF Pro/system UI on macOS; Segoe UI Variable/Segoe UI on Windows. Record the actual family in evidence.
- Upright, weight 500. No synthetic bold, italic, condensed, monospaced, segmented or pixel font.
- All three values share size/weight. Do not change tracking or distort glyphs.
- Use lining numerals and actual ink bounds; tabular advance is not optical alignment.
- Accepted macOS reference: N=22, approximately 13.5 logical px for 3/2/5. Recalibrate for another font.
- Single-digit calibration: start at 0.72N; decrease by 0.1 until all digits 0–9 fit I3. Fix that size per platform/font/N; no per-update jitter.
- The earlier reference fits current values for exploration. Shrinking 12/24/100 is a stress test, not an approved production overflow policy.

## I3 Geometry

Logical coordinates, top-left origin. Ink bounds mean painted glyph edges, not advance width, DOM box or line height.

```text
p = 0.5                         outer inset
g = N >= 24 ? 1.5 : 1           cell gap
L = p; R = N-p; C = N/2
cellW = cellH = (N-2p-g)/2
```

- Two equal columns and rows. Each numeral fits its cell.
- Top row shares a fixed baseline: Y = p + maximum ascent of digits 0–9 in the calibrated font.
- Work ink left = L. Answers ink right = R.
- Ready ink left = L, bottom = R. Invisible font side bearings do not set alignment.
- Allocated graph X = C+g/2; right = R; bottom = R; top = ready numeral's top ink edge. Painted bars use [R-UI5](rendering-contract.md#r-ui5-cadence-raster): integer widths/gaps, residual inset on inner left; outer right/bottom stay fixed.
- Ready and graph therefore have the same visible height; outside edges and bottom define the square.
- Align 1/3/7/8 by ink, not advance centering. Keep the left edge of 1; do not invent a wider glyph.
- Raster tolerance: at most 1 physical pixel from the computed edge. Do not round each element independently into asymmetric margins.

Canvas, textAlign=left and alphabetic baseline:

```text
inkLeft = originX - actualBoundingBoxLeft
inkRight = originX + actualBoundingBoxRight
left alignment: originX = L + actualBoundingBoxLeft
right alignment: originX = R - actualBoundingBoxRight
bottom alignment: baselineY = R - actualBoundingBoxDescent
```

## I4 Graph and color

- Stacked bars on a shared linear scale, using M6 series/order. Never stretch each bar to 100%.
- Exploratory geometry reference: 6 bars, 0.5 logical px gaps. Production uses R-UI5 physical-pixel quantization; this reference does not approve a bin count or fractional raster gaps.
- Earlier reference contains only close/failed. Product preserves all four M6 series; fixture simplification cannot remove data.
- Runtime has 12 bins. Adaptive rebinning is accepted under D11; sum adjacent bins, preserving period and every series total. Do not discard half the history.
- Acceptance fixture has nonzero first/last bars. Real zero edges stay zero. Expose exact totals in the menu when pixels cannot resolve a segment.
- UI colors come from [visual-style.md](visual-style.md#palette). Board labels keep their separate API palette.
- Paused: same geometry/data in gray, existing compact q/pause centered at N/2,N/2. Never substitute a new pause-button shape. Qualify overlay readability separately; partial pause does not gray everything.

## I5 Raster and acceptance

- Draw directly into N×DPR; scale geometry once. Standard antialiasing; no blur, sharpening or repeated resizing.
- Reference DPR=2 and ×8 enlargement do not prove 16 px/DPR=1 legibility.
- Supply natural-size and ×8 contact sheets: both themes; 16/20/22/24/32; DPR=1/2; active/paused.
- Counts: 0/0/0, 3/2/5, 1/7/8, 9/9/9, 12/3/24, 100/12/105. Graph: empty, mixed, equal counts across series, nonzero edges. Unknown follows [unknown is never healthy](https://github.com/taskqorg/specq/blob/main/engineering/verification.md#unknown-is-never-healthy).
- Check bounds and graph totals, then inspect PNGs and native trays on both platforms. Record OS, font, size, DPI and SHA.
- Open before release: overflow, unknown rendering, 6/12 bins and Windows DPR=1. Do not silently choose tiny text, 9+, truncation or a wider container.

## I6 Cross-platform rendering

- Both macOS and Windows expose Icon / Summary. Same fixed square composition, counter meanings, status colors and pause identity. Platform adaptation changes metrics, not semantics or available controls.
- macOS: calibrate against actual status-item/menu-bar size and backing scale; 22 logical points is the reference, not a universal screen constant.
- Windows: obtain notification-area small-icon metrics for the owning monitor DPI; treat 16 physical pixels at 100% as a qualification baseline, not a hardcoded slot for every machine. Supply a matching raster/HICON; re-render on DPI/theme changes and taskbar recreation. Keep stable tray identity and saved view.
- Qualify 16/20/24/32 physical pixels (100/125/150/200% baseline), actual OS slot and light/dark. Exported PNG size alone proves neither Shell scaling nor readability. No stretch of the macOS raster.
- Share data, layout rules and approved vector geometry; calibrate SF/Segoe ink bounds separately. Snap chart fills to physical pixels once; text retains native antialiasing.
- Candidate small-slot rebin: choose the largest divisor of 12 in 12/6/4/3/2/1 that fits integer bar/gap constraints, sum all four series. Keep full 12-bin detail and exact totals in menu. The adaptive policy is accepted; native size calibration and all-series preservation still require proof.
- Candidate overflow: a cell uses its largest readable exact value; beyond the calibrated cell capacity, show a labelled threshold such as 9+ and exact tooltip/menu values. Unknown uses an em dash with an unavailable explanation. No per-update shrinking or widening. Threshold and native legibility require visual acceptance.
- Windows removal of the current compact-only gate ships with native evidence; retaining that gate is not completion of MP1.
- Reference: [Microsoft NOTIFYICONDATAW](https://learn.microsoft.com/en-us/windows/win32/api/shellapi/ns-shellapi-notifyicondataw), [GetSystemMetricsForDpi](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getsystemmetricsfordpi). Query actual platform metrics; the prototype cannot detect the native Shell slot.
