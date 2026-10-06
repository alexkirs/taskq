<p align="center">
  <img src="docs/header.webp" alt="Relaxing while the agents work" width="720">
  <br><em>Agents working.</em>
</p>

# taskq

A task queue for AI coding sessions that lives in GitLab or GitHub issues. Claude Code and Codex sessions use the
same command, `taskq`, from any machine: a manager session files tasks, a coordinator session runs a
5-minute tick that starts one worker session per task, and workers claim, report and hand in their work.
Nothing is stored locally: the issue labels are the task state, one JSON block in the issue description
holds the rest, and the issue notes are the history.

Requirements: Python 3.11+ (standard library only) and the host's CLI logged in: [`glab`](https://gitlab.com/gitlab-org/cli)
for GitLab (gitlab.com or self-managed), [`gh`](https://cli.github.com) for GitHub.

## Install

```bash
pipx install git+https://github.com/alexkirs/taskq
```

To work on taskq itself, install an editable clone instead (`uv tool install -e` works the same way):

```bash
git clone https://github.com/alexkirs/taskq ~/Projects/taskq
pipx install -e ~/Projects/taskq
```

taskq updates itself once a day; `taskq update` updates by hand (`--verbose` says why a check was skipped); `[update] auto = false` turns it off.
The tick checks `main` of this repository at most every `[update] every`: an editable clone is fast-forwarded
(left alone, with the reason, when it has uncommitted changes or commits `main` lacks), an install from Git is
reinstalled, and the pass goes on as the new version. `tick` prints the version it runs. Versions are not pinned.

## Set up a project

```bash
cd <your project checkout>
glab auth login --hostname <gitlab host>
taskq init --project <group>/<project> --host <gitlab host>
git add taskq.toml && git commit -m "taskq: queue config"
```

`taskq init` writes a minimal `taskq.toml` when there is none, then creates the labels (`q-*` states,
`run-*` runtimes, types, `priority-*`, `problem`, configured `area-*`) and a board `taskq` with one column per state. Running
it again changes nothing. Each person uses their own account. Automatic selection never takes
another person's assigned task; unassigned tasks form the shared pool.

On GitHub the queue is the repository's issues (`taskq init --github <owner>/<repo>`, `gh auth login` first):

```toml
[github]
repo = "owner/repo"           # instead of [gitlab]
host = "github.example.com"   # optional: GitHub Enterprise
```

- The board is a Projects v2 project `taskq` (`[github] board` renames it) owned by the repository owner and
  linked to the repository, with the field Status: one column per state. `init` creates it once and prints its
  URL; `tick` prints it too. It needs the token scope `project`: `gh auth refresh -h github.com -s project`.
  Without it `init` names that command and makes the labels only; everything else works without a board.
- The `q-*` label stays the task's state; the board follows it: `add` puts the card in `ready`, every state
  change moves the card in the same command, `close` archives it. `init` deletes the project's own workflows
  (they would close an issue whose card reaches a column and move cards on their own).
- Moving a card is a request to the queue, executed by the next `tick` with the note «moved on the board»:
  `ready`/`waiting` → `later` defers, `later` → `ready` restores, `review` → `ready` rejects. Any other move
  (from `doing`, from `ask` — a question needs an answer) goes back to the label's column and is named under
  «Board mismatch» with the command that does it.
- The task lock is the ref `refs/taskq/lock/<N>` (not a branch: no CI runs, nothing in the UI). Creating
  it twice is a 422 for any user, so the lock is atomic between people with their own accounts.
- `--filter` is GitHub's list-issues query: `labels=area-maps`, `assignee=<login>`, `milestone=<number>`.
- Dependencies are the `deps` field of the task block only; GitHub has no issue links.
- `selftest` deletes its issues only when the token may (`deleteIssue` needs admin); otherwise it closes them.

## taskq.toml

All project-specific settings live here; `taskq` finds the file from the current directory upward, so
commands work from the main checkout and from any worktree. It holds no secrets: the token stays with `glab`.

```toml
[gitlab]
project = "group/project"     # required
host = "gitlab.example.com"   # optional: else glab picks the host from the git remote
board = "taskq"

[areas]                       # your project's work areas, labels created by init
names = ["maps", "engine"]

[codex]                       # needed only for `taskq spawn --runtime codex`
project = "<Codex app project id>"
section = "<Codex app sidebar section id>"

[workspace]                   # worker brief texts; {iid} is the task number
new = "from the main checkout run `git worktree add -b taskq-{iid} ../taskq-{iid} origin/main` and work only there."
continue = "this task was started before in worktree `taskq-{iid}`; continue there."
none = "this task ends in an answer, not a commit: work from the main checkout."
retire = "git worktree remove ../taskq-{iid}"   # printed after `close`
cleanup_helpers = "scripts"   # folder with workspace_gc.py, host_gentle.py, host_tools.py for `taskq cleanup`

[update]                      # written with these defaults when missing
auto = true                   # tick updates taskq from GitHub
every = "24h"                 # at most this often (m, h, d)

[brief]
rules = """
Project rules appended to every worker brief.
"""
```

## Task states

| Label | Meaning |
|---|---|
| `q-ready` | Can start when its runtime has room and its scope is free |
| `q-waiting` | Has an open dependency; only `tick` moves it between ready and waiting |
| `q-doing` | A worker holds it |
| `q-review` | Handed in, waiting for acceptance |
| `q-ask` | A question for the owner |
| `q-later` | Deferred by the owner |

```
add → ready ⇄ waiting → take → doing → result → review → close
                         doing → ask → answer → ready (or doing, when answered in the worker's session)
                                        review → reject → ready
```

## Use from sessions

- **Manager** (the session the owner talks to): `taskq add`, `taskq list`, `taskq answer`, `taskq later`.
- **Coordinator**: every 5 minutes runs `taskq tick` and follows what it prints: accept or
  reject results, start workers (`taskq spawn` for a Claude CLI background session, reached with
  `SendMessage`; `taskq spawn --runtime codex` plus `taskq codex-send` for the Codex app), pass
  questions to the owner. `taskq show <id>` opens a Claude worker in the desktop app on request;
  `taskq retire <id>` ends a finished one.
- **Worker**: its only prompt is ``Run `cd <main checkout> && taskq worker` and follow the instructions it
  prints.`` The brief names the task, the workspace, the history and the exact commands: `take`, `beat`,
  `problem`, `ask`, `result`.

`taskq contract` prints the paths of the full contracts: `taskq.md` (the queue) and `taskq-manager.md`
(the manager and coordinator session). `taskq --help` lists every command.

## Personal tick

Tell the manager what you work on and what you exclude. It shows one confirmation card with the
profile command, local session limits and the board link; after your “ok” it arms that profile.

```bash
taskq tick --filter "labels=area-maps" --mine --limit claude=1,codex=2
taskq worker --filter "labels=area-maps" --mine --limit claude=1,codex=2
taskq add --title "…" --goal "…" --acceptance "…" --type code --area maps --mine
```

No flags: your tasks plus the shared pool in every area, two Claude and three Codex sessions on
this machine. `--mine` excludes the pool. Manual `take N` assigns any ready task to you.
Limits live only in the prompt; the global `[limit]` setting is retired.

## Develop

An editable install runs the clone's working tree: every `taskq` call on the machine, every tick and worker, runs
whatever is in it right now. So the clone's own tree stays clean `main`, and every change happens in a worktree of it:

```bash
cd ~/Projects/taskq
git fetch origin && git worktree add -b <branch> .worktrees/<branch> origin/main
```

Change the worktree, run the tests there, push to `main`; the clone and other machines take it within `[update] every`,
or at once with `taskq update`. `taskq update` leaves a clone with uncommitted changes alone and says so; `tick` prints
a one-line warning while the clone is dirty or off `main`. If the package cannot be imported at all, the command prints
`taskq is broken at <path>: <error>; run git -C <path> status` instead of a traceback. An editable install made
before this wrapper (`taskq_cli`) existed picks it up after one `pipx install --force -e ~/Projects/taskq`.

```bash
python3 -m unittest discover -s tests
TASKQ_CLEANUP_HELPERS=<project>/scripts python3 -m unittest discover -s tests   # also the cleanup tests
```

## License

MIT, see [LICENSE](LICENSE).
