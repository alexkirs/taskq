# One service process per host

Status: proposal for owner approval (#1220). The owner chose option B: one OS unit per host runs every registered project in one `taskq service run` process. This page holds the proposed `taskq.md` diff, the migration and rollback plan, the isolation model and the implementation split. Nothing here is shipped behavior; `taskq.md` stays authoritative until the implementation tasks land the diff together with the code (R8, R13).

## Decision

- One unit per host: `com.taskq.service` (launchd), `TaskQ` (Task Scheduler), `taskq.service` (systemd user). It runs `<install>/taskq service run` with `<install>` as working directory.
- The process serves every root listed in `<install>/projects`, one project thread each.
- Host-wide parts run once, in process: admission, the GitHub request budget and rate-limit pause, the update check and re-exec, the tray spawner, the profile appender, hourly request records and bounded log pruning.
- Per-project parts stay per project: `taskq.json`, `<root>/.taskq/` (`status.json`, `events`, `focus`, `focus.paused`, `kids`, `poke`, `update`, `requests/`, `requests.log`, `incidents/`, attempt logs, `cli-errors.log`, `pm-cursor`, specq cache), worktrees, the project log `<install>/<project>.log` and one heartbeat issue per board.
- `service install` and `service uninstall` in a project edit `<install>/projects` and poke the process; they install or remove the host unit only when it is missing or no project remains.

## Proposed taskq.md diff

These are proposed replacements, not active rules. Apply by section and quoted text, not line number; retain the 300-line/60 KiB limit (R8). The proposal amends R3 (service ownership), R9 (host configuration ownership) and R10 (explicit registration), with details in sections 3-5 and 8-9. No board fold or golden trace changes are proposed.

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

> - One service per root and host name: `service run` waits while a `status.json` under 60 s old names another live pid; a pass that finds this host's heartbeat under `ttl` naming another `service <id>` writes nothing and fails with `another service <id> runs as host <name>`. Restart: rebuild from the board; former children are not adopted (except the § 8 re-exec), their attempts are `gone`. `service restart` writes `<root>/.taskq/restart`: the next pass re-execs (§ 8 step 3).

With:

> - One service per host: `service run` waits while `<install>/service` (the service's pid) is under 60 s old and names another live pid; a project thread waits while its `status.json` under 60 s old names another live pid; a pass that finds this host's heartbeat under `ttl` naming another `service <id>` writes nothing and fails with `another service <id> runs as host <name>`. Restart: rebuild from the board; former children are not adopted (except the § 8 re-exec), their attempts are `gone`. `service restart` writes `<install>/restart` (a project's `<root>/.taskq/restart` counts the same): the process re-execs (§ 8 step 3).

### § 4 Tray (line 168, first sentence)

Replace:

> - Tray (macOS, Windows; [menu](docs/design/menu.md#contract-detail) M1–M14): one per host, started by any installed service when `<install>/tray` (its pid) is older than 30 s.

With:

> - Tray (macOS, Windows; [menu](docs/design/menu.md#contract-detail) M1–M14): one per host, started by the installed service when `<install>/tray` (its pid) is older than 30 s; restart and stop act on the host service and name every project first; pause, resume and uninstall stay per project.

### § 4 Profiling (line 177, first sentence)

Replace:

> Records hold fixed codes and numbers only in `<install>/taskq.log` under its § 4 Data bound, appended under the nonblocking `taskq-profile.lock`;

With:

> Records hold fixed codes and numbers only in `<install>/taskq.log` under its § 4 Data bound; the service appends through one in-process writer, every writer under `taskq-profile.lock`;

### § 5 service row (line 196)

Replace the `Does` cell:

> § 4, § 8; `permissions`: § 8 Access read, or one grant write with readback; `hold`/`release`/`recover`: § 3, § 4 emergency; `stop`: until the next logon, the unit stays; `list`: this host's projects and their focus; `tray --snapshot DIR [--route R,..]`: § 4; `tray --icon FILE`: [custom icon](docs/design/custom-icon.md#contract-detail)

With:

> § 4, § 8; `install`/`uninstall [--host]`: register or drop this project in `<install>/projects` and poke the host service (`--host`: drain, verify exit, then remove only the host unit, § 8 rollback); `permissions`: § 8 Access read, or one grant write with readback; `hold`/`release`/`recover`: § 3, § 4 emergency; `stop`/`restart`: the host service, every project (`stop` until the next logon, the unit stays); `status`: the process, then one block per project; `list`: this host's projects and their focus; `tray --snapshot DIR [--route R,..]`: § 4; `tray --icon FILE`: [custom icon](docs/design/custom-icon.md#contract-detail)

### § 8 Shared core (line 266, unit names)

Replace:

> units `com.taskq.<project>`, `TaskQ-<project>`, `taskq-<project>.service`.

With:

> one host unit `com.taskq.service`, `TaskQ`, `taskq.service` ([host service](docs/design/host-service.md#migration) removes the legacy `com.taskq.<project>`, `TaskQ-<project>`, `taskq-<project>.service`).

### § 8 step 3 (line 269)

Replace:

> 3. At step 7 an installed service older than `current` re-execs through the launcher; live CLI children pass over through `<root>/.taskq/kids`; an active Claude, codex-app or operator turn defers it. Detail: [runtime qualification](docs/runtime-qualification.md#contract-detail).

With:

> 3. At step 7 an installed service older than `current` re-execs through the launcher, once for all projects; on Unix, live CLI children pass over through each `<root>/.taskq/kids` with the same pid; an active Claude, codex-app or operator turn of any project defers it. On Windows, all projects must have no live child or active turn before exit 75; the unit restarts with a new pid, with no child adoption. Detail: [runtime qualification](docs/runtime-qualification.md#contract-detail).

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

> `taskq service capacity [--max N|none] [--claude N] [--codex N] [--codex-app N] [--load-gate on|off] [--cpu-max P] [--memory-min P] [--project P]`

> Without a project root, read or edit this host's host.json; `--project P` selects exactly one registered project's optional runtime limits only. Reject admission/load-gate edits with --project; reject a cap that raises the host ceiling. Show host limits/use and project cap separately, write atomically on explicit owner instruction (R9), poke and report applied only after daemon readback. Remove --mode and remote --host edits; host identity is local. Preserve existing resource-grant checks and revision/receipt guards; no new authority for workers.

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

After approval, save byte-for-byte originals and the chosen mapping in a bounded migration backup under install; preserve unknown keys and original file ownership. Stage host.json and project edits, validate all, then atomically replace each file with restart-safe progress/readback. Keep every affected service admission-held until the whole set reads back; interruption resumes or restores originals, never enables partial policy. Remove only migrated keys, including capacity_hosts, admission/mode, load_gate, hosts, has, codex and permission_mode; keep optional lowering limits and unrelated keys. Preserve per-host backups: project files may contain remote-host copies needed by another host's migration. Re-run is idempotent. Do not commit local paths or secrets. Rollback restores these originals before legacy services start.

## Migration

Owner approval of this drain handoff is required before implementation (R13); it replaces the no-drain promise only for the topology migration and its reverse. The first host-service release is N. Legacy services run N-1 or older. After approval of the design and resolved configuration mapping, migration needs no additional owner step on an automatically updating host; it runs once per host, idempotent, in any order across hosts.

1. **Update.** A legacy service applies N through the existing update (§ 8 step 2) and re-execs at step 7 (§ 8 step 3). On Unix, eligible live CLI children pass over through `<root>/.taskq/kids` with the same pid; active Claude, codex-app and operator turns defer it. Windows waits for every child and active turn to finish, exits 75, and the legacy unit starts N with a new pid; Windows does not adopt children.
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
  - Backoff 1 s, 10 s, 60 s. Three panics in 10 minutes quarantine the project: a reduced loop only reaps, beats and writes `status.json`, reason `quarantined: panic:<class>`; no claim, reviewer or resume. `service restart` or an update clears it. If the reduced loop also panics, the thread stops; its attempts expire through § 3 on other hosts.
  - Other projects never pause: they share no lock with the panicking pass except host state (budget, admission, profile writer). Poisoned admission state holds launches until rebuilt from verified child handles; never grant from potentially inconsistent counters. Budget/profile errors report unknown without blocking other project loops.
- **Process fault.** An abort, stack overflow, OOM kill or a panic outside a project thread stops every project. The launcher restarts the unit; former children are not adopted and their attempts go `gone` (§ 4 restart). This blast radius is the cost of option B; today it is one project.
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
5. **Release and host qualification.** Release N; on mac, Windows and FRA record per host: legacy units gone, every root served by the host pid, no `gone` from migration, rollback drills to N-1 on macOS, Windows and Linux using isolated units and children; verify drain, exit, unit removal order, restored config and update hold before any live rollout.
   `Depends on #<task 4>`

## Open

- Owner approval of this diff and of the drain-based handoff (no admission during handoff).
- The macOS `launchctl bootout` child-survival probe decides whether macOS may adopt instead of drain later; not needed for N.

## Contract detail

Host runtime detail for taskq.md sections 3, 4 and 9. Legacy-unit handoff remains a separate implementation target.

- Data ([specq data](https://github.com/taskqorg/specq/blob/main/engineering/data.md#files)): bounds in `src/data.rs`; `taskq.log` is checked each host pass (each project pass on legacy services), under `taskq-profile.lock`, and keeps its newest half past the collection threshold of 4 MiB − 512 bytes. A busy lock or failed read/write leaves collection unknown and retries next pass. Other files are pruned hourly: `<project>.log` past 4 MiB and `.taskq/cli-errors.log` past 256 KiB keep their newest half; attempt logs the newest 50 plus live attempts; `.taskq/inputs/*.txt` the newest 50 plus files locked by live Claude turns; `incidents/` the newest 50; `requests/comments` past 1000 lines the newest half plus comments still eligible; `events`, `requests/` and `requests.log` follow [taskq.md section 4](../../taskq.md#4-service). A corrupt local state file reads unknown, admits or adopts nothing and deletes no release ([menu](menu.md#contract-detail)).

### Host lifecycle and capacity (§ 5, § 8.5)

Windows stops the exact idle process tree with `taskkill /T`; setup runs outside MSIX app containers.

Install and uninstall serialize under `service.lock`; root registration uses `projects.lock` and atomic replacement, then pokes the daemon. An unreadable registry is unknown and stays unchanged. Install loads the host unit only if missing, changed or stopped; live or unknown work defers a changed unit via `restart` containing `unit`. An OS-owned helper outside the service process tree drains it, verifies exit, unloads and loads the written definition, then verifies native registration and fresh host ownership before clearing the request. Failures retain the request and drain for retry; repeated install retries it. A plain restart request still re-execs. `installed` requires fresh project status naming the live host pid. The topology rollback floor is recorded only after this ownership readback; legacy-unit removal remains the migration task.

`drain` is a host lifecycle request, not task state. The daemon stops new admission, keeps existing claims, resumes, children and operators, and retains draining project rows until their leases and processes end. `service.json` is a rebuilt host snapshot: process pid, policy readback, runtime occupancy, registered and draining roots, drain/idle flags and unknown reason. Its rows are bounded by registered or draining roots; it is replaced each host pass. A missing, stale, corrupt or different-pid snapshot is unknown. Stop and uninstall keep the unit until a fresh drain acknowledgement proves zero children, pending claims and active turns; then stop the unit, verify the recorded process exited, and only then unload/delete. A 30-second wait that expires leaves the drain active and reports pending; retry the same command. `uninstall --host` preserves `projects`; ordinary uninstall removes only its root and removes the unit only for the last root. Successful stop clears the drain, keeping the unit for next logon.

Capacity edits serialize under `host-policy.lock`, retain unrelated keys and reject concurrent edits. Host edits use `host.json`; `--project` names exactly one registered root or unambiguous project name and edits only its runtime caps. A project cap above the host ceiling is refused; a host edit that makes existing project caps invalid is also refused. Saved configuration and fresh daemon policy/project-cap readback are distinct. Resource and permission grants keep their authority and revision checks. The tray names every registered project before a host restart/stop; pause/resume/uninstall remain per project.

macOS install preserves `TaskQ Local Signing`, falling back to ad-hoc signing ([worker permissions](worker-permissions.md#contract-detail)). The first verified host installation pins its running release; later installs retain this topology floor. Native host-unit ownership and legacy cutover remain release qualification gates. Project-scoped resource operations can lower project caps; they cannot acquire host-wide gate/admission authority through this topology change. Host policy edits use the local owner CLI.

The update sweep preserves host.json, host-policy-migration, host-policy.lock, projects.lock, service.lock, service, service.json, drain, poke, restart, rollback-floor and TaskQ.xml. A malformed host pid path stays for inspection (R1, R12); migration must resolve a legacy directory at that path before starting the host process.
