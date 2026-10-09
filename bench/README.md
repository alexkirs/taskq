# Orchestration cadence test (#269)

Each executor has a folder `bench/cadence/<executor>/` (`claude`, `codex`, ...). Each test task writes exactly one
file `bench/cadence/<executor>/<N>.txt` with one line, then goes through the normal flow (worker, PR, review, close,
merge to main):

```
#<N> executor=<executor> taken=<UTC ISO time the file work starts, after the required reads> finished=<UTC ISO time the file work ends, before the final write and commit>
```

One file per task, so parallel tasks never conflict. Series (owner decision 2026-10-09, #522): 1+1, then 3+3, then
10+10 tasks (claude + codex at once), limits unchanged (claude 4, codex 4); each stage starts only after the owner's go.
The earlier runs (10, then 20, then 50 tasks per executor) stay the baseline. Compare per executor:
first spawn to last merge, median and max interval between merges, stalls. Merge times come from `git log`.

Measured interval (#522): `taken` to `finished` is the worker's file work only.

- Required reads (`taskq.md`, the issue) come before it and are not measured.
- `taken`: after the required reads, captured once, by `date -u +%Y-%m-%dT%H:%M:%SZ` in the shell invocation that starts the file work.
  Never re-captured, never edited, never written from memory or an older run.
- `finished`: captured just before the final write and commit of the file.
- Spawn, result, review, merge and close are measured separately, from board comments and `git log`.
- A run where the manager was woken by hand is reported as such; it is not a clean run.
