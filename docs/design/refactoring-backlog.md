# Refactoring backlog and API contracts

Implementation plan for the accepted product, not runtime changes. Use [system change map](system-change-map.md), [decision register](decision-register.md) and canonical taskq.md. Keep existing adapters and deterministic scheduling; add only the missing contracts. Each row becomes one or more reviewable implementation tasks after current-source reconciliation.

| Order / workstream | Build or refactor | API boundary to specify | Acceptance / dependency |
|---|---|---|---|
| 1. Canonical read model | One projection for CLI, tray and agents; eliminate independently computed states/counts | Snapshot: schema, source/host, observed time, completeness, canonical issue identity, state totals per project and aggregate, capabilities | Deduplicate registrations; zero vs unknown; per-project filters; closed pagination; required by all clients |
| 2. Operations and answers | Shared validated operation dispatcher and board receipts | Request: operation ID, authenticated actor, exact target, expected revision/attempt/card, payload, reviewed effects. Response: accepted/readback, rejected, stale/current, unknown/reconcile ID | Concurrent answers, partial writes, timeout/retry and stale revisions; no invented forge transaction |
| 3. Safe plan creation | Hold before eligibility; grouping/dependencies and explicit start | Draft plan revision, existing/new card mapping, per-item creation receipt, held task set, start proposal; cycle/missing-edge validation | Close add --later admission window; retries do not duplicate; requires 2 and lifecycle model |
| 4. Recipes and gates | Reusable project workflows over tasks; meaningful outcomes become cards | Recipe/version and project commands, evidence requirements, owner gate bound to exact revision, invalidation and retry | No separate scheduler; required gate blocks effects after restart; requires 2–3 |
| 5. Host resources | Host-wide admission reservations, Auto and optional intersecting caps | Host policy/revision, runtime enablement, fresh telemetry/window, effective limits and reasons, reservation identity/lifecycle | Two projects cannot consume the same headroom; pressure/unknown holds; reviewer/operator liveness; compatibility gate |
| 6. Access and emergency | Operator-owned grants, qualified boundary, durable hold/recovery | Grant provenance/scope/revision, requested vs effective policy, stop operation, attempt liveness, recovery proposal/receipt | Same-user Full limitation explicit; no board grant escalation; no restart until fencing; OS negative tests |
| 7. Delivery | Separate commit/push/PR/merge choices from permissions and completion | Project policy revision, exact candidate SHA, check/review evidence, owner approval, publication receipt | Stale SHA invalidates approval; partial publication reconciled; preserve existing publish semantics during migration |
| 8. Multi-repo environment | Pinned repos and owned prepare/check/finalization lifecycle | Environment identity, revisions, resource ownership, compatible outputs, terminal attempts, cleanup receipt | Failure/cancel still finalizes safely; preserve work; no atomic multi-repo merge claim; needs real repo fixture |
| 9. AI and client adapters | Menu PM operations, bootstrap, conversations/files, canonical reports and session links | Capability catalogue; typed proposals; conversation/request/attachment identity; independent observer cursors; qualified return/reply operations | Menu-only headless workflow; unknown keyword does not grant tools; no duplicate sessions; provider limits visible |
| 10. Shared icon renderer | Summary preset and Custom on one schema/renderer; agent draft tools | Versioned primitive/font/data catalogue, config revision/hash, render/validate report, exact approval/save/readback | Same config gives same pixels; invalid draft preserves icon; native DPI/theme/overflow and restart proof |
| 11. Native interface | Stable menu + workspace; project counters, settings, recovery and accessibility | Read-only snapshots plus shared operations above; native window owner and preference state | Full feature ledger; Mac/Windows work-area sizing, 16/20px, keyboard/screen reader; no browser-only completion claim |

## Common API rules

- Prefer extending existing CLI/service operations over adding a parallel HTTP server or agent bus. Select transport after inspecting current clients; domain semantics must not depend on transport.
- Version schemas/capabilities. Clients show unsupported actions explicitly. No fabricated CLI names in implementation docs until commands exist.
- Reads are side-effect free. Mutations require exact scope, revision and authority at the authoritative boundary; client refresh is not race protection.
- Machine output is structured; concise human output is rendered from the same result. Preserve source answer numbers, error code/reason, retryability, partial outcomes and freshness.
- Idempotency survives the relevant retry/restart window under the board contract. Never add a hidden local task authority to simulate transactional writes.
- Events carry entity/revision identity; each observer has its own cursor. Reconnection requests a snapshot before applying incremental changes. Watching never auto-answers.
- Secrets stay in established credentials storage, never snapshots, model prompts or icon configs. Project content cannot raise host grants or resources.
- Plan N/N-1 migration and rollback before enabling a new lifecycle. Existing explicit policy remains until reviewed migration; no silent conversion from pr to Review PR.

## Per-task handoff

Name affected R/section rules; identify current implementation and callers; amend contract/model first; define smallest implementation plus fault tests; retain accepted UX; report Specified / Prototyped / Implemented / Qualified separately. Native evidence includes OS, DPI, font, artifact SHA and actual screenshots.

## Remaining inputs, not repeated preference votes

Actual five-repository build graph and owned lifecycle commands; supported client/runtime matrix; native font/size calibration and load measurements. Transport, reservation ownership, durable receipts and compatibility are engineering decisions to resolve with evidence. Escalate only a newly discovered product trade-off.
