# Tray window #1104: bounded Windows evidence

Incomplete acceptance: native desktop dragging and tray-icon recovery are unproved. Windows denied `SetCursorPos` with error 5; no mouse drag was observed. macOS first-click behavior is implemented through Wry `with_accept_first_mouse(true)` but **Not tested** here; G8 owns that platform qualification.

The native GNU development artifact and UI source blob IDs are bound in [observations.json](observations.json). These captures use the artifact's own WebView2 page, real CDP pointer/keyboard events and production IPC; no injected fixture page or mocked bridge. The empty disposable installation has no board or service, and no running installation was changed. This proves the listed renderer/adapter observations, not complete desktop interaction qualification.

- [First content fit](initial-fit.png): 320 ? 525 CSS pixels at WebView2 pixel ratio 1.4; document height 525, no vertical or horizontal overflow.
- [Independent toggles](independent-toggles.png): Pin off and Top off; Win32 `WS_EX_TOPMOST` changed from set to clear. Saved toggles were read back from the disposable tray settings.
- [Resize](resized.png): bottom-right pointer drag changed 320 ? 525 to 425 ? 590; saved dimensions matched. After subsequent keyboard resizing, restart restored the exact saved dimensions and Top off.
- [Fixed header](fixed-header.png): content exceeds the resized viewport; the header's top is 0 after scrolling. Header dragging still uses the existing native drag adapter; the desktop input denial prevents its native acceptance here.

Development before/after: the original `ee0373f` page let the header scroll to ?399 px and had no Top control (browser check). A native development run before the sizing correction requested 320 CSS pixels but rendered at 229 ? 375 with document height 538 at pixel ratio 1.4. The final bridge sizes physical pixels from the page's reported ratio. Browser fit/header/keyboard-resize/refresh checks passed separately; browser checks do not qualify native desktop behavior.

Delivery gap: `docs/shots/check.sh` will detect changed tray source blobs. The required README refresh script is macOS-only (`docs/shots/update.sh`); the existing manifest is retained honestly, not relabelled as fresh. Refresh those native screenshots before merging. No full platform snapshot matrix was run.
