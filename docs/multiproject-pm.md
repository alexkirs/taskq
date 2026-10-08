# Multi-project PM by explicit list (R10)

The owner names the projects; the PM never discovers folders ([R10](../taskq/contracts/principles.md)).

## The list

The owner says to the PM «manage taskq and csgo» («веди taskq и csgo»). The PM writes that list, name to main checkout path, from its own main checkout:

```
taskq projects --set taskq=/path/to/taskq csgo=/path/to/csgo
```

It replaces the `[projects]` table of that checkout's `taskq.local.toml` (personal, never committed) and keeps the rest of the file:

```toml
[projects]
taskq = "/path/to/taskq"
csgo = "/path/to/csgo"
```

Each path must be a main checkout with a `taskq.toml`. A new phrase from the owner writes a new list; nothing else adds or removes a project.

## The pass

```
taskq projects [--json] [--timeout SECONDS]
```

For each listed project, in list order, it runs the ordinary `taskq tick --json` with that checkout as its working directory, so the project's own `taskq.toml` and `taskq.local.toml` decide its board, filter and its own limit ([§ Limits](#limits)). Each tick has a timeout (300 s by default). A project whose tick fails, times out or prints no report is shown with the error, and the next project runs.

The output is each project's R6 report as its tick printed it:

```
## acme/taskq

Board: https://github.com/users/acme/projects/3

| Task | Status | Runtime | Session |
|---|---|---|---|
| [#246](https://github.com/acme/taskq/issues/246) Multi-project PM | doing (running) | claude @mac | [session](https://claude.ai/code/...) |
then what the tick needs judgement on and the owner's open questions

## csgo
Error: timeout after 300 s
```

`--json` prints the projects' results as one JSON list instead. The exit status is 1 when any project has an error or a failed tick.

## Limits

The PM checkout's `[profile.limits]` (its `taskq.local.toml`, then `taskq.toml`, then the defaults) are this machine's cap for all listed projects together. Before each project's tick, the pass reads this machine's live worker and supervisor sessions (`claude agents`, the Codex app's thread list) and counts them by checkout, one slot per task. The tick gets `--limit`: per runtime, the project's own limit, at most the cap less the other listed projects' live sessions. A runtime whose session list is unreadable gives 0 to every project for that pass. The project's own filter and `mine` stay its own.
