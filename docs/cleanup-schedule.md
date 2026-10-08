# Cleanup

`taskq cleanup` reports finished task trees, branches, and `T<N>` / `S<N>` Claude and Codex sessions. `--apply` removes only items the fresh plan proves safe, rechecking each one before acting.

Every owner tick runs the same cleanup at most once per hour. The mtime of `.local/taskq-cleanup-last` is the only schedule state. There is no timer, lock, retry state, timezone, or custom schedule.

```toml
[cleanup]
enabled = false # optional; default is true
```

`enabled` is the only cleanup setting. An idle stop also runs cleanup unless `[idle] cleanup = false`.

Sessions whose name does not start with `T<N>` or `S<N>` are ignored. Open-task, active, dirty, unmerged, protected, or unknown targets stay kept or require an owner decision.
