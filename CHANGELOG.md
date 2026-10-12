# Changelog

Per release: what changed on the board and what each open issue needs. A cutover agent reads this (taskq.md § 9);
there is no migration code. Releases without a **Board** section need nothing.

## v4.0.0

Rust binary replaces `taskq.py` and `host_service.py`.

**Board** (v3 → v4), for each open issue except `taskq host` ones:
- Text: remove the block from `<!-- taskq:start -->` to `<!-- taskq:end -->`. Each id in its `deps` becomes a line
  `Depends on #N` unless the text already has one.
- State: v3 keeps it in the `q-<state>` label, v4 in `taskq:` log entries (§ 2). No entry is needed when the folded
  state already matches: `ready`, `waiting`, and `doing`/`review` (the v3 attempt ends with the v3 service).
- `later` / `failed`: `taskq later N --text` / `taskq failed N --text` with the text of the last `**later**` /
  `**failed**` comment; with none, ask the owner for the reason.
- `ask`: v4 has no `ask` without an attempt. Show the owner the question; the owner answers it in v3 before the
  cutover or the task is parked with `taskq later N --text <question>`.
- No v3 block and no `taskq:` entry: v4 would take it as a `ready` task. Ask the owner: a task, or a user issue to
  leave alone.
- v3 capacity issue (`<!-- taskq:capacity -->`): v4 does not read it; remove grants of closed tasks.
