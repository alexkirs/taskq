# TaskQ rendering contract

Target for native implementation. Apply [shared F1–F6](https://github.com/taskqorg/specq/blob/main/design/implementation-fidelity.md).
[Menu](menu.md), [icons](icon-summary.md), [visual tokens](visual-style.md), [AI](ai-workspace.md) remain semantic authorities.

## R-UI1 Reference ownership

- `final-prototype.html`: interaction/layout candidate. `final-menu.html`: generated preview. Neither is production code.
- Approved composition and feature decisions survive implementation. Candidate demo shortcuts do not: fixed demo heights, sample totals, synthetic links, simulated windows, static permission details.
- Compact q assets and I1–I5 own icon identity. Use ordinary system numerals; no seven-segment substitute. Match ink bounds and pause overlay, not a rough silhouette.
- Each acceptance report applies [F1](https://github.com/taskqorg/specq/blob/main/design/implementation-fidelity.md#f1-source-and-change-control). Implementation must identify pending I5 decisions instead of silently selecting an overflow or bin policy.

## R-UI2 Native menu sizing

Apply [F2](https://github.com/taskqorg/specq/blob/main/design/implementation-fidelity.md#f2-size-to-content-then-constrain). Logical units below. Convert physical monitor/anchor values exactly once using that monitor's scale.

- Windows: monitor containing the tray anchor, usable rectangle from the native work-area API (including taskbar). macOS: owning display's visible frame and menu-bar anchor; normalize coordinate origins.
- Keep 8 logical units at horizontal usable edges. Prefer a 4-unit anchor gap when it fits. Vertical height may use the full usable work area; reserve only native chrome/shadow or OS-required clearance, counted once.
- Base menu width: 360 at 16 px. Target `max(320, 360 × font / 16)`, clamped to usable width minus margins. Thus 18→405, 20→450 when space permits. Screen constraints may reduce below 320; reflow, never overflow off-screen.
- Prefer below a top/menu-bar anchor and above a bottom/taskbar anchor. Try the opposite side if needed, then slide vertically within the full work area before constraining height. The anchor side is a placement preference, not a height limit; vertical taskbars must not force a half-screen viewport.
- `availableHeight` is the full usable work-area height minus OS-required outer clearance, never only the space on one side of the anchor. `outerHeight = min(naturalContentHeight + nativeChrome, availableHeight)`. Up to 100% is allowed, not required. Short content stays short; fitting content has no primary scrollbar. No fixed 400/580/640 px or percentage runtime cap.
- `scrollViewport = outerHeight - nativeChrome - fixedHeader - fixedFooter`. All terms measured at actual font/DPI; clamp nonnegative. If chrome itself cannot fit, use a scrollable compact fallback retaining Back/Close and focused input, never an empty negative viewport.
- Coalesce layout passes; suppress repeated <=1 physical-pixel measurement jitter only when content remains fully reachable. A real 1-pixel overflow still requires fit/scroll handling. Do not resize continuously for same-size status updates.
- Open with correct size before exposure where supported. Changing monitor or taskbar placement recalculates usable space. Keep the anchor association while allowing vertical repositioning; do not cover the taskbar, menu bar or notch exclusion.
- Snapshot mode uses the same sizing algorithm as normal opening. Full-document diagnostic captures may exist, clearly labelled; they do not qualify visible-window fit.

## R-UI3 Scroll ownership

Apply [F3](https://github.com/taskqorg/specq/blob/main/design/implementation-fidelity.md#f3-scroll-and-focus). TaskQ surfaces:

| Surface | Fixed | Scroll only when needed |
|---|---|---|
| Home | view/admission/pause/Controls; Settings/AI footer | Work mode, attention, composer, aggregate, projects |
| Project / filtered tasks | Back, project/Board, current filter | task rows; all required states reachable |
| Settings detail | Back/title, contextual commit/cancel where present | settings groups, validation and detailed effects |
| AI conversation | window controls, context, composer | transcript; conversation list independent when side by side |
| Narrow AI | window controls and Back; composer in conversation view | full-width list OR conversation, not compressed columns |
| Change review | target context, Cancel/Confirm | long diff/evidence; buttons never cover last row |

- No-scroll means no reserved scrollbar strip unless native stable gutter is necessary to prevent wrapping oscillation; never use a visible fake track.
- Refresh keeps the focused task by identity. If it disappears, focus the nearest surviving item and announce the change.
- Test wheel/trackpad, thumb drag, PageUp/Down, Home/End, Tab and Shift+Tab. Last row and final action remain fully reachable.
- AI: growing input remeasures to its approved cap, then input scrolls. Closing a window preserves request identity.

## R-UI4 Cadence data

- Cadence counts events, never sampled active-task count. Current task counters and cadence are separate projections.
- Four series in order from bottom: closed, failed, Human/ask, blocked/waiting. A later successful close does not erase an earlier failure event.
- Human counter is current Ask tasks; Human bars are Ask events in the selected period. Display that distinction in the graph detail.
- Shared immutable histogram feeds tray and menu. Deduplicate events before binning, preserve M6 `[start,end)` and freshness semantics.
- Fixed-time bins: same duration, oldest left. Refresh inside a bin does not move individual bars. In rolling mode, advance the whole grid by one bin at a boundary; do not squeeze/stretch spacing or animate fractional drift. A chosen historical/custom range stays fixed.
- Rolling alignment (approved, M6): whole epoch-aligned bins, newest containing now; label last bin partial and exact displayed range. Existing exact custom bounds remain unchanged.
- Peak = largest sum of ALL four series. Never normalize bars independently. Menu totals match histogram; current Failed/Human counts need not equal event totals.

## R-UI5 Cadence raster

For an approved rendered bin count B and physical allocated width W:

```
g = approved integer physical gap, at least 1
b = floor((W - (B-1)*g) / B)
plotWidth = B*b + (B-1)*g
x0 = rightEdge - plotWidth
x(i) = x0 + i*(b+g)
```

- `b >= 1`; all bars exactly b wide, gaps exactly g, one integer baseline. Remainder stays at the inner left edge of the graph cell; outer right/bottom remain aligned with the square. Menu graph may center its residual inset.
- This refines I3's allocated graph origin into a physical painted origin; never stretch the final bar to consume remainder. Round shared outer bounds once.
- Segment boundaries: round `plotHeight × cumulativeCount / peak`; derive adjacent segment heights by subtraction. No independent rounding that creates holes or exceeds peak height. Quantization may change equal segment heights by at most 1 physical pixel; cumulative boundaries remain exact after rounding.
- Snap fills, not font curves. No border radius, stroke, blur, gradient or line interpolation on cadence bars.
- A positive segment smaller than a pixel may be unresolved; preserve exact labelled totals. Never add an artificial minimum height and pretend it remains quantitative.
- An all-zero bin retains its empty slot. Unknown bin is explicitly unavailable; no fabricated zero or event.
- If B cannot fit, the renderer cannot pass. Preserve 12 source bins; approve a divisor rebin count and sum adjacent bins across EVERY series. I5's 6/12/native-size decision is still open; no nearest-neighbor sample dropping or silent reduction.
- Production tiny icons render directly at native size/DPI; no CSS/OS post-scaling assumption. Test the actual OS icon after installation as well as exported pixels.

## R-UI6 Feature parity and improvements

- Use the complete ledger in [acceptance](ui-acceptance.md); link implementation evidence per row.
- Preserve 4.1.9 Resources access, incident state/consent controls, capacity validation/readback, agent probes, focus recovery and Back position.
- Resources belongs under Controls or Services, not another permanent home block. Keep the existing external resource monitor until replacement is approved.
- Native system contrast, keyboard behavior or a useful implementation addition is an [F1](https://github.com/taskqorg/specq/blob/main/design/implementation-fidelity.md#f1-source-and-change-control) candidate.
- No missing feature is “polish”. Wrong scope, series, confirmation or session routing blocks acceptance before cosmetic review.
