# Changelog

Per release: what changed on the board and what each open issue needs. A cutover agent reads this (taskq.md § 9);
there is no migration code. Releases without a **Board** section need nothing.

## Unreleased

Worker and reviewer briefs name a contract that exists (the release's `taskq.md` URL when the project has none) and forbid searching `/` or `~`: workers no longer raise macOS Desktop, Documents, Downloads, network volume or Photos prompts for the service. No board change.

## v4.1.8

Unused positional arguments now fail with their names before any action; use repeated `--acceptance` flags so every item reaches the task. No board format change.

Services sharing an install restart into a release already made current by another service, without another download or selftest. No board change.

## v4.1.7

GitLab OAuth logins (`glab` with OAuth) no longer get 401: the token goes as `Authorization: Bearer`, not
`PRIVATE-TOKEN`. Each board is its own module (`src/board/github.rs`, `gitlab.rs`) behind the `Board` trait; core,
service and operator no longer branch on the board kind. GitLab without `host` defaults to `gitlab.com`; setup writes
`host` for every board. No board format change.

## v4.1.6

TaskQ faults now produce sanitized, deduplicated public incident reports in `taskqorg/taskq`, through the existing
github.com account; unsupported auth, offline or uncertain delivery keeps a local JSON report. Setup and update
disclose the destination, account, payload and fallback. New setups enable this after owner confirmation; existing
projects without `incident_reports` stay local and show a notice. Set it to `false`, or `TASKQ_INCIDENT_REPORTS=off`,
to write nothing. The service status and tray show the newest outcome. Coreq is pinned to tag v0.4.0, including
launcher fault hooks and replacement on update (effective at the next launcher start). No board format change.

## v4.1.5

Same-host race: a worker that wrote `ask`, `result` or `requeue` and exits after the owner's answer arrived is a clean
exit, so its own service resumes the attempt with the answer instead of posting `gone`. No board change.
Verified live with mac and Windows on 4.1.5 (#844): attempt 6145455653 asked on mac, the owner answered, and the same
attempt resumed on mac with no `gone` entry.

## v4.1.4

Lease grace: an attempt younger than `ttl` by its `spawn` or its newest accepted `answer` or `steer` is judged only by
its host's heartbeat age, so the other host no longer posts `gone` for an answered attempt before its own host resumes
it. Both hosts need 4.1.4 for the fix to hold. An owner free-text comment on a task in `ask` becomes an AI request
whose proposal the owner confirms in the tray. No board change.

## v4.1.3

Update, launcher, service units, `status.json` common keys and the board token come from the shared crate
[coreq](https://github.com/taskqorg/coreq) v0.1.0; paths, units and messages are unchanged, except launcher errors
now read `taskq: <message>`. No board change.

## v4.1.2

Repository moved to `taskqorg/taskq`: releases, `latest.json` and `taskq update` come from there (the old URL redirects
for downloads; the board API does not, so `repo` in `taskq.json` must read `taskqorg/taskq`). R7 points at
`taskqorg/specq`. No board change.

## v4.1.1

In `ask`, a comment whose whole body is an option number or option text counts as `answer K`; 4.1.0 ignores it, so
such answers fold only on hosts running 4.1.1. Windows re-execs after an update once its live children end. Priority
`p0` passes any focus. The menu gains AI requests, Add project, an agents view and capacity: `taskq service capacity`
and `taskq agents`; `taskq.json` may hold `capacity_hosts` and a `codex-app` limit (old binaries ignore both).

**Board**: run `taskq labels` once to create `p0`.

## v4.1.0

One tray per host over every project in `<install>/projects` (heartbeat `<install>/tray`, errors in
`<install>/tray.log`); per-project trays of older versions exit once their service updates. `status.json` gains task
labels, `request`, `update` and `events_since`; `<root>/.taskq/events` keeps 30 days of close and failed moves.
`taskq service focus ... --request TEXT`, `taskq service pause|resume`; resume without a saved focus fails instead of
taking `all`. `<root>/.taskq/update` makes the next pass check for an update at once. No board change.
The tray icon shows run or pause (macOS may show the summary) and a click opens a webview popover: questions and
failed of all projects, project pages, pause/resume, log, folder, restart, stop and uninstall. View, text size and
period persist in `<install>/tray.json`; `taskq tray --snapshot DIR` writes its icons and popover pages as PNG.
`taskq add`, `ask`, `result` and `close` render and check the taskq.md message formats (§ 1 task body, § 2 ask card,
§ 7 verdicts): `ask --text` and decision results take `Done:`/`Problem:` lines, `result --text` takes `Changed:`/`Open:`
with one `--checks` line per check, a reviewed `close --text` takes `Changed:`/`Checks:`/`Open:`; over-limit text is
rejected. Old comments fold unchanged; no board change.

The history graph and the macOS summary icon draw 12 equal close/failed bins of 5 min, hour, day, 7 days, 30 days or
a custom `[start, end)` in local time or UTC; time before history is unknown, not zero. `Update now` appears when a
`status.json` reports a newer version and confirms it only from `status.json`. Pause all and resume all name each
project and its effect before acting; a mixed state reads `Paused N of M`.

## v4.0.10

Windows runs the tray as `<install>/taskq-tray.exe`, so a pinned icon stays pinned across updates. A tray on another
version than the live service exits and the service starts its own: the tray now follows updates.

## v4.0.9

Windows: the tray heartbeat and the poke never changed their file's mtime (Rust rewrote an empty file in place), so
the service started a second tray after each restart and slept a full `poll` after a local command. Both write a byte
now. The v4.0.8 "froze" was this: the tray ran, its heartbeat did not move.

## v4.0.8

Windows: the tray froze after its first pass (the event loop never woke on its timer); it now wakes from a timer thread.
Tray errors go to `<root>/.taskq/tray.log`.

## v4.0.7

macOS and Windows: `taskq tray`, a menu-bar / notification-area icon that the installed service starts and keeps running:
state color, counts, board and issue links, pause/resume, log, project folder, restart, stop until the next logon,
uninstall. New `service restart` and `service stop`; Windows `stop`/`uninstall` also end the service process tree.
`status.json` gains `pid`, `project`, `issues`, `log` and `tasks`. No reinstall: the updated service starts the tray.

## v4.0.6

Windows: workers, reviewers and the `gh`/`hostname` calls run without a console window (a console-less service got a
new window per child). No reinstall: `taskq update` and restart the task.

## v4.0.5

Windows: the v4.0.4 launcher exited with 101 under the Scheduled Task (stale console handles after dropping the console);
it now resets them, logs its own messages to `taskq.log` and keeps restarting the service. `service install` names the
task's user from `whoami` (an ssh session reports `WORKGROUP\<user>`, which schtasks rejects). Reinstall once.

## v4.0.4

Windows: the launcher supervises `service run` (a Scheduled Task restarts only a failed launch, so a crash or the
post-update exit left the host dead until the next logon), runs it without a console window (closing that window killed
the service with `0xC000013A`) and appends its output to `%LOCALAPPDATA%\taskq\taskq.log`.
Reinstall once (`taskq service install` from the new binary): the launcher only changes on install.

## v4.0.3

The host heartbeat carries an `at <board time>` line. GitHub keeps `updated_at` on a same-body edit, so an idle or
unchanged heartbeat aged forever and another host could post `gone` for a live attempt after `ttl`.

## v4.0.2

Windows `service install` registers the Scheduled Task for the current user, so it no longer needs admin ("Access is denied").

## v4.0.1

No token file needed: the board token falls back to the `gh`/`glab` login of the board host (§ 9).

## v4.0.0

Rust binary replaces `taskq.py` and `host_service.py`.

**Board** (v3 → v4), for each open issue except `taskq host` ones:
- Text: remove the block from `<!-- taskq:start -->` to `<!-- taskq:end -->`. Each id in its `deps` becomes a line
  `Depends on #N` unless the text already has one.
- State: v3 keeps it in the `q-<state>` label, v4 in `taskq:` log entries (§ 2). No entry is needed when the folded
  state already matches: `ready`, `waiting`, and `doing`/`review` (the v3 attempt ends with the v3 service).
- `later` / `failed`: `taskq later N --text` / `taskq failed N --text` with the text of the last `**later**` /
  `**failed**` comment; with none, ask the owner for the reason.
- `ask`: v4 has no `ask` without an attempt. Show the owner the question; the owner answers it in v3 before the
  cutover or the task is parked with `taskq later N --text <question>`.
- Only issues labelled `taskq` are tasks (§ 1): add it to every issue with a `q-<state>` label. An issue without
  either stays untouched; ask the owner whether it should become a task.
- v3 capacity issue (`<!-- taskq:capacity -->`): v4 does not read it; remove grants of closed tasks.

**Hosts**: a v4 service admits nothing until `taskq service focus all|<label..>` on that host (§ 4).
