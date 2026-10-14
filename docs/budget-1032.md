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
- **Board user bucket:** two `gh api rate_limit` samples 10 min apart
  (14:57Z and 15:07Z). Other replies in the same series reported a different,
  unused identity and were discarded.

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

## Compared with #1011

| | #1011 baseline | after #1030 |
|---|---|---|
| Idle project-host, core/h | 116 (`GET issues` 58, `PATCH` 58; win taskq 13:03Z) | 29-33 |
| Win services, core/h | 738 (5 projects, 07:15Z) | 294 (6 projects) |
| All 16 project-hosts, core/h | about 1800-2700 (modeled) | 806 (measured) |
| All 16 project-hosts, GraphQL points/h | 0 | about 1870 (936 queries) |
| Board user total, core/h | about 3860 | about 1400 (one 10-min sample) |
| Board user total, GraphQL points/h | about 1100 | about 2200 (one 10-min sample) |

Service core is down about 55-70 %. #1030 modeled at most 580 core/h; the
measured 806 adds the comment and label reads of busy boards. The user's core
bucket now sits near 28 % of its limit instead of 77 %.

## Remaining sources above 10 % of an hourly budget

- **Service open-issue list (GraphQL):** about 1870 of 5000 points/h (37 %).
  Three hosts read the same six boards, so 10 of 16 reads duplicate another
  host's read. Follow-up: [#1043](https://github.com/taskqorg/taskq/issues/1043).
- **Heartbeat `PATCH`:** about 480 core/h (16 × 30), 9.6 %, below the bar.
- **Agents, PM, CLI and DashQ:** about 600 core/h (12 %) remain after the
  services in the user total. They are not split per operation here; mac and
  fra agent usage stays unknown, as in #1011.
