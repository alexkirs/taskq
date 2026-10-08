# GitLab store benchmark

2026-10-08: unknown. This checkout is configured for GitHub only. A GitLab login exists on this machine, but no GitLab test project was supplied; the benchmark must not create a throwaway issue in an unapproved project.

When a test project is available, run:

```sh
cd <GitLab test checkout>
python3 <taskq checkout>/bench/store.py --iid <open-task-iid> --write
```

The script measures list, view, take, note, state move and close-equivalent, including HTTP calls. GitLab requests already use the same shared 60 s timeout and one safe-read retry as GitHub.
