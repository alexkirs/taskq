# Windows service and tray qualification (#1049)

Native qualification on host `win` on 2026-10-14 (UTC), collected for
[#1049](https://github.com/taskqorg/taskq/issues/1049) under
[implementation plan](design/implementation-plan.md) G8. Form:
[UI acceptance report](design/ui-acceptance-report.md). The owner chose an
isolated update test: a temporary install and a scratch project running as host
`winq`, with the real host untouched. Every row below comes from one frozen
build, the candidate, except the notification rows, which come from the
notification candidate after fixes F9 and F10. The real-host update of the published release is
[#1053](https://github.com/taskqorg/taskq/issues/1053); it is still open, so
this report does not prove it.

- Candidate commit: `9ab83b63052a099c826a26cbeca8fe22e99c920d` (this branch on
  main `3057cea`). Release build `4.2.0-dev.1792018150`, `taskq.exe` SHA256
  `3744dfaa93bb31cdc4047e1f8723fec794e72aef6a1ebf1b27bc89efb6497e90`; the
  binary carries the full commit id. Equivalence: `git diff 9ab83b6 HEAD --
  src tests .github Cargo.toml Cargo.lock` touches only `src/notify_windows.rs`,
  `Cargo.toml` and `Cargo.lock` (the Windows notification adapter, F9 and F10)
  and the specq pin of the worker brief in `src/runtime.rs` (main #1175); the
  rows other than notifications exercise neither.
- Notification candidate: commit `c46e83ef1ae4fd784f98cfdcf05607c9f97b3cc9`
  (`de75826` after the rebase onto main `c24dff9`). Release build
  `4.2.0-dev.1792021136`, `taskq.exe` SHA256
  `d8613d0ab4ec18c0b36a00cfa27bffaac6cacf6b244bd0b3c6eec0bb51c97fd3`; the
  binary carries the full commit id. `git diff c46e83e HEAD -- src tests
  .github Cargo.toml Cargo.lock` is only the specq pin above. F9 alone was
  rechecked on `4b08bfe` (build `4.2.0-dev.1792020405`, SHA256
  `0f9c9249ae7347b355251cf4471522ceb81ab982bf7aeb77f49975e3da26b5ef`), which
  also reproduced F10.
- Update source: published `taskq-4.1.11-windows-x86_64.exe`, SHA256
  `ea1e24ddb3c2727f61a9565fa47ebfb71efd49085eb7414c337400f65777f39f` (matches
  the release `SHA256SUMS`).
- Spec revision / UI blobs at the candidate: `taskq.md` `642eb2dd`,
  `src/tray.html` `ad346969`, `src/tray.rs` `b2649591`. The Windows toast
  clause now lives in [menu specification](design/menu.md) M14.
- Fixture: the scratch project is a clone of this repository on the taskq
  board, host `winq`, `limits` claude 1, `capacity_hosts.winq.admission` Auto
  with max 1, focus `host-winq`, no operator grants. Probe task
  [#1155](https://github.com/taskqorg/taskq/issues/1155), held for `winq`.
- Platform / renderer / font / theme / scale: Windows 11 Pro 10.0.26100,
  WebView2 155.0.0.0, toolchain `stable-x86_64-pc-windows-gnu`; Segoe UI;
  light and dark; display scale 100 % (96 dpi), Windows Text size 140 %, so the
  page pixel ratio is 1.4; tray text size 16 and 20.
- Monitor usable rectangle / anchor / native chrome: one display, 2560×1440,
  work area 2560×1392, taskbar at the bottom. The isolated icon sits in the
  hidden-icons flyout. Pinned window (1816,170)–(2280,1351), client 448×1172
  physical px, CSS viewport 320×838 on Home.
- Builder / reviewer: #1049 worker (Claude) / owner.

The GNU build needs `WebView2Loader.dll` beside the executable; the release
directory carries it. Native input came from Win32 mouse and key events into
the isolated tray window only; a guard refused input whenever that window was
not in the foreground or the point fell outside its client area. The page state
was read through the WebView2 DevTools port of the isolated tray. The real tray
and the real services got no input from this run. Logs, UI Automation dumps and
shots: [evidence/1049](shots/evidence/1049/README.md).

For the notification rows, native input also went to two OS surfaces: toasts
in Notification Center (opened with Win+N), and the TaskQ switch in the real
Settings app (System > Notifications), which was set back to On afterwards.

## Notifications on Windows

The isolated tray ran with `TASKQ_INSTALL` set to the temporary install and the
scratch project registered; Questions were set to `Banner with sound`. Each
event was an `ask` line for #1155 appended to the scratch project's
`.taskq/events`, which is all the tray reads. On this host an automatic Do not
disturb rule was active (the bell showed it while the manual switch read Off),
so no toast showed as a banner: each went straight to Notification Center.
Steps, page state and notifier reads are in
[toast.log](shots/evidence/1049/toast.log):

1. 23:22:45, old candidate `9ab83b6`, a host that had never shown a TaskQ
   toast. Notifications read `OS permission unknown`; the event at 23:22 moved
   the cursor and posted nothing (F9).
2. 23:28:52, `4b08bfe`. `Allowed by the OS`; events at 23:29:19 and 23:29:59
   posted toasts.
3. 23:34:46, `4b08bfe`. A native click on the `g8q · Question` toast in
   Notification Center removed it, but the tray stayed on Home with no card
   (F10).
4. 23:42:14, notification candidate. The shortcut reads back toast activator
   CLSID `{AF9B407E-49C7-4B89-9B0E-55EA50346E66}`. An event posted a toast; the
   native click at 23:42:49 turned the tray from Home to project `g8q` with
   the #1155 card focused. A second event and click (23:46:21, the tray on the
   Notifications page) did the same.
5. 23:43:42. Writing `Enabled=0` in the TaskQ notification registry key left
   the notifier on `Enabled`; the write was undone at 23:44:04.
6. 23:44:33. A native click on the TaskQ switch in Settings turned it Off; the
   notifier read `DisabledForApplication` and the tray showed `Blocked by the
   OS: nothing is posted` with `Open OS setting ↗`. The event at 23:44:58 moved
   the cursor and posted nothing (the last toast time stayed 23:42:14).
7. 23:45:37. The switch went back On; the notifier read `Enabled`, the tray
   `Allowed by the OS`, and the event at 23:45:51 posted a toast at 23:45:53.

## Update from 4.1.11

The temporary install held `releases/4.1.11` and the 4.1.11 launcher, with
`TASKQ_INSTALL` pointing at it. No logon unit was registered: the unit's
environment lacks `TASKQ_INSTALL` and would start the real install. The 4.1.11
launcher ran `service run` in the foreground instead
([update.log](shots/evidence/1049/update.log)):

1. 22:53:05. 4.1.11 service on `winq`, pid 5032.
2. 22:53:19. 4.1.11 spawned #1155 attempt `6180277215` (Claude, pid 27580).
3. 22:53:31. The candidate was copied into `releases/`, `previous` (4.1.11) and
   `current` (candidate) were written as an update writes them, and an update
   check was requested.
4. 22:53:33. The status still showed 4.1.11 with the live child: Windows
   `reexec` waits while a child lives (§ 8 step 3).
5. The worker turn ended by itself after 17 s (exit 0; the shell was denied
   because the install has no grants). 4.1.11 logged `gone attempt 6180277215`,
   then `current is 4.2.0-dev.1792018150; restarting`.
6. 22:53:36. `taskq 4.2.0-dev.1792018150 service on winq`; by 22:53:59 its
   status was `complete` with no error and no kids, and #1155 was back in its
   queue state on the board.

A run on the previous frozen build (pre-rebase, same 4.1.11 side) kept its
child for 70 s before the switch, with the same order of log lines. The update
never stopped a live child: 4.1.11 defers the re-exec until the attempt ends.
Windows cannot hand a child to the new image, so on this path a child never
runs under the candidate; that is the specified behavior.

## Results

| Requirement / route | Fixture + actions | Expected | Observed | Result | Evidence |
|---|---|---|---|---|---|
| Update keeps live kids | 4.1.11 with live #1155 attempt; `current` switched to the candidate | Re-exec waits for the live child; no kill | Re-exec deferred until the child ended by itself; candidate up 3 s later | Pass | update steps 1–6 |
| Update keeps board state | Same | Candidate reads the same board; no stray ops | Only spawn and `gone` on #1155; first status complete | Pass | `update.log` |
| Service restart | `service restart` on the candidate at 22:54:26, and at 23:05:29 after the `healthy` mark | Re-exec within a pass, same version | Up again at 22:54:47 (pid 2636 → 10468) and at 23:05:32 (→ 5004); `current` stays the candidate | Pass | `update.log` |
| Auto admission | Candidate on `winq`, Auto max 1, after a restart | Gate from live load; admit only with an operator grant (R9) | Gate `unknown` (no CPU reading yet), then held for cooldown 1 and 2 of 3 calm samples, then open at cpu 6.7 %, memory 44.1 %; mode auto, max 1, `min_service_version` 4.2.0-dev, host_used 0; refusals name only the missing grant per runtime | Pass | `admission.log` |
| Restart with a live kid on the candidate | — | — | No grant in the isolated install, so the candidate admits nothing | Not tested | |
| U-A1 routes, snapshot | `tray --snapshot` of the candidate | Every route renders in both themes and sizes; the overflow guard passes | 52 images (8 icons; Home, actions, project, projects, Attention, Notifications, Settings, Appearance, Cadence, Agents, Services, About); exit 0 with the new label check | Pass | `popover-*.png`, `icon-summary-run-light-22pt@2x.png` |
| U-A2 width (#1073) | Same, Text size 140 %; live window | No horizontal scroll; one vertical scrollbar on long content | Live viewport 320, client 320, scroll width 320 on every page visited; the AI exchange scrolls in its own bounded block | Pass | `native.log`, `popover-light-home-20.png` |
| U-A2 labels (F8) | Notifications at 16 and 20 | Each label inside its button | Labels fit; `Banner with sound` wraps inside its button | Pass (after F8) | `popover-dark-notifications-20.png`, `popover-light-notifications-16.png` |
| U-A1 pinned native navigation | Native clicks: Settings, ‹ Back, Home in the footer | Route changes; focus on the page heading | Settings → Work mode (Back) → Home, focus `H1` each time | Pass | `native.log` 23:00:48–23:00:59 |
| U-A8 keyboard | Tab, Tab, Shift+Tab on Home | Focus moves in order | Work mode… → Pause all… → Work mode… | Pass | `native.log` 23:01:00–23:01:05 |
| U-A4, U-A8 confirm focus (F5) | Enter on Pause all…, then Enter on Cancel | Focus on Cancel, then back on Pause all… | Cancel focused; after Cancel, Pause all… focused and kept over 2 refreshes; nothing paused | Pass (after F5) | `native.log` 23:01:10–23:01:25 |
| U-A4 focus across refresh (F3) | Focus the heading (Settings) and a link (`Comment on #1058`), wait 3 s twice | Focus stays | Heading and link keep focus over 2+ refreshes (8 DOM refreshes in 10 s) | Pass (after F3) | `native.log` 23:01:33–23:02:09 |
| U-A1 press across refresh (F6) | Native press held 2100 ms ×3, short press ×6, Settings ↔ Home | Every press navigates | 9 of 9 navigated | Pass (after F6) | `native.log` 23:02:16–23:02:41 |
| U-A3 resize | Focus the resize grip, arrow keys | Window follows; content still fits | Right ×2 and Down ×2: client 448×543 → 476×571, viewport 340, no horizontal scroll; Left ×2 and Up ×2 restore it | Pass | `native-04-grip-resized.png`, `native.log` 23:08:51–23:09:11 |
| U-A1 pin | Native click on Pinned, later on Pin | Unpins, then pins; focus on the button | `Pin` then `Pinned`, `aria-pressed` follows; window stays | Pass | `native.log` 23:02:57, 23:04:26 |
| U-A1, U-A3 unpinned, from the flyout (F4) | Native click on the isolated icon in the hidden-icons flyout | Popover opens and stays | Visible after 4 s and after 2 s; 4 of 4 opens | Pass (after F4) | `native.log` 23:03:06–23:03:52 |
| U-A1 unpinned close | Escape; focus on another window | Hides | Escape hides 2 of 2; another window's focus hides 2 of 2 | Pass | `native.log` |
| U-A1 icon click while open | — | Hides | Opening the flyout chevron blurs and hides the popover first, then the icon reopens it; not separable with the icon in the flyout | Not tested | `native.log` 23:03:56 |
| U-A8 accessible names | UI Automation tree of the live window, Settings and Home | Every control named; focusable controls reachable | All buttons, links and the request field named; the grip is `Resize window with arrow keys`; menu container and document have no name | Pass with note | `uia-settings.txt`, `uia-home.txt` |
| U-A4 overlap | Shots of every route; live Home, Settings, Work mode | Header, footer and composer clear of content | No overlap in any shot; the footer sits below the last control | Pass | `popover-*.png`, `native-*.png` |
| U-A5 histogram | — | — | Activity history not in the snapshot set; totals not compared | Not tested | |
| U-A6 bars | — | — | Not measured | Not tested | |
| U-A7 icons | `tray --snapshot` icons at 22 pt @2x | Match I1–I5 | Rendered; not compared with the references pixel by pixel | Not tested | `icon-summary-run-light-22pt@2x.png` |
| U-A9 4.1.9 additions | — | — | Not reviewed here | Not tested | |
| U-A10 frozen evidence | One frozen build for every row; hashes above | Complete evidence for the rows claimed | Candidate commit, artifact hash, spec blobs, logs and shots attached | Pass for the rows above | this report |
| U-A8 screen reader audio, 14/18 px | — | — | Not run | Not tested | |
| U-A3 other monitors and scales | One display, 100 % scale | — | — | Not tested | |
| M14 OS setting on a fresh host (F9) | Host that never showed a TaskQ toast; Settings › Notifications; an `ask` event | Allowed by the OS (the OS default); toast posted | `Allowed by the OS`; 2 of 2 events posted | Pass (after F9) | `toast-05-notifications-allowed.png`, `toast.log` 23:28:52–23:29:59 |
| M14 click in Notification Center (F10) | Win+N, native click inside the `g8q · Question` toast | The tray opens that card | Project `g8q`, #1155 card focused, 2 of 2 (from Home and from Notifications) | Pass (after F10) | `toast-03-center-before-click.png`, `toast-04-card-after-click.png`, `toast.log` 23:42:14–23:46:23 |
| M14 OS denial | Native click on the TaskQ switch in Settings (On → Off), then an `ask` event | Blocked shown; nothing posted; the event is not posted later | Notifier `DisabledForApplication`; `Blocked by the OS: nothing is posted` and `Open OS setting ↗`; no toast; cursor at the event | Pass | `toast-06-os-toggle-off.png`, `toast-07-notifications-blocked.png`, `toast.log` 23:44:33–23:45:23 |
| M14 allowed again | Switch back On, then an `ask` event | Allowed; the next event posts | Notifier `Enabled`; `Allowed by the OS`; toast at 23:45:53, and its click opened #1155 | Pass | `toast-08-allowed-again.png`, `toast.log` 23:45:36–23:46:23 |
| M14 banner click | — | — | No banner showed: an automatic Do not disturb rule sent every toast to Notification Center. Windows activates the same toast activator for a banner click | Not tested | |
| M14 click while no tray runs | — | — | The running tray registers the activator and the shortcut names no server to start, so such a click opens nothing; the service starts a new tray within about 30 s | Not tested | |
| M14 all notifications off in Windows | — | — | Not run | Not tested | |
| macOS | — | — | Covered by #1060 | Not tested here | |

The 22:58 clicks on the footer that did nothing were a harness effect: the
tooltip of the flyout icon, left by the harness hovering it, lay over the
footer. Once the tooltip was gone the same clicks worked (23:00:48 on).
UI Automation text is written by PowerShell in the console code page, so
symbols such as `◎` show as `?` in the dumps.

## Findings

| ID / severity | Requirement | Reproduction | Expected → actual | Root cause | Fix | Recheck |
|---|---|---|---|---|---|---|
| [#1061](https://github.com/taskqorg/taskq/issues/1061) high | R9 grants | 4.1.11 `update()` installs a newer published release, then runs its sweep | Grants kept → `worker-policy.json` and other new files deleted | The 4.1.11 sweep keeps only a fixed list of names | Resolved on main by #1082 (docs): copy `worker-policy.json` outside the install before the update and restore it after | Not rechecked here: the isolated install had no grants; the published path is #1053 |
| [#1073](https://github.com/taskqorg/taskq/issues/1073) medium | U-A2 | Home or Services at Windows Text size 140 % | Full width → right edge clipped, horizontal scrollbar | Popover sized by the window scale factor (1.0) while the page ran at pixel ratio 1.4; the scrollbar took width from the body | Fixed on main by #1091 | U-A2 width row above |
| F3 medium | U-A4 | Pinned window, focus the heading or a link, wait for a refresh | Focus kept → focus dropped to `body` | A refresh replaced the focused element | `fix(tray): a refresh keeps focus on the heading or a link` | Focus across refresh row |
| F4 high | U-A1 | Isolated icon in the hidden-icons flyout (the Windows 11 default); click it while unpinned | Popover opens → closes within about 15 ms, 3 of 3 | The flyout takes the foreground back right after the click; the popover read that blur as a close | `fix(tray): popover opened from the hidden-icons flyout stays open` | Unpinned rows above |
| F5 medium | U-A4 | Enter on Pause all…, then Cancel | Focus on Cancel, then back on Pause all… → focus on `body` | The confirm row replaced the button without moving focus | `fix(tray): a click held across a refresh is not lost; Pause all keeps focus` | Confirm focus row |
| F6 high | U-A1 | Press a footer button and hold it across a refresh (about every 2 s) | Navigates → the click was lost 3 of 3 for a 2100 ms press; short clicks lost now and then | A refresh replaced the pressed element between press and release | Same commit as F5; `tests/tray-press.test.cjs` | Press across refresh row |
| F7 observation | coreq launcher | Three `service run` starts of one build within 600 s (test restarts) | Restarts → the 4.1.11 launcher rolled back to 4.1.11 at the third start | Specified launcher behavior: a start counts until the `healthy` mark, and an operator restart is a start | None; the test reset `current` and the start count | The candidate got its `healthy` mark at 23:05:19; a restart after it kept the candidate |
| F8 medium | U-A2 | Notifications at 16 and 20 px | Labels inside buttons → `Banner` crossed its button's border | The buttons shared width without a minimum basis | `fix(tray): notification mode labels fit their buttons`; `tray --snapshot` now fails on such a label | U-A2 labels row |
| [#1072](https://github.com/taskqorg/taskq/issues/1072) medium | Worker CLI context | A shell for this task inherited another project's `TASKQ_ROOT`, `TASKQ_TASK` and `TASKQ_ATTEMPT` | Clean environment → stale ids | Stale environment in a reused session | Closed | Every command here set its own environment |
| F9 high | M14 | Host that never showed a TaskQ toast; Questions on; an `ask` event | Allowed and posted → `OS permission unknown`; the event moved the cursor and posted nothing | Until the first toast records a setting, the notifier's `Setting` fails with `0x80070490` (not found); the adapter read every failure as Unknown and posts only when allowed | `fix(tray): Windows toasts post on a host that never showed one`; menu M14 names the OS default | M14 OS setting row |
| F10 high | M14 | A TaskQ toast in Notification Center; click it | The card opens → the toast goes away and the tray stays on Home | The in-process `Activated` handler is not called for a click in Notification Center; Windows starts the COM activator named by the shortcut's `ToastActivatorCLSID`, and the shortcut named none | `fix(tray): Windows toast clicks reach the tray from Notification Center` | M14 click row |
| F11 low | Isolation | Start a tray with `TASKQ_INSTALL` set to a temporary install | The real shortcut stays → the per-user Start Menu `TaskQ.lnk` now starts the temporary install | The shortcut path is per user while its target follows `TASKQ_INSTALL` | Open, not blocking (test setups only); the run removed the shortcut, and the real tray wrote its own again at its next start | — |
| Toast harness observation | M14 | Write `Enabled=0` in the TaskQ notification registry key | Notifier reads denied → still `Enabled` | A registry write does not reach the running notification platform | None; the denial used the Settings switch | M14 OS denial row |
| UIA minor | U-A8 | UI Automation tree | Named containers → the menu container and the document have no name | Not investigated | Open, not blocking | `uia-home.txt` |

During an earlier build, one harness click landed on the taskbar and likely
opened the real tray's popover once; no other input reached the real tray.

## Useful deviations

None.

## Verdict

- Design accepted: not changed by this qualification.
- Runtime/platform verified: on the candidate, the update from 4.1.11 defers
  the re-exec for a live child and keeps board state; service restart, Auto
  admission, native tray navigation, keyboard, resize and pin routes pass on
  Windows after fixes F3–F6 and F8. On the notification candidate, toasts post
  on a fresh host, a click in Notification Center opens its card, and an OS
  denial shows Blocked and posts nothing, after fixes F9 and F10.
- Not tested / blocked: the published v4.2.0 update with grants (#1053, open);
  restart with a live child on the candidate; icon click while open with the
  icon in the flyout; other scales and monitors; text sizes 14 and 18; screen
  reader audio; U-A5–U-A7 and U-A9, which this Windows run did not measure;
  a toast banner click, a click while no tray runs, and the Windows-wide
  notification switch.
- Deployed: nothing. The temporary install and host `winq` are stopped, the
  probe is closed and the install token is deleted. The isolated tray is
  stopped; the Start Menu shortcut and the notification settings key it
  created were removed at 23:46:40. The scratch `events` file keeps the six
  appended probe lines. The real tray restarted at 23:47:54 for its own update,
  not from this run, and wrote its own shortcut.
- Next exact fix or decision: #1053 proves the real-host update.
