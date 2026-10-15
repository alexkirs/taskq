# Standalone telemetry

Design for #1185, on the records of #1147 and #1173 and the views of #1174 ([profiling detail](profiling.md)). Owner request: a standalone install records the same telemetry as a connected one, keeps it on the host, sends nothing without serverq, and ships the guide and commands an agent needs to analyze it. The decisions are in [taskq.md § Product](../../taskq.md#product). Everything below that is not marked as shipped is a target; the recommendations await owner acceptance and change no runtime rule (R8, R12, R13).

Standalone means an install with no serverq connection. A connected install is one with that connection; its design is not part of this task.

## Historical log implementation

Facts on main at #1188 (shipped):

- Collection never depends on a connection. Every service and CLI process appends `taskq-profile`, `taskq-stage`, `taskq-board`, `taskq-transition` and `taskq-requests` lines to `<install>/taskq.log` ([§ 4 Profiling](../../taskq.md#profiling-988)).
- No code sends profile records off the host. The [DashQ connection](enrollment.md) sends no profile record. Incident reports (§ 4) are a separate fault report, not telemetry.
- `taskq profile`, `taskq profile export|tasks|board [--since 30m|24h|7d]` read only that log. They make no board calls.
- Store bound: `taskq.log` past 4 MiB keeps its newest half ([§ 4 Data](../../taskq.md#4-service)). The bound is shared with service text lines, and no retention window is named.

Measured on the Windows host on 2026-10-15: `taskq.log` held 2.1 MB over 21.5 h. Of about 21,200 records, 20,450 were `taskq-profile` operation lines (mostly `forge.call`); stages were 30, board operations 498, transitions 18. At the 4 MiB bound, a busy host keeps about 1–2 days. A question about the past week reads mostly pruned history.

## Accepted storage change

Owner selected SQLite/WAL for local telemetry. This replaces choice C below; that comparison is historical design context, not the implementation target. SQLite runs inside the product process, with a bundled library on all supported platforms and no server. Keep the existing record definitions, always-on instrumentation and analysis/export commands; JSONL remains the export format. Task state remains on the board (R1).

Retention: 30 days / 64 MiB; name and enforce database, WAL and temporary-space bounds in the implementation plan. Reuse the existing service for pruning. Set durability and busy timeouts per [shared data](https://github.com/taskqorg/specq/blob/main/engineering/data.md). An unavailable, full, corrupt or unsupported store reads unknown; instrumentation must preserve the operation. Never reset a corrupt database silently.

Before implementation, map all record writers, readers, pruners and legacy-process overlap. Migration preserves retained history and is idempotent, with transactional checkpoints and explicit coverage gaps; rollback preserves readable history. Verify crash/restart, failed writes, corruption, contention, retention, migration retry and old/new reader/writer transitions. Do not delete legacy history as an implicit migration step. The SQLite candidate below replaces the historical log path after its release gates pass.

## Historical comparison

| Choice | Options and trade-offs | Original recommendation |
|---|---|---|
| Store and retention | A. Keep `taskq.log` at 4 MiB: no code, but about 1–2 days on a busy host, no named window, and text lines compete with records. B. Raise the shared bound (for example 32 MiB): small change, still no window, and the text log grows with it. C. Own profile store: daily JSON-lines files `<install>/profile/<UTC date>.jsonl`, 30 days and 64 MiB, the pruner removes the oldest whole day: a named window per [specq profiling](https://github.com/taskqorg/specq/blob/main/engineering/profiling.md#analysis-export), `--since` reads only the needed files; costs a migration of the readers and the pruner. D. SQLite: the specq default for state, but these are append-only records; adds a native dependency on three OS for no query the views need. | C. Same lock and record lines as today; `taskq.log` keeps service text. At the measured rate (about 1.7 MB a day) 30 days fit the bound; the export header states the retained window. |
| Data path | A. Each consumer reads `taskq.log` itself: couples DashQ, serverq and agents to the log format. B. `taskq profile export` is the one data path; reports, agents, DashQ and a future serverq upload read its JSON lines. | B. The export schema is versioned (`schema` 1); a new column is a new schema version. Reports stay derived views, never a second source of truth. |
| Sending | A. Upload by default when a collector URL is configured: easy to enable by mistake. B. Nothing leaves the host unless the owner connects the install to serverq through an explicit, revocable grant, like the [DashQ connection](enrollment.md). | B. A standalone install has no send code path enabled. Collection, retention and views are identical with or without the connection; the connection only adds an upload of export rows. Its design is a separate task. |
| Guide delivery | A. A page in this repository: a standalone host has no checkout, and a fetched page can drift from the installed binary. B. `taskq profile guide` prints the guide embedded in the binary: offline, matches the installed commands, works in every client. C. Install a skill or instruction file into each client's configuration: triggers by itself, but writes client settings, needs consent per client and per-client upkeep. | B. `taskq help` lists it. The tray operator brief (§ 4 AI requests) names `taskq profile guide` for time, context and cost questions. C stays a later, opt-in option. |
| Money cost | A. Estimate cost from tokens with a built-in price list: prices change and the records carry no model id or output tokens, so a number would be invented (R12). B. Report tokens and model calls only; money reads unknown until stages record the model and output tokens and the owner supplies prices. | B. The guide says "cost: unknown (no model, output tokens or price)" and names gap G3. |

## Analysis guide

`taskq profile guide` embeds the [analysis guide](profile-guide.md). It reads no store, board or network and rejects `--since`.

## Gaps

Each gap is a named follow-up, not part of this task. Numbers are stable.

| Gap | Missing | Proposed fix |
|---|---|---|
| G1 | The bare `taskq profile` refuses `--since`, so stage, transition and service tables cover all retained records only. | Accept `--since` on the bare report, through the export window. |
| G2 | No per-attempt view, and no attempt elapsed time: stages overlap, so summed `us` is not elapsed time, and a span from the rows' end times (`at_ms`) minus durations also counts idle time between stages. | `taskq profile attempts [--since]`: `Project`, `Task`, `Attempt`, `Runtime`, `Coverage` (`complete` or the missing stages), `Elapsed s` (first stage start to last stage end; unknown unless coverage is complete and every row has `us` and `at_ms`), one `<stage> s` column per stage, `Model calls`, `Context tokens`, `Outcome`, from export rows. |
| G3 | Money cost: stage records hold context tokens (input, cache included) and calls, not model id, output tokens or a cache split. | Record `model` and output tokens per stage (new export schema); prices only from owner-supplied config; otherwise cost reads unknown. |
| G4 | Context by source (brief, embedded rules, files, tool output) is not recorded. | Read it from the runtime's own accounting, or agentq's when installed, without a second collector; needs its own design. |
| G5 | Retention: 4 MiB shared bound, no named window. | Accepted SQLite/WAL store above; 30 days / 64 MiB. |
| G6 | Guide command implemented in this change. | Offline embedded analysis guide, named by the operator brief. |
| G7 | Stage rows carry no host (the log's own host), so merged exports from several hosts lose attribution. | Add `host` to stage records, or the export fills it from the install's host name. |

## Session loading

Any agent session on the host (PM, operator or a plain client session) answers a telemetry question in two steps: run `taskq profile guide`, then run the command it names for the question. No skill install, configuration write or network access is needed. Until G6 ships, the session reads this page and uses the table above.

## Acceptance of the follow-up work

A follow-up task is done when, on one host: the guide command prints this table; each question's command prints the named columns or a named unknown; a fixed record set covers the new views in the existing `export_then_reports` chain check; and no profile record leaves the host without a serverq connection (no send call in the standalone path).

## SQLite implementation and transition

`src/telemetry.rs` owns `<install>/telemetry.log/telemetry.sqlite3`, application ID `0x5451544d`, schema 1. The dedicated `telemetry.log/` namespace survives the old updater's `*.log` allowlist, preserving the database and its sidecars on mixed-version updates or rollback. Existing instrumentation uses the shared profile lock; the five record kinds and report/export schemas stay unchanged. Bundled SQLite uses WAL, FULL durability, a one-second busy timeout, memory-only temporary storage and no cache spill. Reads and writes close their connections within that lock.

A 28 MiB page limit bounds the database. Every transaction starts after a successful TRUNCATE checkpoint; cache spilling is disabled, so one transaction adds at most one frame per database page (under 29 MiB WAL, under 1 MiB SHM). The bounded 4 MiB legacy log and its 64 KiB append allowance keep the total below 64 MiB. Pinned external readers refuse the next write instead of growing the WAL. No VACUUM copy. Pressure removes oldest records in 256-row transactions until space is available. A single append/record is at most 64 KiB; an oversized write is rejected before pruning. The existing host/service prunes after 30 days from local receipt; source timestamps stay unchanged. Imported records get a fresh retention window so migration does not immediately erase legacy evidence.

Legacy records have no IDs. Import checkpoints stream byte positions and SHA256 digests in the same transaction as the rows; equal legitimate observations remain separate. Checkpoints contain hashes only, never service diagnostics. An unchanged prefix imports only appended complete lines. The exact retained suffix of the old bounded-log pruner is recognized; other rewrites/loss fail analysis as unknown and preserve SQLite history and the legacy file. An unrecognized legacy change does not prevent new SQLite records from being stored, but reports the unresolved import. Updated text-log rotation imports before truncating. No implicit removal of legacy files.

Rollback keeps both the untouched legacy file and SQLite database. An old binary can continue writing its log; the new reader imports those appends on upgrade. Old readers cannot display new SQLite records: retain the qualified reader or use a SQLite reader (`SELECT line FROM records ORDER BY seq`) for the complete retained history. JSONL analysis export remains `taskq profile export`; task state stays on the board. Destructive schema contraction is outside this transition.

Checks cover import retry, equal events, recognized rotation, unknown rewrites, atomic rollback, unsupported/corrupt stores, kill/restart, pinned readers, pressure bounds, retention, concurrent writers and terminal-stage repair through the actual profile reader. Platform release gates qualify the bundled library separately.
