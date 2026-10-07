# Cleanup on existing ticks — local pilot (#197)

Status: **partial stage, proposed fixture behavior.** This is not an accepted production specification.
`taskq/cleanup_schedule.py` is not called by `tick`, `worker`, `cleanup` or the CLI, reads no live
configuration, and starts no scheduler or timer. Deterministic tests in `tests/test_cleanup_schedule.py`
are not a receipt of a real existing-tick cleanup. Spec acceptance, Wiki, integration and publication are held.

## Proposed configuration

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

`validate()` reports every invalid or contradictory field in one error: unknown keys, a key the schedule
does not take (`at` with hourly), custom with both or neither form, non-integer or non-positive
`interval_minutes`, bad `HH:MM`, unknown weekday, non-IANA zone. Missing keys take the defaults and are
listed in `defaulted`, so an install without `[cleanup]` resolves to enabled hourly UTC; an explicit
`enabled = false` or custom choice is kept as written. The draft's minimum of 60 minutes for
`interval_minutes` is dropped by the owner's decision.

## When cleanup is due

- No successful run yet: due on the first tick.
- Hourly and `interval_minutes`: last success plus the interval, counted in elapsed UTC time.
- Daily, weekly and custom weekdays: the first occurrence of `at` on an allowed day in the zone, strictly
  after the last success. A repeated wall time (DST fall back) uses its first occurrence; a skipped one
  (spring forward) uses the first valid instant after it, the transition itself (02:30 → 03:00 EDT).
- Downtime: one due instant in the past means one run; the next due is counted from that run. No catch-up.
- A failed or partial attempt sets `retry_at` = attempt + 1 hour. Until then ticks report `backoff`
  instead of rerunning every tick; `last_success` does not move.

## One attempt (`run`)

1. Take the shared cleanup lock (`flock` on `<state>.lock`, non-blocking). Held: report `busy`, change nothing.
2. Reread the state under the lock. A leftover `running` marker means its holder died: record
   `interrupted`, set the one-hour backoff. Its items are never replayed; the retry builds a fresh plan.
3. `tick` and `idle` triggers respect `enabled` and the schedule; `manual` (the owner's explicit
   cleanup) runs regardless but uses the same lock and state, so the next tick dedups against it.
4. Get a fresh native plan. Only `Remove` items are passed to `apply`, which rechecks each at action time
   and may refuse. `Ask the owner` items stay pending, never accepted. `Kept` and unknown sections are
   untouched, so active, dirty, unmerged, unknown-ownership and protected targets stay where native
   cleanup put them.
5. Outcome: `success` with no errors (pending asks and refusals included); `partial` if some removals
   succeeded and some raised; `failed` if the plan or every attempted removal raised. Only `success`
   moves `last_success`.

The report carries observed time, trigger, reason (`first run`, `due`, `not due`, `disabled`,
`backoff`, `retry after backoff`, `manual`), outcome, attempted/succeeded/refused/errors/pending asks,
last success, next due, timezone and whether it is the fallback.

State is one JSON file (`last_success`, `last_attempt`, `last_outcome`, `retry_at`, `running`), written
by atomic replace. Where it lives per checkout/profile is an integration decision, not made here.

## Not done in this stage

Wiring into tick and the idle path, the real state location, reading `[cleanup]` from `taskq.toml` /
`taskq.local.toml`, the tick report lines, #191 provenance fields, and a real bounded existing-tick
cleanup receipt.
