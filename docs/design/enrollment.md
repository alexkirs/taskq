# Independent DashQ connection

Accepted implementation target under [taskq.md](../../taskq.md). Apply [shared connection semantics](https://github.com/taskqorg/specq/blob/main/engineering/product-connections.md) through the coreq linked client and [DashQ C1–C7](https://github.com/taskqorg/dashq/blob/main/docs/design/enrollment.md). Implemented (#913); release qualification below.

`taskq enroll <https-url>`, `taskq connection status` and `taskq disconnect` use the same browser/headless approval, protected credential, projection receipt and revoke lifecycle as AgentQ. No AgentQ installation or Langfuse dependency. The existing service handles delivery; no extra daemon or scheduler. Stable random installation identity survives updates, never derives from hostname; lost identity creates a new installation.

Send only a coalesced latest unsent snapshot. Connection state and credentials are durable and excluded from pruning; they contain no authoritative product state. Heartbeat freshness and underlying observation freshness remain separate. Snapshot transport failure never changes primary command success or blocks primary work. C3's accepted credential lifetime exception is confined to observation, never commands or forge access.

## TaskQ projection

The canonical snapshot remains the only board projection; reuse its revisions, task IDs, card choices, recommendation and completeness. Explicitly approve board scope in enrollment and show it before switching. Forge tokens remain local. Coalesce per board without summing duplicate board snapshots from several hosts; installation identity and board identity are distinct.

R1 exception: protected connection identity/grant/revoke state and the latest unsent projection may live locally; they are not task truth, logs or an offline task queue. Board mutations still follow canonical actor/revision/op-ID/receipt rules. This observation grant cannot answer questions or execute commands; dashboard actions require their separate contract.

## Prepared projection (#912)

`src/enrollment.rs` prepares schema 1 observations; no command or service delivery is enabled yet. `installation_id(install)` creates a random 256-bit lowercase hex `installation-id`, publishing a complete file without replacing a concurrent winner. Updates and data pruning keep it; corruption is an error and stays untouched. Only a missing file creates a new identity. It is not a credential or host identifier.

An exact approved `(kind, host, repo)` scope gates projection of an unfiltered canonical snapshot. Identity fields are validated, host names and GitHub repository names lowercased; GitLab repository spelling stays canonical from the snapshot. No snapshot URL or extra identity keys are copied. Allowlisted task fields are number, state, revision, numbered choices and recommendation from the canonical card parser. Choices are explicitly scoped board content. Titles, raw cards/reasons, sessions, labels, paths, host metadata and configuration are omitted. Incomplete observations carry a fixed reason, never raw forge diagnostics.

Coalescing selects one latest observation per approved board, irrespective of installation; totals are never added across hosts. Equal-time conflicting observations become explicitly unknown. Every approved board gets a row, including missing observations. Freshness uses the original board observation time and the caller's maximum age; future/zero times are unknown, old observations are stale, and unknown/stale totals are null. Completeness remains independent of freshness. This is an observation adapter, never a task-state source or offline command queue.

## Client (#913)

coreq v0.11.0 `enroll` owns pairing, protected state (`<install>/connection/`, kept by sweep and pruning), delivery, receipts and revoke; TaskQ adds no transport. The request carries the TaskQ `installation-id`, product `taskq`, the host name as label, schema `taskq-board/1`, scope `{name: taskq-board/1, value: <kind>:<host>/<repo>}` and no capabilities. One board per installation: enrolling another project's board shows `old -> new` and switches; the old grant is revoked. A bare DashQ origin gets `/machine` appended.

Each service pass hands its latest snapshot to one delivery thread; a projection equal to the last accepted one is resent only after 300 s. Only the root whose board equals the approved scope sends. Retry follows coreq backoff; a stop (401, refused batch) waits 300 s; outcomes go to the service log and `status.json` `connection`. Choice text is `label` (DashQ C4 refuses a `text` key); over 256 tasks the observation is incomplete, reason `task limit`.

## Qualification

Prove an independently installed product can enroll, show a real matching snapshot in DashQ, recover after network/store restart, disconnect offline, revoke and reconnect without identity or scope leakage. Test denied/wrong-workspace requests, lost replies, bounded storage, schema compatibility and primary-function independence. Reuse coreq conformance; do not reimplement its auth or transport. Ship only after applicable E1–E10 and the product's normal checks pass.

| DashQ case | TaskQ evidence | Open |
|---|---|---|
| E1 connect | live macOS desktop enroll and fresh macOS and Windows `--headless` enrolls to dash.ufobe.com: approved code, first snapshot `projected`; DashQ 0.9.2 `/data` shows the `taskqorg/taskq` `taskq-board/1` snapshot (owner readback, #913) | — |
| E2 cancel/expire | live expired code ends with the rerun step; deny and wrong code are coreq conformance | — |
| E3 crash mid-pairing | coreq keeps `pending`; a rerun resumes it (coreq conformance) | — |
| E4 outage | chain `sender_sends_only_the_approved_board_and_never_blocks`: delivery on its own thread, outage only waits | — |
| E5 forged/wrong scope | same chain: another project's board is never sent; capabilities none. Live probe on DashQ 0.9.2 under the `taskqorg/taskq` grant: a `taskqorg/coreq` record is `rejected`, reason `scope`, and its receipt is `unknown_event`; the same record for `taskqorg/taskq` is `projected` | — |
| E6 schema | `taskq-board/1` projection fixture test | — |
| E7 disconnect | live macOS and Windows: revoke confirmed, `connection status` none, second disconnect says none; offline disconnect queues the revoke for the service; disconnect during pairing cancels it, revokes a staged grant, `enroll` cannot resume it (chain check `disconnect_cancels_pending_pairing`) | — |
| E8 TaskQ alone | no AgentQ or Langfuse dependency (Cargo.toml) | — |
| E9 upgrade | chain `identity_survives_pruning_and_concurrent_creation` keeps `connection/` through sweep and pruning; installed 4.2.0-dev sweep deletes it until this release | — |
| E10 UI | DashQ page; not TaskQ | — |

A worker cannot write `<install>` under the § 8 Access profile, so live enrollment runs from the owner's shell, a Full grant or a disposable `TASKQ_INSTALL`.
