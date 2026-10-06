---
type: Contract
status: Active
domain: agent-workflow
canonical: true
---

# taskq — the project task queue

`taskq` is a Python package with a `taskq` command: a task queue on GitLab or GitHub issues for Claude and Codex agent sessions.

It is the only task queue and the only orchestration mechanism. One tool, `taskq`, works the same
from a Codex session and from a Claude session.
The previous DOT system (`dot_tick.py`, `dot_gitlab.py`, labels `flow-*`) was removed on 2026-10-05.

## Where things are stored

| What | Where |
|---|---|
| Task state | Issue label, exactly one (table in § States). A closed issue is done |
| Epic | Project milestone, flat, no nesting; `add --milestone`, `edit N --milestone` |
| Dependencies | `deps` in the block is the source of truth; each dependency also gets a GitLab `relates_to` link (clickable; set by `add` and `edit --deps`, `migrate` adds missing ones) |
| Work area | Project-configured `area-*` labels; `add --area maps` |
| Assignee | GitLab user: empty is the shared pool; `add --mine` assigns the author; `take` assigns the current glab user |
| Runtime | Label `run-claude` or `run-codex`; without one, `any` |
| Type | Label `code`, `docs`, `research` or `asset` |
| Priority | Label `priority-1` or `priority-2` |
| Other data | JSON block in the issue description: `scope`, `deps`, `claim`, `waiting_for` (reason for `later`), `result` — only what a label cannot express |
| Goal and acceptance | Issue description text |
| History | Issue notes: every taskq note starts with `**action** · app:session` (`take`, `beat`, `ask`, `shown`, `result`, `answer`, `reject`, `release`, `close`, `later`, `waiting`, `ready`, `deps`, `runtime`, `problem`) |
| Task lock | Award emoji `lock` on the task's issue; on GitHub the ref `refs/taskq/lock/<N>` (§ Taking a task) |
| Problem without a task | Its own issue with label `problem`; `tick` names it, the coordinator closes it after review |

There is no local state: any machine with `glab` sees and changes the queue the same way. There is no
other log, mirror, service issue or second queue (the former service issue with claims and the `spawn`
log was closed on 2026-10-06). Scoped labels (`key::value`) on GitLab 17.2.9-ee (tested) without a
license are not mutually exclusive (checked 2026-10-06), so states are plain `q-*` labels.

## Project: taskq.toml

Everything project-specific lives in `taskq.toml` at the root of its repository. `taskq` searches for it
from the current directory upward, so commands run from the main checkout or any project worktree.
It holds no secrets: the token belongs to `glab`.

