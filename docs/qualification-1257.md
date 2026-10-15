# Bounded service log pruning (#1257)

The repair restores collection after a log reaches its bound; it does not qualify live activation. R12 and §4 require separate native evidence and readback. The runtime change amends §4 under R8/R13.

## Cause and historical failure

[#1243](https://github.com/taskqorg/taskq/issues/1243#issuecomment-6188995357) records the Windows selftest refusal: `taskq.log at retention bound; collection stopped until service pruning`.
Release v4.4.2 is commit `875b744019dbf985e20f1eb0e16a12612edae976`. Its `src/main.rs` runs `data::prune` after the service pass, initially and then only every 3600 seconds. `profile::append_line` and collection refuse above `LOG_MAX - 512`. Reaching that bound between hourly pruning rounds therefore stops collection until the next round. This is a scheduling gap, not evidence of a Windows lock or filesystem fault.

Read-only Windows inspection found v4.4.2 still running and the log already below its bound at 2,137,080 bytes. The historical failure was not still present. No live log, lock, worktree or service was changed. Native reproduction uses only isolated OS-temp copies and generated logs.

## Regression chain

`cargo test --locked service_pass_restores_bounded_log_collection` checks §4 retention → service pass → collection/selftest. It exercises both the collection threshold and an oversized log, a busy profile lock, an open append handle, readable newest diagnostics, a retained terminal record, resumed appends and successful collection/selftest. No existing assertion or golden trace was changed.

The same test was injected unchanged into v4.4.2 source and failed natively on Windows at `retained.len() as u64 <= crate::data::LOG_MAX / 2` after a service pass (exit 101). The lock-preservation assertion passed before that failure. A second before-run on candidate source with only the new pass-time pruning call removed failed at the same assertion. Restoring the candidate source made that same native Windows command pass (1 passed, exit 0); both log sizes, lock handling, newest records, open writer and collection/selftest assertions passed. The tested implementation is commit `5a5015dc1193ffa48b7944b14a10c1c780a98ad7`; this follow-up adds documentation only.

On macOS, `cargo test --locked` passed 230 unit and 6 integration tests, with 2 live tests ignored. The host memory and Seatbelt probes reported sandbox-blocked, not PASS. `cargo run -- selftest` exited 0 (72 traces, formats, board and terminal guard); profiling/request-log append remained unknown because the sandbox cannot append to the installed log. `git rebase origin/main` and `git diff --check` passed. These checks do not qualify live Windows activation.

## Release and live proof handoff

[#1258](https://github.com/taskqorg/taskq/issues/1258) is the canonical dependent delivery task. #1259 is parked. Use #1257's resulting merge SHA, not the PR head, for release provenance and activation. Reviewer close requires green platform CI on the exact result SHA; the worker does not wait for CI.

Before live repair, copy the service log and terminal evidence to a separate backup and record hashes. Preserve work, locks, live attempts and rollback. Activate only the verified merged release through the approved host-service path; do not run a second service or manually truncate the log. Read back service version, process identity and executable hash. Observe a new pruning pass and verify readable newest diagnostics, retained terminal records, resumed appends and live selftest. If an I/O or lock error remains, retain the evidence and diagnose it separately; do not delete logs, raise bounds or disable guards.

Rollback restores the prior binary, not records legitimately removed by retention; the pre-activation backup preserves those records. Keep #1243 waiting. Short-window recovery does not prove sustained stability.
