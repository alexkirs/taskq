# Bootstrap qualification receipt (#177)

Bounded qualification against Wiki spec revision `6b9027b29bf43d18a24d2cc96ef98d314f396794`.
Scope: standalone macOS route on one machine, read-only probes and fixtures. This is not a live
multi-project rollout, and it starts no worker, tick, timer or claim.

## Live receipt (macOS, 2026-10-07 UTC)

Run by a Claude worker through its Bash tool in the `taskq-177` worktree. Home paths are shortened to
`<main checkout>`; nothing else is edited.

| Command | Exit | stdout | stderr |
|---|---|---|---|
| `pwd` | 0 | `<main checkout>/.worktrees/taskq-177` | empty |
| `date -u +%Y-%m-%dT%H:%M:%SZ` | 0 | `2026-10-07T18:47:32Z` | empty |
| `hostname -s` | 0 | `macbook-m2` | empty |
| `taskq --version` | 0 | `taskq 7ca51ba` | empty |
| `python3 -m taskq preflight --json` | 0 | ACK below | empty |
| `gh api user --jq .login` | 0 | `alexkirs` | empty |

Preflight ACK (`actions[0]`, candidate code):

```json
{"action": "local_command_ack", "status": "ready", "observed_at": "2026-10-07T18:47:33Z",
 "cwd": "<main checkout>", "host": "macbook-m2", "exit_code": 0, "stdout": "<main checkout>\n",
 "stderr": "", "source": "local subprocess", "exact_blocker": null,
 "runtime_capability": "unknown", "effective_launch_policy": "unknown"}
```

The ACK `cwd` is the resolved main checkout, not the worktree: preflight runs where workers are launched.
`gh api user` is the authorized read. Its exit 0 proves login and network on this route at this time.

## Fixture qualification

`python3 -m unittest tests.test_bootstrap_qualification` (8 tests):

- Real ACK: a real subprocess returns stdout, stderr, exit code, `observed_at`, cwd and host.
  `ready` leaves `runtime_capability` and `effective_launch_policy` as `unknown`.
- Negative ACKs: these cases give `status: unknown`, an exact blocker, outcome `failure` and a non-zero exit:
  exit 0 with another cwd, exit 1 (denied), a missing interpreter, a permission error, and a timeout.
- Failed bootstrap: after a failed ACK, there is no store call (so no claim), no spawn (Claude, Codex,
  executor), no wake, no `take` and no tick. The only action is `local_command_ack`.
- Network versus login: if `gh auth status` fails, `doctor` runs one authorized read, `gh api user`.
  A successful read gives no gap. Only a positively established authentication failure (HTTP 401,
  «Bad credentials», «Requires authentication») is a login gap, with the owner step `gh auth login`.
  Every other failure is named as that blocker, with the fix «rerun `gh api user` once the cause is
  fixed», never `auth login`: network denial (connect, DNS, timeout), TLS/certificate failure,
  HTTP 403 (for example «Resource not accessible by integration»), HTTP 5xx, and an unrecognised or
  empty error (`unknown failure`). Nothing writes credentials.

Fixes in scope: `taskq/doctor.py` adds `api_read`/`login_gap`, because `gh auth status` reported
«token invalid» on a sandbox network denial. `taskq/worker.py` adds `host` to the ACK.
`tests/test_taskq.py` pins the authorized read as a login failure in the existing doctor/setup fixtures.

## Not qualified (stays unknown)

- Cloud PM to local executor transport: the probe ran in a local worker. No cloud PM dispatch was observed.
- Runtime capability and effective scoped launch policy: always `unknown` in the ACK, never inferred.
- Live owner approval: permission request, one PM notification with a verified link, owner approval
  in the UI, and the same session continuing without a duplicate worker. Only fixtures cover this
  (`tests/test_runtime_observation.py`: wait is held and not nudged, wake is deduplicated).
  It is a separate live qualification. The safety rule stays: never approve for the owner, and
  never spawn a replacement.
- Claude pending approvals: not visible through `claude agents`, so they stay `unknown`.
- Cross-host launch gate: not enforced. Preflight proves local execution only.
- Codex route, Linux/WSL, other projects: not exercised here.
