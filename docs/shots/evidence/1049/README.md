# Windows G8 qualification evidence (#1049)

Report: [qualification-1049.md](../../../qualification-1049.md). Candidate `9ab83b63052a099c826a26cbeca8fe22e99c920d`,
release build `4.2.0-dev.1792018150`, Windows 11, WebView2 155, display scale 100 %, Text size 140 % (pixel ratio 1.4).
Times are UTC on 2026-10-14. Host `winq` is the isolated scratch project; the real host was not touched.

- `popover-{light,dark}-<route>-{16,20}.png`, `icon-summary-run-light-22pt@2x.png`: `taskq tray --snapshot` of the
  candidate with the isolated install (a selection of the 52 images; the run exited 0 with the overflow and label checks).
- `native-01-home-pinned.png`: live pinned window on Home. The tooltip over the footer belongs to the flyout icon that
  the harness hovered; it caught the first two footer clicks (see `native.log`).
- `native-02-settings-unpinned.png`: unpinned popover on Settings, opened from the hidden-icons flyout.
- `native-03-home-repinned.png`: pinned again, Home.
- `native-04-grip-resized.png`: after Right ×2 and Down ×2 on the resize grip.
- `native.log`: native clicks and keys, with the page heading, focused element and widths after each step.
- `update.log`: the update from 4.1.11 with a live child, then two service restarts.
- `admission.log`: Auto admission gate samples after a restart and the `service capacity` output.
- `uia-settings.txt`, `uia-home.txt`: Windows UI Automation tree of the live window (PowerShell writes symbols such as
  `◎` as `?`).
