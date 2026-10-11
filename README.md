<p align="center">
  <img src="docs/header.webp" alt="Relaxing while the agents work" width="720">
  <br><em>Agents working.</em>
</p>

Your repository board is the task list; your usual AI apps do the work.
Add taskq, then ask Codex or Claude to turn any request into tickets.

The manager picks up tickets, runs as many workers as you choose, and tracks progress.
No extra worker apps required. Compression stays under your global user settings; TaskQ needs no compression provider.

You stay in control: tasks wait for your input and continue when you reply.

1. **⚙ Set up (once per project)**<br>
   Tell your agent: "Install taskq from https://github.com/alexkirs/taskq. Create project Notes from my idea or files in ~/Projects/notes, choose a repository and board, then plan it."<br>
   Existing project: "Set it up here; keep my files and repository."

2. **▶ Start working (each day)**<br>
   "Run taskq pm, then arm the tick." - workers start on ready tasks

3. **✎ Talk to the manager**<br>
   "File a task: fix the login redirect." - new task<br>
   "What is in the queue?" - status<br>
   "What should we do next?" - plan<br>
   "Show me the question from #12." - answer a worker<br>
   "Review #12." - accept or send back

**Mix agents, task by task.** Codex makes the visuals, Claude writes the code - in whatever order your work needs.
Choose each task's agent with `--runtime` (a `run-*` label); use `--deps` to chain tasks.

Agents: install, setup, commands and development are in **[taskq.md](taskq.md)**.

License: [MIT](LICENSE).

If taskq saves you time, [buy me a coffee](https://alex.kirs.online/donate).

### Current-user host service (#657)

The service reuses TaskQ native lifecycle and finite ARM authorization. Install/init remain separate:
register one **existing** project, then claim dispatch only from a reviewed completed-source checkpoint.

```sh
taskq service --home SERVICE_HOME register --project ABS_PROJECT --install-dir ABS_INSTALL --name EXISTING_ARM
taskq service --home SERVICE_HOME claim --checkpoint REVIEWED_JSON
taskq service --home SERVICE_HOME serve --cadence 180
taskq service --home SERVICE_HOME client status --format json
taskq service --home SERVICE_HOME client resume --id UNIQUE_COMMAND_ID
```

`serve` runs through the same existing current-user OS source after disabling its old tick action.
The checkpoint explicitly binds version1/project/scheduler binding, old_source_disabled=true,
owned_controllers_settled=true, unknown_writers=false. This is a reviewed attestation, not automated
universal drain detection. A claim starts paused. Native epoch/PID/birth gates refuse old controllers;
legacy code without those gates must already be settled. Don't run two dispatch sources.

`client pause` stops service admissions; `client stop` exits an idle service; after settlement,
`service release` returns dispatch ownership to legacy mode before restoring the SAME saved source.
Never restore capacity ledgers or board state. Unknown interrupted operations remain blocked unless
the exact own durable terminal receipt, PID/birth and output hash reconcile. No worker is killed/replaced.
`list/status/pm` are inert JSON snapshots, with explicit atomicity and separate native ACK/application;
`explain` returns an explicit analysis assignment, not a hidden model invocation. Native agent ARM
authorization and owner decisions remain outside IPC; clients cannot assert an agent session.
