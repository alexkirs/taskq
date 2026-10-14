# UI acceptance report: 4.2.0 candidate on macOS (G8, #1051)

Filled from the [report template](../../ui-acceptance-report.md) against the [hard gates](../../ui-acceptance.md#hard-gates). Windows: #1049. Linux: #1050. Real-host update and release: #1053.

- Candidate commit / artifact SHA256: `cf16cf5` built as `4.2.0-dev.1791993764`, `taskq` macos-aarch64 `29bc6299b1724ba1378142d30d1421f1719c68479e7f0e9dabf6678bbe2735af`; `selftest` PASS (69 golden traces, formats).
- Baseline: published `4.1.11` macos-aarch64 `6d93cb91096a3c73a614138373a9f7e36677454d8fc477007ac605e92e539e95` (matches the release `SHA256SUMS`).
- Spec revision: taskq.md and docs/design at `cf16cf5`; shot hashes in [SHA256SUMS](SHA256SUMS).
- Platform / renderer / theme / scale: macOS 27.0 on Apple M2, WKWebView via `taskq tray --snapshot`, dark and light, font 16 and 20 px, backing scale 2×, menu-bar icon 34 pt@2×.
- Monitor: one built-in notched Liquid Retina display 2560×1664; no external or 1× display.
- Live data: the six registered projects on this Mac; counts changed between steps (for example Waiting 1 in dark, 2 in light).
- Builder: Claude worker for #1051. Reviewer: pending.

## Results

| Requirement | Fixture + actions | Expected | Observed | Result | Evidence |
|---|---|---|---|---|---|
| Update 4.1.11 → candidate (§ 8 step 3) | Disposable `TASKQ_INSTALL` with 4.1.11 and the candidate, scratch project as host `macq` on this board, limits 0, fixture task #1055 created by 4.1.11 as `later`; `current` set to the candidate and `.taskq/update` touched | Service re-execs into the candidate, keeps pid, state and tasks | `current is 4.2.0-dev.1791993764; restarting` 57 s later; same pid 14635; `status.json` version moved, `kids` [], `complete` true; #1055 fold identical (`later`, no attempt) before and after | Pass (state) | this section; #1055, #1056 closed after the probe |
| Update keeps live kids | Same probe | CLI children pass through `.taskq/kids` | Scratch host had no children (limits 0, owner option 3 on #1049); the real host has only Claude kids, which defer re-exec | Not tested → #1053 | — |
| Rollback to 4.1.11 keeps grants | — | Grants store survives N-1 | 4.1.11 sweep deletes `worker-policy.json` (src/policy.rs:11), so the real host was not rolled back | Finding #1061 | — |
| U-A1 routes reachable | Snapshot steps: home, home actions, attention, projects, project, cadence, settings, appearance, agents, services, about | Every ledger route reachable | 11 routes rendered with live data; Work mode editor, AI workspace, setup, plans, emergency and lifecycle confirmations are not in the snapshot steps | Not tested → #1060 | `popover-*` |
| U-A2 natural fit | Home, Projects, Project, Settings, About at 16/20 px | No primary scrollbar when content fits | Projects, Project, Settings, About fit; Home is cut at the 1818 px cap because the full last AI exchange sits between Attention and Projects | Fail → #1059 | `popover-dark-home-16.png`, `popover-light-home-16.png` |
| U-A3 clamp / anchor / DPI | — | Popover anchored under the menu-bar icon, clamped to the work area | Worker cannot capture the screen; one display | Not tested → #1060 | — |
| U-A4 header/footer overlap, focus/Back | Back and footer on every sub-route | No overlap; Back restores route | No overlap of header, Back or footer at 16/20 px; focus, draft and Back scroll not exercised | Pass (overlap); focus → #1060 | `popover-*-16.png` |
| U-A5 histogram totals | Cadence and Projects aggregate | Same totals across adapter, tray, detail | Totals render; live counts moved during capture, so no exact cross-check | Not tested → #1060 | `popover-*-cadence-16.png` |
| U-A6 bars | Cadence Hour, custom range refused | Equal widths, shared baseline | Equal bars on one baseline; custom start after end refused with reason; chain test `tray::tests::history_bins_are_equal_oldest_left_on_one_scale` | Pass | `popover-dark-cadence-16.png` |
| U-A7 icon at native size | Summary and compact, run/pause, both themes, 34 pt@2× | q, digits, grid; gray pause overlay | Digits and stacked series legible at 68 px; pause shows gray bars under the q; live menu-bar capture missing | Pass (render); live → #1060 | `icon-*` |
| U-A8 keyboard, names, contrast, large text | 20 px both themes | Readable, named controls | 20 px wraps without clipping; keyboard and VoiceOver not exercised | Not tested → #1060 | `popover-dark-home-20.png`, `popover-dark-projects-20.png` |
| U-A9 4.1.9 additions retained | Services, About | Log, folder, restart, stop, uninstall confirm, Resources, incidents, versions | All present per project; uninstall needs a nested confirm; About lists tray and service versions; Update shows only for a newer version | Pass | `popover-*-services-16.png`, `popover-*-about-16.png` |
| U-A10 frozen evidence | This folder | Candidate SHA, hashes, shots | Recorded above and in `SHA256SUMS` | Pass | [SHA256SUMS](SHA256SUMS) |

## Findings

| ID / severity | Requirement | Reproduction | Expected → actual | Fix |
|---|---|---|---|---|
| #1059 major | U-A2, menu.md Home | Finish a long AI request; open Home | Short composer, Projects visible → full exchange pushes Projects below the fold | Bound the Home preview; full text in the AI workspace |
| #1060 evidence gap | U-A1/3/4/5/7/8 | `screencapture -R 0,0,1470,37` in a worker: `could not create image from rect` | Live menu-bar and keyboard evidence → none | Owner-run capture or a Screen Recording grant |
| #1061 major | § 8 N/N-1 | Roll a host with grants back to 4.1.11 | Grants kept → store deleted | Keep or restore the store across rollback |

## Verdict

- Design accepted: no; owner review pending.
- Runtime/platform verified: update 4.1.11 → candidate keeps state on a disposable install; WKWebView snapshots for 11 routes at 16/20 px.
- Not tested / blocked: live kids handover (#1053), live menu-bar and popover, keyboard, VoiceOver, 14/18 px, external and 1× displays (#1060).
- Deployed: no; the real host stays on `4.2.0-dev.1791987853`.
- Next exact fix or decision: #1059, then #1060 capture on the next candidate.