| Key | What | Default |
|---|---|---|
| `[gitlab] project` | GitLab project path (`group/project`) | one of `[gitlab] project`, `[github] repo` |
| `[github] repo` | GitHub repository (`owner/repo`); `[github] host` for GitHub Enterprise | one of the two |
| `[gitlab] board` | Board name | `taskq` |
| `[gitlab] host` | GitLab host for `glab` (commands also work outside the project checkout) | `glab` picks it from the current directory's git remote |
| `[areas] names` | Project work areas; `init` creates `area-*` labels | empty |
| `[codex] project`, `section` | Codex app project and section for `spawn --runtime codex` | none: `spawn --runtime codex` refuses |
| `[workspace] new`, `continue`, `none` | Brief text about the workspace; `{iid}` is the task number | `git worktree add` next to the checkout |
| `[workspace] retire` | What `close` prints to clean up the tree | nothing |
| `[workspace] cleanup_helpers` | Project folder with `workspace_gc.py`, `host_gentle.py`, `host_tools.py` for `cleanup` | none: `cleanup` refuses |
| `[update] auto`, `every` | `tick` updates taskq from `main` on GitHub at most this often (`30m`, `24h`, `7d`); `taskq update` does it by hand. Missing keys are written into `taskq.toml` with the defaults, and named | `true`, `24h` |
| `[brief] rules` | Project rules added to step 6 of every worker brief (budget, approvals, where the project's authorization is written) | nothing |

New project: `taskq init --project group/project` writes a minimal `taskq.toml` if none exists and
creates the labels and the board (§ Schema and migration).

## States

| Label | Meaning | Who sets and removes it |
|---|---|---|
| `q-ready` | Can start when a slot is free and `scope` is free | `add`, `answer`, `reject`, `release`, `tick` (dependencies closed) |
| `q-waiting` | Waits for another task: `deps` has an open issue | Only `tick`: ready→waiting on an open dependency, waiting→ready when all are closed |
| `q-doing` | A worker is on it | `take`; `answer` from the session whose `claim` is on the task |
| `q-review` | Delivered, awaiting acceptance | `result` |
| `q-ask` | Owner's move: a concrete question with options — from a worker (from `doing`) or from the manager (from `ready`, `waiting`, `later`) | `ask`; removed by `answer` |
| `q-later` | Deferred by the owner; nobody waits on it, reason in `waiting_for` | `later`; removed by `answer` or a manual move to `ready` |

There are no other states. There are no umbrella tasks: an epic is a milestone (§ Epics and subtasks).

**Board:** the project's `[gitlab] board` (default `taskq`; created by `taskq init`) — columns ready | waiting | doing | review |
ask | later. Moving a card changes the label. Allowed manual moves: `ready`/`waiting`↔`later`
(defer and restore), `ask`→`ready` (answer without text), `review`→`ready` (send back for rework),
`review`→Closed (accept without SHA check). Do not move `ready`↔`waiting` by hand: the next `tick`
moves the card back by `deps` and prints `Moved #N …`. `tick` lists everything else under "Board
mismatch": `doing` without a worker, `review` without a result, an issue with a block but without exactly
one state label (including one moved to Open).

**Board on GitHub (2026-10-06):** a Projects v2 project named `[github] board`, a view taskq keeps in step with
the labels (§ GitHub). A card move does not change the label: `tick` executes `ready`/`waiting`→`later`
(`later`), `later`→`ready`/`waiting` (`answer`) and `review`→`ready` (`reject`), each with the note «moved on the
board»; `ready`↔`waiting` goes back silently; any other move goes back to the label's column and is listed under
"Board mismatch" with the fix command (`ask`→`ready` is not an answer: `answer N --text`; `doing`→`ready` is
`release N`).

## Epics and subtasks

- An epic is a project milestone (GitLab 17.2.9-ee without a license has no Epics, blocks links, weight or
  swimlanes). Flat, no nesting. The epic description is the milestone description. Every open task
  has a milestone.
- GitLab subtasks (Tasks) are not used: they are not visible on the board. One task = one worker session = one
  issue. If it is bigger, split it into several issues in one milestone and link them with `deps`. A checklist
  in the description is fine for acceptance steps.

## Three roles

A role is what a session is doing right now. There is no role owner and no role handover.

- **Manager** — the session the owner talks to. Creates tasks (`add`), relays the owner's
  answers (`answer`), shows the queue (`list`).
- **Coordinator** — the session running `tick`. The command itself returns stuck tasks
  to the queue and prints exact instructions: what to check and close, how many workers to start,
  which questions to relay to the owner.
- **Worker** — an ordinary visible app session, one per task. Its only prompt:
  "Run `cd <main checkout> && taskq worker` and follow the instructions it prints".
  The command hands out the task, workspace, history and exact delivery commands.

How to start manager and coordinator sessions, enable the tick and run acceptance:
[taskq-manager](taskq-manager.md).

## Task flow

```
add → ready ⇄ waiting                       tick only, by deps
      ready → take → doing → result → review → close
                     doing → ask (worker) → answer → ready     answer via the queue
                     doing → ask (worker) → answer → doing     answer in the worker session
ready/waiting/later → ask (manager) → answer → ready
 ready/waiting/ask → later → answer or by hand → ready
                                       review → reject → ready
```

- A task starts when dependencies are closed, its runtime matches the session and its scope does
  not overlap a started task. Manual `take N` checks these rules, takes any ready task regardless of
  its assignee, and assigns it to the authenticated `glab` user.
- `tick` and `worker` accept a profile: `--filter "labels=area-maps"` (a GitLab issues query
  string, sent unchanged), `--mine` (only assigned to the current user), and
  `--limit claude=N,codex=M` (local machine slots, default 2 and 3; zero disables a runtime).
  Without `--mine`, they consider the user's tasks and unassigned tasks, never another user's.
  No profile file exists: arguments live in the tick prompt and are passed to each worker.
  Every pass prints the profile and candidate count; a nonempty filter with no candidates warns.
- Capacity counts `doing` claims on this machine, across areas. New claims record the hostname;
  pre-upgrade claims are recognized by local Claude import records or local Codex rollout files.
  Legacy detection reads only matching local filenames and does not need an app server.
  Limits are local scheduling guidance, not a global admission gate or cross-machine
  arbitration. Manual `take` does not enforce them.
- Filtered selection keeps a separate unfiltered safety inventory for dependencies and scope
  conflicts. A dependency outside the profile still blocks its task. Profiles never expand
  automatically: an exceptional area goes into the pool with deps, or is taken manually.
- Ask and review are routed only to the assigned user's tick (and must match its area filter).
  Unassigned tasks remain in the shared pool. The owner can see all states on the project board.
- After `answer`, `reject` and `release` the task returns to `ready` together with its branch and
  worktree; `claim` is reset. The next worker from either app gets the full history and continues.
- Exception: the owner answers in the worker's own session. `answer N` from the session whose
  `claim` is on the task moves `ask`→`doing` with the same `claim`, without `ready` and without a new `take`.
  Machine capacity is not checked: the slot was free only during the question; the worker never left the task.
  The worker continues in the same session and delivers `result` itself. An `answer` from the manager,
  the coordinator or the owner's shell (session does not match `claim`) still leads to `ready`. The `worker`
  brief and the `ask` output state this rule.
- A worker on a `code` or `docs` task creates its own worktree (the brief prints the command, from the
  project's `[workspace]`), commits, rebases on `origin/main` and pushes to `main` under the owner's
  standing permission. `close` checks that the result SHA is in `origin/main`.
- Partial result: close the task, file the remainder as a new task.
- A `doing` task whose issue has not changed for longer than `STALE_MINUTES` (`updated_at`; any note,
  including `beat`, updates it) is returned to the queue by `tick`.

## Taking a task

The lock is the award emoji `lock` on the task's issue. GitLab lets one user put one reaction
on one issue once: a second attempt gets 404 "Award Emoji Name has already been taken"
(a unique key in the database; checked 2026-10-06 on GitLab 17.2.9). This is an atomic test-and-set.

`take N`:

1. Reads the queue; the task must be `ready` and pass `refusal` (dependencies, `scope`,
   runtime). Otherwise it refuses with a reason.
2. `POST award_emoji name=lock`. 404 means another worker took the task: refusal `another worker holds its lock`;
   the worker runs `worker` again and takes the next one.
3. One `PUT`: `q-doing` and `claim`.
4. `scope` is a rule across tasks; the lock on one issue does not cover them. So after
   step 3, if the task has a `scope`, `take` rereads the queue. If a path overlaps another task
   with a `claim`, the one that entered `doing` later gives way: the time is the last
   `add q-doing` label event (`resource_label_events`, GitLab clock); on a tie, the higher number gives way.
   Every `take` first moves the task, then reads, so the later of the two always sees the
   earlier one, and both order them the same way. The one giving way returns its task to `ready` with its
   previous data, removes the lock and gets refusal `scope overlaps #N`. Live race on 2026-10-06
   (two takes entered `doing` 18 ms apart): one gave way.
5. Note `take`.

The lock is held while a worker holds the task: in `doing`, `ask`, `review`, and `later` from `ask`. It
is removed by `answer`/`reject`/`release` (a move to `ready`, including the tick returning a stuck `doing`,
which is the same `release`) and by `close`. An `answer` in the worker's own session keeps the lock: the task
is `doing` again for the same worker. `take` is idempotent: a task already in `doing` with this session's `claim`
is accepted again without the lock and without checking `scope` ("#N is yours").

An orphan lock comes from a `take` that failed between steps 2 and 3, or from a card moved by hand from
`ask`/`review` to `ready`. `tick` finds these with one request (`issues?my_reaction_emoji=lock`) and
removes a lock older than `LOCK_SECONDS` (120 s) from a task in `ready`/`waiting` or from an issue that is
not a task ("Unlocked #N"). While the lock is younger, it may belong to a `take` in progress.

**GitHub (2026-10-06).** The package speaks one store protocol — GitLab's REST shape for the few endpoints it uses
(issues, notes, labels, milestones, award emoji, label events, links, boards) — and `Github` speaks it on GitHub
REST through `gh api`; the tests' fake speaks it in memory. Differences that show: the lock is the ref
`refs/taskq/lock/<N>` on a blob holding its time — a second `POST git/refs` is 422 «Reference already exists» for
any user (checked live 2026-10-06), so the lock is atomic between people with their own accounts (the owner's rule
in the amendment to csgo #241); it is no branch, so no CI runs; anyone may remove it, the claim names the holder.
`tick` finds orphan locks through `git/matching-refs/taskq/lock/`. Labels move as the full set in one PATCH
(the set last read in the process, else one GET). Comments have no `sort=desc`: the newest is read from the
last page by the issue's comment count. The board is a Projects v2 project (below), there are no issue links (`deps` in the block is the source of truth), and `DELETE issues/N` is the GraphQL
`deleteIssue` (admin; `selftest` closes instead when refused). `--filter` is GitHub's list-issues query
(`labels=`, `assignee=<login>`, `milestone=<number>`). Pull requests are dropped from issue lists. GitHub's REST issue list lags a
just-created issue by up to half a minute (measured live 2026-10-06: 25–35 s, sometimes none; a `take` right after
`add` found no task), while GraphQL shows it at once and reflects label changes at once — so issue lists are read
through GraphQL (`repository.issues`, 100 per page, with `states`, `labels`, `filterBy`); single issues, comments,
labels and refs stay REST.

The board: `init` creates the Projects v2 project `[github] board` (default `taskq`) once — owner the repository
owner, user or organization (`createProjectV2` with `ownerId` and `repositoryId`, which links it) — gives its
single-select field Status the options STATES in order (`updateProjectV2Field` with new options: the project's
Todo/In Progress/Done go), deletes the project's built-in workflows (`deleteProjectV2Workflow`; the API cannot
disable them: «Auto-close issue» closes an issue whose card reaches the old Done option, others set Status on
add, close and merge — taskq alone writes Status), adds every open task without a card and prints the URL.
A second `init` changes nothing. The store puts a card in its column in the same request batch as the label:
`POST issues` adds the item (`addProjectV2ItemById`) and sets Status (`updateProjectV2ItemFieldValue`); a `PUT`
that changes the `q-*` label sets Status; `close` archives the item (`archiveProjectV2Item`: the board shows open
tasks only; the issue keeps its history). The project, the field and its option ids are looked up once per
process. `tick` reads the cards in one query (100 open issues per page, each with its `projectItems`: the
project's own `items` list stayed empty for minutes after adds, live 2026-10-06) and treats Status as the
owner's intent (§ States).
Without the token scope `project` (`gh auth refresh -h github.com -s project`) GraphQL refuses the lookup: there
is no board, `init` names the command and makes the labels, every other command works as before.

Live on alexkirs/taskq, 2026-10-06: `init` made the labels; the cycle add → tick → worker → take → beat ×2 (one
note) → ask → tick shows the question → answer → take → result → tick shows the review → reject → take → release
(ref gone) → take → result → close ran through with the ref `refs/taskq/lock/1` set while held and gone at the
end; two `take` processes at once on one task: one «is yours», the other «another worker holds its lock»; the
probe issues were deleted through the GraphQL path.

**Multiple GitLab users.** Each person uses their own `glab` account. Reactions are unique per
user, so `take` also reads all lock reactions after posting its own. The earliest reaction wins
(creation time, then reaction id); a losing user deletes only their own reaction and gets refused.
`unlock` removes only the current user's reactions. This is optimistic ordering, not a GitLab
transaction across reaction and issue updates; live concurrent verification is required per project.
An orphan reaction owned by another user must be cleared by that user; taskq does not delete it.

## Scheduled runs

Scheduling and creating sessions is an app action, not a script action.

- **Claude desktop:** the coordinator is an ordinary session with a timer inside (`CronCreate`). A worker
  is created by `taskq spawn --name "T<N> <words>"`: a `claude --bg` background session of the CLI in
  the main checkout (#270, 2026-10-06: the app window does not change). The coordinator sends it the
  worker prompt with `SendMessage` and `notify_when_idle`. The owner watches it by Remote Control
  (claude.ai/code, phone), `claude agents` / `claude attach`, or on request in the app:
  `taskq show <id>` stops the background run and imports the session with the link
  `claude://resume?session=<id>`, which is undocumented, may change with an app update and always
  shows the session for a moment (~0.2 s). A finished worker: `taskq retire <id>`. Worker sessions
  run without permission prompts: `.claude/settings.local.json` in the main checkout sets
  `bypassPermissions` (owner decision, 2026-10-05; the file is not in git).
- **Codex:** an automation with the prompt "Run `cd <main checkout> && taskq tick` and follow
  the instructions it prints". A Codex worker is created by `taskq spawn --runtime codex`; the prompt
  is sent by `taskq codex-send`, state is read by `taskq codex-read`, and after acceptance it is
  archived by `taskq codex-archive` ([taskq-manager](taskq-manager.md) § Other machines).
  `codex-read <id> --limit N` shows the last N turns (default 3), events, the current
  operation and the actual sandbox of the last turn. `codex-send` prints `delivered` for a new
  turn with an explicit policy or for a message steered into an active turn. Tick shows the status and event age
  of `doing` Codex sessions and separately demands intervention when one stopped without result/ask.
  A Codex worker session is interactive: the owner writes to it in the app, and `codex-send` then
  delivers through the app ([taskq-manager](taskq-manager.md) § Shared Codex session).
  Read boundaries, coordinator actions and the limits of live display of an external turn
  in the app are described in [taskq-manager](taskq-manager.md) § 3 One tick pass.
- A task with a `runtime` goes only to a session of that app; `any` goes to anyone. `add` without
  `--runtime` sets it by type (`DEFAULT_RUNTIME`): `asset` → `codex`; `code`, `docs`, `research` →
  `claude`. `--runtime any` leaves the task to anyone. Tasks created earlier without the field are `any`.
- `taskq runtime N claude|codex|any` changes the runtime label of a task in `ready`, `waiting`, `ask`
  or `later` and writes a note to the issue; in `doing` and `review` it refuses.

Workers on different machines share scope and lock checks: the lock decides who takes a task
(§ Taking a task); the extra worker gets a refusal and "No task can start now".

## Cleanup

`taskq cleanup` runs only from the main checkout on branch main.
Without a flag it prints a report "Remove / Ask the owner / Kept" after fetch; it does not change
local branches, trees or sessions. `cleanup --apply` executes only the "Remove" items that are
proven finished, rechecking before each action.
Running and current sessions are kept, sessions are only archived, and remote branches
need a separate answer from the owner. Retire checks stay in `workspace_gc.py`.
Procedure for owner questions and Claude archiving:
[taskq-manager](taskq-manager.md) § Cleaning up finished work.

## History and report

Every action on a task is a note in its issue (§ Where things are stored), visible from any machine.
A worker records everything that took time or went wrong: `taskq problem --task N --text "..."`
(without `--task` it creates a new issue with label `problem`; `list` and `tick` show it, the coordinator reviews
and closes it). `beat` writes a new note (only a new note moves the issue's `updated_at`; editing an
old one does not, checked 2026-10-06) and deletes the previous `beat` if it was the last note: a long
task carries one `beat` between steps, not one every few minutes. `taskq report --hours H` reads taskq notes from issues
changed in the last H hours and shows the time between steps per task, and the duration and note count
per session, plus the list of problems. `doing` liveness is the issue's `updated_at`.
GitLab `resource_label_events` also stores the time of each state change, but needs a request per
issue and does not know the session; notes give the same and more.

## Schema and migration

`taskq init` (former name `migrate`) is one idempotent command: it creates state, runtime, type, priority and project-configured area labels,
the `taskq` board with one column per state in `STATES` order, removes board columns for states that
no longer exist, deletes their label when no issue carries it (otherwise prints which issues still have it),
and adds `relates_to` links by `deps` of open tasks. `claim`, `result` and history are unchanged.
A rerun changes nothing. Transition on 2026-10-06: the former state "deferred or
umbrella" was split into `waiting`, `later`, `ask` and milestones per a layout agreed with the owner.

## Long-term mechanism audit (2026-10-06)

Historical baseline before personal profiles (#257), measured on a live project on 2026-10-06: 249 issues total, 35 open, 214 closed in 30 days,
192 changed in one day. One `glab api` request takes 1.2–1.5 s, almost all of it `glab` startup and network.
The open-issue list is 448 KiB (≈13 KiB per issue: full descriptions; the API has no field selection).
"Before" is the code before the audit, "after" is after. Profiles add a user lookup and, when filtered, a selection query beside the safety inventory;
lock acquisition adds a cross-user reaction read. No regular path reads closed issues
in full: history growth does not slow `tick`, `take`, `list` or `worker`.

| Mechanism | GitLab requests now (measured) | How it grows | What accumulates | Who cleans up | Verdict |
|---|---|---|---|---|---|
| `take` | Before: 6 + `SETTLE` 1 s (claim, service-issue notes, queue, PUT, note, claim deletion). After: 5: queue ×2, lock, PUT, note; 899 KiB, 9.1 s. Without `scope`: 4. On a `scope` race +1 events request per task, the one giving way +3 | Request count is constant; volume is open issues: 100 open ≈ 1.3 MB per read, 1000 is 10 pages. Closed issues and notes do not matter | One `lock` reaction per task while held | `release`/`reject`/`answer`/`close`; a failed `take`'s lock — `tick` after 120 s | ok |
| Allocation: local capacity, `scope`, runtime, machines | 0: computed from the already-read queue | Linear in open tasks, in memory | Nothing | — | ok; local capacity can be exceeded by one in a race (§ Taking a task) |
| Listing: `list`, `worker` | 1 (queue): 448 KiB, 2.7 s; `worker` + history pages of the chosen task | Pages of 100 open issues | Nothing | — | ok |
| `tick` | Before: 1 + 1 per task in `ask` + 1 (summary mark in the service issue). After: 2 (queue, locks in one `my_reaction_emoji=lock`): 464 KiB, 4.1 s. Plus per event: 1 per `ask`, history pages per `review`, 2 per ready↔waiting move, ~5 per stuck-task return, 2 per lock removal, a `shown` note | Constant + linear in tasks in `ask`/`review`; every coordinator machine does its own pass | `shown` note: once per new question and once a day while the question waits | The marks are the question's history; not read after the answer | ok |
| Dependency waiting | 0 beyond `tick`: open numbers from the same queue; 2 per move | Linear in open tasks | One `waiting`/`ready` note per move | History | ok |
| History notes (`take`, `beat`, `result` …) | 1 POST per action; `beat` 3–4 (task, last note, POST, DELETE of the previous one) | Before: `beat` every few minutes; one task had 41 `beat` out of 82 notes. After: one `beat` between steps; growth follows the number of actions, not time | Notes in closed issues | GitLab keeps them as history; not read regularly | ok |
| Pagination `per_page=100` | — | Before: brief history (`notes`, ascending) and the `tick` question (`question`) were silently lost after 100 notes; same for `report`; `ask-summary` and the `spawn` log in the service issue dropped out after 100 new notes; labels in `migrate` after 100. After: `pages()` reads all pages; the question is read from the newest notes | — | — | ok |
| Problems | With a task: 1 note; without a task: 1 new `problem` issue | By number of problems | Closed `problem` issues | The coordinator closes them after review (`tick` names open ones) | ok |
| `report --hours H` | 1 + 1 per issue changed in H hours: one day = 194 requests, 4.3 MB; before 255 s, after 40 s (8 requests in parallel) | Linear in window activity, not in history | Nothing | — | ok |
| Cleanup: trees, branches | local git | By number of trees | Trees, local and remote branches | `close` prints `worktree-retire`; `cleanup --apply` removes merged ones; remote ones are a question to the owner | ok |
| Cleanup: issues for `cleanup` | Before `state=all`: 249 issues, 2.8 MB, 12.1 s; at 5000 it would be 50 pages ≈ 57 MB, ~4 min. After: open + closed in 30 days: 35 + 214 today | Bounded by 30 days of work | — | — | ok; a worker for a task older than 30 days is a question to the owner, not a deletion |
| Cleanup: sessions | Before: the `spawn` log in the service issue (last 100 notes; 47 entries). After: Codex — `thread/list` of the app project (all 13 Codex log entries were found there); Claude — app metadata on this machine: 150 files, 0.13 s; 86 imported from the CLI, a superset of the 34 Claude log entries | Linear in sessions on the machine, local | Sessions | Codex — `codex-archive`; Claude — the coordinator's `archive_session`; archived sessions are no longer listed (before, every worker of a closed task was printed by every `cleanup`) | ok |
| Labels and board | `migrate`: label pages (23 now) | Fixed set + `problem` | Labels of old states | `migrate` deletes them when no issue carries them | ok |
| Service issue | — | Before: claims (deleted), `spawn`, `problem`, `ask-summary` without cleanup: 52 notes in one day | — | Closed; no code reads it | ok |

## Verification

From a package clone: `python3 -m unittest discover -s tests` (the `cleanup` tests need `TASKQ_CLEANUP_HELPERS=<project folder
with workspace_gc.py, host_tools.py, host_gentle.py>`) — full cycles against a fake GitLab, including the
ready↔waiting move by dependencies, showing a question once and the daily summary, the lock (success, 404,
removal on `release`/`reject`/`close`/stuck-task return, tick removing a failed `take`'s lock), races
on one task and on overlapping `scope` in both orders, a single `beat`, and `problem` without a task.

Live cycle on 2026-10-05: tick started a worker, the worker created a worktree and pushed a commit to main, tick accepted the result and closed the task.
Live Codex cycle on 2026-10-06: a coordinator in Claude started a Codex worker with `spawn --runtime codex`; the worker created a worktree and pushed a commit to main.
