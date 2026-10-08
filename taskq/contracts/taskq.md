---
type: Contract
status: Active
domain: agent-workflow
canonical: true
---

# taskq — the project task queue

`taskq` is a task queue on GitLab or GitHub issues for Claude and Codex sessions; one command, the same from both.
Rules: [principles.md](principles.md) (R1–R12). Guides: [Wiki](https://github.com/alexkirs/taskq/wiki).
This file keeps only the commands and the data they read and write. History is in the issues.

## Publication before or after review

| `[workspace] publish` (shared `taskq.toml` only) | Worker pushes | `close` |
|---|---|---|
| `direct` (default) | `main` | checks the result SHA is in `origin/main` |
| `review` | only `taskq-<N>` (`--force-with-lease` after a rebase) | requires the branch head to equal the result SHA, then `git merge --ff-only` and a non-force push to `main`; refuses and returns the task when it cannot |

Review mode is workflow, not a security boundary: use protected branches for that.

## Where things are stored

The board is the only state ([R1](principles.md)).

| What | Where |
|---|---|
| State | Exactly one `q-*` label (§ States); a closed issue is done |
| Epic | Milestone, flat: `add --milestone`, `edit N --milestone` |
| Dependencies | `deps` in the block; GitLab also gets `relates_to` links (`add`, `edit --deps`, `init`) |
| Area | `area-*` labels from `[areas] names`: `add --area maps` |
| Assignee | Tracker user; empty is the shared pool; `add --mine`; `take` assigns the current user |
| Runtime | `run-claude` or `run-codex`; none is `any` |
| Codex full access | Label `codex-full-access`: the task's Codex turns run with `danger-full-access` (GPU tasks only, [Known issues](https://github.com/alexkirs/taskq/wiki/Known-issues)) |
| Machine | `host-<name>`: `add --host win`; name from `TASKQ_HOST`, else `[hosts]`, else the hostname up to the first dot |
| Type, priority | `code`, `docs`, `research`, `asset`; `priority-1`, `priority-2` |
| Block | JSON in the description: `scope`, `deps`, `claim`, `waiting_for`, `result`, `supervisor` (`{"runtime", "session"}`) |
| Goal, acceptance | Description text |
| History | Notes `**action** · app:session` (`take`, `beat`, `ask`, `shown`, `result`, `answer`, `reject`, `release`, `close`, `later`, `waiting`, `ready`, `deps`, `runtime`, `edit`, `launch`, `problem`) |
| Who takes | The earliest trusted `take` note since the task last became ready (§ Taking a task) |
| Problem without a task | Issue with label `problem` |

## Project: taskq.toml

At the repository root; found from the current directory upward. No secrets. Personal overrides:
`taskq.local.toml` of the main checkout (§ Shared and personal configuration). An unknown key fails.

| Key | What | Default |
|---|---|---|
| `[gitlab] project` / `[github] repo` | Tracker project; exactly one | — |
| `[gitlab] host`, `[github] host` | Self-managed or Enterprise host | the CLI's |
| `[gitlab] board` | Board name | `taskq` |
| `[areas] names` | Work areas; `init` makes the `area-*` labels | none |
| `[codex] writable` | Extra writable roots of Codex turns, relative to the main checkout or `~/` | none |
| `[workspace] new`, `continue`, `none` | Brief text for the workspace; `{iid}` is the task number | `git worktree add -b taskq-<N> .worktrees/taskq-<N> origin/main` |
| `[workspace] publish` | `direct` or `review` (§ Publication) | `direct` |
| `[workspace] retire` | What `close` runs to remove the task's tree | `git worktree remove .worktrees/taskq-<N>` |
| `[workspace] protected_refs` | Refs cleanup never suggests deleting | `[]`; `main` and the current branch always |
| `[update] auto`, `every` | `tick` updates taskq from CI-green `main` (`30m`, `24h`, `7d`) | `true` for the owner's repositories, else `false`; `24h` |
| `[profile]`, `[profile.limits]` | Team defaults below the personal file | none |
| `[hosts]` | Hostname → machine name (`"DESKTOP-7" = "win"`) | hostname up to the first dot |
| `[coordinator] machine` | The one machine whose tick spawns shared work and closes; others start only their `host-*` tasks | none: every tick coordinates |
| `[brief] rules` | Project rules in step 6 of every brief | none |
| `[pages] base` | Base URL of the Codex link page (a fork's Pages) | `https://alexkirs.github.io/taskq/` |

New project: `taskq init --project group/project` or `taskq init --github owner/repo` (§ Schema).

### Shared and personal configuration

`<main checkout>/taskq.local.toml`, never committed (`init` adds it and `/.worktrees/` to `.gitignore`).
Precedence: CLI flag > personal > shared > default.

```toml
[profile]
filter = "labels=area-maps" # tracker issue query; empty means all
mine = true                 # false: own assignments plus the unassigned pool
preferred_runtime = "codex" # tie-break for own `any` tasks only

[profile.limits]            # this machine's slots; 0 disables a runtime; default claude 2, codex 3
claude = 1
codex = 2

[codex]                     # optional: Codex app project and sidebar section
# project = "app-project-id"
# section = "app-section-id"

[machine]
notes = "run git and tests via wsl.exe bash -lc; no Codex"  # printed in every brief here

[idle]
stop = 5        # empty ticks in a row before the idle stop; 0 = never
cleanup = true  # cleanup --apply at the idle stop

[cleanup]
enabled = true  # hourly cleanup from the tick (§ Cleanup)

[projects]      # R10 list, written by `taskq projects --set`
```

| Command | What |
|---|---|
| `taskq profile init [--filter …] [--mine \| --no-mine] [--limit claude=N,codex=M] [--preferred-runtime R]` | Write the personal file once; refuses to overwrite |
| `taskq pref add "<wish>"`, `pref list`, `pref rm N` | Free-text owner wishes printed in every brief; no effect on selection |
| `--filter`, `--mine` / `--no-mine`, `--limit` on `tick` / `worker` | Override for one run |

Every `tick` and `worker` prints the effective profile and each key's source. Onboarding:
[taskq-manager.md § 1](taskq-manager.md#1-first-use-and-check-the-place).

## States

| Label | Meaning | Set by |
|---|---|---|
| `q-ready` | Can start | `add`, `answer`, `reject`, `release`, `tick` |
| `q-waiting` | An open issue in `deps` | `tick` only |
| `q-doing` | A worker holds it | `take`; `answer` / `reject` from the claiming session |
| `q-review` | Result handed in | `result` |
| `q-ask` | Owner's move: a question with options | `ask`; the second `release` in a row |
| `q-later` | Deferred by the owner | `later` |

Board: GitLab `[gitlab] board` (columns per state; moving a card changes the label); GitHub the open issues
filtered by the `q-*` labels. Never move `ready`↔`waiting` by hand: the tick moves it back by `deps`.

## Epics and subtasks

An epic is a milestone; every open task has one. One task per issue ([R2](principles.md)); split work is
several tasks linked by `deps`. No tracker subtasks.

## Four roles

Roles: [R2–R3](principles.md). Commands:

| Role | Prompt or command |
|---|---|
| Manager (root PM) | `add`, `edit`, `answer`, `later`, `list`, `view`; never closes a supervised task |
| Coordinator (tick) | `taskq tick` (§ 3 of [taskq-manager.md](taskq-manager.md#3-one-tick-pass)); spawns `S<N> <title>` with `taskq supervise N` |
| Supervisor | `taskq supervise N`: launches its worker once, resumes the same worker after `answer` / `reject`, reviews the exact SHA, then `close` or `reject` |
| Worker | "Run `cd <main checkout> && taskq worker --task N` and follow the instructions it prints" |

- `taskq edit N --supervisor RUNTIME:SESSION` (`''` clears) from the owner's shell; the current supervisor may
  only name its successor. Cooperative, not proof of identity.
- `take` of a supervised task accepts only the worker named by the supervisor's newest `launch` note.
- One slot of a runtime holds a task's supervisor and its worker. The worker runs on its supervisor's runtime.
- A session holding a `doing` claim gets that task's brief again from `taskq worker`.

## Task flow

```
add → ready ⇄ waiting                       tick only, by deps
      ready → take → doing → result → review → close
                     doing → ask → answer → ready     answer from another session
                     doing → ask → answer → doing     answer from the claiming session
ready/waiting/later → ask (manager) → answer → ready
 ready/waiting/ask → later → answer → ready
                                       review → reject → ready (doing from the claiming session)
```

| Command | What |
|---|---|
| `taskq worker [--task N]` | Brief: task, workspace, history, delivery commands |
| `taskq take N` | Claim (§ Taking a task); checks deps, scope, runtime, host |
| `taskq beat N` | Liveness note; replaces the previous `beat` if it was the last note |
| `taskq ask N --text …` | Question to the owner |
| `taskq result N --sha SHA --checks … --text …` | Hand in; `--sha` required for `code` and `docs` |
| `taskq answer N --text …` | Owner's answer |
| `taskq reject N --text …`, `release N --text …` | Back to the queue with the history |
| `taskq close N --text …` | Accept, publish, retire the worker (§ Publication) |
| `taskq later N --text …` | Defer |
| `taskq runtime N claude\|codex\|any` | Change runtime in `ready`, `waiting`, `ask`, `later` |
| `taskq problem [--task N] --text …` | What cost time or went wrong |

- `--text-file PATH` replaces `--text` (strict UTF-8, verbatim).
- Selection: dependencies closed, runtime matches, `scope` does not overlap a started task, `host-*` matches,
  profile filter and `mine` (never another user's assignment). Limits are per machine, not a global gate.
- The second `release` in a row without `answer` or `reject` moves the task to `ask`.
- A `doing` task without an issue change for `STALE_MINUTES` (120) goes back to the queue on the next tick.
- Task trees: `.worktrees/taskq-<N>` of the main checkout. Work on taskq itself only there, never in the
  editable clone's tree.
- `export TASKQ_TASK=<N> TASKQ_RUNTIME=<runtime>` attributes project tool runs to the task.
- Long commands run in the background; no sleep loops (the brief's step 4).
- Partial result: close the task, file the rest as a new one.

## Taking a task

`take N` posts a `take` note, then reads the notes. The earliest trusted `take` since the newest `ready`,
`answer`, `reject` or `release` note wins; a `take` older than `TAKE_SECONDS` (120) before it is void. The loser
deletes its note: `another worker took it first`. Then one write sets `q-doing` and `claim`. With `scope`, take
rereads the queue; of two overlapping claims the later `q-doing` gives way: `scope overlaps #N`.
`take` again by the claiming session prints `#N is yours`. Several users: each uses their own `gh` / `glab`.

GitHub store: issue lists through GraphQL (REST lags new issues), the rest through REST; no issue links; deletion
through GraphQL `deleteIssue` (admin).

## Scheduled runs

| Runtime | Start | Steer | Read | Retire |
|---|---|---|---|---|
| Claude | `taskq spawn --name "T<N> <words>" --text "<prompt>"`: a `claude --bg` session in the main checkout, Remote Control on (`--no-remote-control` off) | `SendMessage`; `claude --bg --resume <id> "<text>"` | `claude agents`, `claude attach <id>` | `close`; `taskq retire <id>` |
| Codex | `taskq spawn --runtime codex --text "<prompt>"` on the app server socket | `taskq codex-send <id> --text …` | `taskq codex-read <id> [--limit N]` | `close`; `taskq codex-archive <id>` |

- Add without `--runtime`: `asset` → `codex`, the rest → `claude`; `--runtime any` leaves it to anyone.
- Codex turns: approval `never`, `workspace-write` with network, roots from `codex_turn_policy()`.
- Codex without the desktop app: `codex login --device-auth && codex app-server daemon start`.
- Per-machine setup: [Required settings](https://github.com/alexkirs/taskq/wiki/Required-settings);
  what doctor cannot fix: [Known issues](https://github.com/alexkirs/taskq/wiki/Known-issues).

## Cleanup

| Command | What |
|---|---|
| `taskq cleanup [--json]` | From the main checkout on `main`: plan "Remove / Ask the owner / Kept" for task trees, branches and `T<N>` / `S<N>` sessions; changes nothing |
| `taskq cleanup --apply` | Removes only proven-finished "Remove" items, rechecking each; never remote branches |

The tick runs `cleanup --apply` at most hourly (mtime of `.local/taskq-cleanup-last`) and at the idle stop;
`[cleanup] enabled = false` turns both off. Rules: [R11](principles.md);
[docs/cleanup-schedule.md](../../docs/cleanup-schedule.md); procedure:
[taskq-manager.md § Cleaning up finished work](taskq-manager.md#cleaning-up-finished-work).

## History and report

| Command | What |
|---|---|
| `taskq view N` | State, claim, worker liveness, last notes, result |
| `taskq list [--links]` | The queue |
| `taskq report --hours H [--json]` | Time between steps per task, sessions, problems |

## Schema and migration

`taskq init` is idempotent: labels (state, runtime, type, priority, areas), the GitLab board with one column per
state, removal of labels of former states no issue carries, `relates_to` links by `deps`.

## Verification

`python3 -m unittest discover -s tests` (fake trackers, no live sessions). Live: `taskq selftest --scope quick`
([taskq-manager.md § Checking the orchestration](taskq-manager.md#checking-the-orchestration-selftest)).
