# Tray answer-card qualification

The fixture contains ask with choices, ask without choices, doing and later on one project page. Non-ask rows deliberately retain a stored question to exercise A1.

From the repository root, with a native macOS or Windows GUI available:

```sh
capture=$(mktemp -d)
python3 docs/shots/evidence/1021/fixture.py "$capture"
TASKQ_INSTALL="$capture/install" cargo run --locked -- tray --snapshot "$capture/shots"
```

Inspect `popover-dark-project-16.png` and `popover-light-project-16.png` with Claude and retain the images and review here. Refresh the README capture manifest with `docs/shots/update.sh` after committing UI sources.

Attempt 6162067503: native capture could not connect to macOS GUI services in the worker sandbox; it produced no PNGs and was stopped. Chrome also exited before the browser test began. A3 and native interaction remain unverified; no Claude visual verdict exists. The fixture is preparation, not evidence of passing visual acceptance.

PM capture on macOS (native `tray --snapshot` from this branch, outside the worker sandbox): `popover-dark-project-16.png`, `popover-light-project-16.png`.

Claude visual review, both themes:
- A1 pass: no question card under #43 (doing) or #44 (later), although the fixture stores one on them.
- Ask with choices (#41): numbered buttons with the star, Other answer and Checked & context.
- Ask without choices (#42): reason field, Requeue (inactive while empty), Later and Close… are visible and readable in both themes.
- Nit, not blocking: each ask shows its title twice (list row, then card header).
