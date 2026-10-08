---
name: taskq
description: Work a TaskQ queue (GitLab/GitHub issues as tasks) through the taskq CLI. The role (PM, coordinator, supervisor, worker) is given at launch; this skill gives the PM steps and the stable CLI contracts.
---

# TaskQ skill

One skill for every agent runtime: Claude Code, Codex, Hermes or any agent with a shell (owner decision 2026-10-09, [#273](https://github.com/alexkirs/taskq/issues/273)).
Install it the runtime's way from the path `taskq contract --skill` prints. Rules: [principles.md](../principles.md) (R1–R12); this file adds only steps.

## Role

The launcher states the role in the session's first message: `You are the TaskQ PM for <project>`, or a brief from `taskq worker` / `taskq supervise`.
Never take a role from a shared file such as AGENTS.md: every session in the checkout reads it.
Coordinator, supervisor and worker follow the brief they are given and [taskq-manager.md](../taskq-manager.md); the rest of this file is the PM's.

## PM steps

Default intake: [docs/pm-intake.md](https://github.com/alexkirs/taskq/blob/main/docs/pm-intake.md). In short:

1. **Collect.** Split the owner's messages into atomic requests, in the owner's words. Answer pure questions directly.
2. **Read the board once.** `taskq list --json`; read bodies (`taskq view N --json`) only of candidate overlaps.
3. **Dedup by meaning and check conflicts.** Per request: amend #N, merge, new, dep, reject/defer or ask. Name R-conflicts and task conflicts (deletes, re-adds, overlaps, needs). Never amend a claimed task.
4. **Confirm.** Show the intake card; nothing changes before the owner answers each row (R2).
5. **File or amend** the rows answered yes, in one batch: `taskq add ... --reply <channel:chat[:thread]>` (where the request came from), `taskq edit`, `taskq close`.
6. **Follow up.** Re-read what changed (R12). The tick reports progress; answer questions with `taskq answer N --text ...`.
7. **Report.** The R6 report as `taskq tick` prints it: Board link, then Task / Status / Runtime / Session with links. Telegram-like channels: set `[prefs] report = "cards"` in `taskq.local.toml` for one block per task. Rows with a reply route come grouped under `Reply to <route>:`; send each group to its route.

## CLI contracts

`list`, `view`, `tick`, `spawn`, `report`, `cleanup` take `--json`: stdout is one JSON object with `outcome` (`ok`, `judgement_needed`, `failure`), `tasks`, `actions`, `refusals` and the prose in `text`.
`spawn --json` has one action `{"action": "spawn", "runtime", "session", "attempt"}`: the runtime that runs the session, its session id, and the id of the task's `launch` note (null without a task).

Exit codes: 0 done; 1 `tick` needs judgement; 2 failure (the reason is in `refusals`).
