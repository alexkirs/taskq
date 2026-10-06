---
type: Contract
status: Active
domain: agent-workflow
canonical: true
---

# taskq — manager and coordinator session (Claude desktop)

How to set up the session that runs the [taskq](taskq.md) queue: it files tasks from the owner's words,
ticks every 5 minutes, starts workers in visible sessions, accepts results and relays questions to
the owner. Two roles (owner's decision, 2026-10-06):

- **Product manager** — the owner says the session is the taskq product manager (e.g. «Ты
  продукт-менеджер taskq»). Step 1 and § 4: checks the place, reads the queue, files and discusses
  tasks. Does not arm the tick.
- **Coordinator** — the owner says "arm the tick" or "you are the coordinator" (e.g. «включи тик»,
  «ты coordinator»). Steps 1–3: a tick every 5 minutes. Only one such session at a time: `taskq tick`
  prints `Last tick: N min ago`; if that is under 15 minutes, another session holds the tick — do not
  make a second `CronCreate`, tell the owner.

The session replies with the role it took and what is in the queue now.

## New person: one confirmation card, three steps

1. Tell the session what you do, what you exclude, whether you want only your assignments,
   and how many Claude/Codex sessions this machine can run. The session reads the project's
   area labels and translates your words into CLI arguments. No profile file is written.
2. Check one card. Example for “maps, only my tasks; no engine; Claude 1, Codex 2”:

   ```text
   Areas: Maps (area-maps)
   Assignments: only mine; do not take the shared pool
   Local sessions: Claude 1 / Codex 2
   Command: taskq tick --filter "labels=area-maps" --mine --limit claude=1,codex=2
   Board: https://gitlab.example.com/group/project/-/boards/7?label_name[]=area-maps&assignee_username=me
   Engine exception: pool task with deps, or manual take; no automatic expansion.
   ```

   The session resolves the actual board id and current glab username. The board link uses the
   same label/assignee constraints (GitLab board URL parameters differ from API query parameters).
   For `--mine`, use the actual username, not the literal `me`. Without `--mine`, the board shows
   the area pool and assignments; the CLI still excludes other people's assigned tasks locally.
   Preserve every exclusion. If an API filter cannot be represented by the board UI, say which
   part cannot be represented instead of claiming an identical board view.
3. After “ok”, arm the existing 5-minute tick with those exact arguments in its prompt and in
   the worker prompt it prints. A product-manager role alone does not authorize arming the tick.
   Check the last-tick age first; do not arm a second coordinator on this machine.

| What the person says | Profile or action |
|---|---|
| I do everything | No filter, no `--mine`; own assignments plus shared pool |
| Only my tasks | `--mine`; shared pool is excluded |
| Maps, except engine | `--filter "labels=area-maps"`; engine stays outside the profile |
| My tasks plus unassigned maps | `--filter "labels=area-maps"`, without `--mine` |
| Owner delegates a task | Set its GitLab assignee to the person; their tick sees it if it matches their filter |
| A map needs an engine exception | Put an `area-engine` task in the pool with deps, or manually `take N` |
| We are not doing this yet | `taskq later N --text "reason"`; excluded area stays outside the profile |

Defaults need no setup: all areas, own assignments plus shared pool, local Claude 2 / Codex 3.
`run-*` still states the app a task needs. Manual `take` assigns the current glab user and keeps
normal dependency/runtime/scope checks. Limits never arbitrate between machines.

## 1. Check the place

- The session is an ordinary Claude desktop session (not a routine and not a scheduled run: the app
  forbids such a session to start other sessions and to receive messages).
- The queue is readable: `cd <main checkout> && taskq list`. The main checkout is the `main` branch
  tree from `git worktree list`; all queue commands run from it.
- Workers run without permission prompts thanks to `<main checkout>/.claude/settings.local.json`
  (outside git). If it is missing, create it:

  ```json
  {"permissions": {"allow": ["Bash", "Read", "Edit", "Write", "Glob", "Grep", "NotebookEdit",
    "WebFetch", "WebSearch", "Agent", "Skill", "ToolSearch", "SendMessage",
    "mcp__ccd_session_mgmt", "mcp__ccd_session", "mcp__scheduled-tasks", "mcp__serena"]}}
  ```

  The project stays in `dontAsk` mode: everything in the list runs silently, the rest is denied.
  The app does not apply the project's "bypass permissions" mode to an imported session.

## 2. Arm the tick

Tool `CronCreate` (loaded via ToolSearch), `recurring: true`, `cron: "*/5 * * * *"`,
`prompt`:

```
taskq tick. Run `cd <main checkout> && git pull -q --ff-only origin main; taskq tick <confirmed profile arguments>`
and do the coordinator pass by taskq-manager.md § 3 (`taskq contract` prints its path). Reply in the owner's language,
one or two lines when nothing changed.
```

The timer lives inside the session: while the app is open and for at most 7 days. After an app
restart the owner says "arm the tick" — repeat this step. An app routine does not fit the tick: its
interval is at most hourly, and its session cannot start workers.

## 3. One tick pass

`taskq tick` itself returns stuck tasks to the queue, moves tasks between `ready` and `waiting` by
their `deps` (lines `Moved #N …`; never do this move by hand) and prints what to do.

**Acceptance (section Review).** For each submitted task:
1. Read the commit (`git show <sha> --stat`, then the diff) and check it against every Acceptance item.
2. Run the task's focused tests yourself; for behaviour, check it in a fresh tree (the project's
   `[workspace] new` command makes the tree; after the check remove the tree and delete the branch).
