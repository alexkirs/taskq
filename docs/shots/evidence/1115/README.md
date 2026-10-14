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
- macOS WKWebView (`taskq tray --snapshot`, debug build, recaptured after the rebase onto the resizable window), `macos-long-proposal-dark-home-16.png` and
  `macos-long-proposal-light-home-20.png`: the op under Send is 4 whole lines, `Confirm: apply 1 change` and the
  Projects header show below it at 16 and 20 px, and the exchange preview ends on a whole line.
- The full text stays reachable by scrolling each block; the AI workspace that keeps it in full is deferred (#1115 Scope).
- After a reader scrolls either block, it realigns on whole lines once the scroll settles (150 ms).
  macOS WKWebView, `tray --snapshot --route scroll` (debug build, same fixture), `macos-{dark,light}-home-scroll-{start,middle,end}-{16,20}.png`:
  both blocks start and end on whole lines at every position; start shows the first lines (`You: Which tasks wait for
  me?`, `storefront: answer #41 PM: reason 1`), end the last (`attempt.`, `keeping the larger quantity`), and
  `Confirm: apply 1 change` and the Projects header show in each. Playwright Chromium
  (`PLAYWRIGHT_CHANNEL=chrome node tests/tray-home-proposal.browser.cjs`) stays a development instrument.
