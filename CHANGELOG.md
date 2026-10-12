# Changelog

Per release: what changed on the board and what each open issue needs. A cutover agent reads this (taskq.md § 9);
there is no migration code. Releases without a **Board** section need nothing.

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
