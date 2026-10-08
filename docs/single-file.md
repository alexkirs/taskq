# Single-file taskq

Design of the rewrite: one file `taskq.py`, stdlib only, python3 >= 3.9. No pipx, no package, no `tomllib`:
the config is `taskq.json` beside the file. Target ~500 lines of Python, a ~250-line contract `taskq.md`
and ~150 lines of tests.

The decisions below were taken by the owner on 2026-10-09. This document records them; it does not reopen them.

## 1. Data model (unchanged)

- A task is an issue on the board.
- Its state is one label `q-<state>`: `ready`, `waiting`, `doing`, `review`, `ask`, `later`.
- The issue description carries one JSON block: `scope`, `deps`, `claim`, `result`.
- Comments are the history: every command that changes a task posts one comment.

No data migration: every open task on the board today is read as-is by the new file.

## 2. Board protocol

Six functions; every command speaks to the board only through them.

| Function | Does |
|---|---|
| `list(state)` | open issues with label `q-<state>` (`None`: all `q-*`) |
| `get(n)` | one issue: title, body, labels, comments |
| `add(title, body, labels)` | new issue, returns its number |
| `update(n, labels=None, body=None)` | replace labels and/or body |
| `comment(n, text)` | append one comment |
| `close(n)` | close the issue |

Implementations in `taskq.py`, ~40 lines each:

- GitHub: `gh api` REST (`repos/{repo}/issues...`).
- GitLab: `glab api` REST (`projects/{id}/issues...`).

Any other board: a file `boards/<name>.py` with the same six module-level functions, named in `taskq.json`
(`"board": "boards/jira.py"`). `taskq.py` loads it with `importlib.util.spec_from_file_location`. An agent
writes such a file by copying the GitHub implementation. There is no command-template DSL in the config.

## 3. Runtime protocol

Four functions per runtime.

| Function | Does |
|---|---|
| `spawn(name, prompt, cwd) -> session` | start a worker session on the prompt |
| `send(session, text)` | deliver one message to the session |
| `alive(session) -> True/False/None` | running / gone / cannot tell |
| `link(session) -> url` | a URL the owner opens to watch the session |

Claude:

- `spawn`: `claude --bg --name T<N>` with the prompt.
- `alive`: `claude agents --json`, find the session.
- `send` / stop: `claude stop` plus a resume with the text, as the CLI allows.
- `link`: the Remote Control URL of the session.

Codex (verified live 2026-10-09: `codex exec`, resume and queueing work headless):

- `spawn`: `subprocess.Popen(['codex', 'exec', '--json', '-C', cwd, prompt])`. The session is the `thread_id`
  read from the first JSONL line. The pid is kept in `.taskq/T<N>.pid`.
- `send`: `codex exec resume <id> <text>`.
- `alive`: the pid in `.taskq/T<N>.pid` is alive.
- `link`: `codex://threads/<id>` through `docs/open.html` (the board cannot link a custom scheme directly).

Not needed and deleted: the app-server WebSocket client, `CodexIpc`, `thread-follower-*`, announce/release,
`project/create`.

Other runtimes: a file `runtimes/<name>.py` with the same four functions, loaded the same way as a board file.

## 4. Publication

Two modes, set in `taskq.json` (`"publish": "direct"` or `"pr"`).

- `direct`: the worker pushes `main` itself. `result` records the pushed SHA.
- `pr`: the worker opens a PR/MR (`gh pr create` / `glab mr create`). `result` records the PR head SHA. The
  manager reviews on the platform. `close` merges: `gh pr merge --squash` / `glab mr merge`.

The current `review` mode is deleted, not kept as an option or plugin: `publish_review`, its temporary detached
worktree, the ff-only merge and the rebase replay all go.

## 5. tick

One function, ~60 lines:

1. Load the open tasks (`list(None)`).
2. `doing` and `alive` is `False`: requeue to `ready` with a comment.
3. `doing`, alive, and no comment for 120 minutes: `send(session, 'continue: read your issue')`.
4. `ask`: nothing; the owner answers.
5. `ready`, every dep closed, a free slot for its runtime: `spawn`.
6. Print the table: Task / State / Runtime / Session link.

