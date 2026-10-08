# Orchestration cadence test

One file per executor: `bench/cadence-<executor>.txt` (e.g. `claude`, `codex`).
Each test task appends one line and goes through the normal flow (claim, result, review, merge to main, CI deploy):

```
#<N> executor=<executor> taken=<UTC ISO time of take> finished=<UTC ISO time before result>
```

Runs: 10 tasks first, then 20, then 50. Compare cadence (interval between finished times, total wall time) per executor.
Tasks are kept for history; delete them with their history only after the comparison.
