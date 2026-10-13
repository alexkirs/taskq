# Windows cutover qualification (#870)

The owner accepted the verified Windows cutover and deferred Ubuntu cutover to
[#877](https://github.com/taskqorg/taskq/issues/877) and idle taskq restart
qualification to [#878](https://github.com/taskqorg/taskq/issues/878).
See the [owner decision](https://github.com/taskqorg/taskq/issues/870#issuecomment-6151720892)
and the [contract](../taskq.md).

## Windows readback

Checked on 2026-10-14, attempt 6151678824, after the manually completed cutover:

- `agentq.exe doctor`: installed and running 0.5.1, service PID 26304;
  RTK 0.51.0 and Headroom 0.40.0 report ok; eight owned edits; legacy removed.
- `agentq.exe telemetry status`: pending 0, oldest none, dropped 0, keys set,
  error none; last delivery 387 seconds before the check. Keys were not printed.
- `taskq.exe service status` and the project's `.taskq/status.json`:
  version 4.1.10, service PID 5468, error null. Installed current reads 4.1.10
  and that release's healthy marker exists.
- Read-only process inspection: legacy Claude serve/worker PIDs 26196/23436 and
  Codex serve/worker PIDs 2900/3628 remain running. None was terminated.

These checks establish installation, service and queue state. They do not prove
telemetry from a new agent request or a new taskq restart.

## Deferred qualification

Ubuntu has a legacy directory and no Rust agentq launcher at its standard install
location. Ubuntu-24.04 has neither at its native standard locations. No WSL
installation was changed. #877 owns the cutover and fresh telemetry readback.

A new taskq restart was not attempted while this worker was live. #878 requires
an externally arranged idle window and service readback after one restart.
