<p align="center">
  <img src="docs/header.webp" alt="Relaxing while the agents work" width="720">
  <br><em>Agents working.</em>
</p>

Your repository board is the task list; your usual AI apps do the work.
Add taskq, then ask Codex or Claude to turn any request into tickets.

A timer picks up tickets, runs as many workers as you choose, and tracks progress.
Taskq collects stats and suggests improvements, with no extra worker apps required.

You stay in control: tasks wait for your input and continue when you reply.

1. **⚙ Set up (once per project)**<br>
   Tell your agent: "Install taskq from https://github.com/alexkirs/taskq and set it up."

2. **▶ Start working (each day)**<br>
   "You are the taskq manager. Arm the tick." - workers start on ready tasks

3. **✎ Talk to the manager**<br>
   "File a task: fix the login redirect." - new task<br>
   "What is in the queue?" - status<br>
   "What should we do next?" - plan<br>
   "Show me the question from #12." - answer a worker<br>
   "Check taskq orchestration." - self-test

**Mix agents, task by task.** Codex makes the visuals, Claude writes the code, another agent runs the tests - in whatever order your work needs.
Choose each task's agent with `--runtime` (a `run-*` label); use `--deps` to chain tasks.

<details>
<summary>Setup, commands and reference</summary>

## Setup

