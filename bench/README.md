# Orchestration cadence test (#269)

Each executor has a folder `bench/cadence/<executor>/` (`claude`, `codex`, ...). Each test task writes exactly one
file `bench/cadence/<executor>/<N>.txt` with one line, then goes through the normal flow (worker, PR, review, close,
merge to main):

```
#<N> executor=<executor> taken=<UTC ISO time when the worker started> finished=<UTC ISO time before result>
```

One file per task, so parallel tasks never conflict. Runs: 10 tasks first, then 20, then 50. Compare per executor:
first spawn to last merge, median and max interval between merges, stalls. Merge times come from `git log`.
