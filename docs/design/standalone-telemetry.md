# Standalone telemetry

Design for #1185, on the records of #1147 and #1173 and the views of #1174 ([profiling detail](profiling.md)). Owner request: a standalone install records the same telemetry as a connected one, keeps it on the host, sends nothing without serverq, and ships the guide and commands an agent needs to analyze it. The decisions are in [taskq.md § Product](../../taskq.md#product). Everything below that is not marked as shipped is a target; the recommendations await owner acceptance and change no runtime rule (R8, R12, R13).

Standalone means an install with no serverq connection. A connected install is one with that connection; its design is not part of this task.

## Current state

Facts on main at #1188 (shipped):

- Collection never depends on a connection. Every service and CLI process appends `taskq-profile`, `taskq-stage`, `taskq-board`, `taskq-transition` and `taskq-requests` lines to `<install>/taskq.log` ([§ 4 Profiling](../../taskq.md#profiling-988)).
- No code sends profile records off the host. The [DashQ connection](enrollment.md) sends no profile record. Incident reports (§ 4) are a separate fault report, not telemetry.
- `taskq profile`, `taskq profile export|tasks|board [--since 30m|24h|7d]` read only that log. They make no board calls.
- Store bound: `taskq.log` past 4 MiB keeps its newest half ([§ 4 Data](../../taskq.md#4-service)). The bound is shared with service text lines, and no retention window is named.

Measured on the Windows host on 2026-10-15: `taskq.log` held 2.1 MB over 21.5 h. Of about 21,200 records, 20,450 were `taskq-profile` operation lines (mostly `forge.call`); stages were 30, board operations 498, transitions 18. At the 4 MiB bound, a busy host keeps about 1–2 days. A question about the past week reads mostly pruned history.

## Choices

| Choice | Options and trade-offs | Recommendation |
|---|---|---|
| Store and retention | A. Keep `taskq.log` at 4 MiB: no code, but about 1–2 days on a busy host, no named window, and text lines compete with records. B. Raise the shared bound (for example 32 MiB): small change, still no window, and the text log grows with it. C. Own profile store: daily JSON-lines files `<install>/profile/<UTC date>.jsonl`, 30 days and 64 MiB, the pruner removes the oldest whole day: a named window per [specq profiling](https://github.com/taskqorg/specq/blob/main/engineering/profiling.md#analysis-export), `--since` reads only the needed files; costs a migration of the readers and the pruner. D. SQLite: the specq default for state, but these are append-only records; adds a native dependency on three OS for no query the views need. | C. Same lock and record lines as today; `taskq.log` keeps service text. At the measured rate (about 1.7 MB a day) 30 days fit the bound; the export header states the retained window. |
| Data path | A. Each consumer reads `taskq.log` itself: couples DashQ, serverq and agents to the log format. B. `taskq profile export` is the one data path; reports, agents, DashQ and a future serverq upload read its JSON lines. | B. The export schema is versioned (`schema` 1); a new column is a new schema version. Reports stay derived views, never a second source of truth. |
| Sending | A. Upload by default when a collector URL is configured: easy to enable by mistake. B. Nothing leaves the host unless the owner connects the install to serverq through an explicit, revocable grant, like the [DashQ connection](enrollment.md). | B. A standalone install has no send code path enabled. Collection, retention and views are identical with or without the connection; the connection only adds an upload of export rows. Its design is a separate task. |
| Guide delivery | A. A page in this repository: a standalone host has no checkout, and a fetched page can drift from the installed binary. B. `taskq profile guide` prints the guide embedded in the binary: offline, matches the installed commands, works in every client. C. Install a skill or instruction file into each client's configuration: triggers by itself, but writes client settings, needs consent per client and per-client upkeep. | B. `taskq help` lists it. The tray operator brief (§ 4 AI requests) names `taskq profile guide` for time, context and cost questions. C stays a later, opt-in option. |
| Money cost | A. Estimate cost from tokens with a built-in price list: prices change and the records carry no model id or output tokens, so a number would be invented (R12). B. Report tokens and model calls only; money reads unknown until stages record the model and output tokens and the owner supplies prices. | B. The guide says "cost: unknown (no model, output tokens or price)" and names gap G3. |

## Analysis guide (draft)

This draft becomes the text of `taskq profile guide` (gap G6). It answers questions from the retained records of this host only. Every command exists on main unless it names a gap.

Rules for the agent:

1. Run the command, then report the table rows that answer the question. Quote the window line: earlier or pruned history is unknown.
2. A value marked unknown stays unknown with its reason. Never read it as zero; never estimate it.
3. Each report covers one host. For several hosts, run the command on each host and name the hosts; do not add their numbers as one total.
4. Use `--since 30m|24h|7d` to match the owner's period. With the current store, a 7-day window can be mostly pruned (see [current state](#current-state)): say so.

| Question | Command | Read these columns | Status |
|---|---|---|---|
| How long do tasks stay in each state? | `taskq profile tasks --since 7d` | `States by p95`: `State`, `Runtime`, `Host`, `Count`, `p50/p95/max s` | exists |
| Which tasks took longest, and where? | `taskq profile tasks --since 7d` | `Tasks by total time in state` (20 tasks): `Project`, `Task`, `State`, `Runtime`, `Host`, `Total s`, `p50/p95/max s` | exists |
| Where does attempt time go (claim, start, build, checks, review, ask, close)? | `taskq profile` | `Attempt stages`: `Stage`, `Runtime`, `Count`, `p50/p95/max ms`, `Outcomes`; rankings `Slow stages`, `Failing stages`, `Retried stages` | exists, all retained records only; windowed: gap G1 |
| Where does context go? | `taskq profile` | `Attempt stages`: `Context tokens`, `Model calls` per stage and runtime | exists per stage and runtime; per attempt: gap G2; by source (brief, rules, files, tool output): gap G4 |
| What does one attempt cost? | `taskq profile export --since 7d` | rows with `kind` `stage`: group by `project`, `task`, `attempt`; sum `us`, `calls`, `tokens`; a null in any row makes that sum unknown with its `unknown` reason | exists as rows; ready view: gap G2; money: gap G3 |
| Why do tasks wait? | `taskq profile` | `Queue transitions`: `From`, `To`, `Runtime`, `Count`, `p50/p95/max s in previous state` | exists, all retained records only |
| Which board calls are slow or rate limited? | `taskq profile board --since 24h` | `Operation`, `Count`, `p50/p95/max ms`, `Failed %`, `Errors`, `Lowest remaining` | exists |
| Who spends the board budget? | `taskq profile` | `Board requests by source` (past hour) | exists, past hour only |
| Is the service itself slow? | `taskq profile` | `Profile` table: `Operation`, `Count`, `p50/p95/max ms`, `Success/failure`, `Failure %`, `Variance ms²`, `Errors` | exists |

Report shape for the owner: one line per question, the number with its unit and window, then the unknowns with reasons. Example (invented): `Build p95 on claude: 412 s over 18 attempts (retained 21 h); context tokens 2.1 M (17 of 18 known; no-usage); cost unknown (no price).`

## Gaps

Each gap is a named follow-up, not part of this task. Numbers are stable.

| Gap | Missing | Proposed fix |
|---|---|---|
| G1 | The bare `taskq profile` refuses `--since`, so stage, transition and service tables cover all retained records only. | Accept `--since` on the bare report, through the export window. |
| G2 | No per-attempt view. | `taskq profile attempts [--since]`: `Project`, `Task`, `Attempt`, `Runtime`, `Stages`, `Total s`, `Model calls`, `Context tokens`, `Outcome`, from export rows. |
| G3 | Money cost: stage records hold context tokens (input, cache included) and calls, not model id, output tokens or a cache split. | Record `model` and output tokens per stage (new export schema); prices only from owner-supplied config; otherwise cost reads unknown. |
| G4 | Context by source (brief, embedded rules, files, tool output) is not recorded. | Read it from the runtime's own accounting, or agentq's when installed, without a second collector; needs its own design. |
| G5 | Retention: 4 MiB shared bound, no named window. | Store choice C above; § 4 Data names the window and bound. |
| G6 | No guide command. | `taskq profile guide` prints this guide from the binary; the operator brief names it. |
| G7 | Stage rows carry no host (the log's own host), so merged exports from several hosts lose attribution. | Add `host` to stage records, or the export fills it from the install's host name. |

## Session loading

Any agent session on the host (PM, operator or a plain client session) answers a telemetry question in two steps: run `taskq profile guide`, then run the command it names for the question. No skill install, configuration write or network access is needed. Until G6 ships, the session reads this page and uses the table above.

## Acceptance of the follow-up work

A follow-up task is done when, on one host: the guide command prints this table; each question's command prints the named columns or a named unknown; a fixed record set covers the new views in the existing `export_then_reports` chain check; and no profile record leaves the host without a serverq connection (no send call in the standalone path).
