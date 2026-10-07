# Recovery checklist: external DOT PM -> local TaskQ -> worker

Companion to [external-pm-tick.md](external-pm-tick.md), which stays the guide
for preflight, bounded selection, tick exit codes and delivery. This checklist
covers what to do when that path breaks. The
[PM contract](../taskq/contracts/taskq-manager.md) stays the source of truth.

## Ground rules

- Record every observation as: command, `observed_at`, `event_at`, source,
  session link, exact blocker, stdout, stderr, exit code.
- Classify each fact as **observed** (fresh output of a supported command) or
  **unknown** (missing, unavailable, or older than the current pass, i.e. stale).
  Stale facts are unknown; re-read them before acting.
- Empty `permission_requests`, empty active flags, `event_at=null` or a missing
  PID never prove that a worker ended or that no approval is pending.
- Keep ownership: the session that holds the claim keeps the task. Never invent
  a session or claim, never take over a live claim, never start a duplicate worker.
- Do not read private databases, auth stores, runtime rollouts or hidden reasoning.
  Use only supported commands.
- Any destructive recovery (release a claim, delete a lock or worktree, reset
  or force-push a branch, archive a session) is an owner decision:
  `taskq ask <iid> --text "..."`, then stop.

## Read-only diagnostics

Run from the main checkout. Capture both streams and the exit code.

| Command | Tells you |
| --- | --- |
| `taskq preflight --json` | Local command execution works (`status=ready`). Runtime capability and launch policy stay `unknown`. |
| `taskq view <iid> --notes 2` | Tracker state, claim holder (`runtime:session @host`), latest notes, result. |
| `taskq runtime-status --runtime {claude,codex} <session> --json` | Supported runtime metadata: status, `event_at`, source, permission requests, approval visibility, exact blocker. |
| `taskq worker --filter ... --no-mine --limit ...` | Selection and brief only. A brief is not a claim. |

Not read-only: `taskq tick` (with or without `--act`), `take`, `beat`, `ask`,
`answer`, `result`, `problem`, `release`, git push. Do not run them as diagnostics.

## Situations

### Stale observation

1. Treat the old value as unknown.
2. Re-run the read-only diagnostics above.
3. Act only on fresh output. If fresh output is still `unknown`, hold: no nudge,
   no release, no replacement.

### Permission wait

Positive evidence only: a typed permission request or `waitingOnApproval`.

1. Report the exact blocker and the session link to the owner.
2. The owner approves in the linked runtime UI. The PM never approves on the
   owner's behalf.
3. The same session continues after approval. Do not nudge, release as idle,
   or spawn a replacement.
4. If the owner answers a worker question in that session, the worker records
   it with `taskq answer <iid> --text "..."` and continues under the same claim.

No positive evidence is not "no wait": Claude approval visibility can be
`unknown`, and requests made before the connection or resolved elsewhere can be
invisible. Hold.

### Runtime source unavailable

`runtime-status` can exit 0 with `status=unknown` and `source=unavailable` or a
blocker. Exit 0 means the command ran; it does not mean the worker is healthy.
Record the blocker verbatim, keep the worker as is, and report it.

### Command failure

Read the exact stderr line before retrying:

- **Network denial** (DNS or connection error, timeout, `Operation not permitted`
  from a sandbox): the request never reached the server. Check the executor's
  network and sandbox policy. Do not change credentials.
- **Auth failure** (`401 Unauthorized`, `403 Forbidden`): the server answered
  and refused. The token or its scope is wrong. This is owner work; do not read
  or rotate credentials.
- For a failed mutation (`take`, `result`, push, acting tick), read the real
  tracker and git state (`taskq view`, `git ls-remote origin refs/heads/taskq-<iid>`)
  before any retry. Never report a result that did not land.
- A tick exit 1 with only an error, or any other nonzero code, is failure, not an
  empty queue (see the tick guide).

### Interrupted worker

1. `taskq view <iid>` shows the claim. `runtime-status` for that session.
2. Claim held and session unknown or active: hold, report it. A missing PID is
   not proof of death.
3. Only the claiming session continues the task. If the owner resumes it, it
   keeps its claim; no new `take`.
4. Releasing the claim or retiring the session is destructive: ask the owner.

### Delivery conflict

Project `publish=review`: push only `taskq-<iid>`, submit the exact SHA with
`taskq result`, and wait for PM review. If a brief or policy demands a push to
main before acceptance, `taskq ask` and stop.

## Evidence for #182

Observed 2026-10-07 UTC from the main checkout by the Claude worker holding
#182. No tick, timer or extra worker was started. Each command exited 0 with
empty stderr.

| Command | Captured stdout evidence |
| --- | --- |
| `taskq preflight --json` | `outcome=ok`; ACK `status=ready` at `2026-10-07T17:30:51Z`, `source=local subprocess`, `exit_code=0`, `exact_blocker=null`; `runtime_capability=unknown`, `effective_launch_policy=unknown`. |
| `taskq view 182 --notes 2` | `state: doing`, `claim: claude:cdeb7f1e @macbook-m2`, take note by that session, `result: none`. |
| `taskq runtime-status --runtime claude cdeb7f1e --json` | `outcome=ok`; `status=unknown` at `2026-10-07T17:30:59Z`, `event_at=null`, `session_link=null`, `source=claude agents`, `permission_requests=[]`, `approval_visibility=unknown`; exact blocker: `CLI status does not expose qualified pending approval events`. |
| `taskq <cmd> --help` for `preflight view runtime-status worker tick ask answer result problem beat release` | Flags match the usage above. |

The worker was live and working while `runtime-status` reported `unknown` with
empty permission requests: a live example of why those values never prove
absence of a wait or the end of a worker.
