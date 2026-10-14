# Icon temporal shift and rebin (U-A6, #1077)

Frames of the Summary icon (dark theme) across one 2 h bin boundary `b` of the 24 h period, drawn by `src/icon.rs` at native size: `22pt@2x` (44 px, macOS reference) and `16px@1x` (Windows 100% baseline). Names give the frame time relative to `b`.

- Fixture and assertions: `tray::tests::history_grid_moves_one_bin_at_a_boundary`; regenerate with `TASKQ_FRAMES=<dir> cargo test history_grid`.
- `-3600s` and `-1s` are byte-identical, as are `+31s` and `+3600s`: refreshes inside a bin move nothing. At `+0s` the 12-bin grid steps one bin left: the 11 retained bins keep their counts, the new partial bin starts empty and fills in place (`+31s`).
- Rebin: 44 px shows 6 bars (pairs of bins), 16 px 3 bars (groups of 4), all four series summed; the fixture's oldest bin is empty, so every total is equal before and after the boundary.
- Rebin groups are counted from the oldest source bin, so a one-bin step regroups the summed bars at the boundary; they change only then.
- Not covered: installed menu-bar and notification-area captures, light theme, 20/24/32 px and native visual approval of the look.
