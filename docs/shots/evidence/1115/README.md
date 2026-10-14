# Home AI proposal bound (#1115)

Fixture: [#1059](../1059/README.md) with state `long-proposal`: a two-entry exchange whose newest answer is a proposal
with one `answer` op of 12 reasons, the shape found by G8 macOS acceptance (#1051).

```sh
capture=$(mktemp -d)
docs/shots/fixture.sh "$capture/fx" && node docs/shots/evidence/1059/fixture.cjs "$capture/fx" long-proposal
TASKQ_INSTALL="$capture/fx/install" cargo run --locked -- tray --snapshot "$capture/shots" --route home
```

Native capture: Windows 11, WebView2, `tray --snapshot --route home`, release build. The Windows window cap is smaller
than the Mac's.

Claude visual review:
- Before (origin/main `e739e07`), `before-long-proposal-dark-home-16.png`: the op under Send runs 13 lines, the preview's
  top line `Needs your answer` is cut mid-line, and Projects is at the bottom edge with no project row.
- After, `long-proposal-dark-home-16.png` and `long-proposal-light-home-20.png`: the op under Send is a scrolling block
  of 4 whole lines, `Confirm: apply 1 change` and the Projects group show below it, and the exchange preview starts and
  ends on whole lines.
- After, no change for the #1059 states: `result-dark-home-16.png` (Next buttons, composer, Projects),
  `failed-dark-home-20.png` (`Edit and send again`), `proposal-dark-home-20.png` (a one-line op is not bounded).
- Not tested here: macOS WKWebView; the full text stays reachable by scrolling each block, as the AI workspace is not
  implemented yet.
