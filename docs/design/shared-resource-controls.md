# Shared resource controls

Accepted target under [taskq.md](../../taskq.md), extending reuse of the [answer card](answer-card.md). TaskQ owns reusable domain components; DashQ supplies layout and authorized transport. No change to live limits, admission or permissions is made by this design.

Follow [shared-block inheritance B1–B4](shared-blocks.md). Compact placement does not authorize independent icons, tokens, counter layouts or simplified editors; the parent package governs visual fidelity.

## Reuse inventory

| Block | Canonical owner | Shared behavior |
|---|---|---|
| Node/runtime summary | taskq.md § 4/§ 5 | Stable installation/runtime identity, freshness, supported runtimes and live usage; unknown is not zero |
| Capacity and guard | taskq.md § 3/§ 9, [capacity](capacity.md) | Runtime used/limit, effective settings, CPU/available-memory guard, blocking reasons, saved versus applied |
| Auto / Fixed policy | [dynamic resources R1–R4](dynamic-resources.md) | Host-wide authority, optional host/project/runtime caps, source of effective limits, migration and admission reasons; implemented through #896 |
| Work mode editor | [menu M12](menu.md#m12-shared-work-mode) | Named host and selected registered projects, Mixed values, explicit edited fields, old→new review and per-target readback |

Reuse compact rows, aligned counts, disclosures and existing theme/text-size tokens. Extract components from existing TaskQ UI alongside the answer-card assets; no general widget framework, new package registry, scheduler or duplicate admission logic. Pin the same versioned local assets and fixtures in both consumers. TaskQ's native adapter remains functional.

## Product behavior

- Show connected nodes and their registered projects/runtimes. A node means stable installation/runtime identity, never a hostname join. Native Windows and WSL remain separate scopes. Resource overview may link to the shared editor; it does not embed a terminal or btop in the browser.
- Reuse existing project selection and runtime overrides; preserve unselected targets and fields. Existing enrollment is the route to connect another node. This work does not install agents/runtimes, mint provider credentials or invent a remote project-setup flow.
- Existing services expose only their qualified limits/guard controls. Auto/Fixed appears only when the target advertises the qualified #896 policy schema and minimum version. Never reinterpret legacy missing limits as unlimited or silently migrate an existing host to Auto.
- Auto is the producer's admission policy, not a browser loop or a cost cap. Host/project/runtime ceilings intersect as the canonical schema defines. Explicit zero, disabled runtimes, pause and existing access restrictions remain effective. Lowering a ceiling preserves running attempts.
- Display current/effective policy, optional ceiling, usage, all blocking reasons and observation age. Unknown/stale/provider limits are distinct from CPU/RAM holds. Auto numerical defaults come from its qualified producer version, not copied constants in DashQ.
- Admission CPU/RAM/pressure samples keep their source/cadence/units; AgentQ historical resource charts are a different observation stream. Do not overwrite a fresh admission signal with a delayed chart sample or use disk usage as an invented admission rule.
- Review identifies exact host, selected projects/runtimes and old→new fields before write. Cancel changes nothing. Saved/pending is not applied; applied requires fresh matching effective policy revision from the actual target authority, with per-target partial outcomes.

## Remote host adapter

The existing capacity CLI mutates its local project config; editing a remote host key on DashQ's machine is not remote application. Add a narrow typed policy adapter at the TaskQ host authority, using the existing enrolled service and HTTPS connection. No SSH, shell, generic command executor, extra daemon or second task queue.

- Observation enrollment and the answer capability confer no resource-write authority. The host operator explicitly grants resource-policy changes for named host/project/runtime scope; a browser administrator role alone does not grant control of arbitrary hosts. The grant can be revoked independently.
- The target validates grant, identity, supported policy schema, scope and expected policy revision immediately before applying the reviewed typed changes. Resolve registered projects locally, never accept paths or executable names from the browser. Reject unknown/malformed fields and privilege changes.
- One immutable operation ID binds actor, target installation/generation, expected revision and change set. Replays recover the same result; mismatched reuse and concurrent edits refuse. A timeout or restart never creates a new operation blindly.
- Bound pending operations, request size, retries and retention; expired or revoked requests must not apply on reconnect. Before implementation, publish exact transport/schema/bounds and client/server conformance fixtures in this specification as routine engineering detail; changes to approved authority or user-visible effects require owner review.
- Preserve unknown config keys and untouched settings; use the canonical writer/admission authority. Read back saved policy and fresh effective service state separately. Offline stays pending/unknown; no optimistic applied receipt. Never stop running work to satisfy a smaller ceiling.
- Reuse the existing connection's service loop and protected storage, but authorize resource mutations separately. Changes and receipts survive interrupted delivery without granting machine observation credentials permission to request mutations themselves.

## Resource policy v1

Contract version `taskq.resources/1`. This is a protocol specification and executable model, not enabled enrollment, transport, policy persistence or installed qualification. #896 remains the admission implementation; [enrollment](enrollment.md) remains the connection implementation. Both transports consume the same [wire examples and traces](../../tests/fixtures/resource-policy-v1.json), checked by `cargo test --locked --test resource_policy_contract`. No fixture operates a real host.

### Connection and identity

Use JSON UTF-8 over the accepted enrolled HTTPS connection (pending transport implementation), with its certificate verification and protected credentials. No inbound host listener. DashQ accepts `POST /api/taskq/resources/v1/operations`, `GET /api/taskq/resources/v1/operations/{op_id}` and `POST /api/taskq/resources/v1/operations/{op_id}/cancel`; the existing host service exchanges deliveries/receipts via `POST /api/taskq/resources/v1/exchange`. These are relative routes under the approved enrollment origin; reject redirects/origin changes. This defines the resource extension to that connection, not a second enrollment protocol. Auth tokens never enter JSON, URLs, receipts or projections.

An operator-issued resource grant is separate from observation and answer credentials: `grant_id`, `actor_id`, `installation_id`, `generation`, approved `project_ids`, approved `runtime_ids`, `host_policy` boolean, `expires_at`, `revoked`. The credential authenticates the actor; claimed actor/grant fields never authorize themselves. Host-wide changes require `host_policy`; project changes require exact project membership; runtime changes require both memberships. The service resolves opaque registered project IDs locally. Runtime IDs are only registered `claude`, `codex`, `codex-app`; display names/hostnames are never joins. IDs are nonempty ASCII `[A-Za-z0-9_-]`, at most 64 bytes; installation ID is the existing 64-character lowercase hex random identity. Generation is a positive u64 changed on identity/grant-store recovery; a grant never transfers to a new generation. Registration removal invalidates the scope, even if a path/name is later reused.

The grant must be active at enqueue and again immediately before the target's serialized write. Revocation/expiry blocks undelivered work, independently of observation/answers. Expired credentials cannot query protected receipts either. Remote administrator status conveys no host grant. TLS, actor authentication, protected storage and anti-CSRF for browser writes are mandatory adapter checks, not certified by the pure fixtures.

### Exact payloads

All objects reject unknown/duplicate keys and non-integer integer fields; no coercion. No shell, paths, executables, arbitrary config patches, credentials, permission fields or task commands. All fields below are required unless marked optional. JSON object key order is immaterial; array order and explicit null are meaningful. Times are Unix UTC integer seconds. Revisions use the ID grammar below and are opaque, never sortable clocks; producer assigns a new revision on every policy edit, including edit-and-revert.

| Payload | Exact fields |
|---|---|
| Target | `installation_id`, `generation`, `project_id` (null for host scope), `runtime_id` (null for host/project scope; project required otherwise) |
| Submit | `schema: "taskq.resources/1"`, `op_id`, `actor_id`, `grant_id`, `target`, `policy_schema`, `expected_revision`, `issued_at`, `expires_at`, `changes` |
| Changes | Optional `mode: "auto"|"fixed"`, `maximum: null|u32`, `runtime_limit: null|u32`, `load_gate: {enabled: bool, cpu_max: 10..100, memory_min: 1..95}`; at least one field |
| Cancel | `schema`, `op_id`, `actor_id`, `grant_id`, `target`; operation binding must match submit |
| Exchange request | `schema`, `installation_id`, `generation`, `receipts: [Receipt]` |
| Exchange response | `schema`, `server_at`, `operations: [Submit]`, `cancellations: [Cancel]`; expiry is checked at delivery and write, never extended |
| Receipt | `schema`, `op_id`, `target`, `state`, `reason` (null or code below), `saved_revision` (null or revision), `effective_revision` (null or revision), `observed_at` (null or time) |
| Lookup response | Receipt, or HTTP 404 with `{"schema":"taskq.resources/1","reason":"unknown_operation"}`; 404 is unknown, never permission to resubmit with a new ID |

Accepted submit returns HTTP 202 plus Receipt; duplicate/lookup/cancel returns HTTP 200 plus the latest Receipt. Exchange returns HTTP 200 with its response object. Refusal before acceptance returns only `{schema, reason}`: HTTP 400 invalid, 401 unauthorized, 403 revoked/scope, 409 conflict/replay_mismatch, 410 expired, 422 unsupported, 429 busy, 503 clock_unknown. Oversize body returns 413 invalid. No raw diagnostics or reflected untrusted values. A host-reported rejection of an already accepted operation is a Receipt returned by lookup, not a failed lookup request. Exchange authenticates the enrolled service installation; it cannot select another installation or mint an actor's write grant.

`op_id` uses the ID grammar above and binds the entire Submit, including actor, grant, generation, expected revision, deadline and exact typed change set. The authority durably records canonical parsed JSON before acknowledging acceptance; retries with the same object return the same operation's latest receipt. Changed reuse returns `replay_mismatch`, preserving the original. Actors cannot inspect another actor's operations. Cancel/lookup do not extend expiry. Transport body limit applies before parsing; duplicate JSON keys must be rejected before canonicalization.

Supported `policy_schema` values are `taskq.capacity/1` (qualified legacy Fixed limits/guard) and `taskq.admission/1` (qualified #896 Auto/Fixed authority, minimum producer `4.2.0-dev`). A target advertises only qualified fields/scopes; consumers never infer compatibility from a version string alone. Capacity permits `runtime_limit` only at runtime scope within a named project: both `project_id` and `runtime_id` must be non-null registered IDs; `runtime_id: null` cannot request a project aggregate limit. Capacity permits project `load_gate`; it forbids Auto, host-wide maximum and null legacy limits. Admission permits host/project `mode` and `maximum`, runtime `runtime_limit`, and project `load_gate`, only where advertised. Fixed retains explicit count ceilings and disables automatic load gating; Auto cannot disable the gate. Validate the resulting policy, including unchanged fields, at the canonical writer. Cross-mode changes require review of resulting effective fields, not an implicit migration. `null` removes a supported optional ceiling/override, zero disables new admissions, omitted fields remain unchanged. Legacy absent runtime limits remain disabled. Unsupported host/project aggregate ceilings stay unavailable until the producer implements and qualifies them; this contract does not invent enforcement. Numerical Auto defaults come from the producer, not this protocol.

Each submit affects one target under one policy authority/revision; several targets use separate operations and per-target receipts. Review shows identities, old→new edited fields, schema, expected revision and expiry before confirmation. Editing the draft invalidates review. Only explicit confirm freezes and sends that exact Submit. Cancel before confirm sends nothing. Grant/registration/revision/schema validation and atomic compare-and-write belong to the canonical writer, preserving unknown config keys and untouched settings. Serialize cancel/revoke/expiry checks with apply; never kill existing attempts or implement admission here.

### Bounds and receipts

| Bound | v1 value |
|---|---|
| JSON body (every route, uncompressed) | 16 KiB; no compressed request bodies |
| Pending operations | 32 per installation, 8 per actor/installation; reject overflow before acceptance (`busy`) |
| Exchange | At most 8 operations, 8 cancellations and 8 receipts per body; existing service loop, at least 5 seconds between resource exchanges |
| Request lifetime | 1..300 seconds from issued_at; issued_at at most 30 seconds ahead of server time; never apply at or after expires_at |
| Clock uncertainty | Refuse acceptance/application (`clock_unknown`) unless server time is verified; offline client never invents a renewed deadline |
| Retry | At most 5 submit transmissions total, same payload/ID; 1, 2, 4, 8 second backoff, each bounded by expiry; 10 second network timeout |
| Reconcile | Lookup no faster than every 5 seconds, stop automatic reconciliation after 300 seconds; retain unknown state and allow manual lookup |
| Retention | Pending expires by deadline; completed receipts and ID bindings 24 hours after expires_at, max 4096 per installation; reject new IDs if full, never evict an unexpired binding |

After retention, an original submit is already expired and cannot reapply. A fresh operation requires fresh observation, review and confirmation, even after unknown delivery. Protected connection operation records are policy-delivery deduplication/receipts, never board task truth or a second task queue; a transport implementation must qualify interrupted-write recovery before enabling this durable extension. If a crash leaves persistence uncertain, receipt is unknown and the authority reconciles saved/effective revisions before any retry; it never blindly repeats the mutation.

| Receipt state | Meaning / allowed transition |
|---|---|
| `pending` | Accepted for delivery, no saved policy; → saved, rejected, expired, cancelled, unknown |
| `saved` | Canonical writer read back saved revision; → applied or unknown; expiry/cancel cannot undo a completed write |
| `applied` | Fresh actual target service readback equals saved revision; final write outcome, observation can later become stale |
| `rejected` | No write; reason `unauthorized`, `revoked`, `scope`, `unsupported`, `invalid`, `conflict`, `replay_mismatch`, `busy`, `clock_unknown` |
| `expired` | Deadline reached before write; no write |
| `cancelled` | Cancellation serialized before write; no write |
| `unknown` | Delivery/persistence/effective readback cannot be established; preserve any known saved revision; lookup/readback may resolve it |

Cancel after save returns the existing receipt; it reports the completed effect, never claims rollback. Revocation after save prevents future changes but does not undo that effect. Lost response is client `unknown`, not a changed server result. Applied requires target/generation and revision equality plus readback age at most 60 seconds; future times or uncertain clocks are unknown. Saved offline remains saved with effective revision null/last-known, never applied. A readback from a different target/revision cannot satisfy the operation.

### Allowlisted read projection

`GET /api/taskq/resources/v1/projection` uses approved observation scope and returns `{schema, observed_at, targets: [Projection]}`. No operation-write grant is implied. Projection fields are exactly `target`, `service_version` (string|null), `supported` (array of `{policy_schema, modes, fields, scopes}`), `current_revision`, `effective_revision`, `current_policy`, `effective_policy`, `runtime_usage`, `sample`, `blockers`, `completeness`. Nullable fields are null when unknown; never emit raw config/status, paths, tokens, task/session lists or arbitrary diagnostics. Policies contain only advertised `mode`, `maximum`, `runtime_limit`, `load_gate` plus `sources` mapping each field to `host|project|runtime|default|unknown`; absent legacy fields are unknown/disabled according to producer semantics, never synthesized Auto/unlimited.

`runtime_usage` is an array of `{runtime_id, used: u32|null, limit: u32|null, limit_kind: "ceiling"|"unbounded"|"disabled"|"unknown"}`. Actual live children/reservations retain the producer's accounting; do not add heartbeat task totals or double-count shared hosts across projects. Over-capacity is valid; stopped/unobserved services are unknown, not zero.

`sample` is null or `{source, sampled_at, age_seconds, cadence_seconds, window_seconds, cpu_busy_percent, memory_available_percent, memory_available_gib, pressure}`; numeric measurements and timestamps are nullable. `source` is `taskq-admission-windows|taskq-admission-macos|taskq-admission-linux`, units are percentages/GiB/seconds, pressure is the producer's platform level or null. Cadence/window come from the producer. macOS reclaimable-memory approximation remains distinct from physical free RAM. No historical AgentQ chart, load average or disk metric substitutes for the admission signal. Add elapsed time to producer monotonic age; missing/future timestamps, clock discontinuities or age >60 seconds make current admission knowledge unknown. Preserve last-known measurements with their age, never relabel them fresh.

`blockers` includes every producer blocker as `{code, scope: Target, knowledge: "known"|"unknown"}`; codes are `cpu`, `memory`, `pressure`, `sample`, `config`, `cooldown`, `spacing`, `max`, `version`, `host`, `runtime_disabled`, `pause`, `focus`, `dependency`, `routing`, `access`, `provider`, `unknown`. Unrecognized producer reasons map to unknown, never disappear or become CPU. `completeness` is `complete|partial|unknown`; missing blocker coverage is explicitly partial/unknown, never an empty complete list. Stale projection adds unknown sample/host knowledge without dropping last-known blockers. Saved/current revision and effective revision stay separate even when usage is known. Observation does not determine grant validity; a separately authenticated resource capability decides whether controls are enabled.

### Executable conformance

The JSON corpus carries exact submit/cancel/receipt/projection examples and ordered client/server traces. The integration test is a deliberately transport-free authority model: it exercises reviewed confirmation, stale revision, immutable replay binding, duplicate/lost reply, credential separation, registration and generation scope, revoked grant, offline expiry, both cancel/apply orderings, and saved versus mismatching/stale/fresh effective readback. Bounds and typed-field refusal are also exercised. Transport adapters must run this corpus alongside authenticated HTTPS, protected-store restart, interrupted-write and installed per-OS qualification; a model pass cannot certify those boundaries (R12).

## Acceptance and delivery

Split shared component/native adoption, target-host adapter, DashQ embedding, DashQ authorized operations and installed qualification. Reuse #896 for Auto and #913 for independent TaskQ enrollment; do not reopen answer-card #919 as a broad resource redesign. Resource control does not gate observation-only enrollment or ordinary answers.

Shared fixtures cover legacy and Auto modes, mixed targets, zero/absent limits, over-capacity running work, stale metrics, multiple blockers, cancel, conflicting edits, revoked authority, duplicate/lost receipts and pending→applied. Verify native/browser 16/20 px, both themes, narrow layouts and draft/focus retention. Live qualification changes a reviewed test policy on the actual target, observes matching application and restores the baseline; installed per-OS proof is distinct from fixtures.

### Shared component (#930)

- Component: [src/resource-controls.js](../../src/resource-controls.js) (`TaskqResources` global or CommonJS) and [src/resource-controls.css](../../src/resource-controls.css), on the [answer-card delivery contract](answer-card.md#implementation). Input is one node/project group `{key, observed_at, labels: {host, project}, targets: [Projection]}`. Pure `policy`, `gate`, `usage`, `applied`, `fields`, `changes`, `status`, `step` and `render` hold all behavior; `mount(el, adapter).update(group)` keeps draft and focus. The adapter gives `capability(group)` = `{edit, reason, note}` and `send(group, op)` resolving to `{state, reason}`, with `op` = `{id, ops: [{target, policy_schema, expected_revision, changes}]}`: one contract change per edited target, untouched fields never travel. It reads policy, used/limit, blockers and freshness from the projection; it has no admission rule, parser or transport.
- Legacy versus qualified: without an advertised `taskq.admission/1` the summary reads "Legacy guard · Fixed limits" and a runtime limit needs a number (0 admits none); Auto/Fixed appears only when the producer advertises it. Explicit zero stays `off`, absent stays `none` (Fixed) or `no cap` (Auto). Saved versus applied compares current and effective revisions on a fresh observation; anything older than 60 s is unknown.
- Tray adapter: [src/tray.rs](../../src/tray.rs) maps `status.json` `capacity` and the saved host capacity into the group (host, project and per-runtime targets) and embeds the unchanged assets in Capacity. A write still goes through `service capacity`'s guarded edit; its `Saved`/`Error` reply echoes the operation ID; a missing reply stays unknown and **Check again** resends the same ID and fields. Revisions there are content hashes, so an edit-and-revert reads as unchanged; the target-host adapter's revisions replace them. `node tests/tray-resources.test.cjs` checks this boundary; `tests/tray-resources.browser.cjs` (Playwright instrument) checks both themes, 16/20 px, 280-360 px, over-capacity rows, draft/focus through a tick, conflict and review.
- Conformance: [tests/fixtures/resource-controls.json](../../tests/fixtures/resource-controls.json) covers legacy zero/absent, Auto without a count cap and its locked gate, over-capacity running work, stale metrics, several blockers, conflict and rebase, cancel, lost receipt with same-ID retry, saved→applied and absent status/read-only. `node tests/resource-controls.test.cjs` replays it; `cargo test --locked --test resource_policy_contract` checks every fixture target against the projection allowlist. Deferred to #954: Work mode target selection, Mixed, Auto/maximum writes and their fixtures; revoked authority is the read-only case until a remote adapter exists.
- Release: `taskq-<version>-resource-controls.js`, `.css` and `-fixtures.json` ship in each release, listed in `SHA256SUMS`. Checksums of this source, checked by the node test:

`src/resource-controls.js` SHA-256: `0ecd8c39d2ca282a8a6438e7f503307588ff6897f0ea0b51cd39c14391ac129a`  
`src/resource-controls.css` SHA-256: `9f81ec867aaf0cf95c4abbd5c37889632e1a6034b0e33331b52c41f92b836907`  
`tests/fixtures/resource-controls.json` SHA-256: `d06ba57ef922d2be6e6d84683ac09c4445cf5467fa45791d8a9e985643317b5a`  

B4 parent-to-child map: parent design `75757d5d843159726a2ea44ca1615f9cf7fd18fd` ([menu-next variants](menu-next-variants.html) `resourceEditor`, [menu M12](menu.md)); child tray Capacity places the shared component under its existing summary without restyling it. Deferred route: Work mode (#954).
