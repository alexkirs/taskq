# Profiling detail

Implemented detail of the profiling records and of `taskq profile export|tasks|board` (#1147, #1173, #1174). The records are decided in [taskq.md § 4 Profiling](../../taskq.md#profiling-988); the command, its views and their columns in [§ 5](../../taskq.md#5-commands). Chain check of the views: `export_then_reports` in `src/profile.rs`.

## Service wakeups

Each service pass snapshots project and selected-install poke mtimes before reading policy/focus. A change during work skips sleep; otherwise sleep checks each second. The command's project poke carries the shared coreq trace context when capture is available, replacing the legacy dot. Old services still use its mtime. New writers exclusively lock the existing signal before truncation; readers take a shared lock, so concurrent context writes cannot mix identities. Both sides use the shared owned lock guard, which unlocks explicitly before closing; a descriptor inherited during process creation cannot extend the retired owner's lock. Busy access fails without waiting. The new service reads at most129 bytes, decodes the at-most128-byte context and links the next pass's generated observations and status publication to it. Repeated content is consumed once per process; a restart may observe it again. The signal carries no command, paths, user text or authority and never overrides focus, grants or a hold.

Signal writes run through the shared execution boundary: their stable failure class is captured even if the caller handles the error. Project focus/pause/resume/restart and handover report a failed wake to their caller; already saved state is not rolled back or reported as unapplied. Board mutation receipts retain confirmed acceptance and add a wake warning; never repeat a board write to repair notification. Hold/release applies its safety state first and attempts every registered project before reporting any wake failures. Periodic reconciliation remains available. These are signal-write outcomes, not durable command receipts or service acknowledgment.

Focus, Pause, Resume and initial focus installation acquire the signal's exclusive lock before reading or changing focus files; contention refuses before mutation. Changed focus files use atomic replacement. Command changes clear the previous link before mutation and notify even after a partial write failure, preserving both errors when notification also fails. The service reads focus and the signal under one shared lock and consumes the link only after a successful snapshot. Busy, malformed or unreadable snapshots hold admission with their reason while supervision continues. Legacy/missing signals carry no link. This serializes current writers and readers, not uncooperative old binaries or manual edits; it is not a crash-atomic multi-file transaction. Durable focus receipts use the command-state protocol below.

The shared focus/pause/resume boundary commits `command.received` before local mutation, then `command.applied` or `command.failed` using the captured command descriptor. CLI, tray and operator proposals all use this boundary; callers cannot omit receipts. Once acquired, the signal lock stays owned through terminal commit, so a later mutation cannot overtake it. Each helper invocation has its own explicit identity, including repeated operations inside one proposal. It prints the terminal receipt id and says service observation is pending; applied means local settings and notification, not admission or worker launch. Resume and an admitting focus refuse before mutation if the received receipt is unavailable. Pause and exactly `focus off` still attempt the restrictive change, reporting applied-local/receipt-unknown if persistence failed. A post-effect receipt failure never rolls back or invites blind retry. Receipts use existing retention/bounds and include no arguments. The command-state protocol below owns service-observed/superseded phases. Native input/display, stable retry ids across invocations and Stop/Uninstall ownership remain pending. Required receipt failure is product behavior; optional export/loss cannot replace the individual commit acknowledgment.

Tray focus/pause/resume dispatch uses one process-owned worker and at most16 queued requests. Submission never waits for storage or starts an unbounded thread per click. It preserves the initiating trace, rechecks registration before applying, and returns queue-full, worker-unavailable, command failure or panic/unknown through the existing project error message. Queue admission is not a durable command receipt; the page remains pending until state readback. Required receipts begin at execution; a process exit before execution does not prove delivery. Other tray actions and native input/paint/exit qualification remain separate.

The page assigns a bounded per-click correlation id, echoed by the worker on success and failure. Success carries the applied receipt id returned by the shared command boundary, never a later reread of the mutable pointer. A project snapshot carries `focus_command`; only a snapshot at most60s old with `state: observed`, a valid observed receipt id and this exact applied id clears that click's pending state. Identical focus is valid; a mode change or another command's acknowledgment is not confirmation. Pending state survives route changes. At30s without matching readback, show confirmation unknown and permit a deliberate new command without replaying the old one. A later matching observation can still resolve that unknown; stale bridge replies cannot clear a newer click. This page correlation id is not a cross-invocation business retry id. Receipt errors preserve their explicit local/unknown explanation.

Pokes coalesce. The signal link alone proves that this pass observed this signal, not that every earlier signal was delivered or that the command launched work. Focus commands additionally use the descriptor protocol below. Other superseded, missing, legacy, malformed or unreadable contexts have no causal acknowledgment; decode/read errors are captured and do not stop service work. Host-wide legacy pokes remain unlinked. A signal arriving after the read waits for the next pass. These are coverage gaps, not successful delivery. The span closes after status publication, before update/maintenance and sleep.

Failed status publication records a shared `status-write` outcome and reports the original error in the service log. The loop retains its children and continues supervision, maintenance and retry; failed publication does not acknowledge observed state. Initial release-health marking requires a successfully published, complete pass without a service or focus/control error. The host requires this evidence from every currently registered project within60s, from its own threads; a publication failure or thread exit invalidates that project's evidence. Empty hosts still require valid policy and registry/ownership. Publication recovery can qualify later; an existing release-health marker is not a continuous health report.

Admission checks attach fixed refusal codes to their own generated spans: `hold`, `config`, the load gate's `sample|cpu|memory|cooldown`, or `launch-interval`. An empty focus marks the pass `refused/focus`. A normal pass or accepted Resume alone is not evidence of a child start: read the linked claim/launch and its result. Board/task filters and runtime-access-specific decisions still need full semantic qualification.

## Focus command state

`<root>/.taskq/focus.command` is a bounded4096-byte correlation pointer under the existing signal lock, never command authority or a second task store. It contains schema1, a SHA256 binding to the canonical project root, the validated original descriptor, phase `pending|applied|failed`, a SHA256 of normalized focus for `applied`, and at most one predecessor descriptor. It contains no focus labels, paths or request text. Required phase history stays in the shared store.

After required receipt acceptance and lock acquisition, commit the new `pending` pointer before changing settings, even for identical focus. Only after this invalidation may the preceding unfinished/applied command receive `superseded`. An outstanding predecessor is acknowledged before another pointer can replace it; failure preserves the pointer for idempotent reconciliation. A failed command already has a terminal and needs no superseded phase. After the local terminal receipt commits, replace the pointer with `applied` plus the focus hash, or `failed`. Use shared durable replacement; neither the pointer nor history makes the multi-file settings mutation crash-atomic. A failure after effects leaves an explicit uncertain result and never replays or rolls back effects.

The service reads focus and this pointer under one shared signal lock. It retries any pending predecessor receipt, requires `applied` and a matching current focus hash, then commits `observed` under the original descriptor. Its status exposes the tracked command's applied lookup id and acknowledged observed id. This proves a snapshot of desired focus, not task admission, launch, request-text readback or persistence of other focus metadata. Repeated passes may reuse that process's acknowledgment after revalidating the pointer and focus; restart commits the same observed id again. A superseding writer cannot enter until observation commits or fails.

Missing pointers are explicitly untracked legacy state. Interrupted/failed pointers, missing signals, mismatched focus, unreadable/corrupt/newer pointers and required observation failure hold new admission with a reason while child supervision continues. Corrupt, foreign-project or inconsistent files are preserved as unknown. Under the exclusive signal lock, a valid staged initial pointer means its effect never began: commit failure for that descriptor and remove only that temporary, preserving the prior current command. A staged terminal may complete a matching canonical pending pointer only after checking the current focus for applied state. Neither recovery path executes settings changes. Removal/re-save interruption leaves canonical pending unknown and may safely repeat. An ordinary later command can replace a complete pending/failed pointer without replaying it. Truncated or unattributable temporaries still require investigation; never guess their effects.

Resume/admitting focus refuses before effects if tracking cannot be prepared. Emergency Pause/exact focus-off still attempts restriction and reports tracking uncertainty. When tracking cannot be invalidated, an old descriptor is never presented as acknowledgment of the failed new command; status identifies only the tracked desired focus, and an unavailable/new effect has no observed receipt. Retention of receipt history remains bounded; a pointer's lookup id does not prove that its historical row is still retained.

## Stage tokens

The [terminal-record invariant](../../taskq.md#profiling-988) uses the same `taskq-stage` records. A lifetime marker has `open: true`, `outcome: unknown`, `unknown: open-stage`, and no duration; reports exclude it from completed duration samples. New records include the host name; historical records without it show unknown until repair proves ownership from the accepted spawn matching this host/runtime and a nonempty readable attempt log in this project root. Missing, unreadable or conflicting ownership evidence leaves the record unchanged. Repair appends a classification with unknown duration and usage; original records and attempt logs stay unchanged. Export keeps open markers as observations with `unknown: open-stage`. Missing terminal records are listed from the retained window only; fresh registered-project service snapshots exclude only children whose liveness is running; task rows and unknown child liveness exclude nothing. Selftest uses a fresh board read of this project instead, excluding fresh heartbeat leases, and uses its configured `ttl`. An unreadable record or unknown age prevents a healthy verdict. A terminal repair requires attributable native outcome or the exact execution release read back from the board. `gone`, issue closure/removal, a missing handle and age alone never prove that execution ended; they may classify task state only. Failed readback or append leaves the execution evidence unknown for a later check.

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

## Analysis guide

`taskq profile guide` prints the embedded [analysis guide](profile-guide.md) without store, board or network access. It rejects `--since`. The operator brief names it for time, context, usage and cost questions.

## View source and window

- Reports read this host's retained `<install>/telemetry.log/telemetry.sqlite3` over all its projects. Schema2 export also reads automatic observations in `universal.sqlite3`, as described below. Neither route makes board calls.
- `--since 30m|24h|7d` sets the window from that time up to now. Default: all retained records. A malformed window (empty, non-ASCII or unknown suffix) is a validation error.
- An unavailable, corrupt or unsupported store fails the view and names the reason.

## Export

`taskq profile export` emits schema2 JSON lines. The header names `coverage: committed-import-cursor-snapshot`, `since_ms` and each source's sample count and available retention bounds. Each subsequent row is `{source,seq,id,received_ms,observation}`. `observation` uses the shared coreq schema, preserving method start/terminal spans, parent identity, timings, outcomes and runtime metadata. Unknown timestamps remain included with their reason under `--since`.

The shared `Reader` reads the existing universal store without write permission, migration, checkpoint or pruning. The transitional legacy view takes a read-only shared profile lock and reads its existing SQLite snapshot; it validates the text checkpoint and refuses undrained/lost history instead of importing or hiding it. Legacy rows use the existing typed migration mapping; their absent immutable ids and pruning counters remain unknown. The universal Reader stays pinned while the archive is filtered by that snapshot’s committed source cursor. Imported positions are excluded even after normal universal retention; remaining archive positions appear once. Missing/replaced identity or unimported removals refuse the view as unknown. Compatibility reports order the version-unknown historical prefix, then unimported archive rows, then versioned live observations; a newer terminal repair cannot be overwritten by the imported prefix. This is local import consistency, not an atomic fleet view. This adapter is removed when the one-store cutover retires the archive. Corruption or unavailable sources fail the export before any JSON is printed. Snapshots and locks are released before stdout output. Export boundary spans are captured; internal telemetry decoding is excluded from recursive collection.

Task/board reports retain the internal schema1 compatibility projection below. Their existing fields and aggregation are unchanged.

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

Terminal-record invariant (R8, R13; § 4): executions with observed terminal outcomes have `ok`, `failed`, or `cancelled` records with an error class. Existing completed-turn `ask`/`retried` stage outcomes retain their meaning and are not missing records. Auxiliary stage records, including completed owner waits, preserve terminal evidence until a new build/review lifetime opens; that lifetime needs its own terminal record. The service repairs missing terminal evidence only from the native outcome or exact board execution release; task-state retirement alone is insufficient. Observations never initiate recovery. The configured `ttl` bounds detection of missing evidence, never execution lifetime; unresolved outcomes remain unknown with their reason.
Append diagnostics distinguish lock timeout and SQLite/migration errors.

Moved verbatim from `taskq.md` (#1179) to keep the contract under its R8 size bound. `taskq.md` keeps the decision and links here; this text keeps the same authority (R8, R13).
`fold` is the behavioral truth; board and process qualification remain separate under R12. Attempt stages (§ 4 Profiling) are adapter observations outside `fold`: `claim` from the `spawn` post through its readback; `start` the runtime start/resume; `build` (worker) and `review` (reviewer) from child start to exit or stop, outcome from the attempt's accepted hand-in (`result`/`close` ok, also when stopped after it; replaced without one is `cancelled`; an issue that left the board read is read back once, no hand-in or no read is `unknown` with reason); `ask` from this service seeing its attempt enter `ask` until it leaves (in memory, lost on restart); `checks` the publication's CI verification and merge call; `close` the whole `close` command. Queue transitions (§ 4 Profiling) are adapter observations too: a service pass replays each task log with entries newer than its previous pass, entry by entry, and each accepted entry that changes the folded state is one transition (`waiting` is not logged and adds none; `held` is outside the queue, so `hold` and `start` add none; the first pass after a service start adds none).
Follow [shared profiling](https://github.com/taskqorg/specq/blob/main/engineering/profiling.md). Time six operations: `service.pass` (board reconciliation, no sleep), `dispatch` (claim and readback up to runtime handoff), `spawn` (runtime start or resume, its failures included, not child lifetime), `forge.call` (one HTTP request), `update` (check, download, verification), `tray.render` (icon rasterization and native apply, not webview paint). Their budgets stay unknown until owner-approved baselines; profiling never changes admission or task state. Per the [shared agent work](https://github.com/taskqorg/specq/blob/main/engineering/profiling.md#agent-work), the attempt stages `claim`, `start`, `build`, `checks`, `review`, `ask` and `close` (#1147) are always measured too, budgets **unknown**, as `taskq-stage` records in the same store, lock and bound (the host is the log's own): project, task and attempt numbers, runtime (`claude`/`codex`/`codex-app`/`other`), duration, outcome (`ok`/`ask`/`retried`/`failed`/`cancelled`/`unknown`), error class (`exit`: a child ended without hand-in; `left-board`/`unreadable`: no hand-in evidence), and for `build`/`review` the [context tokens and call count](profiling.md#stage-tokens); service stages run no model.
Board operations (#1173), always on, budgets **unknown**: each call of the board adapter is one `taskq-board` line with its [kind](profiling.md#board-kinds), duration, outcome (`ok`/`failed`), error class and the lowest rate-limit `remaining` after it (none reported: unknown); cached clock, user and label-schema reads are not operations. Queue transitions (model: § Executable behavioral model): one `taskq-transition` line per transition with project, task, entry id, from and to state, time in the previous state (board seconds between the entries; the first entry's start is unknown), runtime and host of the attempt on either side; the service of that host records it, each service records a move without an attempt (consumers dedupe by task and entry).
All these records hold fixed codes and numbers only (`TASKQ_PROFILE_TRACE=1` adds span ids) in `<install>/telemetry.log/telemetry.sqlite3` under its § 4 Data bound; appends wait up to 1 s for `taskq-profile.lock`; only text-log rotation uses a nonblocking try; SQLite pruning frees the oldest records at the bound; service threads serialize through one in-process writer, with project/host context kept per thread; an instrumentation failure never fails the operation. Its chain check (R12) races appends against a prune that wins the lock: every retained record is a counted successful append.
`taskq profile` reads the retained log without board access, ranks the six by p95, failure rate and variance, then board operations by p95 with failures and the lowest `remaining`, and queue transitions with time in the previous state, and lists this host's board requests of the past hour by source (§ 4 Sources); corrupt or missing records read unknown, counts cover retained records only; collection completeness, earlier or pruned history and adapter retry or cancellation counts stay unknown; an hour with records starting inside it says partial; agent `gh`, PM chat tools and DashQ are not counted. Last it ranks attempt stages per runtime by p95, failure and retry rate, with count, p50/p95/max, outcomes, model calls and context tokens (sum, k of n known, else unknown with reason); DashQ reads this local report, never the board, for stage data, which holds no task text, secrets or host paths.

| Command | Does |
|---|---|
| `taskq profile [export\|tasks\|board] [--since 30m\|24h\|7d]` | no board; this host's retained log, all projects ([detail](profiling.md)). Bare: § 4. `export`: JSON lines, a header (columns, window, samples), one row per observation: `kind at_ms project task attempt entry name from to runtime host us prev_s outcome error remaining calls tokens unknown`. `tasks`: time in state (`Project Task State Runtime Host Count Total p50/p95/max`). `board`: operations (`Operation Count p50/p95/max Failed% Errors Lowest-remaining`). Unknown reads unknown with reason, never zero |

## Coverage and delivery

Coverage is every executable product path, including UI input, windows, layout/resize/render and observed paint, command results, service/board actions, agents, tools, context loading and compression; named operations are not a coverage exemption. New handlers inherit operation capture through shared execution boundaries; owned methods inherit disabled diagnostic sites. Internal per-call tracing is opt-in, while commands, dependencies and decisions remain visible. Input, execution and observed effect share operation/span identity; a restored focus is not admission while the host is held. Collection is local first in coreq's durable bounded store, with agent-readable export; optional sync advances only on verified durable receipt. Legacy retained observations stay readable. Coverage, store, adapters, global ingestion and three-host rollout each require their own evidence; a subset's green CI is not full qualification (R8, R12, R13).

## WebView execution outcomes

R6/R12 coverage includes the page bootstrap and every native script submission. Submission is not execution or paint. Each evaluation has a causal shared operation, a bounded native pending slot (64 per window) and a30s acknowledgment deadline. A fixed successful callback confirms synchronous JavaScript execution only. A caught exception is failed; missing/invalid callbacks, window exit and bootstrap without ready remain unknown. No automatic script replay. Snapshot capture waits for execution acknowledgments and fails on page errors or missing acknowledgments; its layout delay is not paint proof.

An early page observer handles uncaught errors, unhandled promise rejections, resource failures and evaluated-script exceptions before product modules load. Only fixed classes and cumulative counters cross the bridge; no exception text, stack, URL, script or user content. Repeats coalesce into at most one report per2s after the first; native records count deltas through the shared collector. Counters saturate explicitly. Counts cover received cumulative reports; a crash can lose the last coalescing window, so they are lower bounds without final delivery. These are page-wide observations, never guessed attribution to a click.

The first page/execution fault makes interface outcomes unknown and replaces controls with an independent static error notice. Existing submitted work is not cancelled or retried. Native stderr and the tray tooltip retain the failure when page delivery itself fails. Closing/reopening the interface is recovery, not proof of command completion; inspect service status before retrying. Optional observation still cannot block product threads on storage. Real native input/paint, installed hosts and collector-loss persistence remain separate readiness gates.

## Process bootstrap

Shared process execution forwards only the current trace/span identities in coreq's bounded process-context field. The child runtime consumes this link before application dispatch; a malformed field marks capture incomplete without changing the command outcome. Service commands and tray actions carry allowlisted operation suffixes, never argument values. Independent Stop/Uninstall launches use the same context helper and explicitly select the registered project through `TASKQ_ROOT`, so an inherited root cannot reject a command for another selected project. Their separate process group stays unchanged. Child command outcomes can be correlated after the tray exits; this is not a durable command receipt, proof of effective service state, process-tree supervision or UI acknowledgment.

The thin root `entry.rs` initializes shared coreq capture before generated product execution. It records no arguments and reports collector initialization failure as unknown without changing command results. Bootstrap until subscriber installation is an explicit coverage exclusion; source inventory covers `src/`, not this wrapper. The shared runtime guard flushes on normal return and unwind; generated `std::process::exit` and explicit Unix exec boundaries flush before bypassing destructors. Open spans without a terminal remain unknown. Protected sandbox store access still needs qualification through the host writer; no grant changes are implied.

## Shared host writer

The shared runtime elects one local writer thread for the installation. Other processes append authenticated bounded batches through its protected endpoint, without opening SQLite or taking its initialization lock. Clients retry immutable ids after uncertain delivery and re-elect after writer death. No database-write grant is added to a worker. Unix socket access and Windows loopback access must be proved through each supported installed sandbox before qualification. Endpoint credentials stay in the owner-only `telemetry-ipc` directory and are never exported as telemetry.

## Compatibility checkpoint

The draft shared store is `telemetry.log/universal.sqlite3`, protected from older release sweeps by the retained directory. The v4.5.0 archive remains `telemetry.log/telemetry.sqlite3`; it is preserved without schema reset. Consolidation of its profile/stage/board/transition/request adapters and reports into coreq remains required before release. Two active store paths are a temporary development state and do not satisfy the one-store contract or the total physical bound.

Historical import uses coreq's deterministic source-position identities and atomic event/cursor transaction. A failed batch cannot advance the cursor; a replay after process death cannot duplicate records. Source files remain unchanged during expansion. Legacy conversion and report readback must be verified before disabling the old writer; retirement is a separate step. Common IPC returns stable rejection classes without raw SQLite messages or input values.

The shared store now bounds database/WAL/SHM files independently of retained JSON bytes. It rejects a new mutation behind a pinned WAL reader and reports allocation through storage(). This qualifies the shared store only: the v4.5.0 archive plus the draft store still need consolidation and a total transition-budget check before release.

The local-capture selftest requires the installed collector to flush committed observations with no collection loss. An absent, empty or failed collector makes selftest fail. Ordinary commands keep their outcomes. This local barrier does not prove UI coverage, global ingestion, other hosts or queue readiness.

### Native render regression gate

Candidate `cd9316e` fails the first native tray icon with collection enabled. An isolated macOS tray stalled in `icon::compact` → `draw_q` → `Rgba::fill`: generated callback spans run for every4×4 subpixel sample. The collector reported queue overflow and lost observations. The matching isolated service store retained124035 rows and had already pruned240384 rows when the probe stopped. These are diagnostic observations, not a production incident or a throughput target.

`icon::tests::first_icon_with_capture_finishes_without_collection_loss` runs the same first-icon code in two isolated child processes. Baseline capture-off rendering took16868µs; capture-on reported loss and did not complete within the20s diagnostic bound. The bound limits test damage; it is not a UI responsiveness budget. The test requires committed capture with no loss and identical PNG output. Earlier renderer tests had no active collector and missed this path. This failure established the regression gate; removing the render action, enlarging the queue or raising the deadline would not fix its cause. Internal sample callbacks now belong to bounded diagnostic mode under the revised shared contract, not always-on action history. Native input/paint qualification remains incomplete.

The revised normal-mode gate keeps each render action and disables internal callback detail before span construction. The release-mode chain compares30 render samples per mode, requires matching image bytes, exactly30 committed render terminals, no diagnostic method rows and no collection loss. Its p95 limit is the larger of5% or1ms above baseline. The git-pinned macOS candidate measured433µs baseline and831µs capture p95 (+398µs, within the1ms allowance); this qualifies that workload only, not native input/paint, other hosts or deployment.

## Common-store conversion qualification

The draft typed converter in `src/telemetry/semantic.rs` reuses the existing profile/report schemas and request normalization. It preserves operation trace numbers, stage open/usage status, board remaining budget, transitions and request summaries through coreq semantic metadata. Historical product version and textual trace/span identities stay explicitly unknown; numeric provenance is retained separately. Request summaries remain summaries, with reversible fixed-code source/operation keys. The converter is test-only until one-store cutover, source reset detection, physical transition budget and report readback qualify. The installed writer and archive remain unchanged.

The legacy-source identity extension is installation-local metadata in the same SQLite file: one random namespace and an epoch column on records. Existing rows start in epoch0; insert triggers advance the epoch when SQLite reuses a sequence after deletion, while old retained rows keep their original epoch. Deletes track the highest removed position; an unimported removal is a coverage gap, not a healthy empty source. Updates of logical source records invalidate migration. Import progress contains namespace/epoch/sequence and commits with observations through shared import_history; replacing or losing source identity refuses resume. The extension must be atomic and preserve existing legacy readers/writers. Forced termination before its commit leaves the old schema/rows intact; after acknowledgment the same namespace survives reopening and import replay. It remains qualification-only until the total transition budget and one-file cutover pass.

Before source identity installation or target import, validate the explicit installation's complete legacy text against the archive's existing full/rotated checkpoint. Refuse rewritten, truncated or undrained telemetry; diagnostics after the checkpoint add no observations. Verification does not import text with a new receipt time. The caller holds taskq-profile.lock so text/record producers and rotation cannot race this check. Archive-only snapshot conversion does not qualify this cutover precondition; retained-source qualification also supplies the matching frozen taskq.log.

Large legacy request summaries retain every normalized counter through coreq split_identified/assemble; no per-project partition format is added. Each part keeps the original receipt clock and all parts of one source record commit with the cursor in the same import batch. If a complete record would exceed the1024-event batch, the adapter commits only preceding complete records and resumes at that record next time. A partial retained group makes the report unknown; per-part request totals are never exposed as complete summaries. The16KiB event,64KiB source-record and existing SQL/IPC budgets stay unchanged.

The compatibility report adapter reads selected semantic kinds through coreq export_kinds, carries shared assembly across pages and follows more even for an empty page caused by older unindexed writers. Store-wide retention metadata remains distinct from selected report samples. The adapter remains test-only until one-file routing, total transition budget and installed handover qualify.

## Revised readiness gates

These are acceptance targets, not claims about the installed release. Shared mode semantics are owned by [specq profiling](https://github.com/taskqorg/specq/blob/main/engineering/profiling.md); shared collector limits are owned by coreq.

- Normal mode: native icon and common local commands must retain identical results and no unexplained collection loss. On each host, compare30 matched release-mode trials with and without capture; p95 operation latency may increase by at most the larger of5% of baseline or1ms. Measure operation work separately from process/store startup and flush; report both. No product-thread disk/network export or per-pixel clocks/records.
- Render acceptance includes a durable render action; detailed method sites remain selectable. Diagnostic sessions expire within60s and admit at most256 spans. Reaching the budget reports truncation without changing the product outcome or disabling operation records.
- After a project poke is observed, the next admission decision occurs in the current/next pass. Idle wake overhead is at most2s; a pass already blocked on I/O must expose that operation and its finite deadline, not claim a2s end-to-end response. Superseded signals remain explicit delivery gaps until command receipts qualify.
- Telemetry queue payload stays within16MiB and a writer batch within16MiB; the installation's existing64MiB physical store bound includes migration overlap. Bound and report memory/CPU/flush costs on all three hosts; old/new stores do not receive separate allowances.
- Verify missing permissions, network refusal/timeout, child exit/panic, store contention/full/corruption, collector death, export outage and crash recovery. Each yields a scoped outcome or visible uncertainty; emergency stop remains usable. Remote export outage does not stop local collection or task execution.
- Required command receipts must distinguish received, applied, observed and superseded. Never acknowledge durable acceptance before commit; inability to persist required receipts has an explicit command failure policy. Receipts cannot become a second source of task truth.
- Readiness requires real installed Mac/Windows/FRA chains, current running process/client versions, actual spec precedence/freshness and context/compression evidence, plus an isolated controlled task through result/review. Keep production holds until owner release.

## Bounded external commands

Worker cancellation separates request from observed exit. Failed/pending stop retains the owned handle, occupied capacity and explicit reason through normal or reduced supervision. A local replacement or continuation for that task waits for confirmed exit even when another slot is free. A CLI terminal log starts cancellation; it cannot synthesize a process exit. Child/adopted-child signal delivery uses nonblocking exit readback; Windows taskkill uses the shared10s executor. Native Claude stop makes one bounded query/request/readback sequence per pass instead of polling inside the pass. A cancelled app handshake never queues a task; an already queued app turn remains fenced and supervised until native completion/absence. These checks do not yet qualify arbitrary descendant ownership or restart recovery of an uncertain launch.

CLI worker, app handshake and CLI operator input use an opened immutable file as stdin, prepared before process creation in the existing bounded `.taskq/inputs` cache. Bytes are unchanged; no prompt enters argv or a shell. The inherited read handle remains valid if later cache pruning removes the path. Unix files are owner-only. Shared launch observation hands the process handle to its caller immediately after OS creation; it does not wait for the child to consume input or prove task completion. Preparation/spawn failure has no handed-off process. Native Claude background launch and app queue-command uncertainty still require separate ownership qualification.

Finite dependency checks use the shared executor:10s for host identity and app-presence queries,15s for rule downloads around curl's10s,30s for signing, Git qualification/cleanup and Windows sandbox preflight. Output overflow or an unreadable result refuses the affected action; cleanup keeps the worktree when its complete patch cannot be saved. These bounds do not apply to interactive windows or worker lifetimes.

An app-presence query failure, missing/unreadable rollout or invalid completion record keeps the owned app turn unknown with its capacity retained. Only a successful native absence query or a complete new turn record proves termination. File read errors and truncated history never become zero completed turns. A partially written record remains unknown until a later read; it does not release capacity.

Runtime CLI probes use shared coreq command execution:10s for execution plus output drain,1MiB combined stdout/stderr, with precise spawn/exit/timeout/output/cleanup observations under the caller trace. Missing or incomplete output remains unknown. Service/update deadlines belong to [coreq bounded command execution](https://github.com/taskqorg/coreq/blob/main/coreq.md#bounded-command-execution). No input, arguments, credentials or output is exported. Direct-child cleanup stays owned; detached worker trees and native action completion still require their own lifecycle qualification.

Board HTTP uses shared bounded stdin exchange:65s around curl's60s, at most1MiB encoded config and8MiB response. A bound failure is explicit and never interpreted as an empty board or confirmed mutation. Preserve tokens/body on stdin, HTTP/GraphQL/rate-limit interpretation and existing request budgets; no retry is added by the executor. Native curl tests and stalled/full-duplex/inherited-pipe fault checks qualify this adapter separately from live forge behavior.
Expected list responses must be arrays: a successful HTTP response containing another JSON shape is an error, never an empty issue list, comment list or label catalog.


## Product diagnostic controls (contract detail)

Any CLI command accepts `--diagnose <method-prefix|*>`, `--diagnose-ms <1..60000>` (default60000) and `--diagnose-limit <1..256>` (default256). Selection applies to this process's remaining execution; it grants no effects or permissions. Shared Capture records the selection, expiry and truncation. Internal detail stays off otherwise. For an already running service or tray, `profile diagnose --pid N --selector PREFIX --duration-ms 60000 --limit 256` requests the same bounded selection. It binds the native PID/creation identity, never a PID alone. The existing service wake and tray tick deliver it; the response says requested, not applied. Verify the target's diagnostic state in status or export before claiming activation.

`profile actions [--since 24h]` ranks committed normal action and selected method terminals from universal.sqlite3, separately from legacy operation tables. Retained starts without a matching terminal read unknown; never infer completion from an expired selection or a process exit. The report names its retained window and pruning. Known native observation budgets refer to runtime-qualification.md; unassigned operation budgets stay unknown.

`profile health` reports this process's collector state, admitted/committed/lost counts and diagnostic state. Service status includes the same scoped snapshot; a metadata-only health observation uses the existing writer. A missing health record, collector loss, queue failure or unreadable store is unknown. Current local health never establishes historical completeness or health on another process/host. Export preserves the scoped observations for analysis.

The single installation-local `telemetry.log/diagnostic.json` control request is at most2KiB, plus one bounded atomic pending file covered by reserved metadata space. Writers serialize under the existing profile lock; a request expires and may then be replaced. No append journal, worker authority or new service. Each process applies a request at most once; identity mismatch or malformed/unreadable data is explicit unknown, never a renewed session. A1s poll throttle uses existing service and tray work; cold bootstrap also checks the request. A blocked prepare file is preserved and reported for diagnosis.

## JS execution architecture

Owner steering2026-10-17; accepted target, not yet qualified. Use the shared CoreQ module wrapper for ordinary synchronous logic/view methods. One definition wraps every method, including newly added ones; internal calls use the wrapped module. Normal mode aggregates calls/outcomes cheaply; selected diagnostics add bounded timing/detail. No build-time rewriting of product JS. Pure inline expressions stay inside the enclosing measured method; nontrivial helpers are wrapped module methods.

Events, commands, native handoff, layout/render and asynchronous effects enter shared execution boundaries. Components own no detached promises or timers. An effect owner observes settlement, restores the initiating context for its synchronous result callback, and retains missing completion as unknown. Nested callbacks do not inherit an ambient context across unrelated async work. Render submission, JS execution/frame callbacks and native paint are separate observations; unavailable paint proof stays unknown. Module inventory and a new-method/event regression must detect bypasses. Preserve existing component APIs, permissions, content exclusion, control/receipt semantics and the one bounded writer. Optional metadata delivery failure marks collection unknown and notifies once through the native log/tooltip; it never disables command controls. Invalid page metadata still marks the interface untrusted. Required command receipt failure keeps its separate admission/emergency-Pause behavior. Native/browser and normal/detail cost proof is required before audit7 closes.

JS packaging: ordinary CommonJS modules use the same immutable CoreQ revision as Cargo. Esbuild bundles the tray and standalone public components; Acorn is a development-only architecture inventory check over every owned JS source, including future files. It derives event names from literal registrations/lists and rejects raw event/timer calls, unwrapped named/object/class methods and unowned async definitions. Capture/bootstrap infrastructure is excluded from recursive instrumentation; the two Promise-producing native IPC adapters remain effect-owned ports. A single build script packages HTML/CSS and generated JS before Rust includes it. No Rust chain of JS-content substitutions or test extraction by source markers. Node/npm are build/test dependencies, not installed-service dependencies; merge/release builders run the locked package install with the existing900s gate budget. Component public APIs remain, and standalone release bundles include the shared wrapper.
