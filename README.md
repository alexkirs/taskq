<p align="center">
  <img src="docs/header.webp" alt="Relaxing while the agents work" width="720">
  <br><em>Agents working.</em>
</p>

# taskq

A shared task queue for Claude Code and Codex.

Tell your agent: **“Install taskq from https://github.com/alexkirs/taskq in this project and set it up following the onboarding in its README and taskq contract.”**

| Situation | What to say |
|---|---|
| Join a project with taskq.toml; run onboarding | “Join this taskq project and run onboarding.” |
| Repeat onboarding; choose instructions or setup by the agent | “Run taskq onboarding.” |
| Start coordination in this session | “Arm the tick.” |
| Discuss the next work | “You are the Product Manager of taskq. What should we do next?” |
| File work | “File a task for fixing login.” |
| Read the queue | “You are the Product Manager of taskq. What is in the queue?” |
| Check orchestration | “Check taskq orchestration.” |
| Watch a worker | “Show worker session <session id>.” |

<details>
<summary>Setup, commands and reference</summary>

## Setup

Agent: read the [manager onboarding](taskq/contracts/taskq-manager.md#1-first-use-and-check-the-place)
first; inspect readiness and offer instructions or setup by the agent before making changes.
Once installed, `taskq contract` locates both contracts. Resolve placeholders from
`git remote get-url origin`; run the matching commands only under the chosen setup mode.
Requirements: Python 3.11+, [pipx](https://pipx.pypa.io/stable/installation/),
and [glab](https://gitlab.com/gitlab-org/cli) (GitLab) or [gh](https://cli.github.com) (GitHub).
If login needs the person, report it and resume after login.

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
# If project scope is missing:
gh auth refresh -h <host> -s project
```

```bash
taskq init --github <owner/repo> --host <host>
```

Keep existing config. The board is the Projects v2 project linked to the repository and named after it
(`[github] board` overrides the name); init creates it once, manages Status and disables its built-in workflows.
GitHub needs a pushed commit before claiming tasks. For an empty repository:

```bash
git commit --allow-empty -m "Initialize repository"
git push -u origin HEAD
```

Expected: `taskq.toml`, labels and a board with one column per state.
GitLab prints its ID; GitHub prints its URL. `labels only`: obtain `project` scope; rerun init.
Credentials stay in the host CLI.
Codex workers need no `taskq.toml` key: the Codex app project is found by the checkout path, created if missing.

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

Tell the manager your profile, then **“Arm the tick.”** Confirm its profile card.
Expected: the coordinator follows tick instructions every 5 minutes.

| Profile | Arguments for both `tick` and `worker` |
|---|---|
| All areas, your tasks plus shared pool | none |
| Only your assigned tasks | `--mine` |
| One area, your tasks plus its shared pool | `--filter "labels=area-maps"` |

For maps, save in `taskq.toml`; run init:

```toml
[areas]
names = ["maps"]
```
Automatic selection excludes other people's assignments.

```bash
taskq tick --filter "labels=area-maps" --mine --limit claude=1,codex=2
taskq worker --filter "labels=area-maps" --mine --limit claude=1,codex=2
```

Expected: only your maps tasks; at most 1 Claude / 2 Codex workers on this machine.
Defaults: Claude 2 / Codex 3. Profiles live in session prompts.

## Talk to the manager

> **You:** Add a task to fix login; assign it to me.<br>
> **Manager:** Records goal, acceptance, scope, runtime, assignee; returns the task link.<br>
> **You:** Arm the tick for only my tasks.<br>
> **Manager:** After your profile confirmation, starts the tick and workers.<br>
> **Worker:** Asks which login behavior you want.<br>
> **Manager:** Relays it; records your answer with `taskq answer N`.<br>
> **Manager:** Checks commit and acceptance; closes or rejects with exact fixes.

## Supported systems and reference

GitLab Issues (including self-managed) and GitHub Issues (including Enterprise) work today.
Other systems: on request. GitHub queues work without a board.

- [Queue, configuration and states](taskq/contracts/taskq.md)
- [Manager, tick and runtime setup](taskq/contracts/taskq-manager.md)
- `taskq --help`: commands. `taskq update`: update now; automatic updates every 24 hours.

## Develop taskq

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
TASKQ_CLEANUP_HELPERS=<project>/scripts python3 -m unittest discover -s tests
```

License: [MIT](LICENSE).

</details>
