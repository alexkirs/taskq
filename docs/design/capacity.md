# Capacity: limits, controls and load gating

Current host-policy authority is [taskq.md sections3/9](../../taskq.md#3-attempts-lease-capacity) and [host-service acceptance](host-service.md#command-surface-acceptance). The per-project overrides, Fixed mode, admission.json expiry and future patch below describe the superseded design; they are migration/compatibility reference, not current admission authority. Native sampling descriptions remain reference; current defaults and ownership come from the root contract.

## Legacy design reference

Design for [#783](https://github.com/alexkirs/taskq/issues/783), implemented in [#792](https://github.com/alexkirs/taskq/issues/792).
The [contract](../../taskq.md#3-attempts-lease-capacity) (§ 3, § 4, § 9) is the source of truth; where this note differs, taskq.md wins.
Follow [menu M2–M5](menu.md) and [shared density D1–D14](https://github.com/taskqorg/specq/blob/main/design/operator-ui.md); all product text below is English.

## User model

- A slot is one live service child, worker or reviewer, of a runtime on a host **for this project**. A session or board task is not a used slot.
- Keep the existing `limits` defaults; each project's service counts only its own children. Two projects with Codex limit 4 can together run 8 on one machine. This is not a shared machine pool.
- The project has no aggregate cap; its ceiling is the sum of its hosts' effective limits. Offline/unobserved hosts do not imply zero usage.
- `used/limit` is actual children/effective ceiling: `Codex 3/4`. Zero disables new launches of that runtime; missing runtime limits remain disabled.
- Reducing 4 to 2 while 3 children run shows `3/2` until they exit. Never kill, suspend, move or replace an attempt to meet a limit (R11).
- A free slot is permission to start, not a promise. Focus, routing, dependencies and load gating must also allow it. Reviewers share the worker limit.
- Load gating does not change the number configured. `Codex 2/4 · Waiting: CPU` means two occupied slots and no new starts until recovery.
- Running children continue. A resumed session that needs a new process is a new launch: it consumes capacity and obeys the gate. Keep undelivered inbox entries pending; a held resume is not `gone` and is not marked delivered.

## Menu and CLI

In the project's existing scope/service area, add one `Capacity` disclosure, scoped to the named project and local host.
Show one row per runtime, name left and `used/limit` right; disclose `Load gate: Off`, `Ready`, `Waiting: CPU`, `Waiting: memory`, or `Unknown: <reason>` beside the controls.
Keep both reasons if both signals block; show focus/pause separately. A load hold is neither manual pause nor a failed task. Keep the existing icon semantics; do not repurpose cadence as CPU history.
No new home cards or duplicated task counts. Use the existing 16 px default, text-size setting, wrapping, keyboard focus, accessible names and touch targets; color alone cannot carry the gate state.

`Edit limits` opens inline integer fields and a `Load gate` switch with CPU ceiling and available-memory floor fields.
`Apply` first shows project, host, old → new values and `Running tasks continue`; the owner's confirmation is the write. Cancel writes nothing.
No AI session or terminal is needed. Agents may propose values, never apply them without the owner's explicit instruction; this is the proposed capacity extension of R9, not an existing authentication boundary.

Proposed CLI, run in the project's root:

```text
taskq service capacity [--host H]
taskq service capacity --host H --claude N --codex N
taskq service capacity --host H --load-gate on --cpu-max P --memory-min P
taskq service capacity --host H --load-gate off
```

Read defaults to the resolved local host; writes require the explicit resolved host name. Any subset of write fields changes only those fields; one command is one atomic change.
An explicit owner-issued write command supplies confirmation; the menu uses the same validation/persistence operation after its confirmation. No generic `--yes` or privileged remote service control.
Validate runtime limits as integers 0..u32::MAX; CPU ceiling 10..100 and memory floor 1..95 as integer percentages. Reject invalid combinations or unknown host names before any write.
The target host must be the local resolved host or a name in `hosts`; `TASKQ_HOST` is resolved as in §9. Never infer a host from an unrelated folder.
Preserve unknown JSON keys and untouched host/runtime entries; detect concurrent edits against the displayed/read config bytes, refuse conflicts, and replace atomically. Do not overwrite a newer owner edit.
Read back the saved values, poke the local service, then distinguish `Saved; waiting for service` from `Applied`.
The service reloads capacity before each admission decision without restarting children. Invalid edited config holds new launches and reports the error; it never silently falls back to more capacity.
`Applied` requires fresh service readback matching the requested host, effective limits and gate settings. An absent/stopped/old service can acknowledge only saved values.
Editing another host's entry saves a configuration proposal in this checkout; it does not prove deployment or application on that host. Git/config synchronization remains explicit, outside this command.

## Persistence and status

Keep top-level `limits` as existing per-project defaults on every host. Add optional `capacity_hosts`, keyed by resolved host name, with partial runtime `limits` overrides and an optional `load_gate` object.
Top-level `load_gate` supplies the per-project default; a host's `load_gate` replaces that object, then omitted fields use the fixed defaults below. No additive limit or nested partial gate merge.
Menu and CLI write the selected host entry, never change another host's ceiling by editing the defaults.

```json
{
  "limits": {"claude": 4, "codex": 4},
  "capacity_hosts": {
    "win": {
      "limits": {"codex": 2},
      "load_gate": {"enabled": true, "cpu_max": 85, "memory_min": 15}
    }
  }
}
```

Here Windows inherits Claude 4, uses Codex 2, and opts into gating; other hosts retain 4/4 and gating off.
Absent new keys preserve existing valid limits and admission behavior. Do not rewrite existing configs or enable the gate during upgrade.
Old binaries ignore these new keys: enable host overrides/gating only after the target host's compatible version is verified. A rollback to an older binary would lose enforcement; restore compatible enforcement before admitting work, or pause that host explicitly with owner approval.

Add one `capacity` object to the existing local `status.json`:

| Field | Meaning |
|---|---|
| `limits`, `used` | Effective runtime ceilings and actual live-child counts, including zero and over-limit values |
| `load_gate` | Effective `enabled`, `cpu_max`, `memory_min` settings |
| `gate` | `off`, `open`, `held`, or `unknown`; independent of focus and full slots |
| `reasons` | All blockers: `cpu`, `memory`, `sample`, `config`, or `cooldown`, with a short readable explanation |
| `cpu_percent`, `memory_available_percent` | Latest raw percentages, null when unavailable |
| `sample_at` | Wall-clock timestamp for display; null without a valid sample |
| `sample_age_seconds` | Age measured with the service's monotonic clock at status publication |

Use existing `host`, `project`, `at` and `pid` to identify/freshen the snapshot. Missing new fields mean `Unknown: service has no capacity status` ([unknown is never healthy](https://github.com/taskqorg/specq/blob/main/engineering/verification.md#unknown-is-never-healthy)).
Runtime rows come from the union of effective limits and live children; a removed runtime with a child still shows usage and limit 0.
Update the capacity snapshot every sample and after launch/reap/reload, without forcing a board poll every sample. Mark a sample/status older than 60 seconds unknown; add elapsed display time to published age, and treat clock anomalies as unknown.
The tray reads only registered local project snapshots, no board/token. Remote hosts show `Unknown: no local snapshot`; do not invent fleet usage from heartbeat task counts. A future remote transport is separate work.

## Load model

Recommend CPU busy percentage across all logical processors and physical memory available percentage. Both reflect all host activity, including other projects and unrelated programs.
CPU load average is not CPU utilization and has incompatible Windows semantics; per-child RSS does not measure host pressure. Use native APIs/files, no executable parsing or new dependencies.

| Platform | CPU | Available physical memory / total |
|---|---|---|
| Windows | `GetSystemTimes`: delta(kernel + user − idle) / delta(kernel + user); kernel already includes idle | `GlobalMemoryStatusEx`: `ullAvailPhys / ullTotalPhys` |
| macOS | `host_statistics(HOST_CPU_LOAD_INFO)`: delta(user + system + nice) / delta(all CPU ticks) | `host_statistics64(HOST_VM_INFO64)`: (free + inactive pages) × `host_page_size` / `sysctlbyname("hw.memsize")` |
| Linux | `/proc/stat` aggregate `cpu`: busy / total tick deltas; idle includes iowait; exclude guest/guest_nice already counted in user/nice | `/proc/meminfo`: `MemAvailable / MemTotal`; absent `MemAvailable` is unknown |

Existing `windows` and `libc` dependencies cover native bindings (Windows feature flags may need enabling); Linux reads use the standard library. No new crate, admin privilege or monitoring daemon.
macOS free + inactive is a reclaimable-memory approximation, not Apple's pressure metric; do not add purgeable pages again or count compressed pages as available. Document this conservative platform difference in help. Validate on real macOS before release.
Auto's macOS pressure floor reads `sysctlbyname("kern.memorystatus_vm_pressure_level")`; level 4 or higher holds (taskq.md § 3).
Reject nonpositive totals, decreasing CPU counters, impossible values or API/read failures as unknown; reset the CPU baseline. Percentages use unrounded values for decisions, rounded values only for display.

- Disabled by default on old and new configs. When the owner enables it, suggested defaults are CPU ceiling 85% and available-memory floor 15%; these reserve headroom, not a hardware safety guarantee.
- Sample every 30 seconds using monotonic time, independently of board `poll` and while paused/held. First CPU reading establishes a baseline; no new launch until a valid interval sample exists.
- Hold immediately on a valid sample with CPU >= ceiling **or** available memory <= floor. Unknown or >60-second-old required readings also hold. Do not kill running children.
- Reopen only after three consecutive valid 30-second samples with CPU <= ceiling − 10 percentage points **and** available memory >= floor + 5. Any failed/late/mid-band sample resets the recovery count. Restart, re-enable or threshold changes start held and require this recovery.
- With gating enabled, allow at most one child launch per project/host per 30 seconds, counting resumes and reviewers. Recheck freshness and effective limits just before each launch; the next launch waits for a post-launch sample. This limits filling every free slot from one obsolete low reading.
- Check the gate before writing a claim; a held task stays queued without a `spawn`, retry or `gone`. If freshness/config changes after a winning claim, retain that attempt pending launch with its lease listed in the heartbeat; no replacement claim. R2/R11 and normal cancellation still apply.
- Preserve pending resumes/claims in the service's in-memory attempt handling; heartbeat lists leased pending launches as well as live children. `used` still counts only live children. This adapter/heartbeat change must ship with its contract amendment, not be hidden in a sampler.

These rules avoid threshold flapping and startup bursts. They cannot prevent a workload from spiking between samples, cannot reserve RAM for the next task, and do not coordinate different project services' launch spacing.
Non-goals: battery/thermal sensors (no portable stable signal), process throttling/termination, adaptive limits, auto-enabling gates, machine-wide quotas, shared fairness, remote config deployment, historical load graphs, or new log states.

## Proposed taskq.md patch

This is a **future** contract patch, not an amendment applied by this PR. It replaces existing lines and adds four: 296 → 300 on the reviewed base.
The compact contract sets the decisions; this note supplies rationale and examples. R9 gains an explicit capacity confirmation rule; R1/R2/R11 remain unchanged.

```diff
@@ §3 Capacity @@
-- **Capacity** is per host: `limits.<runtime>` minus the service's live children of that runtime. No project cap: the
-  sum of host limits bounds the project.
+- **Capacity** is per project/host: effective runtime limits minus all its live children, reviewers/resumes included; zero disables launches. No project cap or shared host pool. Lowering a limit never stops children.
+- **Load gate** is opt-in: CPU busy >= `cpu_max` or available physical memory <= `memory_min` holds all new child launches; existing children continue. Windows reads GetSystemTimes/GlobalMemoryStatusEx, macOS host CPU/VM statistics (free + inactive)/hw.memsize, Linux /proc/stat (idle includes iowait, excludes duplicate guest ticks)/meminfo MemAvailable; missing/invalid readings hold.
+- Sample on monotonic 30-second ticks, independent of board poll; startup/config changes hold until three consecutive valid samples at CPU <= ceiling − 10 and memory >= floor + 5. Failed/late/mid-band samples reset recovery; readings older than 60 seconds hold. At most one launch per project/host per 30 seconds, after a post-launch sample; check before claim and launch.
+- Held resumes keep inbox pending; a won claim held before launch stays leased in the heartbeat, not counted used, without another claim/retry/gone. Restart follows §4 recovery. No battery/thermal gating or forced retirement.
@@ §4 Heartbeat: replace the attempt-list line @@
-   and `#<N> <id>` per live child, plus `at <board time>` (a same-body edit keeps `updated_at`). Freshness uses the board's `updated_at` and the response `Date`, never local clocks.
+   and `#<N> <id>` per live child or leased pending launch, plus `at <board time>` (a same-body edit keeps `updated_at`). Freshness uses the board's `updated_at` and the response `Date`, never local clocks.
@@ §4 Poke/status @@
-  writes `<root>/.taskq/status.json` (state, open tasks with labels, `request`, `update`: a newer version the last check found and did not install, else null, `events_since`) for `service status` and the tray.
+  writes `<root>/.taskq/status.json` (state, open tasks with labels, `request`, `update`: a newer version the last check found and did not install, else null, `events_since`) for `service status` and the tray; `capacity` adds effective `limits`, live-child `used`, effective `load_gate`, `gate` (off/open/held/unknown), all `reasons`, CPU/available-memory percentages, `sample_at`, monotonic `sample_age_seconds`; unknown readings are null, missing/stale (>60 s) capacity is unknown. Publish capacity on sample/launch/reap/reload; remote usage needs a local snapshot, else it is unknown. Reload capacity before admission; bad config holds launches, children continue.
@@ §5 Commands: insert before selftest @@
+| `taskq service capacity [--host H] [--claude N] [--codex N] [--load-gate on\|off] [--cpu-max P] [--memory-min P]` | read: local host default; write: explicit known host, owner instruction (menu confirms named old/new values first, R9). Same atomic JSON-preserving operation; detect concurrent edits; read back saved config, poke locally, say applied only after fresh matching effective service readback. Other hosts: saved only, no remote deployment. |
@@ §9 Config: insert after hosts/has/limits @@
+- `capacity_hosts` optional resolved-host map: partial `limits` override existing per-project defaults, absent runtime = disabled; optional host `load_gate` replaces the top-level default object. Gate fields: `enabled` false, `cpu_max` 85, `memory_min` 15 when omitted. Validate limits 0..u32::MAX, CPU 10..100, memory 1..95 (integer percentages); preserve other keys. Absent new keys preserve behavior; no auto-migration/enabling. Use new fields only on verified compatible hosts; older rollback loses enforcement and needs compatible restoration or owner-approved pause before admission.
```

## Owner questions before implementation

These choices do not block this proposal; none is applied by a design-only merge. Ask on the implementation task before changing a decision.

1. Keep per-project/host ceilings, or introduce a shared machine pool? Recommend **keep existing scope plus named host overrides**; a shared pool requires separate coordination and migration work.
2. Opt-in gate with CPU 85%, available memory 15%, 30-second samples and 90-second recovery, or enable automatically? Recommend **opt-in with these conservative starting values**, preserving every existing host's behavior.
3. Hold on unknown readings and gate resumes/reviewers, or exempt them/fail open? Recommend **hold every new process**, with pending inbox/claims leased and visible; it may delay answers but respects the ceiling and protects headroom.
4. Confirm in-menu and accept explicit owner CLI writes, or require a separate confirmation command for CLI too? Recommend **menu confirmation plus explicit owner CLI instruction**, with identical validation and readback; local file permissions remain the security boundary.

## Implementation review checklist (not qualification)

- Scope: config parsing/reload and capacity command in `src/main.rs`; admission, claim/resume and heartbeat handling in `src/service.rs`; native sampling in `src/platform.rs`; tray state/action bridge and its UI; help, fixtures and contract. No change to `fold` or task states is proposed.
- Verify unchanged legacy configs, partial host overrides, absent runtimes, 0/full/over-limit counts, workers plus reviewers/resumes, lowering limits, config conflicts/invalid edits, unknown remote/stale/old-service snapshots, saved vs applied and rollback compatibility.
- Use deterministic tick/reading tests for startup, exact thresholds, mid-band recovery resets, stale readings, both blockers, post-launch spacing and pending claims/resumes; native tests must compare CPU and memory readings on Windows/macOS/Linux. Model tests alone do not qualify native sensors (R12).
- Menu acceptance: 320/360 px, long names, 14–20 px, 200% scale, both themes, keyboard/screen reader/touch, cancel, changed-config confirmation and readback errors. No render or sensor qualification is claimed by this note.

## Contract detail

Host admission, occupancy, gate thresholds and unknown handling follow [taskq.md section3](../../taskq.md#3-attempts-lease-capacity); policy scope follows [section9](../../taskq.md#9-config-and-cutover). Runtime limits, admission.max and load-gate changes write the local installation host.json. Optional project limits only narrow it; project config never supplies missing host authority. All command surfaces use the same validated policy writer and service readback.

`taskq service capacity` uses the root command syntax, with explicit local `--host` for writes. No Fixed-mode or remote-host edit is offered by the host-policy target. Preserve untouched fields, grants, revision/concurrent-write checks and the distinction between saved and observed/applied. An invalid host policy holds admission and preserves running work. The old CLI/schema may be interpreted only under an explicitly qualified legacy profile; it cannot become fallback host authority. The [host transition cases](host-service.md#command-surface-acceptance) gate replacement and retirement.
