# WS11 first slice: F5 acceptance package

Scope: native Home / Projects / Attention / Settings and its five categories ([menu M8](../../menu.md#m8-implementation-boundary), #976 accepted split). AI workspace (C-A1–C-A10) and selective recovery (E-A1–E-A9) are deferred and stay Not tested here.

## Artifact

- Source: pre-rebase commit `7883b3f` (same as the README manifest); UI blobs `src/tray.html` `3ad69bf4` and `src/tray.rs` `bdd50424`, unchanged by the rebase. Images come from `cargo build --release --locked` of that commit, binary sha256 `f4685b74ebee7439c9b4e9fdfd8dd85f69aed4dd11c673532b46c2496c75eca8`.
- Environment: macOS 27.0.1 arm64, Retina 2x, WKWebView `taskq tray --snapshot`; usable height 909 pt (0.95 of the screen), width 320 pt at 14/16 px text and 360 pt at 18/20 px.
- Fixtures: [docs/shots/fixture.sh](../../../shots/fixture.sh) (normal: two projects, ask/doing/review/ready, one day of history) for the images here; [docs/design/tray-fixture.sh](../../tray-fixture.sh) (long titles, failed, paused, service error, stale status, duplicate issue number, update available) captured in the same run and inspected, not stored.
- Images: `popover-<theme>-<route>-16.png`, dark and light, 16 px. 20 px variants were captured and inspected for every route.

## Navigation ledger

| Route | Reached from | Content | Requirements | Light | Dark |
|---|---|---|---|---|---|
| Home | launch, footer Home, Back | host, Icon/Summary, Work mode, Pause all, Scope, Attention, inline Ask AI, Projects with counters, Add project, Activity history, footer | M1/M5, M-A4/16, K-A1–K-A7 | Pass | Pass |
| Projects | Home Projects header | aggregate and per-project counters, Board links, admission line, partial/unknown as `?` | K-A1–K-A7, D-A1/2 | Pass | Pass |
| Attention | Home Attention | service errors, stale status, Waiting for answer, Failed, distinct groups | M-A7/10/21 | Pass | Pass |
| Settings | footer Settings | Appearance, Activity history, Agents, Services, About, Work mode | M8 | Pass | Pass |
| Appearance | Settings | system theme note, Icon/Summary, text size 14/16/18/20 | D-A3–D-A8 | Pass | Pass |
| Activity history | Settings, Home | cadence detail | M-A5/6 | Pass | Pass |
| Agents | Settings | host, probe age, runtime/version/account, not logged in, not installed, unknown without status.json | M9, P-A1 | Pass | Pass |
| Services | Settings | Resources, per-project freshness, incidents, log/folder, restart, stop, uninstall with confirmation | M-A10–M-A12 | Pass | Pass |
| About | Settings | tray version, per-project version and age, update offer | M9 | Pass | Pass |
| Back | every non-Home route | restores caller route, task filter, disclosures and scroll | M8 | Pass (model) | Pass (model) |
| AI workspace, AI windows | — | deferred | C-A1–C-A10 | Not tested | Not tested |
| Emergency / recovery | — | deferred | E-A1–E-A9 | Not tested | Not tested |

Windows: Not tested (no Windows run for this slice).

## Automated results

- Data/model: `node tests/tray-navigation.test.cjs` — Pass (nested Back, filter/scroll/disclosure restore, five categories, unknown probes and versions).
- Adapter: `cargo test --locked` — Pass; `cargo run -- selftest` — Pass.
- Geometry: every route in the normal fixture fits without a primary scrollbar; only the Project page reaches the 909 pt clamp, and scrolling to its footer was not driven natively (U-A2 Not tested for overflow). Light and dark heights match per route.
- Runtime: native snapshot run completed for 40 popover images per fixture; native click-through of Back and footer was not driven (Not tested natively; covered by the model check).

## Findings

| ID | Requirement | Expected | Observed | Reproduction | Severity | Disposition |
|---|---|---|---|---|---|---|
| F1 | U-A2, U-A4 | footer visible after a text-size change | 20 px to 16 px clipped the footer (Attention 458 px, Home 1590 px, while the content needed 502 and 1710); 16 to 20 over-sized the window | snapshot run without `fit()`: `popover-light-home-16` follows `popover-dark-about-20` | High | Fixed by the `fit()` height measure: the page measures at the target width |
| F2 | W1–W8 | first viewport carries questions, warnings and current values | Home shows Waiting for answer and counters above the fold; Attention groups errors, stale status, questions and failures apart (W8); unknown counts read `?` with a Partial line, never zero | tray-fixture.sh run | — | Pass |
| F3 | M-A3/17 | long titles flow in full | long title wraps to three lines at 320 pt | tray-fixture.sh Attention | — | Pass |
