# Shared resource controls

Accepted target under [taskq.md](../../taskq.md), extending reuse of the [answer card](answer-card.md). TaskQ owns reusable domain components; DashQ supplies layout and authorized transport. No change to live limits, admission or permissions is made by this design.

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

## Acceptance and delivery

Split shared component/native adoption, target-host adapter, DashQ embedding, DashQ authorized operations and installed qualification. Reuse #896 for Auto and #913 for independent TaskQ enrollment; do not reopen answer-card #919 as a broad resource redesign. Resource control does not gate observation-only enrollment or ordinary answers.

Shared fixtures cover legacy and Auto modes, mixed targets, zero/absent limits, over-capacity running work, stale metrics, multiple blockers, cancel, conflicting edits, revoked authority, duplicate/lost receipts and pending→applied. Verify native/browser 16/20 px, both themes, narrow layouts and draft/focus retention. Live qualification changes a reviewed test policy on the actual target, observes matching application and restores the baseline; installed per-OS proof is distinct from fixtures.
