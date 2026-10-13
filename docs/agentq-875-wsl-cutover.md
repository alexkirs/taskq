# WSL agentq cutover evidence

This records the Ubuntu WSL cutover checked for [task #875](https://github.com/taskqorg/taskq/issues/875) on 2026-10-14. It is a historical verification snapshot; the issue remains the operational source of truth. Native Windows and other hosts are outside this evidence.

## Installed and checked

- Release: [agentq v0.5.2](https://github.com/taskqorg/agentq/releases/tag/v0.5.2), asset `agentq-0.5.2-linux-x86_64`.
- SHA256: `2757dc5d881a37ef964a781eebdc714a12c2cf46ee3b1b8e3193b9a4650cf66a`, matched against the release's `SHA256SUMS`.
- Artifact `selftest`: `ok 0.5.2 linux-x86_64`.
- `onboard --yes codex claude`: retired the legacy install, backed up seven configuration files, applied six journaled owned edits and installed the user service.
- `doctor`: installed current 0.5.2; service running 0.5.2; RTK 0.51.0 and headroom 0.40.0 ready; six owned edits; legacy removed.
- Both MCP registrations resolve to the installed agentq launcher with `mcp codex` or `mcp claude`.
- Direct MCP initialization, indexing, search and original recovery passed for both profiles using a temporary test document. These were protocol checks, not agent turns.

## Preservation and legacy removal

Before and after hashes matched for existing login files. Selected model, effort, approval, sandbox, trust and Claude permission settings also matched.

The legacy monitor was unloaded and its processes disappeared. Before retirement, established legacy connections were internal monitor/worker connections, with no client connections on the public legacy proxy ports.

The retirement folder and its configuration backups were retained under the owner-specified date, 2026-10-14. The retirement command used the UTC date 2026-10-13; the folder was renamed to the authorized date without replacing an existing backup. Active Codex and Claude configuration files contained neither `launcher.mjs`, legacy install paths nor the legacy proxy addresses checked before retirement.

Rollback material is retained, but restoration was not exercised. The installed agentq ownership journal provides its own uninstall path; legacy restoration must preserve later user edits and credentials.

## Telemetry and limits

The operator confirmed that credentials were securely loaded from the approved source before installation. The worker did not read the Mac or compare its key values. The stored host matched `https://langfuse.ufobe.com`; fallback credential storage had mode 0600. No credential values were printed.

Fresh `doctor` and `telemetry status` reads reported keys set, zero pending events, a successful recent delivery, zero dropped events and no error. This establishes delivery, not Langfuse ingestion or native session telemetry.

`telemetry verify` sent synthetic trace `000018de28901cec8fe500000e610000`, but its 60-second readback returned HTTP 404. A later read of the same trace also returned 404. Trace visibility remains unverified.

Real agent turns remain unverified: WSL's `codex` command resolved to a Windows npm installation and failed because `@openai/codex-linux-x64` was missing. No agent runtime was installed or changed to expand this task. The direct MCP checks do not establish native agent sessions, usage observations or routing.
