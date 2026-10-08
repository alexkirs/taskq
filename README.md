<p align="center">
  <img src="docs/header.webp" alt="Relaxing while the agents work" width="720">
  <br><em>Agents working.</em>
</p>

Your repository board is the task list; your usual AI apps do the work.
Add taskq, then ask Claude or Codex to turn any request into tasks.

A timer picks up tasks, runs as many workers as you choose, and shows each one's session link.
You stay in control: a worker's question waits for your answer, and nothing is accepted until it is reviewed.

1. **⚙ Set up (once per project)**<br>
   Tell your agent: "Install taskq from https://github.com/alexkirs/taskq and set it up for this project."

2. **▶ Start working (each day)**<br>
   "You are the taskq manager. Arm the tick." - workers start on ready tasks

3. **✎ Talk to the manager**<br>
   "File a task: fix the login redirect." - new task<br>
   "What is in the queue?" - status<br>
   "Show me the question from #12." - answer a worker<br>
   "Review #12." - accept or send back

**Mix agents, task by task.** Codex makes the visuals, Claude writes the code - in whatever order your work needs.
Choose each task's agent with `--runtime` (a `run-*` label); use `--deps` to chain tasks.

## Install

Requirements: python3 >= 3.9, git, and [gh](https://cli.github.com) (GitHub) or [glab](https://gitlab.com/gitlab-org/cli)
(GitLab), logged in. taskq is one file, stdlib only.

```bash
git clone https://github.com/alexkirs/taskq ~/taskq
ln -s ~/taskq/taskq.py ~/.local/bin/taskq     # or: alias taskq='python3 ~/taskq/taskq.py'
```

Windows (PowerShell): `git clone https://github.com/alexkirs/taskq C:\src\taskq`, then add
`function taskq { python C:\src\taskq\taskq.py @args }` to `$PROFILE`.

Update: `git pull` in the clone.

## Set up a project

At the project root, commit a `taskq.json`:

```json
{"board": "github", "repo": "owner/repo", "publish": "direct", "limits": {"claude": 2, "codex": 1}}
```

GitLab: `"board": "gitlab", "repo": "group/project"`, plus `"host"` when self-managed.
`publish`: `direct` (workers push `main`) or `pr` (workers open a PR; `close` merges it).
Add `.taskq/` and `.worktrees/` to `.gitignore`. Check with `taskq list`.
Claude workers: run `claude` once in the project root and accept the folder trust prompt.

## Run

```bash
taskq add "Try taskq" --type research --goal "Reply: taskq works. No file changes." --acceptance "The result says: taskq works."
taskq tick          # starts a worker; the manager runs it every 5 minutes: /loop 5m taskq tick
taskq list          # the worker hands in: review
taskq close N --text "Checked the reply."
```

Commands: `add list take ask answer result requeue later close tick`.
Everything else - states, the worker's rules, the manager's routine, other boards and runtimes, Windows -
is in the contract: **[taskq.md](taskq.md)**. Give it to any agent; it is all it needs.

## Develop taskq

Every session on the machine runs the clone's `taskq.py`: keep the clone on clean `main`, change taskq in a worktree
(`git worktree add -b <branch> .worktrees/<branch> origin/main`), run `python3 -m unittest tests.test_single` there.
Design: [docs/single-file.md](docs/single-file.md).

License: [MIT](LICENSE).

If taskq saves you time, [buy me a coffee](https://alex.kirs.online/donate).
