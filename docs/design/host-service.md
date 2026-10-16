# One service process per host

Status: design detail for the adopted host model in [taskq.md](../../taskq.md). Runtime recovery follows [execution ownership](../runtime-qualification.md#execution-ownership); remaining migration and native qualification are not implied by this document.

## Decision

- One unit per host: `com.taskq.service` (launchd), `TaskQ` (Task Scheduler), `taskq.service` (systemd user). It runs `<install>/taskq service run` with `<install>` as working directory.
- Startup owns the existing CoreQ native lock at `<install>/service.lock` for the whole run. PID publication is a projection; it cannot serialize simultaneous startup. Keep the lock through cleanup and every project thread; restart reconciliation never treats record age as exit.
- The process serves every root listed in `<install>/projects`, one project thread each.
- Host-wide parts run once, in process: admission, the GitHub request budget and rate-limit pause, the update check and re-exec, the tray spawner, the profile appender, hourly request records and bounded log pruning.
- Per-project parts stay per project: `taskq.json`, `<root>/.taskq/` (`status.json`, `events`, `focus`, `focus.paused`, legacy `kids`, `poke`, `update`, `requests/`, `requests.log`, `incidents/`, attempt logs, `cli-errors.log`, `pm-cursor`, specq cache), worktrees, the project log `<install>/<project>.log` and one heartbeat issue per board.
- Native recovery records stay under protected `<install>/native/`, keyed only by canonical registered project roots; [handover contract](../runtime-qualification.md#adapter-recovery-matrix).
- `service install` and `service uninstall` in a project edit `<install>/projects` and poke the process; they install or remove the host unit only when it is missing or no project remains.

## Historical design map

Original old-to-target proposal, retained as design rationale. The adopted taskq.md contract, current linked owner pages and H-A1–H-A5 below govern implementation; these historical quotations are not independent active rules and include superseded telemetry wording. R8 retains the 300-line/60 KiB root bound. No board fold or golden trace change belongs to this transition.

### § 3 Host admission (line 144)

Replace:

> - **Host admission.** `<install>/admission.json` under `<install>/admission.lock` admits every Auto launch after the load gate; a grant needs every fresh service of `<install>/projects` at `4.2.0-dev` or newer, held reservations under `admission.max` and the 30 s launch spacing. An unreadable file refuses; Fixed services are never refused.

With:

> - **Host admission.** The host service admits every launch across projects after the host load gate, in process: held reservations under admission.max, runtime limits and the 30 s launch spacing. Fixed mode is removed. Until all legacy services have drained and exited, hold new admission host-wide; existing attempts keep leases and may resume to finish. Unknown legacy liveness holds admission.

### § 4 opening (line 148)

Replace:

> `taskq service run` is the host's single long-lived process; `taskq service install` makes the OS keep it running (§ 8).

With:

> `taskq service run` is the host's single long-lived process for every root in `<install>/projects`, one project thread each running the loop below; `service install` registers a project and makes the OS keep the process running (§ 8). A panic in a project thread restarts that project only: its children keep running, re-attached to the rebuilt state; three panics in 10 minutes quarantine it (reap, heartbeat, `status.json` only) until `service restart`.

### § 4 step 7 (line 158)

Replace:

> 7. Every `update_every` (60 s), or at once when a local command wrote `<root>/.taskq/update`: the self-update check (§ 8); an installed service applies a newer version, others only report it. Its outcome is logged.

With:

> 7. Once per host every `update_every` (60 s), or at once when a local command wrote any `<root>/.taskq/update`: the self-update check (§ 8); an installed service applies a newer version, others only report it. Its outcome is logged in every project log.

### § 4 Poke (line 164, first sentence)

Replace:

> - Poke: a local command touches `<root>/.taskq/poke`; the sleeping service checks its mtime every second. Each pass

With:

> - Poke: a local command touches `<root>/.taskq/poke` (that project) or `<install>/poke` (the project list); the service checks their mtimes every second. Each pass

### § 4 One service (line 167)

Replace:

> - One service per root and host name: `service run` waits while a `status.json` under 60 s old names another live pid; a pass that finds this host's heartbeat under `ttl` naming another `service <id>` writes nothing and fails with `another service <id> runs as host <name>`. Restart: rebuild from the board and reconcile original native executions; missing handles stay unknown and never imply `gone` or release. `service restart` writes `<root>/.taskq/restart`: the next pass re-execs (§ 8 step 3).

With:

> - One service per host: `service run` waits while `<install>/service` (the service's pid) is under 60 s old and names another live pid; a project thread waits while its `status.json` under 60 s old names another live pid; a pass that finds this host's heartbeat under `ttl` naming another `service <id>` writes nothing and fails with `another service <id> runs as host <name>`. Restart: rebuild from the board and reconcile original native executions; missing handles stay unknown and never imply `gone` or release. `service restart` writes `<install>/restart` (a project's `<root>/.taskq/restart` counts the same): the process re-execs (§ 8 step 3).

### § 4 Tray (line 168, first sentence)

Replace:

> - Tray (macOS, Windows; [menu](menu.md#contract-detail) M1–M14): one per host, started by any installed service when `<install>/tray` (its pid) is older than 30 s.

With:

> - Tray (macOS, Windows; [menu](menu.md#contract-detail) M1–M14): one per host, started by the installed service when `<install>/tray` (its pid) is older than 30 s; restart and stop act on the host service and name every project first; pause, resume and uninstall stay per project.

### § 4 Profiling (line 177, first sentence)

Replace:

> Records hold fixed codes and numbers only in `<install>/taskq.log` under its § 4 Data bound, appended under the nonblocking `taskq-profile.lock`;

With:

> Records hold fixed codes and numbers only in `<install>/taskq.log` under its § 4 Data bound; the service appends through one in-process writer, every writer under `taskq-profile.lock`;

### § 5 service row (line 196)

Replace the `Does` cell:

> § 4, § 8; `permissions`: § 8 Access read, or one grant write with readback; `hold`/`release`/`recover`: § 3, § 4 emergency; `stop`: until the next logon, the unit stays; `list`: this host's projects and their focus; `tray --snapshot DIR [--route R,..]`: § 4; `tray --icon FILE`: [custom icon](custom-icon.md#contract-detail)

With:

> § 4, § 8; `install`/`uninstall [--host]`: register or drop this project in `<install>/projects` and poke the host service (`--host`: drain, verify exit, then remove only the host unit, § 8 rollback); `permissions`: § 8 Access read, or one grant write with readback; `hold`/`release`/`recover`: § 3, § 4 emergency; `stop`/`restart`: the host service, every project (`stop` until the next logon, the unit stays); `status`: the process, then one block per project; `list`: this host's projects and their focus; `tray --snapshot DIR [--route R,..]`: § 4; `tray --icon FILE`: [custom icon](custom-icon.md#contract-detail)

### § 8 Shared core (line 266, unit names)

Replace:

> units `com.taskq.<project>`, `TaskQ-<project>`, `taskq-<project>.service`.

With:

> one host unit `com.taskq.service`, `TaskQ`, `taskq.service` ([host service](host-service.md#migration) removes the legacy `com.taskq.<project>`, `TaskQ-<project>`, `taskq-<project>.service`).

### § 8 step 3 (line 269)

Replace:

> 3. At step 7 an installed service older than `current` re-execs through the launcher; live CLI children pass over through the protected native handover record; an active Claude, codex-app or operator turn defers it. Detail: [runtime qualification](../runtime-qualification.md#contract-detail).

With:

> 3. At step 7 an installed service older than `current` re-execs through the launcher, once for all projects; on Unix, live CLI children pass over through each project's protected native handover record with the same pid; an active Claude, codex-app or operator turn of any project defers it. On Windows, all projects must have no live child or active turn before exit 75; the unit restarts with a new pid, with no child adoption. Detail: [runtime qualification](../runtime-qualification.md#contract-detail).

### § 8 step 5 (line 271)

Replace:

> 5. `service install` installs its binary as a new file, never over an existing release binary (macOS kills rewritten binaries). While the project's `status.json` lists live children (or is unreadable) it writes `<root>/.taskq/restart` instead of reloading. Windows stops end the process tree (`taskkill /T`); run Windows setup outside an MSIX app. The coreq launcher counts unhealthy starts per project.

With:

> 5. `service install` installs its binary as a new file, never over an existing release binary (macOS kills rewritten binaries), adds the root to `<install>/projects` under `<install>/projects.lock`, writes and loads the host unit when missing or changed, and pokes `<install>/poke`; `installed` only after the project's `status.json` names the service pid. While any project's `status.json` lists live children (or is unreadable) a changed unit writes `<install>/restart` instead of reloading. `uninstall` drops the root, admits nothing more there and keeps its children until they exit (R11); with no project left it removes the unit only after all draining children/turns have ended and process exit is verified. Windows stops end the process tree (`taskkill /T`); run Windows setup outside an MSIX app. The coreq launcher counts unhealthy starts of the host unit and never rolls back below the first host-service version.

### § 9 timers (line 286)

Replace:

> - `ttl`, `poll`, `update_every`, `rollback_after`: seconds, defaults 300, 60, 60, 600.

With:

> - `ttl`, `poll`, `update_every`, `rollback_after`: seconds, defaults 300, 60, 60, 600; `ttl` and `poll` per project, host.json owns `update_every` and `rollback_after`, read once per pass.

### § 3 Capacity and Load gate replacements

Replace the Capacity bullet with:

> - **Capacity** is host-wide: `<install>/host.json` runtime limits minus occupied slots across every registered project, including reviewers and resumes. One slot is keyed by project identity plus attempt: a granted pending launch and its running child are the same occupancy, counted once for host runtime limits, admission.max and project caps. The pending-to-running transition never adds another slot. An optional project `limits` can only lower that project's share: effective remaining capacity is the minimum of host remaining and project remaining. Missing runtime limit means no count ceiling; 0 disables. Lowering never stops children (R11). The daemon reads and validates host.json once per pass; invalid or unreadable policy holds all new launches and reports unknown, without discarding existing children or leases.

Replace `Load gate (opt-in, § 9)` with `Load gate (host policy, § 9)`; retain its thresholds, unknown handling, three-sample reopening and 30 s spacing. Remove every Fixed-mode bypass. Serialize slot grants and pending-to-running transitions; retain the same slot until verified failed launch/exit, then release it. Count the union of pending grants and running attempts by project identity plus attempt, never their sum or issue number alone; rebuilding from child handles preserves that identity. Choose ready projects round-robin, preserving priority then issue order within each project; resumes/reviewers retain their existing precedence. Count legacy work during drain; migration admits no new tasks.

### § 5 capacity command replacement

> Use the canonical `taskq service capacity` syntax in taskq.md section5; `--host` is explicit and local for writes. No --mode or remote-host edit. Optional project limits remain narrowing project configuration; this target adds no unimplemented --project command.

> Read or edit this host's host.json through the same policy operation regardless of invocation directory. Show host limits/use and narrowing project caps separately, write atomically on explicit owner instruction (R9), poke and report applied only after daemon readback. Preserve resource-grant checks and revision/receipt guards; no new authority for workers. Legacy per-project edits remain a qualified compatibility path only until retirement.

### § 9 host and project configuration replacement

Replace the `hosts`/`has`/`limits`, `capacity_hosts`/`admission`, `permission_mode`/`codex`, and timers bullets with these four bullets:

> - Host-owned `<install>/host.json`: `hosts` hostname-to-name mapping (TASKQ_HOST override), `has` name-to-capabilities mapping, runtime `limits`, `admission` {"max": n}, `load_gate`, `codex` args and `permission_mode` (R9). One local host policy, no capacity_hosts or mode. Host admission and load-gate defaults retain the existing Auto/D03 defaults; migration never silently enables a gate or changes runtime permissions.
> - Project taskq.json retains board/repo/board host, publish/delivery, workspace, labels, recipes, incident_reports, pages, assignee and all other project-only keys; optional `limits` only lowers the host limits. No host identity, capability, model/permission or admission copies remain here. Token lookup and worker-policy grants (§ 8 Access) are unchanged; host flags never grant access.
> - `ttl` and `poll` stay per project (300 and 60 s); `update_every` and `rollback_after` move to host.json (60 and 600 s). Missing/invalid host policy reads unknown and holds launches; it never falls back to per-project copies.
> - `<install>/projects` remains the explicit owner-approved list of exact roots; no directory discovery (R10). Install/uninstall edits it under projects.lock, atomically, and pokes the daemon. Removing a project drains its existing attempts before retiring its thread or removing the last host unit; status retains a draining row until verified exit.

Update the § 4 update interval replacement to read host.json timers, rather than aggregating project timers. Update § 8 step 4: normal Unix image updates preserve eligible CLI children; Windows waits for idle across all projects. Topology migration and reverse rollback use the explicitly approved drain below. Keep releases immutable, checksum/smoke verification and atomic current/previous activation. Before activation, preserve the old launcher, unit definitions and configuration; set the topology rollback floor only after verified host-unit ownership. Failure before that boundary restores the legacy state, not a partial host installation.

### Six-copy configuration migration

The six existing project `capacity_hosts` copies are input, not six independent host policies. On each host, read only roots already in projects; record six expected inputs as present/absent/unknown without discovering folders. Resolve each current effective local-host policy using existing override precedence, including top-level defaults, host mappings, capabilities, codex args, permission mode and timers. Do not copy another host's capacity entry into the local policy.

Compare the resolved policies before writing anything. Identical values collapse to one host.json. Differences, missing/unreadable inputs, Fixed-to-Auto changes, and policies that cannot be represented as host limits plus lowering project caps are owner choices: show values and affected projects, recommend a conservative policy, and stop migration until answered. Never infer that a zero in one copy is a host-wide owner decision. This design does not assert those six live copies were inspected in this task.

After approval, save byte-for-byte originals and the chosen mapping in a bounded migration backup under install; preserve unknown keys and original file ownership. Stage host.json and project edits, validate all, then atomically replace each file with restart-safe progress/readback. Keep every affected service admission-held until the whole set reads back; interruption resumes or restores originals, never enables partial policy. Remove only migrated keys, including capacity_hosts, admission/mode, load_gate, hosts, has, codex and permission_mode; keep optional lowering limits and unrelated keys. Preserve per-host backups: project files may contain remote-host copies needed by another host's migration. Re-run is idempotent. Do not commit local paths or secrets. Rollback restores these originals only within a qualified compatible protocol version; legacy services that expire attempts cannot restart against unresolved executions.

## Migration

Owner approval of this drain handoff is required before implementation (R13); it replaces the no-drain promise only for the topology migration and its reverse. The first host-service release is N. Legacy services run N-1 or older. After approval of the design and resolved configuration mapping, migration needs no additional owner step on an automatically updating host; it runs once per host, idempotent, in any order across hosts.

1. **Update.** A legacy service applies N through the existing update (§ 8 step 2) and re-execs at step 7 (§ 8 step 3). On Unix, eligible live CLI children pass over through the protected native handover record with the same pid; active Claude, codex-app and operator turns defer it. Windows waits for every child and active turn to finish, exits 75, and the legacy unit starts N with a new pid; Windows does not adopt children.
2. **Legacy mode.** N started by a per-project unit (working directory is a project root, unit name `com.taskq.<project>`, `TaskQ-<project>` or `taskq-<project>.service`) runs that project in legacy mode:
   - It adds the root to `<install>/projects` if missing, then writes and loads the host unit if missing.
   - It admits nothing new: capacity reason `handoff`. It keeps reaping, resuming answers of its own live attempts, beating and writing `status.json`, so no lease expires.
   - When it has no live child, it removes its own unit (`launchctl bootout` and plist delete; `schtasks /Delete`; `systemctl --user disable` and unit file delete) and exits 0. Run removal through an idle handoff helper where unloading kills the caller; the helper verifies daemon exit and unit absence. A unit removal it cannot read back stays in the project log and in `status.json` `reason`; the process then keeps running in legacy mode and retries every pass.
3. **Host service.** The host unit starts N in host mode. Each project thread waits while that project's fresh `status.json` names another live pid (§ 4 one service), so a legacy or N-1 service and the host service never serve one root together. When the legacy process exits, the thread rebuilds from the board; admission by focus starts only after all legacy roots have drained and host policy migration reads back.
4. **N-1 on the same host.** An N-1 legacy service that has not updated yet keeps its unit and its root; the host thread for that root waits. New admission is held host-wide until all legacy services drain and exit. Legacy Fixed services cannot be made safe merely by joining the old Auto protocol; no legacy service is force-stopped.
5. **Readback.** `service status` shows `Service: running (host, pid <p>)` and one block per project. Migration is complete on a host when no legacy unit file exists and every registered root's `status.json` names the host service pid. The release qualification task records this per host.

Why drain instead of cross-process adoption: removing a legacy unit kills its process tree on systemd (`KillMode=control-group`) and may on launchd; Windows has no adoption today. Children therefore stay with the process that started them until they exit; no attempt goes `gone` and no worker runs twice. The cost is that new admission waits host-wide for the longest legacy attempt and any unresolved configuration decision; no finite completion time is promised. Cross-process adoption on macOS is an upgrade path only after a probe shows a child in its own process group survives `launchctl bootout` (R12).

### Rollback

- **Within host-service versions** (N+k to N): the coreq launcher rolls back after three unhealthy starts of the host unit, as today per project, and never below N (rollback floor, like § 8 Access).
- **Back to per-project units** (owner decision only):
  1. `taskq service uninstall --host`: requests a host-wide drain, keeps `<install>/projects`, and admits no new attempts. Reaping, heartbeats and inbox delivery for existing attempts continue until all children and active turns end. Keep the host unit installed throughout the drain; unknown liveness blocks removal.
  2. Read back zero live children/turns, stop the idle host daemon and verify its recorded pid has exited; only then unload/delete the host unit and verify removal. Never remove a unit while it owns live children. A failed readback stops rollback before any older unit is installed.
  3. Restore the saved per-project configuration and prior launcher/current state, including an explicit update hold so N is not immediately reapplied; then run the preserved older release's `service install` in each registered root. Verify each legacy unit and its new per-project pid in `status.json` before releasing the hold. Keep originals and host.json for retry; do not delete worker worktrees or board history.
- An N-1 binary never starts as a host service: the host unit runs `<install>/taskq service run` without a project root, which N-1 refuses at config load. The rollback floor keeps the launcher from picking N-1.

## Isolation

- **Project panic.** Each project runs in its own thread. A pass runs inside `catch_unwind`. On a panic:
  - The project's `status.json` reads `state` `error`, `reason` `panic:<class>; restarting in <s> s` (class as in the incident catalog `stdio|unwrap|index|other`); the project log keeps the panic text; an incident report follows § 4.
  - Child handles live in a table owned by the project thread, outside the panicking state; the rebuilt state re-attaches them, so live children keep running and their leases stay listed in the heartbeat.
  - Backoff 1 s, 10 s, 60 s. Three panics in 10 minutes quarantine the project: a reduced loop only reaps, beats and writes `status.json`, reason `quarantined: panic:<class>`; no claim, reviewer or resume. `service restart` or an update clears it. If the reduced loop also panics, the thread stops; other hosts report its executions unknown and wait for attributable termination evidence (§ 3).
  - Other projects never pause: they share no lock with the panicking pass except host state (budget, admission, profile writer). Poisoned admission state holds launches until rebuilt from verified child handles; never grant from potentially inconsistent counters. Budget/profile errors report unknown without blocking other project loops.
- **Process fault.** An abort, stack overflow, OOM kill or a panic outside a project thread stops every project. The launcher restarts the unit and reconciles original native identities (§ 4 restart); unresolved executions retain their fence and occupied capacity. Neither restart nor a missing handle implies `gone` or release. This blast radius is the cost of option B; today it is one project.
- **Config.** An unreadable `taskq.json` holds that project only (reason in its `status.json`); a malformed `<install>/projects` keeps the last good list and reads unknown in `service status`.

## Defects this removes

| Class | Today | After |
|---|---|---|
| #1217 profile records lost | Up to six service processes plus CLIs append to `taskq.log` under a nonblocking lock; a collision drops the record and blames the log size. | One in-process writer for all service records; contention is service versus CLI only. #1217's bounded wait still applies to CLI writers. |
| #1084 board budget | Each service keeps its own request counts, its own rate-limit pause and its own hourly record; one service pausing on a 403 does not stop the others on the same token. | One counter and one pause per board host token; one hourly record per host; one update check per interval instead of one per project. |
| #933 duplicate service | A second unit for one root or host name (second Windows service, WSL plus native, a unit left after reinstall) serves the same tasks: false `gone`, two workers on one task. | One unit name per host, a host pid check and the per-root check; no per-project unit to leave behind. |
| Admission skew | `admission.json` across processes refuses grants while any service is older than the minimum version or its entry is stale. | In-process admission; migration holds new admission until legacy exit. |
| Mixed versions per host | Projects re-exec at different times; one host runs several versions at once. | One version per host. |
| Tray races | Any of N services may start the tray. | One spawner. |

## Implementation tasks

Each task is one reviewable diff. Lines `Depends on` go into the task body as written.

1. **coreq: host unit and rollback floor** (repository coreq). `service::host_unit(t)` for launchd, Task Scheduler and systemd; `service::legacy_unit(t, project)` for removal; launcher counts unhealthy starts of the host unit and honors a rollback floor. Tag a coreq release.
2. **Project threads and host policy in one process.** Migrate the six capacity copies and validate host.json once per pass; apply the § 3, § 4 and § 9 diff blocks. `service run` without a root reads `<install>/projects`, one thread per project with `catch_unwind`, child table outside the pass state, backoff and quarantine; shared budget, in-process admission, `<install>/service` pid check, `<install>/poke`. Chain checks: two fake projects, one panics every pass while the other admits and its child survives; concurrent project requests cannot exceed host limits; a project cap only lowers; with host runtime limit 2, admission.max 2 and project cap 2, one running attempt with its retained reservation occupies one slot, a second pending attempt fills the other, its transition to running keeps occupancy at two, and a third is refused until one exits; equal issue/attempt numbers in different projects remain distinct; conflicting migration copies leave every original unchanged.
   `Depends on taskqorg/coreq#<task 1>`
3. **Install, uninstall, stop, restart and status.** Apply the § 5 and § 8 step 5 diff blocks. Project list edits under `<install>/projects.lock`, host unit write and load, `uninstall --host`, host-wide `stop`/`restart`, per-project `status` blocks, host-level `capacity` from any folder and project caps, tray restart/stop labels. Keep permission/resource-grant authority unchanged (R9).
   `Depends on #<task 2>`
4. **Legacy mode and migration.** Apply the § 8 shared core and step 3 diff blocks. Legacy-mode detection, `handoff` reason, own unit removal with readback, host-wide admission hold until all legacy entries have verified exit, rollback floor. Chain check: legacy root with a live child admits nothing, keeps the child, then removes its unit and the host thread takes over.
   `Depends on #<task 3>`
5. **Release and host qualification.** Release N; on mac, Windows and FRA record per host: legacy units gone, every root served by the host pid, no `gone` from migration, rollback drills within the qualified execution-protocol floor, plus refusal below that floor, on macOS, Windows and Linux using isolated units and children; verify drain, exit, unit removal order, restored config and update hold before any live rollout.
   `Depends on #<task 4>`

## Open

- Host topology and drain-based handoff are accepted design decisions; implementation and installed qualification remain subject to H-A1–H-A5.
- The macOS `launchctl bootout` child-survival probe decides whether macOS may adopt instead of drain later; not needed for N.

## Contract detail

Host runtime detail for taskq.md sections 3, 4 and 9. Implementation and installed acceptance are tracked separately by H-A1–H-A5.

- Data ([specq data](https://github.com/taskqorg/specq/blob/main/engineering/data.md#files)): telemetry uses [SQLite/WAL bounds and migration](standalone-telemetry.md#sqlite-implementation-and-transition), pruned each host/project pass under the profile lock. Text-log bounds live in `src/data.rs`; legacy `taskq.log` is checked each host pass (each project pass on legacy services), under `taskq-profile.lock`, and keeps its newest half past the collection threshold of 4 MiB − 512 bytes. A busy lock or failed read/write leaves collection unknown and retries next pass. Other files are pruned hourly: `<project>.log` past 4 MiB and `.taskq/cli-errors.log` past 256 KiB keep their newest half; attempt logs the newest 50 plus live attempts; `.taskq/inputs/*.txt` the newest 50 plus files locked by live Claude turns; `incidents/` the newest 50; `requests/comments` past 1000 lines the newest half plus comments still eligible; `events`, `requests/` and `requests.log` follow [taskq.md section 4](../../taskq.md#4-service). A corrupt local state file reads unknown, admits or adopts nothing and deletes no release ([menu](menu.md#contract-detail)).

## Command-surface acceptance

Contract detail for R3/R8/R9 and sections3/4/5/8/9. These cases complete the accepted host transition; they are not evidence that it is implemented. Reuse the existing model/adapter tests and native qualification matrix.

| Case | Required observation |
|---|---|
| H-A1 Service entrypoints | Invocation from the install or an explicitly registered project selects the same host owner; no cwd-dependent second scheduler. Unregistered roots cannot register themselves through run. |
| H-A2 Policy surfaces | CLI, tray, operator and authorized remote resource edits use one canonical host-policy operation; actor/scope/grant/revision validation remains at each trust boundary. Equivalent accepted changes produce the same saved policy and effective service readback. Unsupported target/schema is refused before effects. |
| H-A3 Failure and restriction | Missing/corrupt/newer host policy holds new admission without fallback to project settings. Owned work remains supervised; restrictive commands retain their explicit receipt-failure policy. |
| H-A4 Shared occupancy | Two projects, pending claims, running children, reviewer/resume and unknown native execution cannot double-spend capacity; one pending-to-running transition adds no slot. |
| H-A5 Lifecycle and retirement | Install/restart/stop/uninstall/update preserve the declared scope and owned work; legacy unit/child absence is verified before retirement. Keep original configuration and qualified rollback; an incompatible old authority cannot restart against unresolved work. |

Update cleanup preserves the host policy and its pending replacement, registry locks/prepares, native recovery directory, migration originals, host control signals and rollback floor. An unreadable registry or a registered root without a readable version makes release ownership unknown; preserve releases until readback resolves it. Cleanup never supplies missing termination or retirement evidence.

The transition owner is the host service. Project loops and status are consumers/projections, not fallback authorities. Legacy project units, capacity_hosts/mode and admission.json expiry may remain only for identified migration/compatibility readers; after cutover no supported writer/admission path may use them. Retire them only after H-A1–H-A5 and the existing installed acceptance pass. Exact receipts and native release evidence remain required.

## Shared core mapping

Contract detail for taskq.md section8.

**Shared core.** Release, update, launcher and rollback, service units, the `status.json` common keys and the GitLab board token follow [coreq.md](https://github.com/taskqorg/coreq/blob/8a99727d3dde12efa0e506052c804528f9122749/coreq.md) (crate `coreq`, immutable revision in Cargo.toml/Cargo.lock; JS packaging uses the same revision) as `Tool::new("taskq", "TaskQ", "taskqorg/taskq", VERSION)`: install `~/.local/share/taskq` (Windows `%LOCALAPPDATA%\taskq`), files `taskq-<ver>-<os>-<arch>`, host units `com.taskq.service`, `TaskQ`, `taskq.service`; per-project units are legacy migration inputs only.
