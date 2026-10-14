# Tray window #1104: bounded Windows evidence

Incomplete acceptance: native desktop dragging and tray-icon recovery are unproved. Windows denied `SetCursorPos` with error 5; no mouse drag was observed. macOS first-click behavior is implemented through Wry `with_accept_first_mouse(true)` but **Not tested** here; G8 owns that platform qualification.

The native GNU development artifact and UI source blob IDs are bound in [observations.json](observations.json). These captures use the artifact's own WebView2 page, real CDP pointer/keyboard events and production IPC; no injected fixture page or mocked bridge. The disposable installation has no board or service (empty for window checks, one synthetic cached project for row checks), and no running installation was changed. This proves the listed renderer/adapter observations, not complete desktop interaction qualification.

- [First content fit](initial-fit.png): 320 ? 525 CSS pixels at WebView2 pixel ratio 1.4; document height 525, no vertical or horizontal overflow.
- [Independent toggles](independent-toggles.png): Pin off and Top off; Win32 `WS_EX_TOPMOST` changed from set to clear. Saved toggles were read back from the disposable tray settings.
- [Resize](resized.png): bottom-right pointer drag changed 320 ? 525 to 425 ? 590; saved dimensions matched. After subsequent keyboard resizing, restart restored the exact saved dimensions and Top off.
- [Fixed header](fixed-header.png): content exceeds the resized viewport; the header's top is 0 after scrolling. Header dragging still uses the existing native drag adapter; the desktop input denial prevents its native acceptance here.

- [Task rows](task-rows.png): metadata and the local session link are above the complete wrapped title. The native Home, project and Attention routes all retained the exact title; no horizontal overflow. The shared renderer and inherited composer provenance bindings were updated, with the existing checksum checks retained.

Development before/after: the original `ee0373f` page let the header scroll to ?399 px and had no Top control (browser check). A native development run before the sizing correction requested 320 CSS pixels but rendered at 229 ? 375 with document height 538 at pixel ratio 1.4. After rebase, the bridge reuses the existing `popover_size` helper, including scrollbar gutter sizing, with the page's reported ratio and full work-area height. Browser fit/header/keyboard-resize/refresh checks passed separately; browser checks do not qualify native desktop behavior.

README freshness: `docs/shots/check.sh` reports changed tray source blobs as stale (warning under the newly accepted R12), not as a failed merge gate. The macOS README/release refresh retains its own qualification scope; the existing manifest is not relabelled as fresh. No full platform snapshot matrix was run.

The recorded code commit binds all embedded tray/project-blocks sources. The following evidence-only commit preserves those UI blobs; canonical checks run after the final evidence commit.
