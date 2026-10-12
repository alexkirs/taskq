# Owned Codex worker admission: separate model and resource gates

Read-only installed-API audit, 2026-10-10. This is not model-worker execution qualification.

The repair implementation at 2e0d17a passed 359 tests on Mac (17 skips) and Windows
(13 skips). Historical controlled-artifact component tests do not qualify ordinary workers.

The owner selected active model-turn concurrency on 2026-10-10. Task ownership and
no-duplicate-worker protection remain independent of the model budget. A correlated
terminal turn plus exact CLI PID/birth death can release a model-only reservation;
application acknowledgement, accepted result and heavy/resource settlement require their
own evidence. Missing universal descendant inventory therefore blocks generic resource
drain, not model-budget release.

The uninstalled candidate adds model-only lease verification and native child binding
to ordinary Codex spawn/resume, with durable board intents and schema3. The authorized
real two-turn TaskQ model probe remains unconsumed; no ordinary model qualification
is claimed from the finite Python child fixture.

## Evidence

Installed `codex --version`: `codex-cli 0.159.3`. Generated with the supported command:

```
codex app-server generate-json-schema --out /tmp/taskq-model-api-audit
```

- `Codex.exec` owns a CLI PID/birth and appends JSONL. `Codex.state`/`observe` use
  process death and `turn.completed`; neither provides a complete command/descendant inventory.
- Generated `CommandExecTerminateParams.json` describes termination of a running
  `command/exec` session. Its processId is client-supplied and connection-scoped to the
  original request. It is not termination authority over model commands started by the
  separate `codex exec` connection.
- `ThreadReadResponse.json` defines model `commandExecution.processId` as optional,
  identifying the underlying PTY when available. It supplies no OS PID/birth binding or
  completeness guarantee for descendants.
- `ThreadUnsubscribeResponse.json` returns only notLoaded/notSubscribed/unsubscribed.
  This is not a drain receipt. `TurnInterruptResponse.json` is an empty response; alone
  it does not establish that all descendants stopped.
- `SQLiteCapacity.release` keeps controlled-artifact resource verification. The candidate
  model verifier is restricted to exactly one `model:codex` unit and checks immutable log
  prefix, native session attribution, one ordered terminal turn and exact CLI PID/birth
  death. It cannot settle heavy/resource demand or application receipts.

The existing generic Codex adapter cannot claim heavy/resource drain from CLI death,
unsubscribe or interrupt acknowledgement. A narrowly owned resource adapter needs its
own qualified containment boundary. On Windows, the independently reported disposable
Python CreateProcess-tree Job Object proof passed; the subsequent real Codex probe failed:
Codex was assigned to the strict job but fixture descendants escaped it. Empty job state
therefore is not universal drain evidence. Exact broker/escape cause remains unresolved.
This is reported remote evidence, not an independently fetched Mac receipt. Corrected
cleanup IDs are retained only in the Windows receipt, not inferred from message prose.

No production operations, permissions or shared application settings were changed here.
Model-only settlement intentionally does not change generic resource containment claims.

Code updates and board repair are separate: global installation does not enumerate projects.
The selected release gates ordinary project operations and requires explicit repair of an
incompatible board. No actual global install/update was performed in this qualification.