No timer: a sender session loops `taskq wait` and messages the manager per event (`taskq arm tick`, #407). There is no coordinator machine. A tick starts only
tasks with no `host-*` label or with its own `host-<name>` label; any machine can tick.

## 6. Windows

- `subprocess` with list arguments, never a shell string.
- `pathlib` for every path.
- `shutil.which` to find `gh`, `glab`, `claude`, `codex`.
- No bash anywhere: not in the code, not in the contract's commands.

## 7. Deleted entirely

- `selftest`.
- `doctor`, `init`, the setup wizard, `migrate`.
- `cleanup` and its schedule.
- Multiproject (`taskq projects`).
- Auto-update and `update`: the contract tells the agent to `git pull`.
- The supervisor role.
- Reservations.
- Legacy compatibility shapes.
- The Projects v2 mirror.
- `codex.py`, the app-server client.
- `docs/*.md` except `open.html` and this design note.

## 8. File layout

`taskq.py`, top to bottom:

| Part | Planned | Final (#290) | Holds |
|---|---|---|---|
| Config + task model | ~60 | 62 | read `taskq.json`; parse/render the JSON block; state labels |
| Board | ~120 | 111 | the protocol, GitHub, GitLab, file loader |
| Runtime | ~100 | 119 | the protocol, Claude, Codex, file loader |
| Commands | ~200 | 207 | `add list take ask answer result close requeue later tick`, argparse |
| Total `taskq.py` | ~500 | 499 | |

Beside it:

- `taskq.md`, the contract: planned ~250 lines, final 249. What a worker and the manager do, in the commands below.
- `tests/test_single.py`: planned ~150 lines, final 320. An in-memory fake Board and fake Runtime; `tick` runs
  without network. CI runs `python3 -m unittest tests.test_single`.

### Commands

| Command | Does |
|---|---|
| `add` | create a task issue: title, goal, acceptance, scope, deps; label `q-ready` (or `q-waiting` with open deps) |
| `list` | print open tasks by state, or one state |
| `take N` | claim a `ready` task: write `claim`, label `q-doing` |
| `ask N --text` | the worker asks the owner: comment, label `q-ask` |
| `answer N --text` | the owner's answer: comment, label back to `q-doing` |
| `result N --sha --text` | hand in: write `result`, label `q-review` |
| `close N` | accept: in `pr` mode merge the PR, then close the issue |
| `requeue N` | drop the claim, label `q-ready` |
| `later N` | park the task: label `q-later` |
| `tick` | one pass of section 5 |

## Old modules and where they landed

All deleted in #290, with `pyproject.toml`, `taskq.toml` (now `taskq.json`) which also replaces the pipx install.

| Old module | Lines | Surviving behavior lands in | Dropped |
|---|---|---|---|
| `taskq/__init__.py` | 906 | config + task model (`parse`, `render`, `data`, labels, `STATES`), argparse `main` | TOML config, profiles/preferences, machine identity beyond the host label, `selftest_env`, JSON output mode, GitLab-shaped API layer |
| `taskq/store_github.py` | 148 | Board: GitHub implementation | GitLab-REST emulation over `gh api` |
| `taskq/worker.py` | 864 | commands `add list take ask answer result close requeue later`; Claude runtime (`claude_spawn`, `claude_agents`, `claude_url`, `claude_stop`, `claude_wake`) | `edit`, `worker` brief, `beat`, `problem`, `report`, `view`, `supervise`, `spawn`/`retire` as commands, `publish_review`, delegation/adopt/fork tracking |
| `taskq/tick.py` | 582 | `tick` (section 5), the table | coordinator, supervisor wake, auto-update, contract news, idle-stop, `retire` |
| `taskq/runtimes/__init__.py` | 33 | Runtime file loader | orchestrator name codes (`CLD`/`CDX`/...) |
| `taskq/runtimes/claude.py` | 40 | Runtime: Claude | |
| `taskq/runtimes/codex.py` | 51 | Runtime: Codex via `codex exec` | app-server calls |
| `taskq/runtimes/dot.py`, `hermes.py` | 9 each | `runtimes/<name>.py` files if still wanted | stubs |
| `taskq/codex.py` | 443 | nothing | all: app-server client, `CodexIpc`, announce/release |
| `taskq/doctor.py` | 532 | nothing | all: `doctor`, `init`, `setup`, `migrate`, `update` |
| `taskq/cleanup.py` | 430 | nothing | all |
| `taskq/selftest.py` | 433 | nothing | all |
| `taskq/multiproject.py` | 113 | nothing | all |
| `taskq/contracts/taskq.md` | 235 | `taskq.md` | |
| `taskq/contracts/taskq-manager.md`, `principles.md` | 323 | the manager part of `taskq.md`, short | the rest |
| `taskq_cli.py`, `taskq/__main__.py` | 18 | `taskq.py` is the entry point | |
| `tests/` | 4173 | ~150 lines: fake Board, fake Runtime, tick | the rest |
| `bench/` | | nothing | all |
| `docs/` | | `open.html` | `cleanup-schedule.md`, `multiproject-pm.md` |

## Open points (not decided here)

- The new command list has no `beat` or `problem`. A worker's progress note and a problem report become a
  plain comment on the issue; for boards beyond GitHub/GitLab the agent has no CLI for that. A `note N --text`
  command (one line over `comment`) would cover it. Owner's call.
