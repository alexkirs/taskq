<p align="center">
  <img src="docs/header.webp" alt="Relaxing while the agents work" width="720">
  <br><em>Agents working.</em>
</p>

Your repository board is the task list; your usual AI apps do the work.
Add taskq, then ask Codex or Claude to turn any request into tickets.

The manager picks up tickets, runs as many workers as you choose, and tracks progress.
No extra worker apps required.

You stay in control: tasks wait for your input and continue when you reply.

1. **⚙ Set up (once per project)**<br>
   Tell your agent: "Install taskq from https://github.com/alexkirs/taskq and set it up."

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