3. A push to `main` is the deploy. Close a code task after green CI on its commit:
   `glab api "projects/:id/pipelines?ref=main&per_page=3"`.
4. Accepted: `taskq close N --text "<what exactly was checked and what was not>"`, then the tree
   cleanup that `close` prints (the project's `[workspace] retire`); archive the worker session:
   Claude — `taskq retire <id>` as the Review section prints it (`claude stop` + `claude rm`: the
   background run ends, the transcript stays; a worker the app imported: `archive_session`), Codex —
   `taskq codex-archive <id>` (the command leaves a running
   session alone; a session open in the Codex app is held by the app's own server — the command
   then says so: open it with `open -g codex://threads/<id>` and press Cmd+Shift+A in the window
   (Archive chat; via Computer Use — `app_key`). A session with live background work is not
   archived — retry later.
5. Not accepted: `taskq reject N --text "<exact fixes>"`. If a fix needs a file outside the task's
   scope, allow it in the same text.

**Starting workers (section Start).** The section names the runner for each task, respecting the
separate limits (Claude and Codex each have their own slots; an `any` task is given a runner with a
free slot). The other runtime will not take the task. One at a time:
1. Claude: `taskq spawn --name "T<N> <words>"` — a `claude --bg` session in the main checkout,
   idle, no app window change (§ "Window focus on spawn"); prints its session id. Codex:
   `spawn --runtime codex --name "T<N> <words>"` — prints the Codex session id (§ "Other machines").
2. Send it the worker prompt: Claude — `SendMessage` with `to` = the name from step 1 (as
   `ListAgents` shows it) and `notify_when_idle: true`, so the end of its turn comes back to you;
   Codex — `taskq codex-send <id> --text "<prompt>"`.
   Use the exact worker prompt printed by tick, including the confirmed profile arguments.
3. Workers may be started back to back: `worker` may hand the same task to two concurrent workers,
   but `take` gives it to one, the other is refused and takes the next ([taskq](taskq.md) § Taking a task).
4. Tell the owner, in one line per worker, how to watch it: the tick's section "Claude worker
   sessions" lists every doing Claude worker with `claude attach <id>` and `taskq show <id>`.
   The owner watches by link: Remote Control in claude.ai/code and the phone app, `claude agents`
   in a terminal, or on request in the desktop app (§ "Showing a worker in the app").

**Window focus on spawn (#270, 2026-10-06, app 2.19675.0, CLI 2.1.291).**
- *Claude.* A worker is a `claude --bg` session (documented CLI: `claude agents`, `attach`, `logs`,
  `stop`, `rm`). The app's log shows no `setFocusedSession` for it: no window change. It is not in
  the app's session list; `ListAgents` shows it (`bg`), `SendMessage` reaches it live and
  `notify_when_idle` reports the end of its turn; Remote Control turns on by itself. Works from a
  `CronCreate` fire. Steering without SendMessage: `claude stop <id>`, then
  `claude --bg --resume <session id> "<text>"` wakes the same id.
- Importing into the app is only the link `claude://resume?session=<id>`; its handler, after
  `importCliSession`, always switches the main pane to the session (no flag; `-g` only keeps the
  app in the background). A session record written straight into `claude-code-sessions` is not
  read until the app restarts; the app's own "CLI sessions in the sidebar" is compiled off.

**Showing a worker in the app** (the owner asks «покажи сессию»): `taskq show <session id>`. It
stops the background run first (the app does not refuse a live one and would be a second writer of
the transcript: the worker's turn ends; continue it in the app with a message), imports it, and
returns the pane to the calling session (`--restore <local_id>` names another) as soon as the
app's log has the line `setFocusedSession: sessionId=local_<id>` (~0.2 s of the new session; the
record file ~1.1 s is the fallback).
- *Codex.* The spawn steps (thread/start, name, section, first turn, unsubscribe, broadcast
  `thread-unarchived`) do not switch the window: screenshot before and after shows the same session,
  the new one is in Recents/<project>. Only `open -g codex://threads/<id>` switches it (it also hands
  the session to the app's own server). It is needed only to archive a session the app holds; after
  that, return the window to the session open before, with the same link and its id.
  Do not open worker sessions with this link without need.

**Questions to the owner (sections Waiting for the owner and Still waiting for the owner).** These
are `q-ask` tasks (the board's `q-ask` column): a question from a worker or the manager. The tick
prints a new question once and puts a `**shown**` mark in the issue; a question shown more than a
day ago returns in the summary and gets a new `**shown**` mark (marks are notes on the task itself;
there is no local state). A new question after an answer is new again. Relay it verbatim, then a
short summary in the owner's language. Do not answer yourself. Pass the owner's answer with
`taskq answer N --text "<answer verbatim and its interpretation>"` and start a new worker: a new
session continues the task with the history from the issue. If the owner answered directly in the
worker session, the worker records the answer with its own `answer N`: the task is `doing` again
with its `claim`, and it continues and submits `result` without you and without a new worker
([taskq](taskq.md) § Task flow). Such a task leaves `q-ask` without your `answer` — that is normal.
`q-later` tasks are deferred by the owner and the tick does not show them; bring one back with
`answer N` or by moving it to `ready` by hand.

**Work progress and worker replies.** On every tick read the latest events of doing sessions:
Claude — `list_events`, Codex — `taskq codex-read <id>` (last 3 turns; `--limit N`, 1 to 20). Codex
shows user and agent messages, commands with exit code and short output, event times in UTC, the
current operation and the age of the last event. Watch for questions in messages, not only the last
completed reply. Send a reply to Claude via `send_message`, to Codex via
`taskq codex-send <id> --text "<reply>"`: the output `delivered` confirms the message was received.
For idle/notLoaded a new turn starts; for active the message is delivered into the current turn via
`turn/steer` with an `expectedTurnId` check. A session held by the app is delivered by the app
itself: output `delivered … (new turn in the Codex app)` or `(steered the active turn in the Codex
app)` (§ "Shared Codex session"). This confirms receipt of the message, not that the worker finished.

**Codex turn policy (2026-10-06).** Never use the CLI `codex queue --thread … --message …` for
worker sessions. The coordinator checked their `turn_context`: CLI queue started the turn with
`workspace-write` and `network_access: false`, so Git could not write the main checkout's refs and
`glab` could not get its token from the macOS keyring. Thread settings alone are not enough: a turn
policy can persist into later turns. Every taskq `turn/start`, including the first ready in spawn
and codex-send, explicitly passes `approvalPolicy: never`, `sandboxPolicy: {type: dangerFullAccess}`
from `CODEX_TURN_POLICY`. The thread/start and resume settings are derived from the same policy in
their protocol's format. In the installed schema `thread/queue/add` does not accept a turn policy,
so taskq does not use it for active-send. `turn/steer` does not start a new turn and does not change
the sandbox of a running turn; a restricted turn stays restricted until it ends. The next new turn
via codex-send gets the explicit policy. `codex-read` shows the sandbox and approvalPolicy of the
last turn from its own `turn_context` record, including `network_access` if recorded; a missing
record is shown as `unknown`, not replaced with the desired policy.

**Silent worker.** The `Codex sessions` section prints the status and last-event age of each Codex
doing task. `Codex idle` means idle or notLoaded while doing, with no result/ask:
the worker stopped without submitting. Run the printed `codex-send`, ask it to continue the task and
submit a result or send an ask. Do not start a second worker for the same doing task. If the status
is active and a command is running, check its progress; event age alone does not prove a hang. No
marks for more than 20 minutes — look at the session and send a message. After `STALE_MINUTES`
without issue changes the tick returns the task to the queue itself.

**Board mismatches (section Board mismatch).** The owner may move cards on the project board (the
`[gitlab] board` or the GitHub Projects v2 board from taskq.toml). The tick lists moves the queue cannot execute, with a fix command (on GitHub it executes the allowed ones itself and puts the others back);
fix it or ask the owner what was meant. For Codex in review the tick reminds to archive after
acceptance; for ask and later — to archive the stopped worker, since after the answer the task
continues in a new session. A manual `ready`↔`waiting` move is not an error; the tick moves it back
by `deps`.

**Codex observation limits.** Reading is limited to the last 100 events of each selected turn;
skipped older events are marked. Times come from `thread/items/list`, not from the `updatedAt`
metadata. A missing timestamp is printed as `unknown`. In the installed app-server the current
commandExecution appears in the paginated history only after the process ends. So an active turn is
supplemented with unfinished tool calls and exec processes from the last 256 KiB of its own
server-provided rollout. This reads existing history without resume; it is not a second log and does
not change the worker. If the command started outside that tail or the rollout is unavailable, the
current operation may be unknown. An unreachable server prints `status unknown`; it does not mean
idle and does not block the rest of the coordinator's work.

**App and server turn (2026-10-06).** `thread-unarchived` via IPC refreshes the session list and
metadata (`handleThreadUnarchived` in the installed app), but does not subscribe the window to item
events of the external app-server. The app has a separate owner/follower protocol
`thread-stream-state-changed` version 11 and `thread-follower-start-turn` version 2; it is not a
single broadcast indicator of turn start. A probe task received the broadcast during an active turn;
server events were confirmed. Reading the handlers shows that repeating `thread-unarchived` does not
give a live feed. Do not build it into every send as a fix without visual confirmation. A coordinator
check via Computer Use after the turn ended (owner's answer, 2026-10-06) confirmed the session in
Recents: after opening, the prompt, "Worked for 47s" with the comment and command collapsed, and the
probe's final marker were visible. This confirms history after opening; live updates during the turn
were not verified. The probe also showed the banner "This is open in another app — Close it there to
continue" with Retry / Fork chat: a session held by the shared server cannot be continued from the
window. Since 2026-10-06 the shared server releases the session after a turn (§ "Shared Codex
session").

**Shared Codex session (2026-10-06).** The worker session is interactive: the owner writes in it in
the app, the coordinator reads and writes via taskq. This is the default mode, there is no flag.
- *One writer.* Only the process holding the session's lock
  `~/.codex/thread-writer-locks/<id>.lock` can write to it: the shared server or the app's own
  server. Others get `already has an active writer`; the app window shows the banner.
- *The shared server releases.* Only an explicit `thread/unsubscribe` unloads the session: 60 s after
  the turn ends the lock is free. Without it the session stays loaded until the server restarts.
  `spawn` unsubscribes after the ready turn, `codex-send` after `turn/start`.
- *The app takes it.* The owner opens the session (Recents or `open -g codex://threads/<id>`): the
  app gets the lock and holds it while running, even after switching to another chat. If the window
  is opened before the 60 s have passed, the banner shows; Retry or reopening the session clears it.
- *The coordinator writes through the app.* For a session the shared server has not loaded,
  `codex-send` asks the app's IPC `thread-owner-discovery`. If there is an owner, the message goes as
  if from a second app window: `thread-follower-steer-turn` into the running turn or
  `thread-follower-start-turn` with `CODEX_TURN_POLICY`. The app applies the policy from the request
  (a probe with `readOnly` produced a `read-only` turn), so taskq passes full access explicitly.
  No owner — the old path via the shared server.
- *The coordinator reads.* `codex-read` and the tick read the session through the shared server from
  storage, without the lock. A turn running in the app is shown by the shared server as
  `interrupted`: the turn has no end in the rollout yet. taskq treats it as running until the
  rollout tail has `task_complete` or `turn_aborted` for that turn. Then the output is
  `notLoaded (turn running in the Codex app …)`, and the tick does not consider the worker idle.
- *Both write at once.* If the app holds the session, owner and coordinator messages go into one turn
  as steer, in arrival order. If the shared server holds it (the coordinator's turn and 60 s after),
  the owner sees the banner and waits; the coordinator writes as before.
- *Limits.* With no owner window but the lock held by the app (window closed, app server alive),
  `codex-send` refuses with a hint to open the session. The app's IPC protocol is not public
  (versions from its table: discovery 1, start-turn 2, steer-turn 1); after an app update, verify
  with a probe. Typing into the window via Computer Use works (`app_type` with
  `overwrite_existing`, then `app_key return`), but is slower, needs the session open and breaks on
  layout changes — fallback only.

The coordinator uses `codex-read` as the liveness source and `codex-send` to reply.

**Codex screen check (owner's decision, 2026-10-06).** The coordinator checks the window itself via
Computer Use; it does not ask the owner "is it visible on the Codex screen". In the Claude UI:
`request_access` for `com.openai.codex` (display name ChatGPT), then `app_screenshot`; select the
session with `app_ax_find` by title and `app_click` with `element_index`. Some buttons have hidden
titles and clicks do not work. In the Codex UI it uses the app's available Computer Use API
(`cua.getApp("com.openai.codex")`) and follows the documentation it returns. If the tool refuses,
record the exact refusal and do not work around it. Record the history-after-opening check and the
live-update-during-turn check separately; a completed reply does not prove a live subscription.

**Reply to the owner.** When nothing changed — one or two lines. Do not write "no changes" without
running the command.

## Cleaning up finished work

After accepting tasks or at the owner's request the coordinator runs `taskq cleanup` from the main
checkout. First a fetch updates `origin/*`; then the command only reads branches, trees, open issues
and issues closed within `CLEANUP_DAYS` (30 days), Codex session metadata and this machine's Claude
app session metadata (`claude-code-sessions/*/*/local_*.json`: cwd, time, archive flag; it does not
read conversations). This run changes no local branches, trees or sessions. The report has three
sections: "Remove", "Ask the owner", "Kept".

1. `cleanup --apply` re-checks each "Remove" item before acting. Trees are removed by the project's
   existing `[workspace] retire` command with deletion, then branches by `git branch -d`. To check
   deletion, Git uses `origin/main` as upstream only for the duration of the command; the branch
   configuration is not changed. A rebase is recognised via `git cherry`, but `-d` may refuse to
   delete a branch with different SHAs. Such a branch stays; there is no force and no ref rewriting.
   The command prints the actions taken and the size of deleted data; physically freed space may
   differ due to APFS clones or a WSL disk image.
2. The coordinator relays "Ask the owner" items as questions with the listed options. Without an
   answer it runs none of the changing commands from that section. An answer applies only to the
   chosen item; before acting, re-check its state. "Keep" means do nothing (`true`). Show commands
   are run per the chosen option. `--apply` does not delete remote branches; it never touches main
   or the calling session. A printed `git branch -d` may refuse for an unmerged branch or a branch
   in a worktree; relay such a refusal to the owner, without `-D`, force or bypassing checks.
3. Codex sessions from "Remove" are archived via `codex-archive`; active ones, a turn in the app,
   unknown state, an unclosed task and the current session are kept. An already archived session is
   confirmed by the command (`already archived`). Sessions are never deleted.
   *Session open in the app.* Its lock is held by the app's own server (stdio, not reachable from
   outside) for 3 h after leaving the window, or while there are more than 10 such sessions; the
   shared server refuses with `active writer`. There is no IPC request to archive or unload:
   broadcast `thread-archived` only hides the row, the lock stays. `osascript` has no assistive
   access. So `codex-archive` (and `cleanup --apply`) prints a recipe, and the coordinator runs it
   itself via Computer Use, without the owner: `request_access` `com.openai.codex`,
   `open -g codex://threads/<id>`, `request_full_control`, activate the app
   (`osascript -e 'tell application id "com.openai.codex" to activate'`), click the body of the open
   chat, Cmd+Shift+A (Archive chat) — verified 2026-10-06 on two sessions. Then a repeated
   `codex-archive <id>` confirms `already archived`. The "Archive chat" button in a Recents row
   appears only on the owner's mouse hover: `app_click` on it via accessibility does nothing;
   Cmd+Shift+A via `app_key` in the background does not work either, full control is needed.
   The tree of such a session stays until the next `cleanup --apply`.
4. Claude background workers (`claude agents`, cwd the main checkout): a worker of closed tasks is
   removed by `taskq retire <id>` (`--apply` does it); one with an open task or busy is kept; one
   without a claim, older than `STALE_MINUTES`, is a question with a `taskq retire` option.
   For Claude sessions in the app (imported workers, `taskq show`) the script prints
   `coordinator: archive_session local_<id>` for workers of closed tasks, found by claim, if the
   session exists in this machine's app and is not archived. Only the coordinator has this app tool:
   it checks that the worker finished and holds no background command, then runs `archive_session`
   for each such id. An imported session that never took a task is found by the app metadata: imported from the CLI
   (`adoptedFromOtherSurface`, `sessionId` = `local_<cliSessionId>`), cwd is the main checkout, not
   archived, no activity for longer than `STALE_MINUTES`, no claim. This is a question to the owner
   with an `archive_session` option. Codex spawn sessions are found in `thread/list` by the app's
   project; no separate log is needed for them.

If the Codex inventory is unavailable, trees are kept until the next check. Task state comes only
from GitLab; session metadata names sessions but does not by itself prove completion. A worker of a
task closed earlier than `CLEANUP_DAYS` is no longer proven finished: its session goes to "Ask the
owner", not "Remove". For closed code/docs tasks the result SHA is checked in `origin/main`. Trees of
open tasks, including ready ones to be continued, are kept.

## Checking the orchestration (selftest)

The same procedure in a Claude and in a Codex session, for any worker app (owner's request,
2026-10-06).

**The owner says** «проверь свои инструменты оркестрации», «теперь ты оркестратор, проверь, работают
ли у тебя механизмы», "check your orchestration tools".

**The session offers** a scope in one message and waits for the choice:

- **quick** — about 1.5 min, no sessions started: the queue commands run as separate worker
  processes with a test identity of this session's app.
- **full** — about 4–5 min per app (Claude measured 4.5 min on 2026-10-06), with a real worker session of each app (`--runtime claude codex …`;
  default: every configured app). The offer names the apps.
- Where the report goes: printed here, and a note on the issue the owner names (`--note <N>`), or
  nowhere else.

**It runs** from the main checkout:

```
taskq selftest --scope quick [--runtime <app>] [--note <N>]
taskq selftest --scope full [--runtime claude codex …] [--note <N>]
```

`quick` checks, each against GitLab: `add` (a `research` task labelled `selftest`, scope under
`.local/selftest/`), `list` sees it, `take` (state `doing`, claim of the worker, glab user as
assignee, `take` note), `beat`, `ask`, the tick prints the question under "Waiting for the owner",
`answer`, `take` again, `result`, the tick prints "Review #N", `close`, then the traces are removed.
`full` first races two worker processes on one task (exactly one may hold it; GitLab's lock decides),
then per app: `spawn` a session through the package's own path (`claude_spawn`, `codex_spawn`, or the
app's configured command), sends it the worker prompt with `--filter labels=selftest`, and waits for
the worker to `take`, `beat` and `ask`; then `answer` → the worker takes again and hands in a result;
`reject` → again; `release` (lock removed) → again; `close`; the session is retired. At the end: the
selftest issues are deleted (closed if the token may not delete), no `taskq-<N>` worktree of them
exists, and the tick names no selftest task under "Board mismatch". A selftest tick keeps the real
tick's last-run time. A selftest task is invisible to every profile whose `--filter` does not name
`selftest`: no real worker or tick takes it.

**The report** is a table `mechanism / runtime / result / seconds / detail`. Every row is read back
from GitLab (state label, claim, assignee, the newest note and its author), not taken from what a
worker says. After a failed row the rest of its chain is `skipped`; the exit code is 1. Show the
table to the owner as printed, failed rows first in the reply.

**A broken mechanism is named, not hidden.** `--worker-env KEY=VALUE` changes only the worker side:
`taskq selftest --worker-env GITLAB_TOKEN=broken` reports `take … FAIL … 401 Unauthorized`
(verified 2026-10-06). A Codex worker runs in the app's server, whose environment this cannot change.

**A Claude worker session** is archived only by the app tool: after `full` the session runs
`archive_session local_<id>` for the id the report names, then `taskq selftest --scope check`, which
repeats only the trace checks (tasks, worktrees, sessions, board) of the last run
(`.local/selftest/last.json`).

**A new worker app** is one table in `taskq.toml`, no code change:

```toml
[runtimes.grok]
env = "GROK_SESSION_ID"                          # its session id variable: claims and notes
spawn = "run-grok spawn --name {name}"           # prints the session id as its last line
send = "run-grok send {session} {text}"          # one turn; may return before the turn ends
archive = "run-grok archive {session}"           # optional
```

Commands are split before the values are filled in: no value reaches a shell. The app then also
works in `add --runtime`, `--limit grok=N` and `selftest --runtime grok`.

**Speed.** Each glab call costs about 1.05 s here (221 ms round trip to the GitLab host; the open
issues page, 344 KB, 2.5 s), and `take` makes 8 of them. `quick` makes about 60 calls; under one
minute needs a persistent HTTP connection instead of one glab process per call. `TASKQ_TRACE=1`
prints every call with its time, and each selftest step.

## 4. File a task

`taskq add --title … --type code|docs|research|asset --goal … --acceptance …
--scope <paths> --deps <numbers> --priority 1|2 --milestone "<epic>" [--runtime claude|codex|any]`

- **Epic** — a project milestone (flat). Every task gets `--milestone`; a new epic is a new GitLab
  milestone with a description, not an umbrella issue. To change the epic or dependencies:
  `taskq edit N --milestone "<epic>"`, `taskq edit N --deps <numbers>` (sets `relates_to` links;
  the next tick moves the task between `ready` and `waiting` itself).
- **One task — one worker session.** Do not create GitLab sub-tasks (Tasks): they are not visible on
  the board. Work larger than one session is several issues in one milestone linked by `--deps`; a
  checklist in the description is only for acceptance steps.
- **Waiting on an owner decision** — do not hold a task without a question. Either
  `taskq ask N --text "<question with options>"` (the manager asks about an unstarted task; column
  `q-ask`), or `taskq later N --text "<why deferred>"`, or propose closing it to the owner.
- **Runner** (owner's decision, 2026-10-06). Claude — code; Codex — visual work: models, rig,
  animation, 3D, frame comparison. Without `--runtime` the `run-*` label is set by type: `asset` →
  `codex`, everything else → `claude`; check it when filing and again before starting. The owner can
  override: `taskq runtime N claude|codex|any` (`ready`, `waiting`, `ask`, `later`) or the `run-*`
  label by hand.
- **One runner per task.** Split a mixed task (code and visual check) in two and link with `--deps`:
  e.g. code in Claude, then frame comparison in Codex.
- **Accepting visual tasks:** the coordinator checks code and tests; the picture is checked by the
  owner or a separate check task in Codex.
- **Goal** — what to do and why, with exact paths, commands and owner decisions with dates.
- **Acceptance** — checkable items: a command and the expected result. Acceptance follows them.
- **Owner's rule 2026-10-06: do not get in the agents' way.** A worker may edit any file the task
  needs and names it in the result; scope is the expected area for separating parallel tasks, not a
  ban. During development the budget is unlimited: paid generation, provider credits and model time
  are allowed; the worker tries options and writes what was spent in the result. Ask the owner only
  about product choices and irreversible actions (publishing, deleting data). The manager tracks
  execution through results and `taskq report` and turns recurring problems into rules.
- **Type and commit.** `research` and `asset` are tasks that end with an answer; if a task puts
  something in the repository (a tool, a report), file it as `code` or `docs` with an explicit
  `--runtime`.
- **Project rules.** Task rules specific to a project (owner decisions about its product) live in
  the project document that its `AGENTS.md`/`CLAUDE.md` points to; the manager reads it before
  filing a task.
- **Scope** — the files the task is expected to edit. Tasks with overlapping scope run one at a time;
  too broad a scope stalls the queue, too narrow leaves loose ends. Name individual files, not whole
  directories.
- **Irreversible and external** (publishing a package, DB migration, deleting data, protocol change):
  write into the Goal the condition "if anything differs from what is expected — do not do it, ask
  via ask".
- **Waiting for an external event** (access, a file from another machine): file an ordinary issue
  without a `q-*` label and list it in `--deps`; the task sits in `q-waiting` until you close that
  issue.
- Before filing, check that the commands and files named in the task exist.

## 5. What the manager does not do

- Does nothing the owner did not ask for: to "why?" it answers rather than reverting.
- Does not edit tasks or the repository bypassing the queue, except the queue itself and these
  documents.
- Removes other trees and data only within the bounds of `cleanup --apply` above; anything else
  needs the owner's answer. Packages are not removed by this mechanism.
- Does not change security settings of other machines; another Claude session takes such a decision
  only from the owner directly.

## Other machines

- **Codex.** The tick in Codex is an automation with the prompt `Run \`cd <main checkout> && taskq
  tick\` and follow the instructions it prints.` Visible Codex session (verified 2026-10-06):
  `taskq spawn --runtime codex --name "T<N> <words>"` through the app's shared server
  (`~/.codex/app-server-control`, WebSocket) does `thread/start` in the `[codex] project` project
  (`ephemeral: false`, cwd — main checkout), sets the name, moves it to the `[codex] section`
  section, sends the first "ready" turn and the `thread-unarchived` message to the app's IPC
  (`~/.codex/ipc/ipc.sock`); prints the session id. The owner sees such a session in the sidebar
  in their project without searching. A separate section header in the sidebar and visibility of
  messages inside the session were not checked by the owner — accepted as is. Owner's decision
  2026-10-06: the Codex worker runs outside the sandbox and without approvals; the only policy in
  the taskq package is `CODEX_TURN_POLICY`. The thread-parameter format is produced by
  `CODEX_ACCESS`. Sending messages, reading events and the turn sandbox, the CLI queue ban and
  intervening on idle are described in § 3 of this document; the coordinator follows those steps
  for both apps.
  - `codex exec` does not appear in the shared session list; `codex://threads/<id>` does not put it
    in the sidebar.
