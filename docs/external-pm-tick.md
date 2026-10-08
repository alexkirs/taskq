# External PM tick guide

Use the [PM contract](../taskq/contracts/taskq-manager.md), especially External
scheduler, Permission observations and External PM bootstrap, as the operational
source of truth. The CLI flags are defined in `taskq/__init__.py`.

## Preflight and observation

Before launching a worker, have the selected local executor run
`taskq preflight --json` through its supported command tool from the main
checkout. Capture stdout, stderr and exit status. Its ACK comes from a real
read-only subprocess that prints the resolved main-checkout cwd. `ready` proves
only local-command execution. Conversation activity, a chat acknowledgement or
an execution item does not prove runtime capability or effective launch policy;
those remain `unknown` until separately qualified. Preflight neither grants
launch authority nor enforces a cross-host gate or proves later worker permissions.
Record the actual launch route and policy separately. The agent performs
explicitly authorized ordinary settings in the appropriate scope; do not assign
that authorized setup to manual owner work. Security-sensitive expansions require
the applicable confirmation. Never weaken safeguards or bypass denials.

Read-only commands include `taskq preflight --json`, `taskq view 181 --notes 2`,
`taskq worker` (selection and brief only), and
`taskq runtime-status --runtime codex <session> --json` (also supports `claude`).
Runtime status uses supported metadata without resume, private rollout reads,
approval replies or worker creation. Preserve `observed_at`, `event_at`, source,
session link and exact blocker. Missing runtime or approval data stays `unknown`;
empty permission requests or active flags do not prove no pending approval.

Positive permission evidence includes typed requests and `waitingOnApproval`.
The owner approves in the linked runtime UI. Keep the same worker: never approve
on their behalf, nudge a permission wait, release it as idle, or spawn a replacement.
Unknown and still-active observations are held. Requests predating the connection
or resolved elsewhere may be invisible; Claude approval visibility may be unknown.
Without a request ID, do not promise exactly-once notifications.

## Bounded selection and an authorized tick

For this canary, use explicit per-run bounds:

```sh
taskq worker --filter "labels=selftest,canary-dot-20261007-1635" --no-mine --limit codex=1,claude=0
```

Flags override `taskq.local.toml`, then project settings, then defaults; omitted
keys retain their resolved values. `--no-mine` includes own assignments and the
shared pool, not tasks assigned to other people. Limits are per machine, not
cross-host arbitration. Preserve exclusions; never broaden the filter to fill a
slot. Here only #181 is authorized: a brief is not a claim, and an unavailable
#181 is a stop, not permission to select another task.

Only after separate owner authority for timer and worker launches, an external
scheduler can run one bounded pass from the main checkout:

```sh
taskq tick --act --filter "labels=selftest,canary-dot-20261007-1635" --no-mine --limit codex=1,claude=0 --json
```

This is an acting command, not a demonstration to run during this docs task.
Plain `taskq tick` is also not read-only: it can auto-update, reconcile queue and
board state, release dead claims and stale locks, record stamps and archive finished
Codex sessions. `--act` also launches, nudges, retires and performs idle cleanup.
Do not arm a timer or dispatch another task for this canary.

Capture both streams and exit code. Exit 0 means the pass completed without
judgement, or overlap was skipped with `Skipped: another tick pass is running.`
on stderr. Exit 1 with a pass report means judgement is needed; inspect failures
and relay reviews/questions without running another tick. Exit 1 with only an
error, or another nonzero code, is failure, not an empty queue. For `--json`, retain
the structured outcome, actions, refusals and report text. Never start duplicate
workers from captured output. The checkout-local OS lock protects overlapping
passes; do not delete its leftover file. External schedulers need neither Claude
tools nor the removed wake flag. Honor the contract's idle-stop handling through the external
scheduler; configure the sender again only on owner request.

## Claim, candidate and publication

`taskq take 181` mutates the tracker and claims with the actual worker session;
never invent a session or claim. Work only on `docs/external-pm-tick.md` in the
`taskq-181` worktree, with `TASKQ_TASK=181 TASKQ_RUNTIME=codex` for attribution.
`beat`, `problem`, `ask`, `result`, branch push and acting tick are mutations.
If a mutation fails, read its actual tracker/git state before retrying. Report
the exact blocker; never fabricate a result. Owner-only decisions go through
`taskq ask 181 --text "..."`, then stop.

Run `python3 -m unittest discover -s tests`. Commit on `taskq-181`, fetch and
rebase onto `origin/main`, check the frozen candidate, and push only
`HEAD:refs/heads/taskq-181`. Use a normal push for a new or fast-forward task branch.
Use `--force-with-lease` only when actually needed after rebasing your own task
branch and permitted by the contract: check the expected remote head first and
pin the lease to that SHA (`--force-with-lease=refs/heads/taskq-181:<expected-SHA>`).
Do not push other refs. Submit its exact full SHA:

```sh
taskq result 181 --sha <pushed-full-SHA> --checks "<commands and outcomes>" --text "<summary>"
```

A result submits a candidate for review; it is neither PM acceptance nor
publication. Project `publish=review`: the manager publishes after review.
Never push main before PM acceptance. If the generated delivery policy instead
requires direct main publication, ask through TaskQ and stop rather than override it.

## Read-only evidence for #181

Observed 2026-10-07 UTC through the local command tool; no tick, timer or extra
worker was launched for these demonstrations. All three commands exited 0 with
empty command stderr:

| Command | Captured stdout evidence |
| --- | --- |
| `taskq preflight --json` | `outcome=ok`; ACK at `2026-10-07T17:07:07Z`, `status=ready`, `source=local subprocess`, `exit_code=0`, cwd and stdout both identify the main checkout, subprocess stderr empty, `exact_blocker=null`; `runtime_capability=unknown`, `effective_launch_policy=unknown`. |
| `taskq view 181 --notes 2` | `state: doing`, `claim: codex:01a11749 @macbook-m2`, take note by that session, `result: none`. |
| `taskq runtime-status --runtime codex <actual-session> --json` | `outcome=ok` at `2026-10-07T17:07:10Z`; `status=unknown`, `event_at=null`, `source=unavailable`, `permission_requests=[]`, `approval_visibility=unknown`; exact blocker: `Status unavailable: [Errno 1] Operation not permitted`. |

The successful status-command exit does not make its unavailable observation
healthy or prove approval absence. These reads qualify local execution only,
not full runtime or launch-policy readiness.
