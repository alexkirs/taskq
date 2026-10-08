# Runbook: CyprusBZ on hermes-fra1 moves to taskq.py

For Tight, on hermes-fra1, as the user that runs Hermes. Owner decision 2026-10-09 (#482, after #477, #479, #480).
Each step: commands, a check, a rollback. Run the steps in order. A check that fails: stop, roll back that step,
and report on the board. Do not change anything that this runbook does not name.

Before → after:

| | Before | After |
|---|---|---|
| CLI | the pipx `taskq` package | `taskq.py` of a git clone, through a symlink |
| Config | the old profile (`taskq*.toml`) | `taskq.json`, committed in CyprusBZ |
| Workers | `taskq tick --act`, supervisor sessions, Codex app-server | `taskq tick` spawns `codex exec` workers |
| Manager | old prompts and commands | `taskq pm` (the role is in `taskq.md`, § 7) |
| Tick | the old Hermes job | one Hermes cron job: a `taskq wait` pre-check script, then the agent |

## 0. Names

Set these in every shell of this runbook. Fill in the `<...>` values first. The table explains each one.

```sh
P=/home/hermes/projects/alexkirs        # the old clone is $P/taskq; it stays as it is until step 11
NEW=$P/taskq-main                       # the new clone, beside the old one
CBZ='<CyprusBZ checkout root>'          # the main checkout that the tick runs in
WT='<parent folder of the host-owned task worktrees>'
SNAP=$HOME/taskq-migration-$(date +%Y%m%d)
```

| Value | Where it comes from |
|---|---|
| `<gitlab-host>` | the host of the CyprusBZ remote (`git -C $CBZ remote get-url origin`) |
| `<chat>` | the Telegram chat id (or `chat_id:thread_id`) where the owner talks to the CyprusBZ manager |
| `<hostname>` | the output of `hostname` |

## 1. Snapshot (read only)

```sh
mkdir -p $SNAP && cd $CBZ
command -v taskq; ls -l "$(command -v taskq)"; pipx list > $SNAP/pipx.txt 2>&1
pipx runpip taskq show taskq > $SNAP/pipx-taskq.txt 2>&1   # "Editable project location" names a clone, if one is used
git -C $P/taskq rev-parse HEAD > $SNAP/old-clone-sha.txt
taskq list > $SNAP/old-list.txt 2>&1
glab api --paginate "projects/:id/issues?state=opened&per_page=100" > $SNAP/open-issues.json
git worktree list --porcelain > $SNAP/worktrees.txt; git branch -a > $SNAP/branches.txt
find $HOME $CBZ -maxdepth 4 -name 'taskq*.toml' -not -path '*/.worktrees/*' > $SNAP/old-configs.txt
tar czf $SNAP/old-configs.tgz -T $SNAP/old-configs.txt $( [ -d $CBZ/.taskq ] && echo $CBZ/.taskq )
hermes cron list > $SNAP/hermes-cron.txt; crontab -l > $SNAP/crontab.txt 2>&1
pgrep -af 'codex|taskq' > $SNAP/processes.txt
systemctl --user list-units --all --no-pager | grep -iE 'codex|taskq' > $SNAP/units.txt
cp $CBZ/AGENTS.md $SNAP/AGENTS.md.old
```

Check: every file in `$SNAP` is non-empty, except files for things that do not exist on this host. Rollback: none.
Nothing changed. Keep `$SNAP` until step 11 is done.

## 2. Stop new launches

Pause the old tick. Find its job in `$SNAP/hermes-cron.txt` (and `crontab.txt`, if it is a cron entry):

```sh
hermes cron pause '<old taskq job>' --reason 'taskq.py migration'
crontab -e                      # only if crontab.txt has a taskq line: comment it out
```

Check: `hermes cron list` shows the job paused. No `taskq tick` runs on this host: `pgrep -af 'taskq tick'` is empty.
Rollback: `hermes cron resume '<old taskq job>'`, then restore the crontab line.

## 3. Drain the old claims

The new taskq does not read old claims (old block format, `node` instead of `name`). Finish them with the old CLI.

1. Let every `doing` task end in `review` or `ask`. Do not start new tasks.
2. `review`: review and close it with the old CLI as before.
3. `ask`: get the owner's answer and let the worker finish. Alternatively, release it to `ready` with a comment that
   names the open question; the new worker reads the history.
4. Stop and archive the old supervisor and worker sessions with the old CLI (`taskq cleanup` report, then the
   owner-approved actions). Remove no host-owned worktree or branch.

Check: `taskq list` (old CLI) has no `doing`, `ask` or `review` task. `pgrep -af 'codex'` shows no worker of a task.
Rollback: none needed. This is normal work with the old CLI; step 2 can be undone.

## 4. New clone beside the old one

```sh
git clone https://github.com/alexkirs/taskq $NEW
python3 $NEW/taskq.py --help | head -3
```

Check: the usage line lists `add,list,take,ask,answer,result,requeue,later,close,tick,wait,arm,pm`.
Rollback: `rm -rf $NEW`.

## 5. Symlink switch

Two links: the path `$P/taskq` (Hermes notes and skills name it) and the `taskq` command on `PATH`.

```sh
BIN=$(command -v taskq)                                   # the pipx shim, e.g. ~/.local/bin/taskq
mv $P/taskq $P/taskq-old && ln -s taskq-main $P/taskq
mv "$BIN" "$BIN.pipx" && ln -s $P/taskq/taskq.py "$BIN"
```

Check: `readlink -f "$(command -v taskq)"` is `$NEW/taskq.py`. `git -C $P/taskq status -sb` is clean `main`.
`tick` and `wait` pull this clone themselves (`git merge --ff-only`). Keep it clean: never edit or commit there.
Rollback:

```sh
rm "$BIN" && mv "$BIN.pipx" "$BIN"
rm $P/taskq && mv $P/taskq-old $P/taskq
```

## 6. taskq.json in CyprusBZ

Commit through the normal CyprusBZ MR flow, from a host-owned worktree (not the main checkout). Remove the old
config files of the repository in the same MR, if `$SNAP/old-configs.txt` names any that are tracked in git.

`taskq.json` at the CyprusBZ root:

```json
{
 "board": "gitlab",
 "host": "<gitlab-host>",
 "repo": "gradus/cyprusbz",
 "publish": "pr",
 "workspace": "external",
 "limits": {"codex": 3, "claude": 0},
 "assignee": "me",
 "hosts": {"<hostname>": "fra1"},
 "codex": ["-s", "workspace-write", "-c", "sandbox_workspace_write.network_access=true",
           "--add-dir", "<CyprusBZ checkout root>/.git", "--add-dir", "<parent folder of the host-owned task worktrees>"]
}
```

- `assignee: "me"`: the `glab` user of this host. The tick starts, and `tick`/`wait`/`list` show, only tasks assigned
  to that user. Assign each task that this host must run: `glab issue update N --assignee <login>`.
- `codex`: with `workspace: external` the sandbox must be able to write to the task worktrees and the main `.git`.
  Write absolute paths: JSON does not expand `$CBZ` or `$WT`.
- `.gitignore`: add `.taskq/` and `.worktrees/` if they are not in it.
- The project needs the MR pipeline (`.gitlab-ci.yml` that runs on MRs) and squash merge allowed (taskq.md § 6).

Check, in `$CBZ` after the MR is merged and pulled:

```sh
glab auth status && codex login status
taskq list                                               # no error; lists the tasks assigned to this user
python3 -c 'import json,sys; print(sum(1 for i in json.load(open(sys.argv[1])) if any(l.startswith("q-") for l in i["labels"])))' $SNAP/open-issues.json
```

The `taskq list` lines together with the unassigned tasks must equal the number printed. A task that is missing is
not a valid new-format task (old block, two `q-*` labels). Fix it by hand or ask the owner. Do not go on.

Rollback: revert the MR. The old CLI did not read `taskq.json`.

## 7. CyprusBZ AGENTS.md

Replace the old taskq section (old commands, `tick --act`, supervisor, app-server, `cleanup`, `doctor`) with the
text below. Same MR as step 6, or a new one. Keep the project's own rules; fill in the worktree command.

```markdown
## Tasks (taskq)

Tasks are GitLab issues of gradus/cyprusbz with a `q-*` label; the contract is `taskq.md` of the taskq clone
(/home/hermes/projects/alexkirs/taskq). `taskq` is that clone's `taskq.py`.

- Manager (the session the owner talks to, Hermes cron runs): run `taskq pm` in this checkout and follow it.
  Commands: add, list, answer, requeue, later, close, tick, wait. The manager never does task work.
- Worker (started by `taskq tick` with a brief): follow the brief. Workspace: `<host command that creates or
  reuses the worktree for branch taskq-<N> under the task worktree folder>`. The host owns the worktree and
  the branch: never remove them. Deliver with `git push --force-with-lease origin HEAD:refs/heads/taskq-<N>`,
  one `glab mr create`, then `taskq result`.
- Only these commands exist. There is no `tick --act`, `supervise`, `doctor`, `cleanup`, `update`, `view` or Codex app-server.
```

Check: `grep -nE 'tick --act|supervise|app-server|selftest|taskq (doctor|cleanup|update|view)' $CBZ/AGENTS.md` is empty.
Rollback: revert the MR (`$SNAP/AGENTS.md.old` is the old text).

## 8. Hermes cron: `taskq wait`, then the manager

A pre-check script blocks in `taskq wait`. On `tick` (no event in the window) it runs the event pass (steps 1–3
of taskq.md § 7, which includes the 120-minute nudge) and does not wake the agent. On `review`, `ask` or `gone` it
wakes the agent. A `flock` on `.taskq/hermes-cron.lock` stops overlapping runs (a manual run plus a scheduled run,
too). A second `tick` at the same time is safe: taskq holds `.taskq/dispatch.lock` for each pass.

`$HERMES_HOME/scripts/taskq-cyprusbz.py`:

```python
#!/usr/bin/env python3
"""Hermes cron pre-check: wake the CyprusBZ manager only when the board needs it."""
import fcntl, json, os, subprocess, sys
ROOT, TASKQ = '<CyprusBZ checkout root>', '/home/hermes/projects/alexkirs/taskq/taskq.py'
os.makedirs(f'{ROOT}/.taskq', exist_ok=True)
lock = open(f'{ROOT}/.taskq/hermes-cron.lock', 'a')
try:
    fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
except BlockingIOError:  # another run still waits: this one does nothing
    sys.exit(print(json.dumps({'wakeAgent': False})))
waited = subprocess.run(['python3', TASKQ, 'wait', '--window', '4'], cwd=ROOT, capture_output=True, text=True)
if waited.returncode:
    sys.exit(f'taskq wait failed: {waited.stderr.strip()}')  # a failed run is always delivered
if waited.stdout.strip() == 'tick':
    with open(f'{ROOT}/.taskq/dispatch.log', 'a') as log:
        subprocess.run(['python3', TASKQ, 'tick', '--quiet'], cwd=ROOT, stdout=log, stderr=log)
    print(json.dumps({'wakeAgent': False}))
else:  # review/ask/gone lines, or "The manager contract changed"
    print(waited.stdout.strip())
    print(json.dumps({'wakeAgent': True}))
```

Run it once by hand: `python3 $HERMES_HOME/scripts/taskq-cyprusbz.py`. Check: it ends in about 4 minutes, and its
last line is JSON with `wakeAgent`.

The job. Create it paused, run it once, then resume:

```sh
hermes cron create "every 5m" "$(cat <<'EOF'
You are the taskq manager of CyprusBZ. In the working directory run `taskq pm` and follow it from now on.
Then run one pass: `taskq tick`. Do § 7 "After each pass" for every task in ask, review or gone:
relay each ask verbatim; review each result and close it or requeue it with exact fixes.
Never answer a worker's question for the owner. Do no task work.
Report as one card per task that needs the owner: Task (#N, linked) / State / Runtime / Session (link, else id).
Nothing needs the owner: reply exactly [SILENT].
EOF
)" --name taskq-cyprusbz --workdir "$CBZ" --script taskq-cyprusbz.py \
   --deliver telegram:<chat> --paused --paused-reason 'taskq.py migration check'
hermes cron run taskq-cyprusbz && hermes cron runs taskq-cyprusbz --limit 1
hermes cron resume taskq-cyprusbz
```

Run `hermes cron create --help` first to check the flags against the installed Hermes. The owner answers in the
Telegram chat. The Hermes session there must run in `$CBZ` as the manager (`taskq pm`) and records the answer
with `taskq answer N --text "<verbatim>"`.

Known: Hermes has no taskq session id, so its board comments show `owner` and workers it starts are named
`T<N> UNK ...`. This is cosmetic.

Check: `hermes cron list` shows `taskq-cyprusbz` active and the old job paused. `.taskq/dispatch.log` grows every
few minutes.
Rollback: `hermes cron remove taskq-cyprusbz`, delete the script, then roll back step 5 and step 2.

## 9. One full cycle

```sh
cd $CBZ
taskq add "Migration probe: one line in docs" --type docs --runtime codex \
  --goal "Owner decision 2026-10-09 (#482). First ask the owner which line to add (any text). Then add it to a new docs/taskq-probe.md." \
  --acceptance "MR with docs/taskq-probe.md holding the owner's line; pipeline green."
glab issue update <N> --assignee <login>
```

| Stage | Check |
|---|---|
| claim | within one cron run: `q-doing`, a `spawn` comment, worker `T<N> ... (fra1)` |
| worker | `codex` process running; the host worktree for `taskq-<N>` exists under `$WT` |
| ask / answer | the question arrives in Telegram as a card; reply there; the issue gets an `answer` comment, `q-doing` |
| result | `q-review`; `result` comment with the MR head SHA and checks |
| review | the cron card arrives; the manager checks the MR diff and the pipeline of that SHA |
| MR / CI | `taskq close <N>` waits for the MR pipeline `success`, squash-merges at that SHA |
| close | issue closed; the close comment says `kept: owned by host`; `git worktree list` still has the worktree; branch `taskq-<N>` still exists |

A stage that fails: `taskq later <N> --text "<what failed>"`, then report on the board with the stage and output.
Leave the new path running only when the owner says so. Otherwise roll back steps 8, 5 and 2.
Remove the probe worktree with the host's own lifecycle, not with taskq.

## 10. Watch

Run in production for at least one day with real tasks. Check that the Telegram cards arrive, that `wait`/`tick`
errors are delivered, and that `git -C $P/taskq status -sb` stays clean `main`.

## 11. Retire the old install (do last; harder to undo)

Only after step 10 and with the owner's yes:

```sh
pipx uninstall taskq                                     # it also removes "$BIN.pipx"
xargs -a $SNAP/old-configs.txt rm -f                     # the old profiles; the repository copies went in step 6
hermes cron remove '<old taskq job>'
rm -rf $P/taskq-old
```

The Codex app-server: stop it only if `$SNAP/units.txt` or `processes.txt` shows that taskq started it
(`systemctl --user disable --now <unit>`). Leave it running if anything else uses it.

Rollback (slow): `git clone https://github.com/alexkirs/taskq $P/taskq-old && git -C $P/taskq-old checkout $(cat $SNAP/old-clone-sha.txt)`,
`pipx install $P/taskq-old`, `tar xzf $SNAP/old-configs.tgz -C /`, recreate the old job from `$SNAP/hermes-cron.txt`.
Then do the rollbacks of steps 8, 5 and 2.
