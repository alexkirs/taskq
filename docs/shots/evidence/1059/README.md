# Home AI exchange qualification (#1059)

The fixture is `docs/shots/fixture.sh` plus a two-entry AI exchange on storefront: a clarify, then the newest request in
one of four states. `result` and `failed` carry a 40-line answer.

From the repository root, with a native macOS or Windows GUI available:

```sh
capture=$(mktemp -d)
docs/shots/fixture.sh "$capture/fx" && node docs/shots/evidence/1059/fixture.cjs "$capture/fx" result
TASKQ_INSTALL="$capture/fx/install" cargo run --locked -- tray --snapshot "$capture/shots"
```

On Windows pass `fixture.cjs` a native path (`cygpath -w`), so the binary can read the projects file.

Native capture: Windows 11, WebView2, `tray --snapshot` of this branch's release build. Files are named
`<state>-<theme>-home-<px>.png`.

Claude visual review:
- A1 pass: `result-dark-home-16.png`, `result-dark-home-20.png`, `result-light-home-20.png` show Attention, the bounded
  AI preview opened at the newest entry, the Next buttons, the composer, Send and the Projects group in the first screen.
- A2 pass: `clarify-dark-home-16.png` shows Answer and Reply; `proposal-dark-home-20.png` shows the op and
  `Confirm: apply 1 change`; `failed-dark-home-20.png` shows the reason and `Edit and send again`. Each is above Projects.
- The Windows window cap here is smaller than the Mac's 1818 px cap, so the Mac shows more of Projects.
- Not tested: macOS native capture (CI `shots` refreshes the README images on macOS).
- Seen, not changed: the 320 px Windows popover has a horizontal scrollbar on every Home, with or without AI.
