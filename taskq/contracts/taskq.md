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
| Codex full access | Label `codex-full-access` (owner's choice per task, #157): the task's Codex turns run with `danger-full-access` instead of workspace-write: no file, network or IOKit limits. Set it only for a task that needs the GPU (Blender, Metal). `tick` spawns its Codex worker with `spawn --codex-full-access`, `codex-send` keeps the policy for the session that claims it, and a sandboxed Codex session (`CODEX_SANDBOX` set) does not see or `take` it |
| Machine | Label `host-<name>` (`add --host win`): only a worker on that machine takes it; without one, the machine whose worker takes it first. A machine's name: `TASKQ_HOST`, else `[hosts]` of taskq.toml (`"DESKTOP-7" = "win"`), else the hostname up to the first dot |
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
| `[github] board` | Title of the Projects v2 board linked to the repository | the repository name (`owner/repo` → `repo`) |
| `[gitlab] host` | GitLab host for `glab` (commands also work outside the project checkout) | `glab` picks it from the current directory's git remote |
| `[areas] names` | Project work areas; `init` creates `area-*` labels | empty |
| `[codex] project`, `section` | Override of the Codex app project for `spawn --runtime codex`, and its sidebar section; kept for compatibility below the personal `[codex]` until migrated | project: the app's project whose root is the main checkout (`project/list`), created by `project/create` when none is; no section |
| `[codex] writable` | Extra writable roots of Codex worker turns, for project files outside the checkout (e.g. `["../csgo-media"]`): paths relative to the main checkout or `~/…`, resolved to absolute and added after the built-in roots by `codex_turn_policy()`; a missing path is skipped and `doctor` warns. A root that is a linked worktree of this checkout also adds its gitdir, so Codex on Linux lets git commit there (#163) | none: the main checkout's `.git`, `.worktrees` and the taskq state dir |
| `[workspace] new`, `continue`, `none` | Brief text about the workspace; `{iid}` is the task number | `git worktree add -b taskq-<N> .worktrees/taskq-<N> origin/main` inside the main checkout (§ Task flow, «Where task trees live») |
| `[workspace] retire` | What `close` runs from the main checkout to remove the task's tree (then `git branch -d taskq-<N>`) | `git worktree remove .worktrees/taskq-<N>`; nothing when the project sets its own `new` without `retire` |
| `[workspace] cleanup_helpers` | Project folder with `workspace_gc.py`, `host_gentle.py`, `host_tools.py` for `cleanup`, an optional override | none: built-ins (`git worktree remove`, `lsof`) |
| `[update] auto`, `every`, `ref` | `tick` updates taskq from REPO (github.com/alexkirs/taskq) at most `every` (`30m`, `24h`, `7d`); `taskq update` does it by hand. `ref = "main"`: the newest `main` commit whose CI check-runs passed; `ref = "stable"`: the `stable` tag, only when CI passed and the tag is signed by a key in the package's `allowed_signers` (README: Develop taskq). A refused update prints one line and nothing new runs; a clone whose new code does not start (`python3 -m taskq --version`) goes back. Missing keys take their defaults in memory; `taskq.toml` is never written | `auto`: `true` when the project's repository has REPO's owner, else `false`; `24h`; `main` |
| `[profile]`, `[profile.limits]` | Team defaults for the tick/worker profile, below the personal file (§ Shared and personal configuration); never one person's choices | none |
| `[hosts]` | Hostname → short machine name (`"DESKTOP-7" = "win"`), for `host-<name>` labels and tick lines | the hostname up to the first dot |
| `[coordinator] machine` | The one machine (its name, e.g. `"mac"`) whose tick coordinates: spawns shared work, shows reviews and questions, closes (#145). A tick on any other machine starts only its `host-<name>` tasks within its own limits, releases only its own stalled work, never reviews or closes, and prints `coordinator is <name>`. No failover: the owner moves it by editing this line. `doctor` names a ref left by the old lease (`refs/taskq/coordinator/*`), `doctor --fix` deletes it | none: every tick coordinates (a single-machine project) |
| `[brief] rules` | Project rules added to step 6 of every worker brief (budget, approvals, where the project's authorization is written) | nothing |

The Codex worker sandbox (workspace-write, Codex CLI 0.159) denies the GPU on macOS: `iokit-open-user-client AGXDeviceUserClient` and `IOSurfaceRootUserClient`, so `MTLCreateSystemDefaultDevice()` returns nil. Blender 5.2 then exits 139 at start (`supports_barycentric_whitelist` → `strstr(NULL)`), even `--background --factory-startup`; no `--gpu-backend` avoids it and Codex has no setting that allows only the GPU (#157). A task that needs Metal (Blender, a GPU browser) runs in a Claude worker (`run-claude`) or, by the owner's choice, in a Codex worker with the `codex-full-access` label (§ Data model). Risk of that label: the worker can write and delete anything the user can, outside the checkout too.

New project: `taskq init --project group/project` writes a minimal `taskq.toml` if none exists and
creates the labels and the board (§ Schema and migration).

### Shared and personal configuration (#32, shipped by #48)

On 2026-10-06 the owner answered “yes to all three” through `ask`/`answer` on #32: use
`taskq.local.toml` in the canonical main checkout, keep machine capacity local, and remove
profile flags from the permanent coordinator prompt. #48 implemented it as described below;
the “Before” column is the state before #48.

| Setting | Owner | Before | Location |
|---|---|---|---|
| Tracker, host, repository/project, board | Project | `taskq.toml` | Shared file, unchanged |
| Area names, workspace commands/helpers, brief rules, update policy | Project | `taskq.toml` | Shared file, unchanged |
| Runtime adapter definitions | Project | `[runtimes]` in `taskq.toml` | Shared file, unchanged |
| Selection filter, own assignments versus pool | Person | Tick prompt flags, copied into worker prompts | Personal `[profile]` |
| Assignee identity | Person | Authenticated `gh`/`glab` user; issue assignee | CLI identity and tracker, never a configured username |
| Preferred runtime for own `any` tasks | Person | No preference setting; runtime label and scheduler | Personal `[profile] preferred_runtime` |
| Reply language | Person | Session/agent instructions | Existing instructions; no new language key in this change |
| Claude/Codex capacity | Person on this machine | `--limit` in prompts; defaults Claude 2, Codex 3 | Personal `[profile.limits]`; CLI override for one invocation |
| Running sessions and occupied slots | Session on this machine | Tracker claims with a hash of the machine id (`~/.local/state/taskq/machine-id`, made once) and local legacy evidence | Existing claims and detection; never config |
| Idle stop (optional) | Person | None: the timer fired on an empty queue forever | Personal `[idle] stop = 5`: empty ticks in a row before the idle stop, 0 = never |
| Cleanup on the idle stop (optional) | Person | None | Personal `[idle] cleanup = true`: run `cleanup --apply` on the idle stop |
| Codex app project/section | Person on this machine | Shared `[codex]` override or discovery by main-checkout path | Personal `[codex]`; discovery remains the default |
| Claude worker permissions | Machine/user | `.claude/settings.local.json` | Same local permissions file, outside git |
| Folder trust, app/CLI login and credentials | Machine/user | App/CLI secure state | Same native state; never either TOML file |

The leaks are selection/capacity embedded in persistent prompts and app-specific Codex IDs
allowed in the shared file. Move those values into the personal file; do not duplicate login,
trust, permission or live session state there.

Resolve the personal file as `<canonical main checkout>/taskq.local.toml`, using the existing
`main_checkout` helper. All of that checkout's worktrees read the same file; a worktree-local
copy does not override it. A checkout used by multiple people needs separate OS-user
checkouts. No user registry, hostname map or automatic cloud synchronization is needed.

```toml
[profile]
filter = "labels=area-maps" # empty means all areas
mine = true                # false includes own assignments and the unassigned pool
preferred_runtime = "codex" # optional: claude/codex/another configured runtime

[profile.limits]
claude = 1
codex = 2

[codex]
# Optional local overrides; omit to discover/create by canonical checkout path.
# project = "app-project-id"
# section = "app-section-id"

[coordinator]
# The Claude session the launchd tick timer wakes (manager contract § 2); `tick --install-timer` writes it.
# session = "claude-session-id"

[machine]
# Free text every brief on this machine prints (#139): how this machine differs, e.g. Windows claude.cmd, checkout in WSL.
# notes = "run git and tests via wsl.exe bash -lc; no Codex"
```

Merge supported preference keys individually: explicit CLI flag > personal > shared >
built-in default. Project-owned settings remain shared and are not personal overrides.
Shared profile values may supply team defaults but must not contain one person's choices.
Absent preferences preserve today's defaults: empty filter, `mine=false`, Claude 2/Codex 3
(other configured runtimes 1), no preferred runtime, no Codex override. Limits are
non-negative integers; zero disables that runtime. Reject invalid TOML/types/runtime names
with the file/key and repair instruction; never silently broaden a malformed profile.

Explicit false, empty filter and zero must override lower layers: add `--no-mine`, preserve
`--filter ''`, and merge only runtime entries explicitly supplied in `--limit`. Distinguish
absent arguments from argparse defaults. Preferred runtime is only a tie-break for the
current user's `any` tasks when eligible capacity exists; it never overrides a task's
`run-*` label, selects another person's work, reserves a slot, or prevents fallback to an
eligible runtime. Unassigned tasks keep existing selection behavior.

`tick` and `worker` reload the personal file on every invocation and print effective profile,
candidate count and configuration source. Their unfiltered dependency/scope inventory and
local-host capacity counting stay intact. The permanent CronCreate/automation prompt runs
`taskq tick` without profile flags. Worker and retry prompts carry only explicit invocation
overrides, including false/empty/zero; they do not freeze resolved personal defaults.
Existing timers with profile flags must be inspected and migrated with coordinator authority:
old flags otherwise continue winning. Do not create a second timer or start workers during
profile setup. Until migration, print the overrides visibly in the profile card.

Onboarding integrates with both modes in `taskq-manager.md` § 1. Missing personal file:
show one short card asking areas/exclusions, only own assignments or pool, and this machine's
Claude/Codex capacity; offer optional preferred runtime. Translate areas into the tracker
filter without losing exclusions. After the person's confirmation, mode A supplies a
concrete creation command for them to run; mode B writes it under agreed setup authority.
Present file: print its effective card and ask “keep?”; keep preserves it, edits merge only
confirmed preferences. Preserve unrelated existing keys. Saving a profile grants neither
timer nor worker-launch authority. Do not persist the conversation, setup authority, secrets,
trust decisions or permissions as profile answers.

The non-interactive local command `taskq profile init [--filter …] [--mine | --no-mine]
[--limit claude=N,codex=M] [--preferred-runtime R]` writes confirmed CLI values
and defaults when missing, refuses to overwrite an existing file, and changes no tracker,
app permissions or timer. Doctor reads only: a missing personal file is a readiness gap
with `taskq profile init` as the repair command and an instruction to confirm preferences
through onboarding first. Invalid personal configuration or a tracked personal file is also
a gap with a concrete repair instruction. Missing file does not block ordinary tick/worker:
use existing defaults and show the gap; doctor still exits nonzero until it is resolved.

`init` ensures exactly one `/taskq.local.toml` line and one `/.worktrees/` line (an unanchored line
already there counts) in the main checkout's `.gitignore`, preserving existing content. Personal configuration is never committed. If already tracked,
report it and propose `git rm --cached -- taskq.local.toml` while retaining the local file;
do not silently delete it. On another machine, copy person preferences by hand or answer
the onboarding card again; reconfirm capacity, discover the local Codex project, and do not
blindly copy app IDs. Existing shared Codex overrides need an explicit migration: offer
copying them into this person's local file, then remove shared IDs only with project-owner
agreement; retain them as lower-priority compatibility values until migrated.

## States

| Label | Meaning | Who sets and removes it |
|---|---|---|
| `q-ready` | Can start when a slot is free and `scope` is free | `add`, `answer`, `reject`, `release`, `tick` (dependencies closed) |
| `q-waiting` | Waits for another task: `deps` has an open issue | Only `tick`: ready→waiting on an open dependency, waiting→ready when all are closed |
| `q-doing` | A worker is on it | `take`; `answer` from the session whose `claim` is on the task |
| `q-review` | Delivered, awaiting acceptance | `result` |
| `q-ask` | Owner's move: a concrete question with options — from a worker (from `doing`) or from the manager (from `ready`, `waiting`, `later`) | `ask`; the second `release` in a row (#157); removed by `answer` |
| `q-later` | Deferred by the owner; nobody waits on it, reason in `waiting_for` | `later`; removed by `answer` or a manual move to `ready` |

There are no other states. There are no umbrella tasks: an epic is a milestone (§ Epics and subtasks).

**Board:** the project's `[gitlab] board` (default `taskq`; created by `taskq init`) — columns ready | waiting | doing | review |
ask | later. Moving a card changes the label. Allowed manual moves: `ready`/`waiting`↔`later`
(defer and restore), `ask`→`ready` (answer without text), `review`→`ready` (send back for rework),
`review`→Closed (accept without SHA check). Do not move `ready`↔`waiting` by hand: the next `tick`
moves the card back by `deps` and prints `Moved #N …`. `tick` lists everything else under "Board
mismatch": `doing` without a worker, `review` without a result, an issue with a block but without exactly
one state label (including one moved to Open).

**Board on GitHub (2026-10-06):** the Projects v2 project linked to the repository and titled `[github] board` (default: the repository name), a view taskq keeps in step with
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
- `tick` and `worker` select by a profile: filter (`labels=area-maps`, a GitLab issues query
  string, sent unchanged), mine (only assigned to the current user) and limits (local machine
  slots, default Claude 2 and Codex 3; zero disables a runtime). Each key comes from
  `taskq.local.toml`, reread on every run, below an explicit flag of that run (`--filter`, `--mine`
  / `--no-mine`, `--limit claude=N,codex=M` with only the named runtimes) and above `[profile]` of
  `taskq.toml` and the defaults (§ Shared and personal configuration).
  Without mine, they consider the user's tasks and unassigned tasks, never another user's.
  Every pass prints the effective profile, candidate count and the source of each key
  (`Source: flag: …; taskq.local.toml: …; default: …`); a nonempty filter with no candidates warns.
  The worker prompt carries only the run's explicit flags, never the resolved personal values.
- Capacity counts `doing` claims on this machine, across areas. New claims record a hash of the machine id
  (`~/.local/state/taskq/machine-id`, made once: macOS changes the hostname with the network) and the machine's
  name when `[hosts]` or `TASKQ_HOST` gives one; hostname-hash claims from before still count; pre-upgrade claims
  are recognized by local Claude import records or local Codex rollout files.
  Legacy detection reads only matching local filenames and does not need an app server.
  Limits are local scheduling guidance, not a global admission gate or cross-machine
  arbitration. Manual `take` does not enforce them. So a tick on the Mac and a tick on Windows each run
  their own `--limit` at once (csgo #303); the profile line names the machine (`Profile: host=mac; …`).
- A `host-<name>` task is refused on any other machine (`host is win`), by `worker`, `tick`, `list` and
  manual `take` alike: sending work to another machine is a label, never a side effect of where a tick runs.
- Filtered selection keeps a separate unfiltered safety inventory for dependencies and scope
  conflicts. A dependency outside the profile still blocks its task. Profiles never expand
  automatically: an exceptional area goes into the pool with deps, or is taken manually.
- Ask and review are routed only to the assigned user's tick (and must match its area filter).
  Unassigned tasks remain in the shared pool. The owner can see all states on the project board.
- After `answer`, `reject` and `release` the task returns to `ready` together with its branch and
  worktree; `claim` is reset. The next worker from either app gets the full history and continues.
- Exception (#157): the second `release` in a row without an `answer` or `reject` between them (a worker
  that fails the same way each time, by its own `release` or the tick's dead/stalled one) moves the task to
  `ask` instead, with a question naming both reasons. The owner's `answer` returns it to `ready` and starts the count over.
- Exception: the owner answers in the worker's own session. `answer N` from the session whose
  `claim` is on the task moves `ask`→`doing` with the same `claim`, without `ready` and without a new `take`.
  Machine capacity is not checked: the slot was free only during the question; the worker never left the task.
  The worker continues in the same session and delivers `result` itself. The same holds for `reject N` from
  that session (the owner's change request reached the worker in review): `review`→`doing`, the result is
  dropped, the claim stays, so no `tick` sees a `ready` task to start a second worker for (#127). An `answer` from the manager,
  the coordinator or the owner's shell (session does not match `claim`) still leads to `ready`. The `worker`
  brief and the `ask` output state this rule.
- A worker on a `code` or `docs` task creates its own worktree (the brief prints the command, from the
  project's `[workspace]`), commits, rebases on `origin/main` and pushes to `main` under the owner's
  standing permission. `close` checks that the result SHA is in `origin/main`.
- Where task trees live: `.worktrees/taskq-<N>` inside the main checkout (gitignored by `init`), so trees of
  different projects never share one parent folder and their `taskq-<N>` names never collide. Trees made before
  2026-10-07 sit next to the checkout (`../taskq-<N>`): `doctor` names each such tree of this project with
  `cd <main checkout> && mkdir -p .worktrees && git worktree move <tree> .worktrees/taskq-<N>` and never moves
  it (a worker may still run there). `cleanup` finds trees in both places through `git worktree list`; an open
  task keeps its `taskq-<N>` tree and branch wherever the tree is.
- A task on taskq itself works only in a worktree of the editable clone (`.worktrees/taskq-<N>`, the
  command is in the brief from taskq's `[workspace]`), never in the clone's working tree: every session on the
  machine runs that tree, so it stays clean `main`. `tick` warns in one line when it is not.
- Project tools may read `TASKQ_TASK`; empty means work outside a task. The brief's first command after `take`
  is `export TASKQ_TASK=<N> TASKQ_RUNTIME=<runtime>` (load attribution, csgo: heavy runs recorded as `main`).
- Partial result: close the task, file the remainder as a new task.
- A worker runs a long command (build, CI wait, deploy, prepare) in the background and waits for its completion
  notice (Claude: `run_in_background`, the harness wakes the session; Codex: its equivalent), never a sleep loop.
  For what the harness cannot see (CI), one delayed check sized to the real duration, not a loop every 10 s.
  Why (#130): in the csgo manager's 3-day load analysis, 327 `until`/`for … sleep` loops took 4.2 h of LIMIT
  slots; a long foreground turn also blocks session cron (#91). The brief prints this rule in step 4 for every runtime.
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

The board: the Projects v2 project titled `[github] board` (default: the repository name) among the projects
linked to the repository (`repository { projectsV2 }`). A project belongs to the owner, not the repository: an
owner-level project of that title not linked to this repository is another queue's board and is ignored, so two
repositories of one owner never share one. `init` creates it once when the repository has none — owner the
repository owner, user or organization (`createProjectV2` with `ownerId` and `repositoryId`, which links it) — gives its
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
owner's intent (§ States) only when the card changed after the issue's last `labeled` event. The card step is
best effort: a failed one prints a line and the command still succeeds (the label is the queue's state). The
next read of the cards puts a stale card back to its label and adds a missing card, one printed line each.
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
  is created by `taskq spawn --name "T<N> <words>" --text "<worker prompt>"` as tick prints it: a `claude --bg`
  background session of the CLI in the main checkout that starts on the prompt (#270, 2026-10-06: the app
  window does not change; #41: no SendMessage). Remote Control is on (#83; `--no-remote-control` turns it
  off): the tick's "Workers" table links each worker's `https://claude.ai/code/session_…`. The owner watches
  it there (browser, phone), `claude agents` / `claude attach`, or on request in the app:
  `taskq show <id>` stops the background run and imports the session with the link
  `claude://resume?session=<id>`, which is undocumented, may change with an app update and always
  shows the session for a moment (~0.2 s). A finished worker: `close` retires it on its machine; by hand `taskq retire <id>`. Worker sessions
  run without permission prompts: `.claude/settings.local.json` in the main checkout holds the allow list and
  `defaultMode: dontAsk` (the file is not in git), and spawn pins `--permission-mode dontAsk` (#71;
  [taskq-manager](taskq-manager.md) § 1 «Permissions»).
- **Codex:** an automation with the prompt "Run `cd <main checkout> && taskq tick` and follow
  the instructions it prints" (no profile flags: the profile is `taskq.local.toml`). A Codex worker is created by `taskq spawn --runtime codex --text "<prompt>"`
  (the prompt is the thread's first turn); later turns are sent by `taskq codex-send`, state is read by `taskq codex-read`, and after acceptance `close`
  archives it by `taskq codex-archive` ([taskq-manager](taskq-manager.md) § Other machines); every tick pass archives a worker
  thread no open task claims (closed, or answered and continued by a new session); one the Codex app holds (the owner
  viewed it) is asked through the app to archive itself with its `codex_app` tool `set_thread_archived` (#165).
  `codex-read <id> --limit N` shows the last N turns (default 3), events, the current
  operation and the actual sandbox of the last turn. `codex-send` prints `delivered` for a new
  turn with an explicit policy or for a message steered into an active turn. Tick's "Workers" table shows the status and event age
  of `doing` Codex sessions and separately demands intervention when one stopped without result/ask.
  A Codex worker session is interactive: the owner writes to it in the app, and `codex-send` then
  delivers through the app ([taskq-manager](taskq-manager.md) § Shared Codex session).
  Read boundaries, coordinator actions and the limits of live display of an external turn
  in the app are described in [taskq-manager](taskq-manager.md) § 3 One tick pass.
  The socket `~/.codex/app-server-control/app-server-control.sock` belongs to the Codex CLI's app-server daemon;
  the app only starts it. Headless (Linux without the app, #160): `codex login --device-auth && codex app-server daemon start`
  (`codex app-server daemon bootstrap` keeps it across reboots) serves the same socket and protocol, so spawn, codex-send,
  codex-read, codex-archive, the turn policy and the tick's liveness work unchanged; only the app's sidebar
  announcement and app-window delivery are skipped (no app IPC). `doctor --codex` names that command when the
  socket is missing; with `claude = 0` it checks no Claude login, folder trust or permissions.
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
Task trees are found in `.worktrees/taskq-<N>` and in `../taskq-<N>` alike (§ Task flow, «Where task trees live»).
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
| Cleanup: trees, branches | local git | By number of trees | Trees, local and remote branches | `close` runs `[workspace] retire` and `git branch -d taskq-<N>`; `cleanup --apply` removes merged ones; remote ones are a question to the owner | ok |
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
