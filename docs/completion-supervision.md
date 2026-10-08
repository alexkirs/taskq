---
type: Design
status: Accepted subset implemented (#223); deferred items listed
domain: agent-workflow
---

# Completion supervision (#223)

Base: main `90786be` (after #217 late output, #219 Codex budget, #221 idempotent close, #226 Claude
budget, #230 focused testing). Line references are of `taskq/` at that commit, read on 2026-10-08.

The board-first contract holds: a `result`, `ask`, `answer`, `reject`, `close` is one `save()` (label and
block in one PUT, then the trusted note, `taskq/__init__.py:465`), the tick reads them from the tracker
(`tick.py` review and ask lists), and no step needs a session UI. What was missing was not a transport
but the correlation between what the tick delivered and what is still pending.

## Diagnosis

| # | Case | Existing path | Was missing |
|---|------|---------------|-------------|
| A | Review wake after a same-SHA resubmission, a changed `checks`, a research result (`sha` None), or a second question after an answer | wake key `sha256(session, judgement)`; judgement items `review N SHA` and `ask N` | no revision in the key: an identical set was "already woken", so a resubmitted result or a repeated question never woke the coordinator again |
| B | Wake delivered, coordinator interrupted before it applied a decision | `woken()` written right after `claude_wake` returned | delivered was treated as applied; the pending items were not re-read before delivery |
| C | Worker turn ends with an incomplete outcome (a `problem` note by the claim session, no `result`/`ask`) | `problem --task` is history only (`worker.py:488`); the idle worker was nudged every tick | the note never reached judgement |
| D | Codex app metadata `notLoaded`/`interrupted` while the rollout tail shows the turn running (session 01a11a17, turn 01a11a17-b416) | `codex_app_running` overrides the turn to `inProgress`; `codex_observation` reports `active` | the observation did not say that two sources disagreed nor which one decided |
| E | Duplicate or out-of-order decisions | `core.task(iid, states)` refuses any decision whose state does not fit; `unchanged()` guards tick writes | nothing: regression only |
| F | Result while the Codex session is `notLoaded`; owner answer through the board | `review` and `ask` come from labels and the block, never from a session | nothing: regression only |
| G | Partial write: PUT applied, note failed | `handed_in` prints `none`; the block still holds `result` | the revision now includes the block |

Not touched: #217 record recovery, #219/#226 budget exclusion, #221 close receipt and recovery, the
lock, reservations, `unchanged()` guards, #216 over-cap admission guard, global caps (Claude 2, Codex 6).

## Correlation: pending line, decision, acknowledgement

A **pending line** (`tick.pending`) is `<state> <iid> <claim runtime>:<claim session> <revision>`.
The **revision** (`tick.revision`) is `<kind> <note id> <result block>`: the newest trusted note that put
the item where it is (the claim session's `result` for review; anyone's `ask`, as `question` reads it;
the claim session's `result`, `ask` or `problem` for doing) and the result block itself (`sha` and
`checks`, `json.dumps` sorted). With no such note (a partial write) the revision is `block <result block>`.
The block is always part of it, so a PUT that landed with a new `checks` and the same SHA while its note
failed is a new revision with the old note id.

The pending set of a pass is every `review` item with a result, every `ask` item (shown or not; the daily
summary hides a question from the judgement lines, never from the set) and every `stuck` doing item.

A **decision** is exactly one of the existing state-changing commands by a session other than the claim
session: `close`, `reject`, `answer`, `later`, `release`. Each changes the state in the same `save()` as
its note, so the item leaves the pending set or changes its line. `shown`, `beat`, `problem` and any
other note are not decisions and change nothing in the line.

**Delivered** means `claude_wake` returned for a key that covered the line. A line that is gone on a
later read is **resolved or superseded**. Neither is an acknowledgement: the tick records every wake as
`acknowledged: unknown` and nothing in the product reads a receipt or an application from a delivery.
An acknowledgement would need correlated evidence, the decision note on that revision by that claim,
which only the coordinator's own command writes.

## Implemented (`taskq/tick.py`, `taskq/codex.py`)

1. **Revision in the wake key.** `question()` also returns the ask note id. `wake(output, judgement,
   pending)` hashes `session + judgement lines + pending lines`. The judgement lines and the report's
   refusals are unchanged.
2. **Delivered is not applied.** Immediately before the send, each pending item is re-read
   (`still_pending`: fresh issue, fresh trusted notes, same line); one that moved stops the delivery for
   this pass (`pending changed`). `woken()` stores the key and the time. The same key is sent once more
   to a listed, idle coordinator after `TICK_LIVE_MINUTES` (`delivered again`); a busy or unlisted
   coordinator is left alone as before. The report action `wake` carries `status` (`delivered`,
   `delivered again`, `already delivered`, `pending changed`, `coordinator busy`, `coordinator unknown`,
   `no coordinator`), `acknowledged: unknown` and the pending lines sent.
3. **Problem note is judgement.** A doing item without result whose session is not busy and whose
   revision is a `problem` note is `stuck`: listed under `## Worker problems` with the note, judgement
   line `stuck N <note id>`, removed from the idle and quiet nudges. Its claim stays; nothing is released
   or replaced. The coordinator's routes are the existing ones: a send to that same session (`claude
   --bg --resume`, `codex-send`) or `release` only for a stopped session. `answer` is not accepted on
   `doing` and was not added; a worker that needs a decision uses `ask`.
4. **Codex observation provenance.** `codex_rollout(metadata, turn)` reads the thread's own tail:
   `running`, `ended` or `unread`. `codex_snapshot` keeps `sources: {app: '<status>/<turn status>',
   rollout: ...}` on the newest turn (`not consulted` when the app server did not say `notLoaded`).
   `codex_observation` adds `sources` and `conflict` (True only when the app said `notLoaded` and the
   tail said running: the tail decided) and names `source: rollout tail over app metadata` then; `status`
   and `exact_blocker` are unchanged. `codex-read` prints the sources line. No platform fix is claimed.

## Deferred, not in #223

- `tick --observe`, a read-only pass: its CLI flag lives in `__init__.py`, owned by #232. No read-only
  mode exists today; over-cap and unknown inventory keep refusing the native `--act` pass (#216).
- Intake triage (G1): a criterion for later review only. Dispatchable when the assignee is the
  coordinator or owner uid, or history has a trusted `answer` by a non-claim session, a `reserve` or a
  `take`; author equality is positive evidence only, inequality proves nothing. Needs `parse()` to carry
  `author`. #227 stays as the owner parked it.
- Over-cap native pass with zero starts (G2): rejected; reconcile, release, update, archive and cleanup
  are effects.

## Tests — `tests/test_completion_supervision.py`

Six cases, each with a mutation that fails it: resubmitted result and second question wake again
(block, note id); every open question is pending while the summary hides it, and an owner answer through
the board resolves it; the block is part of the revision (same SHA, new checks, old note), delivery is
rechecked and re-sent once after `TICK_LIVE_MINUTES` with `acknowledged: unknown`, a decision leaves
nothing that says applied; a problem note reaches judgement and keeps the claim, `answer` on doing is
refused, a busy worker is never stuck; the observation names both sources and the conflict, the snapshot
reads running, ended and unread tails; a result while the thread is `notLoaded` is reviewed without
nudge or release, out-of-order decisions are refused by state, the next brief holds the answer.

Negative controls run on 2026-10-08: dropping the pending lines from the key, the pre-send recheck, the
block from the revision, the stuck detection, or the conflict flag each fails the module.

## Qualification on the actual route (read only, 2026-10-08)

Within the approved capacities (global Claude 2, Codex 6) nothing was launched and no tick was run on
the live queue: a tick pass writes (release, board moves, `shown`), and a read-only pass is deferred.
What was run against the real tracker and the real Codex app server, with a guard that refused any
non-`GET` call:

- `tick.pending` and `still_pending` on the live board (14 calls, all `GET`): reviews #185 (claim
  claude:7b1c192f, result note 6046172806), #191 (codex:01a117a1, note 6045339349), #198
  (claude:aac1d605, note 6047682662), #205 (claude:3b45fa67, note 6047773821), each still pending on a
  fresh read; #223 doing (claude:e3055b14) with revision `ask 6056530693`, not stuck.
- `codex-read 01a11a17-af1a-7780-8b45-a4e266948bad --limit 1` (the #219 repro thread): `status:
  notLoaded`, `sources: app notLoaded/completed, rollout ended`, no conflict now that the turn has its
  end record. `codex-read 01a11a3b-…` (#221 thread): the same shape.
- #228 read once (`GET issues/228`): open, no labels, `updated_at 2026-10-08T06:46:46Z`; nothing written.

Not qualified on the actual route, because it needs a write or a launch: a result written while the
worker thread is `notLoaded`, a wake re-sent to a real coordinator, a real problem note. Those are
fixture evidence. The cloud `limit 1/2` stale page of the Codex platform stays a reported limitation.
