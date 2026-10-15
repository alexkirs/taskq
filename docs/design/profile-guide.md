# Local telemetry analysis

Offline guide shipped with this binary. Collection is local and always on; remote export is optional.

Rules for the agent:

1. Run the command, then report the table rows that answer the question. Quote the window line: earlier or pruned history is unknown.
2. A value marked unknown stays unknown with its reason. Never read it as zero; never estimate it. In export rows, a null `calls` and `tokens` with `unknown` `no-model` means the stage ran no model (claim, start, ask, checks, close): count it as none, not unknown. A null `calls` with a null `unknown` means the runtime does not report a call count (Codex): say "calls not reported", keep its tokens. Any other null is unknown with its `unknown` reason.
3. Stage durations nest and overlap: `checks` runs inside `close`, and the reviewer runs `close` inside `review`. Report each stage's duration on its own line. Never add the `us` of several stages and call the sum attempt time.
4. Each report covers one host. For several hosts, run the command on each host and name the hosts; do not add their numbers as one total.
5. Use `--since 30m|24h|7d` to match the owner's period. A requested window can include pruned history: report the actual retained window.

| Question | Command | Read these columns | Status |
|---|---|---|---|
| How long do tasks stay in each state? | `taskq profile tasks --since 7d` | `States by p95`: `State`, `Runtime`, `Host`, `Count`, `p50/p95/max s` | exists |
| Which tasks took longest, and where? | `taskq profile tasks --since 7d` | `Tasks by total time in state` (20 tasks): `Project`, `Task`, `State`, `Runtime`, `Host`, `Total s`, `p50/p95/max s` | exists |
| Where does attempt time go (claim, start, build, checks, review, ask, close)? | `taskq profile` | `Attempt stages`: `Stage`, `Runtime`, `Count`, `p50/p95/max ms`, `Outcomes`; rankings `Slow stages`, `Failing stages`, `Retried stages`. One row per stage; rows overlap (rule 3), so never add them | exists, all retained records only; windowed: gap G1 |
| Where does context go? | `taskq profile` | `Attempt stages`: `Context tokens`, `Model calls` per stage and runtime | exists per stage and runtime; per attempt: gap G2; by source (brief, rules, files, tool output): gap G4 |
| How long does one attempt take, and what does it use? | `taskq profile export --since 7d` | rows with `kind` `stage`, grouped by `project`, `task`, `attempt`: `name` and `us` per stage, one line each (rule 3); `tokens` summed over `build`/`review` rows and `calls` with rule 2. Coverage: an attempt without its `claim` row began before the window or was pruned, and one without its `build` or `review` row is still running or ended while no service ran; mark it `incomplete (<missing stages>)` and report no total for it | per stage: exists as rows; elapsed time, coverage and ready view: gap G2; money: gap G3 |
| Why do tasks wait? | `taskq profile` | `Queue transitions`: `From`, `To`, `Runtime`, `Count`, `p50/p95/max s in previous state` | exists, all retained records only |
| Which board calls are slow or rate limited? | `taskq profile board --since 24h` | `Operation`, `Count`, `p50/p95/max ms`, `Failed %`, `Errors`, `Lowest remaining` | exists |
| Who spends the board budget? | `taskq profile` | `Board requests by source` (past hour) | exists, past hour only |
| Is the service itself slow? | `taskq profile` | `Profile` table: `Operation`, `Count`, `p50/p95/max ms`, `Success/failure`, `Failure %`, `Variance ms²`, `Errors` | exists |

Report shape for the owner: one line per question, the number with its unit and window, then the unknowns with reasons. Example (invented): `Build p95 on claude: 412 s over 18 attempts (retained 21 h); context tokens 2.1 M (17 of 18 known; no-usage); calls on codex not reported; attempt elapsed time unknown (gap G2); cost unknown (no price).`

Named gaps G1–G4: no windowed bare report, per-attempt elapsed-time view, monetary cost, or context-source breakdown. These values remain unknown.
