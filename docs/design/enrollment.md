# Independent DashQ connection

Accepted implementation target under [taskq.md](../../taskq.md). Apply [shared connection semantics](https://github.com/taskqorg/specq/blob/main/engineering/product-connections.md) through the coreq linked client and [DashQ C1–C7](https://github.com/taskqorg/dashq/blob/main/docs/design/enrollment.md). Pending implementation and qualification.

`taskq enroll <https-url>`, `taskq connection status` and `taskq disconnect` use the same browser/headless approval, protected credential, projection receipt and revoke lifecycle as AgentQ. No AgentQ installation or Langfuse dependency. The existing service handles delivery; no extra daemon or scheduler. Stable random installation identity survives updates, never derives from hostname; lost identity creates a new installation.

Send only a coalesced latest unsent snapshot. Connection state and credentials are durable and excluded from pruning; they contain no authoritative product state. Heartbeat freshness and underlying observation freshness remain separate. Snapshot transport failure never changes primary command success or blocks primary work. C3's accepted credential lifetime exception is confined to observation, never commands or forge access.

## TaskQ projection

The canonical snapshot remains the only board projection; reuse its revisions, task IDs, card choices, recommendation and completeness. Explicitly approve board scope in enrollment and show it before switching. Forge tokens remain local. Coalesce per board without summing duplicate board snapshots from several hosts; installation identity and board identity are distinct.

R1 exception: protected connection identity/grant/revoke state and the latest unsent projection may live locally; they are not task truth, logs or an offline task queue. Board mutations still follow canonical actor/revision/op-ID/receipt rules. This observation grant cannot answer questions or execute commands; dashboard actions require their separate contract.

## Qualification

Prove an independently installed product can enroll, show a real matching snapshot in DashQ, recover after network/store restart, disconnect offline, revoke and reconnect without identity or scope leakage. Test denied/wrong-workspace requests, lost replies, bounded storage, schema compatibility and primary-function independence. Reuse coreq conformance; do not reimplement its auth or transport. Ship only after applicable E1–E10 and the product's normal checks pass.
