# One-off run outside the current scope (#652)

Design target, not shipped behavior. [taskq.md](../../taskq.md) wins on conflict (R8, R13).
Shipped instead (owner decision 2026-10-13, #790): `p0` alone passes any focus on every host; no grant, no `p0-extra`.
Scope: one urgent task of a registered project runs end to end while the host keeps its current focus.
Chosen model: a board label grant (owner answer 3 on #652). First release: one local host; cross-host is Later.

## Problem

Today a task starts only when a service's focus passes one of its labels (§ 4).
To run one urgent task outside that focus, the owner must widen the focus and later narrow it by hand.
That is two scope edits, a window where other tasks of the same label can start, and no exact restore.
`service focus` plus `taskq add` does not cover it (owner note on #652).

## Decision summary

| # | Choice | Chosen | Rejected alternatives |
|---|---|---|---|
| D1 | Grant | labels `p0` + exactly one `host-<name>` on the task issue | local file `<root>/.taskq/once`; `#N` items in `service focus` |
| D2 | Syntax | `taskq urgent N [--extra]` / `taskq urgent N off` (sets/removes the labels) | `taskq service once N`; `taskq run <project> N` |
| D3 | Who may set | a trusted account (§ 1 roles); the service checks the actor of the label event | any account with triage rights |
| D4 | Identity | an existing open task number; never creates a task | create from text inside the command |
| D5 | Admission | normal: next free slot of the named host, before other ready tasks | preempt a live child |
| D6 | Extra slot | separate explicit label `p0-extra`: `limits.<rt> + 1` for this task only | no extra mode; extra implied by `p0` |
| D7 | End | the service removes `p0`, `p0-extra` when the task is `later` or `failed`; closed issues are never admitted | expiry by time |

Why D1: the grant is visible on the board where the owner already looks; `p0` is red (board.md B2), so an urgent task
is seen at a glance. `host-<name>` keeps the first release on one host: other hosts already skip it by the § 4 filter.
Focus files are never touched, so restore of the original scope is exact by construction.
Later, cross-host is the same label with a different or no `host-*` (see Later); a local file could not grow there.

Why D2: `run` reads as "start now", which normal mode does not promise. `urgent` names what the owner asks for.
The command only edits labels; a person may set the same labels on the board by hand (§ 1 style).

Rejected, kept for the record:
- Local `once` file (`service once N`): narrow blast radius, but invisible on the board and local-only forever.
- `#N` in `service focus`: mixes a one-off into the standing scope; restore would need a read-modify-write of focus.

## Boundary of the grant

A grant is an open task issue with `p0` and exactly one `host-<name>`, both added by trusted accounts.
- Project: one registered project of the named host (`<install>/projects`, `service list`). The owner names it; the PM
  maps the name to one board and asks when two match (R10). Folders are never discovered.
- Task: an open issue, trusted author, label `taskq` (§ 1). `p0` without `host-*`, or with two, admits nothing outside
  focus; `status` shows `p0 needs one host-*`.
- Who: the service reads the label events (GitHub issue events, GitLab resource label events) and accepts `p0`,
  `p0-extra` and that `host-*` only when the last `labeled` event of each is by a trusted account. Else the label
  counts as plain priority 0 within focus, never as a grant. The event (actor, time) is the consent receipt.
- Host: the named host only. The grant bypasses only the focus filter. Every other filter stays: `run-*`, `needs-*`,
  `assignee-only`, `Depends on`, capacity, claim and fencing (§ 3, § 4).
- Roles: worker and reviewer admissions of the task, and rework after `requeue`, until the grant ends.
- One per project and host: if several issues carry a valid grant, only the lowest number passes focus; `status`
  lists the others as waiting. Keeps "one-off" one.
- Not covered: other tasks, other projects, model, effort, permissions or provider (R9), project registration or
  config. Answers and steers already reach a live attempt without focus (§ 4 step 6).
- Version: an older service treats `p0` as an unknown label: no focus bypass, no admission outside focus. The command
  reads back the named host's `status.json`/heartbeat; no newer version there means `unknown`, never success (R12).
- Legacy label schema boards: `p0` is canonical only; on a legacy board the command refuses (`label_schema`, § 9).

## Confirmation flow (PM)

1. Owner asks for an urgent run. The PM names the project and searches its open tasks (`status`, issue search).
2. Match found: the PM shows `#N` and its state. No match: the PM drafts `taskq add` text and asks to create it.
   Duplicates: the PM picks an existing task over a new one and says which.
3. One summary, one confirmation. Template (R7, one line each):
   - `Urgent: <project> #N <title>`
   - `Host: <host> · Mode: next free slot` or `Mode: +1 slot over limit <rt>=<n>`
   - `Focus stays: <labels|off>` · `Ends: when #N is closed, failed or parked`
   - `Blocked by: <why>` only when a filter or dependency will stop it (e.g. `run-codex` with limit 0, missing
     `needs-*`, open `Depends on`, state `later`). Then the owner decides first: requeue, edit labels or drop.
4. After `yes`: `taskq urgent N [--extra]` from the owner's own CLI session (the board token is the owner's, so the
   label event actor is trusted). It succeeds only when the host's fresh `status.json` names the grant.
5. The PM watches only task N (`inbox N` or `wait`) and relays: each `ask` card, then one final line, the close
   verdict or the `failed` reason. Then it stops watching. Questions keep the normal `ask` path.

`--extra` appears in the summary only when the owner asked for more than the normal limit in this request.
Urgency, `p0` or the project being out of scope never implies `--extra`.

## Admission

- Normal mode: in step 6, task N sorts before all other ready tasks of this service (`p0` < `p1` < `p2`). It takes
  the next free slot. It never stops, pauses or replaces a live attempt; it waits for a child to end.
- Capacity today is per service (§ 3): each project's service has its own `limits`. A target project with focus
  `off` usually has free slots, so normal mode often starts at once. No host-wide cap exists; this design adds none.
- Extra mode (`p0-extra`, valid only with a valid `p0` grant): when normal capacity has no free slot, task N may use
  `limits.<rt> + 1`. Exactly one extra child, of task N, at a time. A runtime with limit 0 is disabled, not full:
  `p0-extra` does not enable it.
- While that child lives, live children exceed the limit; other tasks start only after the count drops below the
  limit again. After the grant ends, admission is the normal limit again.

## Dedup, repeat, restart

- Same `urgent N` again: labels already set, no write; prints the current grant. `urgent M` while N holds a valid
  grant on the same host: refused, `Error: urgent #N active` / `Next: taskq urgent N off`.
- N already `doing` or `review` on any host: the grant adds nothing; claim (§ 3) and R2 prevent a second attempt.
  The PM reports where it runs.
- N admitted by a normal focus first: the same task runs once; the grant ends with it.
- Service restart: labels are on the board; former children are `gone` (§ 4) and N is admitted again under the grant.
- PM restart or lost chat: grant and task log are on the board; the PM resumes watching from `inbox N`.

## Cancel and stop

- `taskq urgent N off`: removes `p0`, `p0-extra` (keeps `host-*`, which may predate the grant). No new admission of
  N outside focus. A live attempt is not stopped.
- Stop the work too: `taskq later N` (§ 3 cancellation); the service stops the child (R11) and removes the grant.
- A slot is free only after the service reaped the child. Unknown host or stale `status.json`: report unknown; never
  call the slot free, the task stopped or the run done (R12).

## Outcomes

| Outcome | Seen as | Grant | Owner sees |
|---|---|---|---|
| Success | `close` after review and publication (§ 7) | ends (issue closed) | close verdict |
| Owner question | `ask` | stays | the card |
| Rework | `requeue` | stays | nothing until next card or end |
| Technical failure | `gone`, retries to `ask`, or `failed` | stays on `ask`, removed on `failed` | card or `Failed: <reason>` |
| Parked | `later` | removed | `Parked: <reason>` |
| Lost contact | stale heartbeat or `status.json` | stays | `Unknown: <reason>` |

No close without the § 7 verdict and CI on the exact SHA. Unknown, `ask` and lost contact are never success.
`p0` stays on a closed issue as history; closed issues are never admitted.

## Draft taskq.md diff

Applies after owner approval of this note. No new log kind, no `fold` change: N-1 compatible (§ 8). Net +2 lines.

```diff
@@ § 1 Tasks
-Labels: `q-<state>` (projection; `waiting` maps to `q-blocked`); type `code|docs|research|asset`; `p1|p2` (lower first, absent = 2);
+Labels: `q-<state>` (projection; `waiting` maps to `q-blocked`); type `code|docs|research|asset`; `p0|p1|p2` (lower first, absent = 2; `p0`: urgent grant, § 4);
@@ § 3 Attempts, lease, capacity
-  sum of host limits bounds the project.
+  sum of host limits bounds the project. A valid `p0-extra` adds one slot for its task only (§ 4).
@@ § 4 Service
 - Pause: `service pause` (and the tray's) saves the focus to `focus.paused` and writes `off`; resume restores it. ...
+- Urgent: an open task with `p0` and one `host-<this>`, each last added by a trusted account (label events), passes focus; the lowest such number only. Every other filter and claim stay; no child is stopped. `p0-extra` lets it alone exceed `limits.<rt>` (> 0) by one child. On `later`/`failed` the service removes `p0`, `p0-extra`. Focus is never changed. `status.json` names the grant.
@@ § 5 Commands
+| `taskq urgent N [--extra]` / `taskq urgent N off` | after one confirmed summary: set/remove `p0` (+`p0-extra`), needs one `host-*`; success only after the host's readback |
```

## Proof obligations (implementation task)

Service unit tests (`src/service.rs`), board fakes, no real spawn:
1. Focus `off` + valid grant on N for this host: N is admitted, other ready tasks are not.
2. `p0` without `host-*`, with two `host-*`, or `host-other`: N is not admitted outside focus here.
3. `p0` added by an untrusted actor: no bypass; within focus it sorts as priority 0 only.
4. Grant with missing `needs-x`, open dependency or `run-codex` with limit 0: N is not admitted.
5. Busy normal limit: without `p0-extra` N waits and starts first when a child ends; no child is stopped.
6. `p0-extra`: exactly one child over the limit, only for N; another task gets no extra slot; `p0-extra` without
   `p0` grants nothing.
7. N `failed` or `later`: `p0`, `p0-extra` removed; next pass admits under focus only; `focus` bytes unchanged.
8. `ask`, `requeue`, `gone`: the grant stays; reviewer of N is admitted under it.
9. Two valid grants: only the lower number bypasses focus. Repeat `urgent N`: no write. `urgent M` during N: refused.
10. Command readback: stale, missing or older-version `status.json` gives `unknown`, not success.
11. Older service fake: `p0` gives no bypass (N-1 safety).
Process check on one host (R12): a second registered project with focus `off`, one real urgent run to close.
Windows, GitLab label events and multi-host: unknown until tested.

## Later (separate proposals)

- Cross-host urgent: the same `p0` with another host's `host-*`, or none and a host choice rule; needs a two-host test.
- Tray action `Urgent…` (menu.md M7 AI channel) using the same command.
- Host-wide cap across projects, if urgent runs crowd a host.
- `p0` code catalog entry (`src/labels.rs`) and `taskq add --priority 0` ship with the implementation task.
