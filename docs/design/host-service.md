# One service process per host

Status: proposal for owner approval (#1220). The owner chose option B: one OS unit per host runs every registered project in one `taskq service run` process. This page holds the proposed `taskq.md` diff, the migration and rollback plan, the isolation model and the implementation split. Nothing here is shipped behavior; `taskq.md` stays authoritative until the implementation tasks land the diff together with the code (R8, R13).

## Decision

- One unit per host: `com.taskq.service` (launchd), `TaskQ` (Task Scheduler), `taskq.service` (systemd user). It runs `<install>/taskq service run` with `<install>` as working directory.
- The process serves every root listed in `<install>/projects`, one project thread each.
- Host-wide parts run once, in process: admission, the GitHub request budget and rate-limit pause, the update check and re-exec, the tray spawner, the profile appender, hourly request records and data pruning of `<install>` files.
- Per-project parts stay per project: `taskq.json`, `<root>/.taskq/` (`status.json`, `events`, `focus`, `focus.paused`, `kids`, `poke`, `update`, `requests/`, `requests.log`, `incidents/`, attempt logs, `cli-errors.log`, `pm-cursor`, specq cache), worktrees, the project log `<install>/<project>.log` and one heartbeat issue per board.
- `service install` and `service uninstall` in a project edit `<install>/projects` and poke the process; they install or remove the host unit only when it is missing or no project remains.

## Proposed taskq.md diff

The diff is line-neutral: each block replaces the quoted line in place. `taskq.md` has 297 lines today and must stay within 300 (R8). Section anchors are those of `main` at 27aa614.

### § 3 Host admission (line 144)

Replace:

> - **Host admission.** `<install>/admission.json` under `<install>/admission.lock` admits every Auto launch after the load gate; a grant needs every fresh service of `<install>/projects` at `4.2.0-dev` or newer, held reservations under `admission.max` and the 30 s launch spacing. An unreadable file refuses; Fixed services are never refused.

With:

> - **Host admission.** The host service (§ 4) admits every Auto launch of all its projects after the load gate, in process: held reservations under `admission.max` and the 30 s launch spacing. While a legacy per-project service of the host is fresh (§ 8 migration), the host service also keeps its entry in `<install>/admission.json` under `<install>/admission.lock` and a grant needs every fresh entry at `4.2.0-dev` or newer; an unreadable file refuses. Fixed projects are never refused.

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

> § 4, § 8; `install`/`uninstall [--host]`: register or drop this project in `<install>/projects` and poke the host service (`--host`: remove only the host unit, § 8 rollback); `permissions`: § 8 Access read, or one grant write with readback; `hold`/`release`/`recover`: § 3, § 4 emergency; `stop`/`restart`: the host service, every project (`stop` until the next logon, the unit stays); `status`: the process, then one block per project; `list`: this host's projects and their focus; `tray --snapshot DIR [--route R,..]`: § 4; `tray --icon FILE`: [custom icon](docs/design/custom-icon.md#contract-detail)

### § 8 Shared core (line 266, unit names)

Replace:

> units `com.taskq.<project>`, `TaskQ-<project>`, `taskq-<project>.service`.

With:

> one host unit `com.taskq.service`, `TaskQ`, `taskq.service` ([host service](docs/design/host-service.md#migration) removes the legacy `com.taskq.<project>`, `TaskQ-<project>`, `taskq-<project>.service`).

### § 8 step 3 (line 269)

Replace:

> 3. At step 7 an installed service older than `current` re-execs through the launcher; live CLI children pass over through `<root>/.taskq/kids`; an active Claude, codex-app or operator turn defers it. Detail: [runtime qualification](docs/runtime-qualification.md#contract-detail).

With:

> 3. At step 7 an installed service older than `current` re-execs through the launcher, once for all projects; live CLI children pass over through each `<root>/.taskq/kids`; an active Claude, codex-app or operator turn of any project defers it. Detail: [runtime qualification](docs/runtime-qualification.md#contract-detail).

### § 8 step 5 (line 271)

Replace:

> 5. `service install` installs its binary as a new file, never over an existing release binary (macOS kills rewritten binaries). While the project's `status.json` lists live children (or is unreadable) it writes `<root>/.taskq/restart` instead of reloading. Windows stops end the process tree (`taskkill /T`); run Windows setup outside an MSIX app. The coreq launcher counts unhealthy starts per project.

With:

> 5. `service install` installs its binary as a new file, never over an existing release binary (macOS kills rewritten binaries), adds the root to `<install>/projects` under `<install>/projects.lock`, writes and loads the host unit when missing or changed, and pokes `<install>/poke`; `installed` only after the project's `status.json` names the service pid. While any project's `status.json` lists live children (or is unreadable) a changed unit writes `<install>/restart` instead of reloading. `uninstall` drops the root, admits nothing more there and keeps its children until they exit (R11); with no project left it removes the unit. Windows stops end the process tree (`taskkill /T`); run Windows setup outside an MSIX app. The coreq launcher counts unhealthy starts of the host unit and never rolls back below the first host-service version.

### § 9 timers (line 286)

Replace:

> - `ttl`, `poll`, `update_every`, `rollback_after`: seconds, defaults 300, 60, 60, 600.

With:

> - `ttl`, `poll`, `update_every`, `rollback_after`: seconds, defaults 300, 60, 60, 600; `ttl` and `poll` per project, the host service uses the smallest `update_every` and the largest `rollback_after` of its projects.

## Migration

The first host-service release is N. Legacy services run N-1 or older. Migration needs no owner step on a host that updates automatically; it runs once per host, idempotent, in any order across hosts.

1. **Update.** A legacy service applies N through the existing update (§ 8 step 2) and re-execs at step 7 (§ 8 step 3). Its live CLI children pass over through `<root>/.taskq/kids` as today; the re-exec keeps the pid, so no child changes parent.
2. **Legacy mode.** N started by a per-project unit (working directory is a project root, unit name `com.taskq.<project>`, `TaskQ-<project>` or `taskq-<project>.service`) runs that project in legacy mode:
   - It adds the root to `<install>/projects` if missing, then writes and loads the host unit if missing.
   - It admits nothing new: capacity reason `handoff`. It keeps reaping, resuming answers of its own live attempts, beating and writing `status.json`, so no lease expires.
   - When it has no live child, it removes its own unit (`launchctl bootout` and plist delete; `schtasks /Delete`; `systemctl --user disable` and unit file delete) and exits 0. A unit removal it cannot read back stays in the project log and in `status.json` `reason`; the process then keeps running in legacy mode and retries every pass.
3. **Host service.** The host unit starts N in host mode. Each project thread waits while that project's fresh `status.json` names another live pid (§ 4 one service), so a legacy or N-1 service and the host service never serve one root together. When the legacy process exits, the thread starts, rebuilds from the board and admits by focus.
4. **N-1 on the same host.** An N-1 legacy service that has not updated yet keeps its unit and its root; the host thread for that root waits. Host admission keeps the `admission.json` protocol while any legacy entry is fresh (§ 3 diff).
5. **Readback.** `service status` shows `Service: running (host, pid <p>)` and one block per project. Migration is complete on a host when no legacy unit file exists and every registered root's `status.json` names the host service pid. The release qualification task records this per host.

Why drain instead of cross-process adoption: removing a legacy unit kills its process tree on systemd (`KillMode=control-group`) and may on launchd; Windows has no adoption today. Children therefore stay with the process that started them until they exit; no attempt goes `gone` and no worker runs twice. The cost is that a project admits nothing new during handoff, at most the length of its longest live attempt. Cross-process adoption on macOS is an upgrade path only after a probe shows a child in its own process group survives `launchctl bootout` (R12).

### Rollback

- **Within host-service versions** (N+k to N): the coreq launcher rolls back after three unhealthy starts of the host unit, as today per project, and never below N (rollback floor, like § 8 Access).
- **Back to per-project units** (owner decision only):
  1. `taskq service uninstall --host`: removes the host unit, keeps `<install>/projects`. Live children of the host service keep running until their attempts end; with none live the process exits at once.
  2. In each root listed in `<install>/projects`, run the older release's `service install` from `<install>/releases/<old>/`. The project list format is unchanged, so older releases read it.
  3. Pin `current` to the older release, so the host unit is not recreated by an N update. Readback: each root's `status.json` names its own per-project pid.
- An N-1 binary never starts as a host service: the host unit runs `<install>/taskq service run` without a project root, which N-1 refuses at config load. The rollback floor keeps the launcher from picking N-1.

## Isolation

- **Project panic.** Each project runs in its own thread. A pass runs inside `catch_unwind`. On a panic:
  - The project's `status.json` reads `state` `error`, `reason` `panic:<class>; restarting in <s> s` (class as in the incident catalog `stdio|unwrap|index|other`); the project log keeps the panic text; an incident report follows § 4.
  - Child handles live in a table owned by the project thread, outside the panicking state; the rebuilt state re-attaches them, so live children keep running and their leases stay listed in the heartbeat.
  - Backoff 1 s, 10 s, 60 s. Three panics in 10 minutes quarantine the project: a reduced loop only reaps, beats and writes `status.json`, reason `quarantined: panic:<class>`; no claim, reviewer or resume. `service restart` or an update clears it. If the reduced loop also panics, the thread stops; its attempts expire through § 3 on other hosts.
  - Other projects never pause: they share no lock with the panicking pass except poison-tolerant host state (budget, admission, profile writer), which recovers the inner value on a poisoned lock.
- **Process fault.** An abort, stack overflow, OOM kill or a panic outside a project thread stops every project. The launcher restarts the unit; former children are not adopted and their attempts go `gone` (§ 4 restart). This blast radius is the cost of option B; today it is one project.
- **Config.** An unreadable `taskq.json` holds that project only (reason in its `status.json`); a malformed `<install>/projects` keeps the last good list and reads unknown in `service status`.

## Defects this removes

| Class | Today | After |
|---|---|---|
| #1217 profile records lost | Up to six service processes plus CLIs append to `taskq.log` under a nonblocking lock; a collision drops the record and blames the log size. | One in-process writer for all service records; contention is service versus CLI only. #1217's bounded wait still applies to CLI writers. |
| #1084 board budget | Each service keeps its own request counts, its own rate-limit pause and its own hourly record; one service pausing on a 403 does not stop the others on the same token. | One counter and one pause per board host token; one hourly record per host; one update check per interval instead of one per project. |
| #933 duplicate service | A second unit for one root or host name (second Windows service, WSL plus native, a unit left after reinstall) serves the same tasks: false `gone`, two workers on one task. | One unit name per host, a host pid check and the per-root check; no per-project unit to leave behind. |
| Admission skew | `admission.json` across processes refuses grants while any service is older than the minimum version or its entry is stale. | In-process admission; the file only during migration. |
| Mixed versions per host | Projects re-exec at different times; one host runs several versions at once. | One version per host. |
| Tray races | Any of N services may start the tray. | One spawner. |

## Implementation tasks

Each task is one reviewable diff. Lines `Depends on` go into the task body as written.

1. **coreq: host unit and rollback floor** (repository coreq). `service::host_unit(t)` for launchd, Task Scheduler and systemd; `service::legacy_unit(t, project)` for removal; launcher counts unhealthy starts of the host unit and honors a rollback floor. Tag a coreq release.
2. **Project threads in one process.** Apply the § 3, § 4 and § 9 diff blocks. `service run` without a root reads `<install>/projects`, one thread per project with `catch_unwind`, child table outside the pass state, backoff and quarantine; shared budget, in-process admission, `<install>/service` pid check, `<install>/poke`. Chain check: two fake projects, one panics every pass; the other admits and its child survives.
   `Depends on taskqorg/coreq#<task 1>`
3. **Install, uninstall, stop, restart and status.** Apply the § 5 and § 8 step 5 diff blocks. Project list edits under `<install>/projects.lock`, host unit write and load, `uninstall --host`, host-wide `stop`/`restart`, per-project `status` blocks, tray restart/stop labels.
   `Depends on #<task 2>`
4. **Legacy mode and migration.** Apply the § 8 shared core and step 3 diff blocks. Legacy-mode detection, `handoff` reason, own unit removal with readback, `admission.json` only while legacy entries are fresh, rollback floor. Chain check: legacy root with a live child admits nothing, keeps the child, then removes its unit and the host thread takes over.
   `Depends on #<task 3>`
5. **Release and host qualification.** Release N; on mac, Windows and FRA record per host: legacy units gone, every root served by the host pid, no `gone` from migration, a rollback drill to N-1 per the procedure above on one host.
   `Depends on #<task 4>`

## Open

- Owner approval of this diff and of the drain-based handoff (no admission during handoff).
- The macOS `launchctl bootout` child-survival probe decides whether macOS may adopt instead of drain later; not needed for N.
