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

- There is no board: a column is the issues list filtered by its `q-*` label, e.g.
  `https://github.com/<owner>/<repo>/issues?q=is:open+label:q-ready`. `init` prints so.
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

Change the clone, run the tests, push to `main`; other machines take it within `[update] every`, or at once with `taskq update`.

```bash
python3 -m unittest discover -s tests
TASKQ_CLEANUP_HELPERS=<project>/scripts python3 -m unittest discover -s tests   # also the cleanup tests
```

## License

MIT, see [LICENSE](LICENSE).
