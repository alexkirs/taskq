---
type: Proposal
status: PROPOSED (docs-first stage of #223; nothing below is implemented)
domain: agent-workflow
---

# Completion supervision: the missing correlations (#223)

Base: main `620d020` (after #217 late output, #219 Codex budget, #221 idempotent close, #226 Claude budget).
Read against the actual sources on 2026-10-08; line numbers are of that commit.

The board-first contract of #223 holds already: a `result`, `ask`, `answer`, `reject`, `close` is one
`save()` (label + block in one PUT, then the trusted note, `taskq/__init__.py:465`), the tick reads
them from the tracker (`tick.py:835-843`), and no step needs a session UI. What is missing is not a
transport but four correlations between what the tick delivered and what was applied.

## Diagnosis: what exists, what is missing

| # | Case | Existing path | Missing |
|---|------|---------------|---------|
| A | Review wake after a same-SHA resubmission, a changed `checks`, a research result (`sha` None), or a second question after an answer | wake key `sha256(session, judgement)` at `tick.py:547`; judgement items `review N SHA` and `ask N` at `tick.py:960` | the key carries no revision: an identical set is "already woken" (`tick.py:548`), so a resubmitted result or a repeated question never wakes the coordinator again |
| B | Wake delivered, coordinator interrupted (or `--resume` died after the 15 s window, `worker.py:923`) before it applied a decision | `woken()` written right after `claude_wake` returns (`tick.py:561`) | delivered is treated as applied: the same key is never re-sent; nothing re-reads whether a decision was written |
| C | Worker turn ends with an incomplete outcome (a `problem` note by the claim session, no `result`/`ask`) | `problem --task` is history only (`worker.py:488`); the idle worker is nudged every tick (`tick.py:886-903`) | the note never reaches judgement; the nudge repeats until the 120 min stale release |
| D | Codex app metadata `notLoaded`/`interrupted` while the rollout tail shows the turn still running (repro of session 01a11a17, turn 01a11a17-b416) | `codex_app_running` overrides the turn to `inProgress` (`codex.py:413-432`); `codex_observation` then reports `active` | the observation does not say that two sources disagreed nor which one decided; the PM report cannot tell "active by rollout tail" from "active by app" |
| E | Duplicate or out-of-order decisions (`answer` twice, `reject` on a doing task, stale `close`) | `core.task(iid, states)` refuses any decision whose state does not fit (`worker.py:294`, `:335`); `unchanged()` guards tick writes (`__init__.py:449`) | nothing: the state machine already refuses them. Needs a regression, not code |
| F | Result while the Codex session is `notLoaded`; owner answer through the board | `review` and `ask` are read from labels and the result block, never from a session (`tick.py:837-839`); `liveness` runs only for `doing` (`tick.py:329`) | nothing; needs a regression that no nudge, spawn or release follows |
| G | Partial write: PUT applied, note failed | `handed_in` returns `none` (`__init__.py:514`); `question` returns `no question note` (`tick.py:53`); the block still holds `result` | the review line already prints both; the proposal keys the wake on the block when no note exists |
| H | Intake: a collaborator-authored issue is `ready` by `add`'s default label (`worker.py:84`) and dispatched by the next tick | `later`/`answer` exist (`worker.py:120`, `:291`); #227 was parked by hand | the tick cannot tell a manager-approved task from a raw request: `parse()` drops the issue author (`__init__.py:384`) |
| I | Over-cap or unknown compute budget | `admitted()` refuses the whole native pass above cap (`multiproject.py:891-893`) and on unknown inventory (`:887`) | above a known cap, review listing, board moves, accepted close/retire and the wake are also skipped, although the pass itself admits nothing when its limit is 0 (`starts()`, `tick.py:458`) |

Not missing, not touched: #217 record recovery, #219/#226 budget exclusion, #221 close receipt and
recovery, the lock, reservations, `unchanged()` guards, capacity admission.

## Proposed changes (smallest diff, existing APIs only)

### 1. Revision in the wake key (A, G) — `taskq/tick.py`

`question(iid)` returns the question text, its `shown` stamp and the id of the `**ask**` note it found.
A new local `revision(item)` returns the id of the newest trusted note by the claim session whose head
is `**result**`, `**ask**` or `**problem**` (one page of `core.comments`, same read `handed_in` does);
`None` when there is no such note (partial write: the block revision `json.dumps(item['result'])` stands in).
The judgement lines at `tick.py:960` stay as they are (report contract unchanged); `wake()` gets a second
argument, the revisions, and hashes `session + judgement + revisions`. Same SHA with new `checks`, a
research result, a second question after an answer: a new note id, a new key, one more wake.

### 2. Delivered is not applied (B) — `taskq/tick.py`

`woken()` stores `key` and the wake time. `wake()` sends again when all of these hold: the key is equal,
the coordinator is listed and not busy (`busy()`, existing), the wake is older than `TICK_LIVE_MINUTES`,
and no judged item has a trusted note newer than that wake time (the coordinator wrote nothing on them:
no `close`, `reject`, `answer`, `shown`). Bounded: one wake per `TICK_LIVE_MINUTES` while the items stay
untouched. Restart safe: the file is the only state and already exists. A coordinator that is busy or
unlisted is left alone exactly as now (`tick.py:551-559`).

### 3. Problem note is judgement, not a nudge (C) — `taskq/tick.py`

In the idle loop (`tick.py:875`): a `doing` item whose `revision()` is a `**problem**` note newer than
the item's `take`/`answer` note is removed from `idle`/`claude_idle`/`quiet` and added to a new list
`stuck`, printed as `## Worker problems` with the note (fenced by `core.data`) and the three existing
commands: `answer N` (in-session continuation keeps the claim), `release N`, `reject` is not applicable.
Judgement line `stuck N <note id>`. No new state, no new note type.

### 4. Source precedence in the Codex observation (D) — `taskq/codex.py`

`codex_snapshot` keeps on `turns[0]` the API status it read before the override (`api_status`).
`codex_observation` adds `'sources': {'app': <metadata status type>/<api turn status>, 'rollout': 'running'|'ended'|'unread'}`
and `'conflict': True` when the app says `notLoaded`/`interrupted` and the rollout tail says running.
`'source'` becomes `'rollout tail over app metadata'` in that case. `exact_blocker` and `status` are
unchanged: the precedence (fresh timestamped rollout over stale app metadata) is what the repro
already does; the change only makes it visible and testable. No platform fix is claimed; the cloud
`limit 1/2` stale page is reported as a platform limitation in the qualification record.

### 5. Intake from collaborators (H) — needs scope gap 1

`parse()` adds `'author': (issue.get('author') or {}).get('id')` (one line, `taskq/__init__.py:384`).
In `tick.py` `starts()`: a `ready` task whose author is not the profile uid (`args.profile['uid']`) and
whose trusted history has no coordinator decision (`answer`, `ready`, `reserve`, `take` by any session)
is not started; with `act` it is moved to `later` once with the note `intake: awaiting manager triage`
(`core.save(item, 'later', 'later', text, waiting_for=text)` after `unchanged()`), else the command is
printed. The coordinator's `answer N` is the correlated approval: `later -> ready` once, by the existing
command. Grandfathered: every task with any trusted taskq note (all currently owned or reserved work).
#227 stays as the owner parked it; #228 is never read for writing.

### 6. Over-cap runs the pass with zero admission (I) — needs scope gap 2

`admitted()` at `multiproject.py:891-893`: for a runtime known above its cap, set `limits[runtime] = 0`
and append `'<runtime> occupancy N above cap M: no new admission'` to `result['errors']` instead of
returning. The native pass then lists reviews, shows questions, moves boards, retires closed work and
wakes the coordinator under the existing per-action guards; `starts()` admits nothing at limit 0.
Unknown inventory (`:887`) and unknown catalog ownership (`:881`) stay refused: the native pass has
no read-only mode, and its dead/stalled release on unknown ownership is exactly what the owner forbids.
A read-only listing for the unknown case is a separate proposal, not part of #223.

## Tests — `tests/test_completion_supervision.py` (new, `test_taskq.Cycle` fixtures)

1. Same-SHA resubmission after reject, research result (`sha` None), second question after answer: each wakes once more; an unchanged set does not (A).
2. Wake sent, coordinator idle, no decision note, `TICK_LIVE_MINUTES` passed: one re-wake; with a `reject` note after the wake: none; busy coordinator: none (B).
3. `problem` note on a doing task with an idle session: no nudge, one `stuck` judgement with the note id; after `answer` in-session the task continues with the same claim (C).
4. `codex_observation` with app `notLoaded` + running rollout tail: `status active`, `conflict True`, `sources` filled; with an ended tail: `terminal`, `conflict False` (D).
5. Second `answer`, `reject` on doing, `close` on a changed result: refused by state, no note written (E).
6. Result while the Codex thread reads `notLoaded`: review listed, no nudge, no spawn, no release; owner `answer` from the shell: `ready`, next worker brief holds the answer (F).
7. PUT applied and note failed: review line prints `none`, wake key uses the block, a later note changes the key once (G).
8. Collaborator-authored ready task with no trusted history: not started, parked to `later` once; `answer` makes it `ready`; a task with a `take` note is started as before (H, after scope gap 1).
9. Known over-cap: native pass runs with limit 0, zero `spawn`/`reserve`, review and retire steps recorded; unknown inventory: refused as now (I, after scope gap 2, in `tests/test_multiproject.py`).

Focused runs while changing: `python3 -m unittest tests.test_completion_supervision`; full gate once frozen.

## Actual-route qualification (separate from unit tests)

One bounded cycle on this repository, recorded in `docs/completion-supervision.md` § Qualification with
exact identities: task iid, claim session, result note id and SHA, wake key, coordinator session, the
decision note id, close receipt. Cases: a `result` written while the Codex worker thread is `notLoaded`;
a Claude PM wake interrupted (`claude --bg --resume` killed after delivery) and re-woken once; an owner
`answer` from the shell. Nothing is launched above the current budget (Claude 2/6, Codex 4/4 complete
catalog); #228 is read only as a negative control (unchanged labels, body, comments).

## Scope gaps (need coordinator ACCEPT before any edit)

1. `taskq/__init__.py:384` `parse()`: one key `author`. Without it, item 5 cannot tell a request from an approved task.
2. `taskq/multiproject.py:891-893` `admitted()`: zero limit instead of refusal above a known cap; `tests/test_multiproject.py` one regression.

Items 1-4 and tests 1-7 stay inside the registered scope (`codex.py`, `tick.py`, the new test, this file).
No `worker.py` change is needed.
