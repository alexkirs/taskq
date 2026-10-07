# Cleanup on existing ticks (#197)

Accepted spec: [Wiki Cleanup-schedule](https://github.com/alexkirs/taskq/wiki/Cleanup-schedule/6642908fbd612b7a3d85df1e8ac64d46e63f9bd5).
Contract summary: [taskq](../taskq/contracts/taskq.md) § Cleanup.

Status: **implemented, fixture-tested; live qualification pending.** `tests/test_cleanup_schedule.py` covers the
schedule deterministically; `tests/test_taskq.py` `Cleanup` runs the real native cleanup, tick call and idle
stop against disposable Git repositories. Neither is a receipt of a real existing-owner tick: that bounded
receipt (fresh plan, rechecks, verified state; no eligible Remove means a no-op receipt, not deletion
qualification) is recorded separately.

## Configuration

```toml
[cleanup]
enabled = true          # default
schedule = "hourly"     # default; hourly | daily | weekly | custom
timezone = "Etc/UTC"    # default fallback, reported as such; any IANA zone
# daily:  at = "HH:MM"
# weekly: at = "HH:MM", weekday = "mon".."sun"
# custom: interval_minutes = <any positive integer>   (no 60-minute minimum)
#     or: weekdays = ["mon", "thu"], at = "HH:MM"
```

The table lives in `taskq.toml` (team default) or `taskq.local.toml` (personal). A personal `[cleanup]`
replaces the shared one whole, so schedule keys never mix across files. Both are validated when read and
an invalid one stops every command with its file and the reason. Nothing is written back: a missing
table resolves to enabled hourly UTC, and explicit disable or custom choices stay as written.

Validation reports every invalid or contradictory field in one error:
- an unknown key, or a key the schedule does not take (`at` with hourly);
- custom with both forms or neither;
- `interval_minutes` that is not a positive integer, or too large to represent;
- `at` that is not a two-digit `HH:MM` string (`1:002`, `9:30` and `930` are refused);
- a schedule or weekday that is not a string, an unknown weekday, a zone that is not IANA.

A huge interval that is still representable is allowed. If adding it to the last success would pass the
last representable date, `next_due` raises a visible `ValueError`.

## When cleanup is due

- No successful run yet: due on the first owner tick.
- Hourly and `interval_minutes`: last success plus the interval, in elapsed UTC time.
- Last success is the attempt's completion, not its start. A three-hour run on a five-minute interval is
  next due five minutes after it ends.
- Daily, weekly and custom weekdays: the first occurrence of `at` on an allowed day in the zone, strictly
  after the last success. A repeated wall time (DST fall back) uses its first occurrence. A skipped one
  (spring forward) uses the first valid instant after it: 02:30 becomes 03:00 EDT.
- Downtime: a past due instant means one run, and the next due counts from that run. No catch-up.
- A failed or partial attempt sets `retry_at` to its completion plus one hour. Until then ticks report
  `backoff` instead of rerunning; `last_success` does not move.

## Where it runs

| Trigger | Caller | Respects `enabled` and the schedule |
|---|---|---|
| `tick` | every pass of the owner's tick (`[coordinator] machine`, or every machine when unset), with or without `--act`, from the main checkout on `main` | yes |
| `idle` | the idle stop of `tick --act`, unless `[idle] cleanup = false` | yes |
| `manual` | `taskq cleanup --apply` | no: the owner asked, but it shares the lock and state |

There is no timer of its own. After the idle stop no ticks run, so no cleanup runs. A tick outside the
main checkout or off `main` records a visible `refused` action and changes nothing. Another machine's tick
(#145) never cleans.

## One attempt

1. Take the shared lock (`<main checkout>/.local/taskq-cleanup.json.lock`, non-blocking, as the tick's own
   lock). If another holder has it, report `busy` and change nothing.
2. Reread the state under the lock. A leftover `running` marker means its holder died. Record it as
   `interrupted` with a one-hour backoff from its start; the retry builds a fresh plan and never replays.
3. Decide due or not due. On due, write `running`, then fetch and build a fresh native `cleanup_plan`.
4. The native executor acts on "Remove" items only, rechecking each against a fresh plan. "Ask the owner"
   items stay pending. A Claude app session stays a visible refusal (`requires coordinator application
   tool`). Active, dirty, unmerged and unknown-ownership targets, protected refs and other projects never
   reach Remove.
5. Outcome:
   - `success`: no errors; pending asks and refusals are listed apart.
   - `partial`: some acts succeeded and some raised.
   - `failed`: the plan failed, or every act raised.

   Only `success` moves `last_success`.

The state file (`last_success`, `last_attempt` = start, `last_finished`, `last_outcome`, `retry_at`,
`running`) is written by atomic replace.

## Report

Every attempt is one `cleanup` action in the tick report. That report is the #191 envelope, unchanged:
contract, provenance and unknown evidence stay as they are. The action carries:
- trigger, settings source, reason (`first run`, `due`, `not due`, `disabled`, `backoff`,
  `retry after backoff`, `manual`) and outcome;
- attempted, succeeded, refused, errors and pending asks;
- observed (start), due, finished, last success, next due in UTC and in the zone;
- timezone, and whether it is the missing-setting fallback.

A failed attempt marks the action `failed`, so the tick outcome is `failure`.
