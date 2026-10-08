# GitHub store benchmark

2026-10-08, `alexkirs/taskq`, `N=1`. `TASKQ_TRACE=1 taskq worker` used the real tracker and task #267.

| Operation | Before | After | HTTP calls |
|---|---:|---:|---:|
| Worker tick pass: list, current user, task notes | 2.80 s | 2.00 s | 3 -> 2 |

Removed call: `GET /user`. The existing GraphQL issue-list query now returns `viewer.databaseId`; the store caches it for the profile's user lookup. The list is still one GraphQL request and all tracker CLI calls retain the shared 60 s timeout and one safe-read retry.

Run the full live operation benchmark only with an intentional throwaway issue:

```sh
python3 bench/store.py --iid 267 --write
```

It records list, view, take, note, state move, result, close, and per-operation HTTP calls. Without `--write`, it runs only list and view.

## Tick calls

The ordinary worker/tick read path now uses no more calls than before: 2 rather than 3 for this task's worker pass. Remaining task-note read is required to print the assigned brief.
