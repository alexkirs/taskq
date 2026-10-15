# v4.4.1 release and Windows staging

Delivery for [#1251](https://github.com/taskqorg/taskq/issues/1251), under
[taskq.md § 8](../taskq.md#8-runtimes-and-deployment). Post-restart acceptance
belongs to [#1252](https://github.com/taskqorg/taskq/issues/1252).

- Tag: `v4.4.1`; source: `782023e5d598338eb76e4aa551cab607e36c93cb`.
- Includes #1248's terminal-record repair, merged as #1250. Neither local
  `taskq-1230` nor `taskq-1243` head is an ancestor of the tag.
- Exact-source checks passed, including Windows, macOS, Linux and Windows GNU.
- Release workflow: https://github.com/taskqorg/taskq/actions/runs/38695831945.
  The tag path builds versioned artifacts and runs each staged binary's selftest.

Before staging, Windows status reported version `4.4.0`, service PID `15012`,
and only task #1251, attempt `6186362021`, as a live child. Both #1230 and #1243
were open with `q-ask`; this delivery does not answer or restart them.

The retained `profile export --since 24h` already contained a review record for
#1239 attempt `6185426441`: `at_ms=1792043434016`, `outcome=unknown`,
`error=left-board`, `unknown=no-usage`. This observation is not post-update
recovery proof (R12). Logs and profile records are preserved.

## Follow-up

## Verified publication and staging

- Release workflow finished successfully: all three builds and staged offline
  selftests, attest and publish. Release: https://github.com/taskqorg/taskq/releases/tag/v4.4.1.
- `gh attestation verify <file> --repo taskqorg/taskq --format json` exited 0
  for both `SHA256SUMS` and the Windows binary. Provenance names the source SHA
  above and `refs/tags/v4.4.1`.
- Downloaded and installed Windows binaries both have SHA256
  `4ae8f20d2e531863542e25eb66cef9a028270c99f012f81b28530eced16b0094`,
  matching the attested checksum list. Manifest version is `4.4.1`.
- The existing updater staged `current=4.4.1`, `previous=4.4.0`;
  `taskq update` read back `up to date (4.4.1)` (exit 0). Both immutable release
  binaries remain. `taskq service restart` requested the normal deferred restart;
  no process was stopped.
- Local `cargo test --locked`: 214 passed, 1 failed, 4 ignored. Failure:
  `service::tests::cross_board_dependency_waits_with_link_until_completed`,
  expected 1 dependency read, observed 3. The focused rerun also failed.
  Tests were not changed. Exact-source CI was green; local failure remains open.
- Local `cargo run -- selftest`: fold 72 traces and formats passed; exit 1 on
  one nonlive unresolved attempt older than 300 seconds. This is not a recovery
  pass; #1252 owns fresh post-restart diagnosis and live acceptance.

## Remaining acceptance

#1252 must read back the running service version and process identity after a
normal child drain, then inspect that attempt's terminal evidence and run live
selftest. Staging alone does not prove recovery. Do not force restart, clear
evidence, answer #1230/#1243 or restart their attempts.
