# Log-pruning release and Windows recovery (#1258)

R12 and §§4,8 separate source checks, release integrity and live process evidence. This task does not change a rule or the queue; #1243 remains waiting for the owner.

## Source and release

The release source is `f3c6fb055d01d0a6d182f0a3680391dc58492911`, the merge of [#1260](https://github.com/taskqorg/taskq/pull/1260) for #1257. Tag `v4.4.3` points directly to it through the existing manual-tag release path. No unmerged #1243 publication or host-service changes are included. The already merged #1247 host-service implementation is an ancestor; it is not silently removed from release provenance. Legacy project-root service startup remains supported (`src/main.rs`, `service_run(..., None)`); this task does not install the host unit or migrate the other registered projects.

[Exact-source CI](https://github.com/taskqorg/taskq/actions/runs/38717417218) passed macOS, Windows MSVC, Windows GNU and Linux checks. [Release run](https://github.com/taskqorg/taskq/actions/runs/38717829666) builds the three shipped platforms and runs selftest on staged artifacts before attestation/publication.

Local `cargo test --locked`: 230 unit and 6 integration tests passed, 2 live tests ignored. Host memory and Seatbelt probes were sandbox-blocked, not PASS. The unchanged #1257 regression is included; its native Windows failure before and pass after are recorded in [#1257 evidence](qualification-1257.md). No test assertion or trace changed here.

## Before activation

SSH `desktop` reaches the owner-authorized Windows host outside the service child tree. Inspection used only the TaskQ root selected from the explicit install registry and the install. The project service was v4.4.2, PID 16688, parent 10820, running the installed `releases/4.4.2/taskq.exe`; its SHA-256 was `7357c0853566a9669e2ddfb86ea91949ae212b44c52bda19160c398f74cca31d`. Its status listed no live children. Current/previous were 4.4.2/4.4.1. No host policy or host-service PID file existed.

A coherent byte snapshot was saved under `.taskq/release-1258-backup/taskq-coherent.log` while holding the existing profile lock. It contains 2,169,757 bytes, SHA-256 `9d96383cf5056c70cb37fe2d4f6af8411747dc57a7efc9a5bad898d7124ea6bd`. Backup readback matched the captured bytes. Its 344 stage records were separately retained in `terminal-evidence.txt`, SHA-256 `4a68ca16ba236c7675e63c2a7d49648f0a6e921228785ff86c6896676aefc5b6`; a manifest, original project config and status are beside them. Raw records/config remain private on the host.

The first copy attempt succeeded, but `Get-FileHash` could not open the live file beside its writer. The completed snapshot uses `File.Open(..., Read, ReadWrite)` and hashes the captured bytes, then verifies the saved copy. No live log was rewritten to work around sharing.

Installed v4.4.2 `selftest` exited 0: 72 golden traces, formats, board and terminal guard. The log was already below the retention bound. The historical refusal therefore was not currently reproducible; this baseline is not a live before-failure claim.

Rollback restores the previous binary, not records removed by normal retention. Keep the diagnostic backup. Short observation does not prove sustained stability.

## Published artifact verification

[v4.4.3](https://github.com/taskqorg/taskq/releases/tag/v4.4.3) was published at 10:08:46 UTC. All 18 entries in `SHA256SUMS` matched the downloaded release files; `latest.json` names 4.4.3. `gh attestation verify <file> --repo taskqorg/taskq --format json` verified `SHA256SUMS` and each Linux, macOS and Windows binary. The verified provenance resolves `refs/tags/v4.4.3` to source `f3c6fb055d01d0a6d182f0a3680391dc58492911`. Windows binary SHA-256: `25a5a3d8ea15600414ffa3638c06ff3a64e822b96b9bbea8a72ead0b67c3e14b` (5,765,632 bytes). The verifier needed `XDG_CACHE_HOME` in OS temp because its default cache was sandbox-blocked; the failed first verifier attempt was not accepted as verification.

Local `cargo run --locked -- selftest` exited 0 (72 traces, formats, board and terminal guard); installed profiling/request-log append remained sandbox-blocked and unknown.

The installed v4.4.2 `update` returned exit 1 with `EOF while parsing a value at line 1 column 0`. Readback nevertheless showed `current=4.4.3`, `previous=4.4.2` and the installed 4.4.3 binary hash matching the verified release. This is partial updater success, not a successful command outcome; the EOF cause is unverified. The new installed binary's live Windows `selftest` exited 0 with 72 traces, formats, board and no overdue nonlive terminal defects. At that point the project service was still PID 16688 on 4.4.2; downloaded/selected state did not establish activation. With the taskq project's status still listing no live children, v4.4.2 `service restart` requested the legacy project-root restart (exit 0); no `service install`, host-unit creation or multi-project policy migration was requested.

## Running process and live retention

The initial 4.4.3 status briefly named PID 33464 after re-exec, but a process read found it absent; that snapshot was not accepted as live activation. Fresh status then named v4.4.3 PID 9052, parent 10820, created at 10:11:26.606 UTC. CIM readback found the running installed 4.4.3 executable and `service run` command; its hash matched `25a5a3d8ea15600414ffa3638c06ff3a64e822b96b9bbea8a72ead0b67c3e14b`. The same process and hash remained present in the repeat observation. No live attempt was retired; taskq's status listed no children throughout activation.

At 10:12:24 UTC, under the existing profile lock, another complete pre-probe snapshot was saved as `.taskq/release-1258-backup/pre-probe.log`: 2,231,253 bytes, SHA-256 `dd39d0eb30082daa65410d0ec286c21c0fbbd474389ea16b1a7192dc9d99076e`. A bounded append added 1,964,241 bytes of explicitly invented diagnostic padding and a final sentinel naming the backup hash. This was append-only instrumentation; it did not insert stage records, fabricate task outcomes or manually truncate/rewrite the live log. Resulting live length was 4,195,494 bytes, above both the unchanged 4 MiB bound and its collection cutoff (`LOG_MAX - 512`). A project poke requested the existing service pass; no second service was started.

At 10:12:41 UTC, the log had fallen to 2,097,358 bytes through service retention. The status advanced to board time 1792059146; the retained log included a completed `service.pass` observation starting at 1792059145404 ms. The probe's final sentinel survived; all 1,334 recognized retained profile/board/stage/transition JSON records parsed, with zero corrupt records. This establishes real pruning after an above-bound append, separately from the model regression. The shared install log has multiple legacy service writers; these observations do not identify which project's pass won the profile lock and performed the trim.

The post-pruning installed Windows `selftest` exited 0: 72 traces, formats and board read passed. Terminal records reported `unknown (no stage observations)`, not PASS: the bounded append moved the older stage records outside the newest half. Before the probe, the same installed binary's live selftest had reported no overdue nonlive defects; after it, historical terminal evidence remains in the verified backups. This does not prove terminal accounting with fresh post-retention stages. The unchanged native Windows regression separately proves preservation when a terminal record is inside the retained tail.

At 10:13:10 UTC, after actual selftest and service appends, the log had grown to 2,104,346 bytes, still below collection cutoff. The sentinel survived, 1,407 recognized JSON records parsed, none corrupt. Additional completed pass starts included 1792059167499, 1792059170859, 1792059182439 and 1792059184521 ms. PID 9052 and its verified binary hash were unchanged; previous remained 4.4.2, the pre-probe backup hash still matched, and no host.json was created. The snapshots/manifests remain in the project-local backup. No guard was disabled, bound raised, log deleted or manual truncation counted as recovery.

## Limits and rollback

A1 artifact integrity/provenance and A2 running legacy project-service activation were verified. The first A3/A4 probe left the terminal claim unknown; the corrective observation below proves retained genuine terminal evidence, collection and selftest after live pruning. The updater's EOF error remains unexplained. Short-window recovery is not sustained stability or host-service migration qualification. #1243 was not answered or requeued.

Rollback uses the existing updater/launcher previous-version mechanism with 4.4.2 retained; no live rollback was exercised, because that would reintroduce the pruning scheduling defect. Binary rollback cannot reconstruct pruned records; restore evidence only from the verified backup and do not overwrite the live log during normal operation.

## Corrective live verification after review

The review rejected A3 because the first probe discarded all stage records. This is a qualification repair under R12 and §7, with no code, rule, test or guard change. The earlier unknown remains recorded above.

Through SSH `desktop`, a bounded read-only observation waited for an actual service-owned attempt to finish. The existing Windows reviewer for task #1231, attempt `6189747861`, produced a genuine `review` terminal record with `outcome=retried`, `at_ms=1792059705979`. No task was created, answered, stopped or otherwise changed for this probe; no stage was fabricated or replayed from backup.

At 10:23:20 UTC, the probe acquired the existing `taskq-profile.lock` with `FileStream.Lock(0, Int64.MaxValue)` and read the live log through `File.Open(..., Open, Read, ReadWrite)`. It saved the complete 2,141,541-byte snapshot to `.taskq/release-1258-backup/terminal-pre-probe.log`. SHA-256 of captured bytes and saved backup matched `cf9a22ae5c09adbfbe533ad5aca54931921cce6a4b34d76cd478627429dca7fe`. The exact genuine terminal line was also saved separately in `genuine-terminal.txt`. The existing earlier backups remain untouched.

The terminal began at byte 2,134,210. Before writing, the probe asserted that it would remain inside the newest half, with a 1,024-byte safety margin. It appended 2,053,787 bytes of explicitly invented plain diagnostic padding, ending with a newline, through `File.Open(..., Append, Write, ReadWrite)`; it did not overwrite or truncate. The resulting 4,195,328-byte live log exceeded the unchanged 4 MiB bound and collection cutoff. The probe released the lock and touched only the registered project's `.taskq/poke`; the existing service performed retention. Its JSON manifest is `terminal-probe.json` beside the backup.

At 10:23:57 UTC, readback found 2,099,038 bytes and the exact genuine terminal line retained. All 470 recognized `taskq-profile`, `taskq-board`, `taskq-stage` and `taskq-transition` JSON records parsed, with zero corrupt records. Completed `service.pass` records started at `1792059802220` and `1792059811438` ms. Status advanced to `1792059815`; CIM still found PID 9052 on 4.4.3, with executable SHA-256 `25a5a3d8ea15600414ffa3638c06ff3a64e822b96b9bbea8a72ead0b67c3e14b`. Backup hash readback still matched.

Commands executed from the registered TaskQ root, with `$exe` resolved to the installed `releases/4.4.3/taskq.exe`:

```powershell
& $exe selftest
& $exe profile export
```

Live `selftest` returned exit 0: 72 golden traces, formats and board read passed; terminal guard reported `no overdue nonlive defects in retained window`, rather than `unknown (no stage observations)`. `profile export` returned exit 0 with 550 valid JSON rows and no invalid rows. The raw export remains private in `.taskq/release-1258-backup/post-retention-export.jsonl`. These commands and continuing service passes append real diagnostics without disabling the guard or raising the bound.

Repeat observation at 10:24:53 UTC, after these commands and further real appends, found 2,107,002 bytes, the same exact terminal line, 554 valid recognized JSON records and zero corrupt records. New completed pass starts were `1792059872500` and `1792059885003` ms; status advanced to `1792059886`. PID 9052, version and executable hash were unchanged, and the diagnostic backup hash still matched. The repeated `& $exe selftest` again exited 0 with 72 traces, formats, board and `no overdue nonlive defects in retained window`. Thus the corrective probe closes the observed A3/A4 gap; it does not establish sustained stability or a live rollback. #1243 remains waiting for the owner.

On this corrective attempt, `cargo test --locked` again passed 230 unit and 6 integration tests, with 2 ignored. Memory/Seatbelt checks were sandbox-blocked, not PASS. `cargo run --locked -- selftest` exited 0; local installed profiling/request-log appends were sandbox-blocked/unknown. No tests or golden traces were changed.

## Owner-directed incident recovery

The owner stopped all tasks and authorized direct repair, publication and readiness verification. No queued attempt was resumed. Recovery separates this compatible maintenance release from the still-unqualified host-topology migration in #1232/#1233; the lifecycle candidate remains in PR #1261.

A fresh append-only Windows probe retained the genuine cancelled build terminal for #1231 attempt `6189839694` through service pruning: 2,175,441 bytes before padding, 4,194,909 filled, 2,097,839 after pruning. The byte-exact backup SHA-256 is `f6cc46977c9889d3ec5280607d7ceaf5e233d2b2a94de0c63b1b354c4d76fc14`.

The recorded hash above must match the operator receipt; raw backups remain private. The exact terminal and newest diagnostic sentinel survived; all retained stage rows parsed. The subsequent installed Windows selftest returned `no overdue nonlive defects in retained window`, not unknown, and real appends continued. No stage record was manufactured, restored or replayed.

The maintenance correction amends R3/R5 and section 7: an unchanged external blocker uses `later` with the failed check and one unblock condition, and resumption requires proof. Runtime brief checks retain delivery across both roles and all runtimes. Existing fold traces already prove that later prevents spawning and fenced stale writes.

The update sweep was independently reproduced deleting the emergency hold and admission files, violating section 3's hold-until-release rule. The allowlist now retains them and the existing sweep chain checks the hold payload. The dependency-cache check reproduced 3 reads instead of 1 on Windows because /nonexistent contained profile records; a unique test fixture preserves the one-read assertion. Neither repair weakens a guard or changes the board format.

All registered projects on all three hosts are paused before release, in addition to the durable emergency holds. Final readiness requires verified published artifacts, fresh process/version readback and selftest, zero children and retained holds. These requirements remain pending until actual rollout; no production host unit is installed by this correction.
