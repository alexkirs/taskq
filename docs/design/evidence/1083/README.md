# #1083 History summary wraps instead of truncating

Finding from [#1051](../g8-macos/report.md) U-A5: the Activity history summary ended `ask 1 …` at 16 px and `ask…` at 20 px, hiding `waiting` (and at 20 px the ask total).

Fix (`src/tray.html`): the History summary wraps between totals and never inside one (`kind N` and the separator stay joined), right-aligned, no ellipsis.

Fixture: the [ua5 events](../g8-macos/ua5-events.txt) in the Day window, with the bins #1051 observed the tray push (bins 1, 7 and 12; totals closed 3, failed 2, ask 1, waiting 2), page `cadence`.
[ua5-page.js](ua5-page.js) writes the real `tray.html` with that state per theme and text size; Chrome 1× headless (`--force-device-scale-factor=1`, Windows 11, Segoe UI) renders it at the popover width the page sets (320 px for 14/16, 360 px for 18/20). Chromium layout instrument, not the native tray.

| Text size | Before (origin/main) | After |
|---|---|---|
| 14 px | one line, all four totals | one line, all four totals ([light](ua5-light-14.png), [dark](ua5-dark-14.png)) |
| 16 px | ellipsis after `ask 1 · w` ([light](before-ua5-light-16.png)) | two lines, all four totals ([light](ua5-light-16.png), [dark](ua5-dark-16.png)) |
| 18 px | ellipsis | two lines ([light](ua5-light-18.png), [dark](ua5-dark-18.png)) |
| 20 px | ellipsis ([dark](before-ua5-dark-20.png)) | two lines ([light](ua5-light-20.png), [dark](ua5-dark-20.png)) |

Measured in each page: summary `scrollWidth > clientWidth` false after (true before at 16-20 px); text `Day · closed 3 · failed 2 · ask 1 · waiting 2`.

Not shown: Windows `taskq tray --snapshot` on this host (100%, 2560×1440) captures every route, Home included, with the page wider and taller than the window (both scrollbars), so its cadence images cannot show the summary; macOS `tray --snapshot` of this fixture not run.
