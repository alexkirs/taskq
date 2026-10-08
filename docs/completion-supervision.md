---
type: Proposal
status: PROPOSED, revision 2 (docs-first stage of #223; nothing below is implemented)
domain: agent-workflow
---

# Completion supervision: the missing correlations (#223)

Base: main `90786be` (after #217 late output, #219 Codex budget, #221 idempotent close, #226 Claude
budget, #230 focused testing). Line numbers are of `taskq/` at that commit, read on 2026-10-08.
Revision 2 applies the coordinator's technical corrections on e613a4f.

The board-first contract of #223 holds already: a `result`, `ask`, `answer`, `reject`, `close` is one
`save()` (label and block in one PUT, then the trusted note, `taskq/__init__.py:465`), the tick reads
them from the tracker (`tick.py:835-843`), and no step needs a session UI. What is missing is not a
transport but a few correlations between what the tick delivered and what was applied.

## Diagnosis: what exists, what is missing

| # | Case | Existing path | Missing |
|---|------|---------------|---------|
| A | Review wake after a same-SHA resubmission, a changed `checks`, a research result (`sha` None), or a second question after an answer | wake key `sha256(session, judgement)` at `tick.py:547`; judgement items `review N SHA` and `ask N` at `tick.py:960` | the key carries no revision: an identical set is "already woken" (`tick.py:548`), so a resubmitted result or a repeated question never wakes the coordinator again |
| B | Wake delivered, coordinator interrupted (or `--resume` died after the 15 s window, `worker.py:923`) before it applied a decision | `woken()` written right after `claude_wake` returns (`tick.py:561`) | delivered is treated as applied: the same key is never re-sent; the pending items are not re-read before delivery |
| C | Worker turn ends with an incomplete outcome (a `problem` note by the claim session, no `result`/`ask`) | `problem --task` is history only (`worker.py:488`); the idle worker is nudged every tick (`tick.py:886-903`) | the note never reaches judgement; the nudge repeats until the 120 min stale release |
| D | Codex app metadata `notLoaded`/`interrupted` while the rollout tail shows the turn still running (repro of session 01a11a17, turn 01a11a17-b416) | `codex_app_running` overrides the turn to `inProgress` (`codex.py:413-432`); `codex_observation` then reports `active` | the observation does not say that two sources disagreed nor which one decided |
| E | Duplicate or out-of-order decisions (`answer` twice, `reject` on a doing task, stale `close`) | `core.task(iid, states)` refuses any decision whose state does not fit (`worker.py:294`, `:335`); `unchanged()` guards tick writes (`__init__.py:449`) | nothing: the state machine already refuses them. Needs a regression, not code |
| F | Result while the Codex session is `notLoaded`; owner answer through the board | `review` and `ask` are read from labels and the result block, never from a session (`tick.py:837-839`); `liveness` runs only for `doing` (`tick.py:329`) | nothing; needs a regression that no nudge, spawn or release follows |
| G | Partial write: PUT applied, note failed | `handed_in` returns `none` (`__init__.py:514`); `question` returns `no question note` (`tick.py:53`); the block still holds `result` | the review line already prints both; the proposal keys the wake on the block when no note exists |
| H | Intake: a collaborator-authored issue is `ready` by `add`'s default label (`worker.py:84`) and dispatched by the next tick | `later`/`answer` exist (`worker.py:120`, `:291`); #227 was parked by hand | the tick has no explicit triage evidence to read (criterion below, outside scope until reviewed) |
| I | Over-cap or unknown compute budget | `admitted()` refuses the whole native pass above cap (`multiproject.py:891-893`) and on unknown inventory (`:887`); #216 forbids native effects over cap | there is no native pass without effects: review listing and the wake can only come with release, reconcile, board moves, archive, cleanup |

Not missing, not touched: #217 record recovery, #219/#226 budget exclusion, #221 close receipt and
recovery, the lock, reservations, `unchanged()` guards, #216 over-cap admission guard.

## Correlation: pending revision, decision, acknowledgement

A **pending item** is the tuple `(iid, claim.runtime, claim.session, kind, revision)` where `kind` is
`review`, `ask` or `stuck` and `revision` is the id of the newest trusted note by the claim session whose
head is `**result**`, `**ask**` or `**problem**`; with no such note (a partial write) the revision is the
block, `json.dumps(item['result'])`. The reservation attempt, when the claim came from one, is part of
the claim's `take` note and does not change within a claim; the claim session is the attempt identity.

A **decision** on a pending item is exactly one of the existing state-changing notes written by a
session other than the claim session: `close`, `reject`, `answer`, `later`, `release`. Each of them
changes the item's state in the same `save()` as its note, so the item leaves the pending set or
changes its revision. `shown`, `beat`, `problem` and any other note are not decisions.

**Applied** means: on a fresh read the item is no longer pending with the same tuple. **Delivered**
means: `claude_wake` returned for a key that covered the tuple. Between the two the acknowledgement
is **unknown**; nothing claims it and nothing repeats a decision. The tick never infers applied from a
note; it reads the state.

## Proposed changes (smallest diff, existing APIs only)

### 1. Revision in the wake key (A, G) — `taskq/tick.py`

`question(iid)` also returns the id of the `**ask**` note it found. A new local `revision(item)` returns
the pending revision defined above (one page of `core.comments`, the read `handed_in` already does).
The judgement lines at `tick.py:960` stay as they are (report contract unchanged); `wake()` gets the
pending tuples as a second argument and hashes `session + judgement + tuples`. Same SHA with new
`checks`, a research result, a second question after an answer: a new note id, a new key, one more wake.

### 2. Delivered is not applied (B) — `taskq/tick.py`

Before delivery, `wake()` re-reads each pending item with `core.unchanged(item)` (existing: state,
claim, reservation, `updated_at`, lock). Any item that moved makes this pass skip the wake; the next
tick recomputes the set from fresh reads. `woken()` stores the key and the wake time. The same key is
sent again only when the coordinator is listed and not busy (`busy()`, `tick.py:392`) and the wake is
older than `TICK_LIVE_MINUTES`; the key is equal exactly when no pending tuple changed, which is the
definition of not applied. The report records `wake` with `status: delivered` and `acknowledged:
unknown`; applied is never written by the tick. Bounded: one wake per `TICK_LIVE_MINUTES` while the
tuples stay pending. Restart safe: the file is the only state and already exists. A busy or unlisted
coordinator is left alone as now (`tick.py:551-559`).

### 3. Problem note is judgement, not a nudge (C) — `taskq/tick.py`

In the idle loop (`tick.py:875`): a `doing` item whose revision is a `**problem**` note by the claim
session is removed from `idle`, `claude_idle` and `quiet` and listed as `stuck`, printed as
`## Worker problems` with the note fenced by `core.data`. Ownership is preserved: the claim stays, no
release, no nudge, no replacement. Judgement line `stuck N <note id>`.

The coordinator's routes are the existing ones. Continuation in the same conversation: the supported
send to the exact claim session, `claude_wake(session, text)` for Claude (`worker.py:923`, what the
idle nudge uses) and `codex-send` for Codex, guarded as today by `local_claim` and a liveness that is
not `dead`; the text names the next step. A dead session (positive evidence only, `liveness` returns
`dead`) is released by the existing dead-release path, never by the stuck listing. `answer` is not
accepted on `doing` (`worker.py:294`) and this proposal does not add it: the worker that needs an
owner decision uses `ask`, which is the state for it. After the send, the item stays `stuck` with the
same revision until the worker writes `ask`, `result` or a newer `problem`; acknowledgement is unknown
until then, and item 2 bounds the re-wake. No new state, no new note type, no lifecycle scope gap.

### 4. Source precedence in the Codex observation (D) — `taskq/codex.py`

`codex_snapshot` keeps on `turns[0]` the API turn status it read before the override (`api_status`).
`codex_observation` adds `'sources': {'app': '<metadata status type>/<api turn status>', 'rollout':
'running' | 'ended' | 'unread'}` and `'conflict': True` when the app says `notLoaded`/`interrupted` and
the rollout tail says running. `'source'` becomes `'rollout tail over app metadata'` in that case.
`status` and `exact_blocker` are unchanged: the precedence (fresh timestamped rollout over stale app
metadata) is what the repro already does; the change only makes it visible and testable. No platform
fix is claimed; the cloud `limit 1/2` stale page stays a reported platform limitation.

### 5. Observation lane without effects (I) — `taskq/tick.py`, no multiproject change

New flag `tick --observe` (with `--json` and `--wake` as today, exclusive with `--act`). It runs
`queue_pass` with every write site skipped: dead/stalled release (`tick.py:788-796`), reservation
reconcile (`:798`), unlock (`:811-815`), board moves (`:818`), ready/waiting moves (`:822-834`),
`retire_closed`, `archive_finished_codex`, cleanup `scheduled` (`:852-855`), nudges, launch, `shown`
notes (`:939`, `:946`) and Codex archive. What remains is read-only: fresh tracker reads, liveness
reads, the v1 report, the pending tuples and the judgement list. The only effect it can have is the
wake of items 1-2, which gives the coordinator one turn; every action the coordinator then takes runs
through its own command and guards (`close` with #221, `reject`/`answer` with the state guards,
`release` only on positive dead evidence). `--observe` is a ceiling: `core.api` is wrapped for the pass
so that any method other than `GET` fails the pass instead of writing.

Multiproject is not edited. Above a known cap, or with unknown inventory, `admitted()` keeps refusing
the native `--act` pass exactly as #216 accepted. Whether a catalog binding may select `observe` as an
effect is a separate proposal for the multiproject owner; until then the observe lane is run by the
owner's own tick on this checkout. Unknown inventory or ownership blocks all new admission and every
dependent action; it does not block a read-only pass.

### 6. Intake criterion (H) — outside scope until reviewed, no edit proposed now

Explicit triage must be read from existing history and preserved assignees, never inferred from the
author alone or from the existence of a trusted note. Criterion for review: a `ready` task is
dispatchable when at least one holds, else it is intake.

- The task is assigned to the coordinator or owner uid (`add --mine`, `take` assigns; `assignees` is in `parse()` already).
- Its history has a trusted `answer` note by a non-claim session (an explicit owner or coordinator decision), a `reserve` note (an explicit admission), or a `take` note (owned or formerly owned work: grandfathered).
- The issue author is the coordinator uid (positive evidence only; `author.id` is provider compatible, GitHub mapped at `store_github.py:215`). Author inequality proves nothing.

Intake handling would be: not started by `starts()`; with `--act`, moved to `later` once with the
note `intake: awaiting manager triage` after `unchanged()`; the coordinator's `answer` is the
correlated approval, `later -> ready` once, by the existing command. This needs `parse()` to carry
`author` (`__init__.py:384`, one key) and is not edited under #223 until the criterion is accepted.
#227 stays as the owner parked it. #228 is never touched.

## Tests — `tests/test_completion_supervision.py` (new, `test_taskq.Cycle` fixtures)

Focused runs while changing: `python3 -m unittest tests.test_completion_supervision`. Full suite once
the implementation is frozen, as [brief].rules requires for tick behavior.

1. Same-SHA resubmission after reject, research result (`sha` None), second question after answer: each wakes once more; an unchanged set does not (A).
2. Wake delivered; a pending item moved between the read and the wake: no wake this pass. Delivered, coordinator idle, `TICK_LIVE_MINUTES` passed, tuples unchanged: one re-wake, report `acknowledged: unknown`; with a `reject` on the item: key changes, no re-wake of the old key; busy coordinator: none. Interruption is fixture evidence only (B).
3. `problem` note on a doing task with an idle session: no nudge, no release, one `stuck` judgement with the note id, claim unchanged; a newer `ask` by the worker ends `stuck` (C).
4. `codex_observation` with app `notLoaded` and a running rollout tail: `status active`, `conflict True`, `sources` filled; with an ended tail: `terminal`, `conflict False` (D).
5. Second `answer`, `reject` on doing, `close` on a changed result: refused by state, no note written (E).
6. Result while the Codex thread reads `notLoaded`: review listed, no nudge, no spawn, no release; owner `answer` from the shell: `ready`, next worker brief holds the answer (F).
7. PUT applied and note failed: review line prints `none`, wake key uses the block, a later note changes the key once (G).
8. `tick --observe` on a queue with a dead claim, a stale reservation, a hand-moved card, a closed task to retire and a review: zero writes (the `GET` ceiling records no PUT/POST/DELETE), report and judgement equal to the read-only part of a normal pass (I).

## Actual-route qualification (separate from unit tests)

One bounded cycle on this repository within the approved capacities (global Claude 2, Codex 6, no new
launch above them), recorded in this file under § Qualification with exact identities: task iid, claim
session, result note id and SHA, wake key and time, coordinator session, the decision note id, the
close receipt. Cases: a `result` written while the Codex worker thread is `notLoaded`; an owner `answer`
from the shell; `tick --observe` on the live queue with the write ceiling on. Delivery evidence is the
`woken()` key and time; apply evidence is the decision note id and the item leaving the pending set on
the next read. No coordinator is killed to simulate interruption: that case is fixture evidence only.
#228 is read once as a negative control (labels, body, comments unchanged) and not polled.

## Scope gaps

None for items 1-5 and tests 1-8: they stay inside `codex.py`, `tick.py`, the new test and this file.
Item 6 (intake) is a reviewed criterion only; its one-key `parse()` change is proposed for a later
scope decision. No `worker.py` and no `multiproject.py` change.
