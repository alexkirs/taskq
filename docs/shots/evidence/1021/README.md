# Tray answer-card qualification

The fixture contains ask with choices, ask without choices, doing and later on one project page. Non-ask rows deliberately retain a stored question to exercise A1.

From the repository root, with a native macOS or Windows GUI available:

```sh
capture=$(mktemp -d)
python3 docs/shots/evidence/1021/fixture.py "$capture"
TASKQ_INSTALL="$capture/install" cargo run --locked -- tray --snapshot "$capture/shots"
```

Inspect `popover-dark-project-16.png` and `popover-light-project-16.png` with Claude and retain the images and review here. Refresh the README capture manifest with `docs/shots/update.sh` after committing UI sources.

Attempt 6162067503: native capture could not connect to macOS GUI services in the worker sandbox; it produced no PNGs and was stopped. Chrome also exited before the browser test began. That attempt did not qualify A3. The later PM capture and Claude review below supply the visual evidence; live native board writes remain untested.

PM capture on macOS (native `tray --snapshot` from this branch, outside the worker sandbox; committed in `ec9c988d05cc9d4283ba7778514d0c81b3f99439`): `popover-dark-project-16.png`, `popover-light-project-16.png`.

Claude visual review, both themes:
- A1 pass: no question card under #43 (doing) or #44 (later), although the fixture stores one on them.
- Ask with choices (#41): numbered buttons with the star, Other answer and Checked & context.
- Ask without choices (#42): reason field, Requeue (inactive while empty), Later and Close… are visible and readable in both themes.
- Nit, not blocking: each ask shows its title twice (list row, then card header).

README shots were regenerated from the same UI source by [macOS capture run 38522341267](https://github.com/taskqorg/taskq/actions/runs/38522341267). The downloaded manifest and PNG hashes were verified before inclusion.