Agent: the sentences above start the manager's onboarding
([first use](taskq/contracts/taskq-manager.md#1-first-use-and-check-the-place)); do not run the
reference commands below straight through.

1. Check read-only: `git remote get-url origin`, `taskq --help` (if missing: `python3 --version`, `pipx --version`).
   With taskq installed: `taskq contract`, read both contracts, then `taskq doctor`.
2. Nothing missing: say so, show the profile card, stop.
3. Otherwise ask once: “Setup needs: [missing items]. Choose ‘instructions’ for steps you run,
   or ‘do it for me’ for setup I run. Neither starts workers or a timer.”
   - **instructions**: print the missing steps, numbered, one command each; check each after the person's ‘done’.
   - **do it for me**: ask once for setup authority (the contract's wording), then install what is missing
     and run `taskq doctor --fix`. Its `you:` lines are the person's: show each command, wait for ‘done’, rerun.
4. Setup never starts workers, arms a tick or runs `selftest`; it ends with the contract's summary line.

Requirements: Python 3.11+, [pipx](https://pipx.pypa.io/stable/installation/),
and [glab](https://gitlab.com/gitlab-org/cli) (GitLab) or [gh](https://cli.github.com) (GitHub).
Reference commands (placeholders from `git remote get-url origin`):

```bash
# If taskq is not installed:
pipx install git+https://github.com/alexkirs/taskq
taskq --help
```

GitLab:

```bash
glab auth status --hostname <host>
# If not authenticated:
glab auth login --hostname <host>
taskq init --project <group/project> --host <host>
```

GitHub:

```bash
gh auth status --hostname <host>
# If not authenticated:
gh auth login --hostname <host>
```

```bash
taskq init --github <owner/repo> --host <host>
```

Keep existing config. The board is the repository's issue list filtered by the `q-*` labels: init makes the
labels and prints its link.
GitHub needs a pushed commit before claiming tasks. For an empty repository:

```bash
git commit --allow-empty -m "Initialize repository"
git push -u origin HEAD
```

Expected: `taskq.toml`, labels and a board with one column per state.
GitLab prints its ID; GitHub prints its URL. `labels only`: obtain `project` scope; rerun init.
Credentials stay in the host CLI.
Codex workers need no `taskq.toml` key: the Codex app project is found by the checkout path, created if missing.
Without the Codex app (headless Linux): `codex login --device-auth && codex app-server daemon start` serves the same app server; `taskq doctor --codex` checks it.

## Product Manager

Tell your agent: **“You are the Product Manager of taskq.”**

Agent: run `taskq contract`; read both printed contracts. Follow the manager's
[first-use script](taskq/contracts/taskq-manager.md#1-first-use-and-check-the-place):
run the read-only `taskq doctor`, offer **instructions** (its steps) or **do it for me**
(`taskq doctor --fix` after one confirmation),
and re-check each completed step. Setup does not arm the tick.

## Check it works

```bash
taskq list
taskq selftest --scope quick
```

Expected: queue contents, then passing rows for
`add → take → beat → ask → answer → result → close`. Quick starts no sessions.
Test tasks are deleted (closed when deletion is not permitted).

Tell the manager: **“Check full orchestration with Claude and Codex.”**

```bash
taskq selftest --scope full --runtime claude codex
```

Expected: workers test claims, questions, answers, rejection and release; report verifies stored state.
Read the manager contract for runtime setup and cleanup.
Failures exit 1; dependent checks are skipped.

## Try one task

```bash
taskq add --title "Try taskq" --type research --runtime any \
  --goal "Reply: taskq works. No file changes." \
  --acceptance "The result says: taskq works."
# Replace N with the task number printed by add:
taskq take N
taskq result N --checks "Reply matches acceptance" --text "taskq works."
taskq close N --text "Checked the reply against acceptance."
```

Expected: `ready → doing → review → closed`; the board card disappears on close.
Code/docs results require `--sha` of a commit pushed to `main`.

## Start workers

Configure one sender for the project to run `taskq tick`. Each pass does the mechanical steps itself
(spawn, nudge, wake, retire) and prints the R6 report: heading, Board link, one table Task | Status |
Runtime | Session, then what needs judgement and the owner's open questions. It exits 1 for judgement.

For CLI automation, `list`, `view`, `tick`, `spawn`, `report` and `cleanup` accept `--json`. Stdout is one JSON object with
`outcome` (`ok`, `judgement_needed`, `failure`), `tasks` and the captured prose in `text`; `report` and
`cleanup` also carry `actions` and `refusals`, and cleanup its `plan` (remove/ask/keep) and, with `--apply`,
actual removals and freed bytes. JSON does not make a command read-only. `tick --json` exits 1 for judgement
and 2 for failure; other JSON commands exit 0 for a completed report/plan and 2 for failure.
`spawn --json` records one action with `runtime`, `session` and `attempt` (the task's `launch` note id).
`taskq add --reply channel:chat[:thread]` stores where a request came from; the tick report groups its rows by it.
`[prefs] report = "cards"` in `taskq.local.toml` prints one block per task (Task, Status, Runtime, Session) instead of
the table, for chats without tables. Agent skill for any runtime: `taskq contract --skill`.


Your profile lives in `taskq.local.toml` of the main checkout: personal, never committed
(`taskq init` adds it to `.gitignore`). Workers make their trees in `.worktrees/taskq-<N>` of the same
checkout (also gitignored).
`taskq profile init` writes the profile once:

| Profile | `taskq profile init` arguments |
|---|---|
| All areas, your tasks plus shared pool | `--no-mine` |
| Only your assigned tasks | `--mine` |
| One area, your tasks plus its shared pool | `--filter "labels=area-maps"` |

For maps, save in `taskq.toml`; run init:

```toml
[areas]
names = ["maps"]
```
Automatic selection excludes other people's assignments.

```bash
taskq profile init --filter "labels=area-maps" --mine --limit claude=1,codex=2
```

```toml
# taskq.local.toml
[profile]
filter = "labels=area-maps"
mine = true
preferred_runtime = "codex"  # optional: tie-break for your own tasks of any runtime

[profile.limits]
claude = 1
codex = 2
```

Expected: only your maps tasks; at most 1 Claude / 2 Codex workers on this machine.
Defaults: all areas, own tasks plus pool, Claude 2 / Codex 3. Every tick prints the profile and
where each value came from. A flag (`--filter`, `--mine`/`--no-mine`, `--limit`) overrides the
file for that one run.

Two machines each run their own tick and limits. A task for one machine only: `taskq add … --host win`
(label `host-win`). Name machines in taskq.toml or with `TASKQ_HOST=win`, and name the one coordinator: its tick
starts shared work, reviews and closes; another machine's tick starts only its `host-*` tasks and says
`coordinator is mac`. Without `[coordinator]` every tick coordinates. To move it, edit the line:

```toml
[hosts]
"DESKTOP-7" = "win"
"macbook-m2.local" = "mac"

[coordinator]
machine = "mac"
```

**Windows.** Keep the checkout, taskq, git and gh/glab in WSL; the Windows Claude Code (`claude.cmd`) runs the
workers and sees the checkout as `//wsl.localhost/<distro>/…`: `taskq doctor` checks its folder trust and login under
that path. The desktop app is optional (watch workers through Remote Control); macOS-only steps say so and are skipped. Tell workers what is special about a machine in its `taskq.local.toml`; every brief there
prints it, with the checkout root (task text uses repository-relative paths, `add` warns on absolute ones):

```toml
[machine]
notes = "Windows claude.cmd; checkout in WSL; run git and tests via wsl.exe bash -lc; no Codex"
```

Worker sessions are named `T<N> … (mac)`, their supervisors `S<N> … (mac)`; spawned Claude sessions run with Remote Control, so `tick` links each
one at `https://claude.ai/code/session_…` (`taskq spawn --no-remote-control` turns it off). `taskq list --links`
adds each task's URL.

`taskq view N` prints a task read only: state, claim, last notes, result.

## Talk to the manager

> **You:** Add a task to fix login; assign it to me.<br>
> **Manager:** Records goal, acceptance, scope, runtime, assignee; returns the task link.<br>
> **You:** Arm the tick for only my tasks.<br>
> **Manager:** After your profile confirmation, starts the tick and workers.<br>
> **Worker:** Asks which login behavior you want.<br>
> **Manager:** Relays it; records your answer with `taskq answer N`.<br>
> **Manager:** Checks commit and acceptance; closes or rejects with exact fixes.

## Publication before or after review

`[workspace] publish = "direct"` (default) or `"review"`: [queue contract](taskq/contracts/taskq.md#publication-before-or-after-review).

## Supported systems and reference

GitLab Issues (including self-managed) and GitHub Issues (including Enterprise) work today.
Other systems: on request. GitHub queues work without a board.

- [Principles R1–R12](taskq/contracts/principles.md): the rules every other document follows
- [Queue, configuration and states](taskq/contracts/taskq.md)
- [Manager, tick and runtime setup](taskq/contracts/taskq-manager.md)
- Docs: https://github.com/alexkirs/taskq/wiki
- `taskq --help`: commands. `taskq update`: update now; automatic updates every 24 hours.

## Develop taskq

An editable install runs the clone's working tree: every `taskq` call on the machine, every tick and worker, runs
whatever is in it right now. So the clone's own tree stays clean `main`, and every change happens in a worktree of it:

```bash
cd ~/Projects/taskq
git fetch origin && git worktree add -b <branch> .worktrees/<branch> origin/main
```

Modules: `taskq/__init__.py` the core (config, store protocol, issue parse/save, the command line),
`worker.py` the task commands (`add` … `take`, `beat`, `ask`, `result`, `answer`/`reject`/`release`, `close`) and worker
sessions, `tick.py` the `tick` command (its steps and the R6 report), `doctor.py` `doctor`, `init` and
`update`, `store_github.py` the GitHub store, `codex.py` the Codex app server, `cleanup.py` the `cleanup` command,
`selftest.py` the `selftest` command. Each module reaches the core as `core.<name>`; the core re-exports what it moved,
so `taskq.<name>` still works, and a test patches a moved function in its own module where that module calls it.

Change the worktree, run the tests there, and submit through the project's publication/review flow.
CI (`.github/workflows/tests.yml`) runs tests on every push to main or a `taskq-*` review branch,
and every PR. The clone and other machines take the commit once its exact-SHA tests passed,
within `[update] every` or at once with `taskq update`.

An update needs every `tests.yml` run of the exact SHA completed with success
(`gh run list --commit <sha> --workflow tests.yml`); none, running or failed refuses it. Pages
(`.github/workflows/pages.yml`) deploys `docs/` on every push to `main` and does not gate updates.
A commit that does not start (`python3 -m taskq --version`) is rolled back in the clone.

`taskq update` follows only CI-green `main`. It leaves a clone with uncommitted changes alone and says so; `tick` prints
a one-line warning while the clone is dirty or off `main`. If the package cannot be imported at all, the command prints
`taskq is broken at <path>: <error>; run git -C <path> status` instead of a traceback. An editable install made
before this wrapper (`taskq_cli`) existed picks it up after one `pipx install --force -e ~/Projects/taskq`.

```bash
python3 -m unittest discover -s tests
```

License: [MIT](LICENSE).

</details>

If taskq saves you time, [buy me a coffee](https://alex.kirs.online/donate).
