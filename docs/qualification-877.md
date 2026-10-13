# Ubuntu cutover qualification (#877)

Read-only verification on 2026-10-14, attempt 6151909883, for
[#877](https://github.com/taskqorg/taskq/issues/877), following the cutover in
[#875](https://github.com/taskqorg/taskq/issues/875) and its
[evidence PR](https://github.com/taskqorg/taskq/pull/881).
The owner accepted 0.5.2 and amended A1 to accept released 0.5.2 or newer.
See the [contract](../taskq.md). This is a historical snapshot, not a live status.

## Fresh Ubuntu readback

- Release: [agentq v0.5.2](https://github.com/taskqorg/agentq/releases/tag/v0.5.2),
  asset `agentq-0.5.2-linux-x86_64`.
- SHA256: `2757dc5d881a37ef964a781eebdc714a12c2cf46ee3b1b8e3193b9a4650cf66a`.
  The downloaded release `SHA256SUMS` entry matches the installed launcher,
  release binary and running process executable, independently hashed in Python.
- `agentq selftest`: `ok 0.5.2 linux-x86_64`.
- `agentq doctor`: current and running service 0.5.2, PID 3599;
  RTK 0.51.0 and headroom 0.40.0 ok; six owned edits; legacy removed.
- Read-only Python assertions matched all six ownership journal entries against
  the active files. Codex and Claude MCP registrations both use the installed
  launcher, with arguments `mcp codex` and `mcp claude`, respectively.
- `agentq telemetry status`: pending 0, oldest none, dropped 0, keys set,
  error none, last delivery 342 seconds before readback. Credential file mode
  is 0600; key values were not read or printed. Credentials were supplied by
  the operator in #875; this attempt did not provision them.

## Preservation and other distro

The dated legacy retirement directory and all seven configuration backups remain.
Selected model, reasoning effort, approval, sandbox, project trust, Claude
permissions and other user settings matched those backups. Both active MCP
registrations and journaled hooks matched their owned values. Active Codex and
Claude configuration files contain neither the legacy launcher nor install path.
The standard legacy directories are absent; `pgrep -af agent-context` found no
legacy processes.

This attempt performed no uninstall, service restart, configuration change or
process termination. #875 records unchanged login hashes and no client connections
on the public legacy proxy ports before retirement. Historical process and login
preservation relies on that evidence; it cannot be reconstructed from today's
absent legacy processes. Restoration was not exercised.

In Ubuntu-24.04, native legacy directories and the native agentq install directory
are absent (`test -e` returned 1 for each). No cutover was performed there.
Windows deployment was not changed.

## Limits

No new agent turns or synthetic telemetry were sent. Delivery status does not
prove Langfuse ingestion: #875's trace readback returned 404, and real agent turns
remain unverified. These limitations do not turn configuration checks into native
agent telemetry proof.
