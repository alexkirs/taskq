# GitHub budget after #1030 (#1032)

Live service request counts on 2026-10-14, compared with the
[#1011 baseline](https://github.com/taskqorg/taskq/pull/1030). Every build
below includes c97297e (#1030).

## Source and build

- **Counts:** the hourly `requests 1h:` service log lines and the `status.json`
  `requests.last_hour` of each project. Both describe the same process hour.
- **mac:** `4.2.0-dev.1791969098` for taskq, `4.2.0-dev.1791987853` for the
  other five. Hour ended 15:16Z (taskq) and 15:25-15:26Z (others).
- **win:** `4.2.0-dev.1791987849`, running since 14:25Z. Hour ended 15:25-15:26Z.
  Earlier dev builds restarted about every 30 min (13:22Z, 13:56Z) and wrote
  no hourly line.
- **hermes-fra1:** `4.2.0-dev.1791987850`, running since 14:25Z. Hour ended
  15:25-15:26Z. The previous hour (`4.2.0-dev.1791983965`) shows the same
  pattern: 487 requests.
- **Board user bucket:** `gh api rate_limit` once a minute from 15:37Z to
  16:44Z on win, 67 samples, as the board user `alexkirs`. The replies
  alternate between two buckets. The busy one (core window 15:43:09Z-16:43:09Z,
  GraphQL window 15:26:51Z-16:26:51Z) is the board user's. The other used
  0-13 core and about 150 GraphQL points/h; its identity is unknown and it is
  left out.

## Service requests per hour

`core` is REST. `graphql` counts queries; one open-issue list query costs
about 2 points (measured in #1030).

| Host | Project | total | core | graphql |
|---|---|---|---|---|
| mac | agentq | 98 | 42 | 56 |
| mac | coreq | 84 | 29 | 55 |
| mac | dashq | 174 | 115 | 59 |
| mac | serverq | 86 | 31 | 55 |
| mac | specq | 85 | 29 | 56 |
| mac | taskq | 107 | 51 | 56 |
| win | agentq | 100 | 40 | 60 |
| win | coreq | 91 | 31 | 60 |
| win | dashq | 156 | 96 | 60 |
| win | serverq | 93 | 33 | 60 |
| win | specq | 91 | 31 | 60 |
| win | taskq | 122 | 63 | 59 |
| hermes-fra1 | agentq | 102 | 42 | 60 |
| hermes-fra1 | dashq | 145 | 85 | 60 |
| hermes-fra1 | serverq | 92 | 32 | 60 |
| hermes-fra1 | taskq | 116 | 56 | 60 |
| **all 16** | | **1742** | **806** | **936** |

Per host: mac 634 (core 297), win 653 (core 294), hermes-fra1 455 (core 215).

An idle project-host now costs about 30 core (heartbeat `PATCH issues/N`
25-31) and 55-60 GraphQL list queries. Busy boards add comment, issue and
label reads: dashq adds 50-85 core.

## Board user hour and the agent residual

The owner chose one residual bucket for non-service use on mac and fra (no
host split). Hour 15:43Z-16:43Z (core) and 15:27Z-16:27Z (GraphQL); win
service and CLI counts use 15:26Z-16:26Z, so windows differ by up to 17 min:

| Source | core/h | GraphQL points/h | Evidence |
|---|---|---|---|
| Board user total | about 2850 (2743 at 16:40:16Z) | about 2640 (2634 at 16:26:07Z) | rate_limit series |
| win services | about 400 | about 720 (358 queries) | five `requests 1h:` lines 16:25:51Z-16:27:18Z (148, 256, 87, 88, 88); coreq restarted 16:10Z, idle value 31 core / 60 queries |
| mac and fra services | about 510 | about 1150 | hour to 15:26Z above (no newer lines readable from win) |
| win taskq CLI | about 390 | unknown share | `forge.call` profile records 15:26Z-16:26Z: 1143, minus services about 757 |
| win agents (`gh`) | at most about 150 | at most about 120 | Claude and Codex session logs: 82 `gh` commands, 3 `gh pr checks --watch` (60 s) |
| **Residual: mac and fra agents and CLI, PM, DashQ** | **about 1400-1550 (29 %)** | **about 650-770 (15 %)** | total minus the rows above |

The hour was busy: win alone spawned 23 attempts (profile `spawn` records). In
the quiet hour to 15:26Z win taskq CLI made about 10 board calls (663 `forge.call`
records against 653 service requests) and win agents ran about 48 `gh` commands.

## Compared with #1011

| | #1011 baseline | after #1030 |
|---|---|---|
| Idle project-host, core/h | 116 (`GET issues` 58, `PATCH` 58; win taskq 13:03Z) | 29-33 |
| Win services, core/h | 738 (5 projects, 07:15Z) | 294 (6 projects) |
| All 16 project-hosts, core/h | about 1800-2700 (modeled) | 806 (measured) |
| All 16 project-hosts, GraphQL points/h | 0 | about 1870 (936 queries) |
| Board user total, core/h | about 3860 | about 2850 (1 h series, busy hour) |
| Board user total, GraphQL points/h | about 1100 | about 2640 (1 h series) |

Service core is down about 55-70 %. #1030 modeled at most 580 core/h; the
measured 806 adds the comment and label reads of busy boards. The user's core
bucket sat at about 57 % of its limit in a busy hour instead of 77 %. The
earlier 10-min sample (about 1400 core/h, 14:57Z-15:07Z) understated the hour:
within the series the rate ran at about 1650 core/h from 15:43Z to 16:03Z and
about 3550 core/h after.

## Remaining sources above 10 % of an hourly budget

- **Service open-issue list (GraphQL):** about 1870 of 5000 points/h (37 %).
  Three hosts read the same six boards, so 10 of 16 reads duplicate another
  host's read. Follow-up: [#1043](https://github.com/taskqorg/taskq/issues/1043).
- **Heartbeat `PATCH`:** about 480 core/h (16 × 30), 9.6 %, below the bar.
- **Agents, CLI, PM and DashQ (non-service):** about 1400-1550 core/h (29 %)
  and 650-770 GraphQL points/h (15 %) on mac and fra, plus about 390 core/h
  of win taskq CLI in a busy hour. Not split per host or operation by the
  owner's choice. Follow-up: [#1084](https://github.com/taskqorg/taskq/issues/1084)
  counts them per host and source and cuts each above 10 %.

## Counting format

Moved from taskq.md § 4 Log and board budget, which keeps the rules.

- **Log line:** `<UTC time> <text>`, time ISO 8601 `YYYY-MM-DDThh:mm:ssZ` from the local clock; the lines of one message, an HTTP error body or a panic included, fold into one line.
- **Operation:** `<METHOD> <path>`, the path API-relative, at most 3 segments, numbers as `N`, other ids as `*`; later pages count under their first page.
- **Pause:** a 403 or 429 with `retry-after`, or with zero `x-ratelimit-remaining`/`ratelimit-remaining`, pauses the process's board calls on the monotonic clock until `retry-after`, else the reset header against the reply `Date`, else 60 s; at most 1 h. A GraphQL `RATE_LIMITED` reply pauses like a 403. Log lines: `rate limit: board requests paused until <time>` and `... resumed`.
- **Buckets:** GitHub budgets per `x-ratelimit-resource` (`core`, `graphql`); `remaining`, `limit`, `reset` and `resource` are those of the bucket with the lowest remaining share.
- **`status.json` `requests`:** `since`, cumulative `total` and `ops`, `last_hour` (null until a full hour), last `remaining`/`limit`, `paused_until`; under the alarm also `reset`, `resource` and `top`.
- **Hourly log:** `requests 1h: <total>; <op> <n>, ..`, top 8 operations, then `other`.
- **Alarm log:** `board budget low: <remaining>/<limit> <resource> until <reset>; top here: <op> <n>`, `top` being the largest operation of this process's current hour.
- **Sources (#1084):** `source` is `service` or `<role> <command>`; `service` adds its subcommand; the role is `worker`/`reviewer` from `TASKQ_ROLE`, else `cli`; other words are `*`.
- **Pass cost:** on github.com one GraphQL query per 100 open issues, 1 point: labels are its only nested connection (#1043). Assignees, which would double it, are read per issue only for an `assignee-only` candidate or a report `assignee` filter. Other boards use REST. A dependency of this board is read off the open list once per process until listed open again; one of another repository each pass until done.

## Contract detail

Moved verbatim from `taskq.md` (#1179) to keep the contract under its R8 size bound. `taskq.md` keeps the decision and links here; this text keeps the same authority (R8, R13).

- Log and board budget (#1009): each service log line is `<UTC time> <text>` (ISO 8601, local clock); a multi-line message (HTTP error body, panic) folds into it. Every process counts its board HTTP requests per operation and, on a rate-limit reply, pauses its board calls until the reset, at most 1 h: calls fail at once naming the reset, the service logs one paused and one resumed line and resumes without a restart (operation names and header rules: [counting format](budget-1032.md#counting-format)). `status.json` `requests` holds the counts, the last `remaining`/`limit` and `paused_until`; the log gets `requests 1h: ..` hourly; `service status`/`list` print `Board requests: <n>/h; ..` (else unknown with reason) and the pause. #988 profiling times operations; this counts requests. CLI processes and workers count and pause alone; coreq update and incident requests are not counted. Sources (#1084): a CLI process with board requests at exit, and the service hourly, append one `taskq-requests {at_ms, source, ops}` line to `<install>/taskq.log` (profile lock and bound). Board access budget (#1011): one board user's 5000 requests/h serve every host, project, service, CLI, worker, PM and DashQ. A service pass costs one open-issue list (github.com: GraphQL, labels only); it re-reads comments only of issues whose `updated_at` moved, writes the heartbeat only on change or past `ttl / 3`, reuses that issue for leases, verifies each label once per process and reads dependencies only for a would-be `ready` task (§ 1). Agent CI waits: § 7 rule 7. Budget alarm: at `remaining` <= 20 % of `limit` the service logs `board budget low` once per reset, and `service status`/`list` and R6 line 1 show `Board budget low: <remaining>/<limit> <resource> until <reset> (<host>); top here: <op> <n>`.
