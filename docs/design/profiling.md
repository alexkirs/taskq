# Profiling detail

Implemented detail of the profiling records and of `taskq profile export|tasks|board` (#1147, #1173, #1174). The records are decided in [taskq.md § 4 Profiling](../../taskq.md#profiling-988); the command, its views and their columns in [§ 5](../../taskq.md#5-commands). Chain check of the views: `export_then_reports` in `src/profile.rs`.

## Stage tokens

The [terminal-record invariant](../../taskq.md#profiling-988) uses the same `taskq-stage` records. A lifetime marker has `open: true`, `outcome: unknown`, `unknown: open-stage`, and no duration; reports exclude it from completed duration samples. New records include the host name; historical records without it show unknown until repair proves ownership from the accepted spawn matching this host/runtime and a nonempty readable attempt log in this project root. Missing, unreadable or conflicting ownership evidence leaves the record unchanged. Repair appends a classification with unknown duration and usage; original records and attempt logs stay unchanged. Export keeps open markers as observations with `unknown: open-stage`. Missing terminal records are listed from the retained window only; fresh registered-project service snapshots exclude only children whose liveness is running; task rows and unknown child liveness exclude nothing. Selftest uses a fresh board read of this project instead, excluding fresh heartbeat leases, and uses its configured `ttl`. An unreadable record or unknown age prevents a healthy verdict. A repair is appended only after board readback proves `gone` ended the attempt, or a fresh issue read proves closure or removal from the board; failed readback or append leaves the evidence unknown for a later check.

For `build` and `review`, a `taskq-stage` line holds the context tokens (model input read across the stage's calls, cache included) and the call count that the runtime logged into the attempt log after launch.

- Claude: usage per assistant message id.
- Codex: `turn.completed` input; calls are not reported.
- A call without usage makes the stage's tokens unknown.
- `codex-app`, unreadable logs and children adopted across a re-exec read unknown with reason; their duration too.

## Board kinds

Each call of the board adapter is one `taskq-board` line with one kind:

| Kind | Call |
|---|---|
| `list` | open issues |
| `inbox` | a task's comment log |
| `read` | one issue |
| `claim` | a `spawn` post |
| `comment` | another post or edit |
| `label` | a label change or label catalog setup |
| `close` | an issue close |
| `write` | an issue create or edit |
| `publish` | the § 7 publication |

## View source and window

- The views read this host's retained `<install>/taskq.log` over all its projects. They make no board calls.
- `--since 30m|24h|7d` sets the window from that time up to now. Default: all retained records. A malformed window (empty, non-ASCII or unknown suffix) is a validation error.
- A log that cannot be read fails the view and names the log.

## Export

JSON lines. The first line is a header:

```json
{"schema":1,"columns":[...],"since_ms":N,"window_ms":[first,last],"samples":N,"corrupt":N}
```

- `window_ms` holds the first and last observation time in the window, or `null` when the window is empty.
- `samples` counts the exported observations. `corrupt` counts skipped lines: corrupt or with an unknown code.

Then one object per observation, with exactly the header columns in this order. A column that a kind does not have is `null`.

| Column | Meaning |
|---|---|
| `kind` | `operation`, `stage`, `board` or `transition` |
| `at_ms` | record time |
| `project` | board `owner/name` |
| `task`, `attempt`, `entry` | task number, attempt number, board entry id |
| `name` | operation, stage or board kind |
| `from`, `to` | transition states |
| `runtime`, `host` | runtime and host of the attempt |
| `us` | duration in microseconds |
| `prev_s` | time in the previous state, seconds |
| `outcome`, `error` | outcome and error class |
| `remaining` | lowest rate-limit `remaining` after a board call |
| `calls`, `tokens` | model calls and context tokens of a stage |
| `unknown` | reason a value is unknown |

## Tasks

`taskq profile tasks` reads the export. It counts one transition per project, task and entry; when copies differ, a known time wins. The export itself keeps every row.

1. States by time in state, per runtime and host.
2. The 20 tasks with the most time per state.

Columns: `Project`, `Task`, `State`, `Runtime`, `Host`, `Count`, `Total s`, `p50/p95/max s`.

## Board

`taskq profile board` ranks board operations by p95, then failure rate, then lowest `remaining`.

Columns: `Operation`, `Count`, `p50/p95/max ms`, `Failed %`, `Errors`, `Lowest remaining`.

## Unknown values

- A missing value reads unknown with its reason: no entry time, project not recorded, not reported. It never reads as zero.
- Unknown values rank after known ones.
- A total over partly known times reads `<s> partial (k of n known)`.

## Contract detail

Terminal-record invariant (R8, R13; § 4): ended attempts have `ok`, `failed`, or `cancelled` records with an error class. Existing completed-turn `ask`/`retried` stage outcomes retain their meaning and are not missing records. Auxiliary stage records, including completed owner waits, preserve terminal evidence until a new build/review lifetime opens; that lifetime needs its own terminal record. The existing service pass repairs missing terminal evidence after normal `gone` recovery, or on the next pass after a fresh issue read proves closure or removal from the board; observations never initiate recovery. The bounded window is the project's configured `ttl`; live attempts are excluded as described above.
Append diagnostics distinguish full, lock timeout and other append failures.

Moved verbatim from `taskq.md` (#1179) to keep the contract under its R8 size bound. `taskq.md` keeps the decision and links here; this text keeps the same authority (R8, R13).
`fold` is the behavioral truth; board and process qualification remain separate under R12. Attempt stages (§ 4 Profiling) are adapter observations outside `fold`: `claim` from the `spawn` post through its readback; `start` the runtime start/resume; `build` (worker) and `review` (reviewer) from child start to exit or stop, outcome from the attempt's accepted hand-in (`result`/`close` ok, also when stopped after it; replaced without one is `cancelled`; an issue that left the board read is read back once, no hand-in or no read is `unknown` with reason); `ask` from this service seeing its attempt enter `ask` until it leaves (in memory, lost on restart); `checks` the publication's CI verification and merge call; `close` the whole `close` command. Queue transitions (§ 4 Profiling) are adapter observations too: a service pass replays each task log with entries newer than its previous pass, entry by entry, and each accepted entry that changes the folded state is one transition (`waiting` is not logged and adds none; `held` is outside the queue, so `hold` and `start` add none; the first pass after a service start adds none).
Follow [shared profiling](https://github.com/taskqorg/specq/blob/main/engineering/profiling.md). Time six operations: `service.pass` (board reconciliation, no sleep), `dispatch` (claim and readback up to runtime handoff), `spawn` (runtime start or resume, its failures included, not child lifetime), `forge.call` (one HTTP request), `update` (check, download, verification), `tray.render` (icon rasterization and native apply, not webview paint). Their budgets stay unknown until owner-approved baselines; profiling never changes admission or task state. Per the [shared agent work](https://github.com/taskqorg/specq/blob/main/engineering/profiling.md#agent-work), the attempt stages `claim`, `start`, `build`, `checks`, `review`, `ask` and `close` (#1147) are always measured too, budgets **unknown**, as `taskq-stage` lines in the same log, lock and bound (the host is the log's own): project, task and attempt numbers, runtime (`claude`/`codex`/`codex-app`/`other`), duration, outcome (`ok`/`ask`/`retried`/`failed`/`cancelled`/`unknown`), error class (`exit`: a child ended without hand-in; `left-board`/`unreadable`: no hand-in evidence), and for `build`/`review` the [context tokens and call count](profiling.md#stage-tokens); service stages run no model.
Board operations (#1173), always on, budgets **unknown**: each call of the board adapter is one `taskq-board` line with its [kind](profiling.md#board-kinds), duration, outcome (`ok`/`failed`), error class and the lowest rate-limit `remaining` after it (none reported: unknown); cached clock, user and label-schema reads are not operations. Queue transitions (model: § Executable behavioral model): one `taskq-transition` line per transition with project, task, entry id, from and to state, time in the previous state (board seconds between the entries; the first entry's start is unknown), runtime and host of the attempt on either side; the service of that host records it, each service records a move without an attempt (consumers dedupe by task and entry).
All these records hold fixed codes and numbers only (`TASKQ_PROFILE_TRACE=1` adds span ids) in `<install>/taskq.log` under its § 4 Data bound; appends wait up to 1 s for `taskq-profile.lock`; only the pruner uses a nonblocking try; collection stops as unknown at the bound; an instrumentation failure never fails the operation. Its chain check (R12) races appends against a prune that wins the lock: every retained record is a counted successful append.
`taskq profile` reads the retained log without board access, ranks the six by p95, failure rate and variance, then board operations by p95 with failures and the lowest `remaining`, and queue transitions with time in the previous state, and lists this host's board requests of the past hour by source (§ 4 Sources); corrupt or missing records read unknown, counts cover retained records only; collection completeness, earlier or pruned history and adapter retry or cancellation counts stay unknown; an hour with records starting inside it says partial; agent `gh`, PM chat tools and DashQ are not counted. Last it ranks attempt stages per runtime by p95, failure and retry rate, with count, p50/p95/max, outcomes, model calls and context tokens (sum, k of n known, else unknown with reason); DashQ reads this local report, never the board, for stage data, which holds no task text, secrets or host paths.

| Command | Does |
|---|---|
| `taskq profile [export\|tasks\|board] [--since 30m\|24h\|7d]` | no board; this host's retained log, all projects ([detail](profiling.md)). Bare: § 4. `export`: JSON lines, a header (columns, window, samples), one row per observation: `kind at_ms project task attempt entry name from to runtime host us prev_s outcome error remaining calls tokens unknown`. `tasks`: time in state (`Project Task State Runtime Host Count Total p50/p95/max`). `board`: operations (`Operation Count p50/p95/max Failed% Errors Lowest-remaining`). Unknown reads unknown with reason, never zero |
