# One-off run outside the current scope (#652)

Design target, not shipped behavior. [taskq.md](../../taskq.md) wins on conflict (R8, R13).
Scope: one urgent task of a registered project runs end to end while the host keeps its current focus.
First release: one local host. Cross-host dispatch is out of scope (see Later).

## Problem

Today a task starts only when a service's focus passes one of its labels (§ 4).
To run one urgent task outside that focus, the owner must widen the focus and later narrow it by hand.
That is two scope edits, a window where other tasks of the same label can start, and no exact restore.
`service focus` plus `taskq add` does not cover it (owner note on #652).

## Decision summary (recommended)

| # | Choice | Recommended | Alternatives |
|---|---|---|---|
| D1 | Syntax | `taskq service once N [--extra] [--request TEXT]`, `taskq service once off`, run in the target project root | `#N` items in `service focus`; top-level `taskq run <project> N` |
| D2 | Grant store | local `<root>/.taskq/once`, like `focus` | board label `once-<host>` |
| D3 | Identity | an existing open task number; never creates a task | create from text inside the command |
| D4 | Admission | normal: next free slot of this service, before other ready tasks | — |
| D5 | Extra slot | opt-in `--extra`: `limits.<rt> + 1` for task N only, until the grant ends | no extra mode |
| D6 | End | the service deletes the grant when a fresh read shows N closed, `failed` or `later` | grant expiry by time |

Why D1: the grant is a host admission setting, like focus and pause, so it lives under `service`.
A separate verb keeps `focus` untouched, so restore is exact by construction: focus is never read-modified-written.
`run` is avoided: it reads as "start now", which the normal mode does not promise.

Why D2: focus is already local per host (§ 4). A board label would let other hosts admit the task.
That is the cross-host case, which this release does not cover. R1 holds: the file is owner config like `focus`.
The board stays the truth for task state; the file only allows admission.

## Boundary of the grant

A grant is the file `<root>/.taskq/once` with one task: `#N`, optional `extra 1`, optional `request <owner words>`.
- Project: one registered project of this host (`<install>/projects`, `service list`). The owner names it; the PM maps
  the name to one root and asks when two match (R10). Folders are never discovered.
- Task: issue N on that project's board, open, trusted author, label `taskq` (§ 1). One grant per project.
- Host: this host only. The grant bypasses only the focus filter. Every other filter stays: `host-*`, `run-*`,
  `needs-*`, `assignee-only`, label validation, `Depends on`, capacity, claim and fencing (§ 3, § 4).
- Roles: the grant covers task N's worker and reviewer admissions, and rework after `requeue`, until it ends.
- Not covered: other tasks, other projects, other hosts, model, effort, permissions or provider (R9), project
  registration or config. Answers and steers already reach a live attempt without focus (§ 4 step 6).
- Version: the target service must support `once`. An older service ignores the file; the command then cannot read
  the grant back and says unknown, never success (R12).

## Confirmation flow (PM)

1. Owner asks for an urgent run. The PM names the project and searches its open tasks (`status`, issue search).
2. Match found: the PM shows `#N` and its state. No match: the PM drafts `taskq add` text and asks to create it.
   Duplicates: the PM picks an existing task over a new one and says which.
3. One summary, one confirmation. Template (R7, one line each):
   - `Run once: <project> #N <title>`
   - `Host: <host> · Mode: next free slot` or `Mode: +1 slot over limit <rt>=<n>`
   - `Focus stays: <labels|off>` · `Ends: when #N is closed, failed or parked`
   - `Blocked by: <why>` only when a filter or dependency will stop it (e.g. `host-win`, `run-codex` with limit 0,
     open `Depends on`, state `later`). Then the owner decides first: requeue, edit labels or drop.
4. After `yes`: `taskq service once N [--extra] --request "<owner words>"` in that root. It succeeds only when a
   fresh `status.json` shows the grant.
5. The PM watches only task N (`inbox N` or `wait`) and relays: each `ask` card, then one final line, the close
   verdict or the `failed` reason. Then it stops watching. Questions keep the normal `ask` path.

`--extra` appears in the summary only when the owner asked for more than the normal limit in this request.
Urgency, priority or the project being out of scope never implies `--extra`.

## Admission

- Normal mode: in step 6, task N sorts before all other ready tasks of this service. It takes the next free slot.
  It never stops, pauses or replaces a live attempt; it waits for a child to end.
- Capacity today is per service (§ 3): each project's service has its own `limits`. A target project with focus
  `off` usually has free slots, so normal mode often starts at once. No host-wide cap exists; this design adds none.
- Extra mode: when the normal capacity has no free slot, task N may use `limits.<rt> + 1`. Exactly one extra child,
  of task N, at a time. A runtime with limit 0 is disabled, not full: `--extra` does not enable it.
- While that child lives, live children exceed the limit; other tasks start only after the count drops below the
  limit again. After the grant ends, admission is the normal limit again.

## Dedup, repeat, restart

- Same `once N` again: no write, prints the current grant. Another `once M` while N is active: refused,
  `Error: once #N active` / `Next: taskq service once off`.
- N already `doing` or `review` on any host: the grant adds nothing; claim (§ 3) and R2 prevent a second attempt.
  The PM reports where it runs.
- N admitted by a normal focus first: the same task runs once; the grant ends with it.
- Service restart: the file survives; former children are `gone` (§ 4) and N is admitted again under the grant.
- PM restart or lost chat: the grant and the task log are on disk and board; the PM resumes watching from `inbox N`.

## Cancel and stop

- `taskq service once off`: deletes the grant. No new admission of N outside focus. A live attempt is not stopped.
- Stop the work too: `taskq later N` (§ 3 cancellation); the service stops the child (R11) and then the grant ends.
- A slot is free only after the service reaped the child. Unknown host or stale `status.json`: report unknown; never
  call the slot free, the task stopped or the run done (R12).

## Outcomes

| Outcome | Seen as | Grant | Owner sees |
|---|---|---|---|
| Success | `close` after review and publication (§ 7) | ends | close verdict |
| Owner question | `ask` | stays | the card |
| Rework | `requeue` | stays | nothing until next card or end |
| Technical failure | `gone`, retries to `ask`, or `failed` | stays on `ask`, ends on `failed` | card or `Failed: <reason>` |
| Parked | `later` | ends | `Parked: <reason>` |
| Lost contact | stale heartbeat or `status.json` | stays | `Unknown: <reason>` |

No close without the § 7 verdict and CI on the exact SHA. Unknown, `ask` and lost contact are never success.

## Draft taskq.md diff

Applies after owner approval; keeps the file at 298 lines. No new log kind, no `fold` change: N-1 compatible (§ 8).

```diff
@@ § 3 Attempts, lease, capacity
-- **Capacity** is per host: `limits.<runtime>` minus the service's live children of that runtime. No project cap: the
-  sum of host limits bounds the project.
+- **Capacity** is per host: `limits.<runtime>` minus the service's live children of that runtime. No project cap: the
+  sum of host limits bounds the project. A `once --extra` grant adds one slot for its task only (§ 4).
@@ § 4 Service
 - Pause: `service pause` (and the tray's) saves the focus to `focus.paused` and writes `off`; resume restores it. ...
+- Once: `service once N [--extra] [--request TEXT]` writes `<root>/.taskq/once` after the owner confirmed one summary (project, task, host, mode); task N passes focus and sorts first, every other filter and claim stay; `--extra` lets N alone exceed `limits.<rt>` (> 0) by one child. Another N is refused; `once off` deletes it; the service deletes it when N is closed, `failed` or `later`. `status.json` names it; success only after that readback. Focus is never changed.
@@ § 5 Commands
-| `taskq service install\|run\|focus\|pause\|resume\|status\|restart\|stop\|uninstall\|list` / `taskq tray` | ...
+| `taskq service install\|run\|focus\|once\|pause\|resume\|status\|restart\|stop\|uninstall\|list` / `taskq tray` | ...
```

## Proof obligations (implementation task)

Service unit tests (`src/service.rs`), board fakes, no real spawn:
1. Focus `off` + grant N: N is admitted, other ready tasks are not.
2. Grant N with `host-other`, `needs-x` missing, open dependency or `run-codex` with limit 0: N is not admitted.
3. Busy normal limit: without `--extra` N waits and starts first when a child ends; no child is stopped.
4. `--extra`: exactly one child over the limit, only for N; a second N attempt or another task gets no extra slot.
5. N closed, `failed` or `later`: the file is deleted; next pass admits under focus only; `focus` bytes unchanged.
6. `ask`, `requeue`, `gone`: the grant stays; reviewer of N is admitted under it.
7. Repeat `once N`: no write. `once M` during N: refused. Restart: grant reloaded from the file.
8. Command readback: stale or missing `status.json` gives `unknown`, not success.
Process check on one host (R12): a second registered project with focus `off`, one real once run to close.
Windows and multi-host: unknown until tested.

## Later (separate proposals)

- Cross-host once: needs a board-visible grant and host choice; conflicts with local focus ownership.
- Tray action `Run once…` (menu.md M7 AI channel) using the same command.
- Host-wide cap across projects, if one-off runs crowd a host.
