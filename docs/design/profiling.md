# Profiling detail

Implemented detail of the profiling records and of `taskq profile export|tasks|board` (#1147, #1173, #1174). The records are decided in [taskq.md § 4 Profiling](../../taskq.md#profiling-988); the command, its views and their columns in [§ 5](../../taskq.md#5-commands). Chain check of the views: `export_then_reports` in `src/profile.rs`.

## Stage tokens

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
