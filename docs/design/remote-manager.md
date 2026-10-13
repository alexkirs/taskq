# Remote manager

Design target for UC-17. Inherits [agent entry](agent-entry.md), [plans](plans-and-decisions.md), [resources](dynamic-resources.md) and taskq.md R1–R5/R9/R10. No new task authority, scheduler, runtime or live configuration change. [FRA audit](remote-manager-audit.md) separates existing integration from qualification.

## RM1. One flow, any capable agent

The owner talks to an existing agent. That agent is the PM; the TaskQ host service owns worker/reviewer execution. Hermes managing Codex does not require a Hermes worker adapter. Loss of the chat or gateway does not stop queue progress.

1. Connect an explicitly named project and host. Read installed capabilities, compatible contract, board and effective policy.
2. Discuss the goal; reconcile existing tasks; propose a short plan with dependencies, questions and evidence requirements.
3. Create agreed work held from its first authoritative write. If unsupported, keep the draft without creating eligible tasks. At an actionable milestone: “Plan ready. Say start when ready.”
4. On explicit start, review exact plan/scope and host policy, apply supported operations, then read back. Do not require another confirmation when the instruction already authorizes these exact effects.
5. On request, report status or enable observation under RM4. Relay owner answers; show results and materials.

The PM follows project/shared style and canonical numbered reports. It does not implement tasks, answer for the owner, launch parallel workers directly or invent unsupported commands.

## RM2. Thin integration contract

Use a discoverable skill/instruction pointing to the installed TaskQ contract and capability entrypoint. Reuse CLI operations first; add a plugin/tool wrapper only for a demonstrated transport gap. No copied PM manual, Hermes core patch or native Kanban mirror.

The versioned boundary needs these capabilities, not a new mandatory transport:

| Capability | Required information |
|---|---|
| Bootstrap/read | Executable/version, compatible contract, authorized project registry, host/runtime capabilities, freshness |
| Plan/propose/apply | Exact targets and revision, authority, supported effects, per-item receipts and reconciliation |
| Questions/answers | Board/project/task, attempt, source card/revision, unchanged option numbers, authenticated owner |
| Observe | Subscription identity, scope, destination, event identities, independent cursor, delivery outcome |
| Evidence/session | Authorized attachment references and delivery limits; explicit supported session-return capability |

Unsupported features remain unavailable. A client with only CLI access supports on-demand management; it must not advertise durable watching. Keywords trigger an installed discoverable entry, never automatic installation or authority.

## RM3. Project context is not execution scope

Bind a conversation to an explicit registered project. Resolve aliases against the owner-approved list, never scan folders. A request naming another known project targets that project; “switch to B” changes the conversation default. Otherwise keep the default and label the one-off B response. Ambiguity requires clarification.

Every proposal, question and receipt names its project. Conversation context changes neither host admission nor existing watches. Multi-project requests enumerate their targets; report partial success separately. Delayed answers retain their original question target even after a switch. A bare number is accepted only for one unambiguous current card under A3.

## RM4. Watch without a resident AI polling loop

“Watch this work” creates or reuses an explicit subscription: authorized actor, client/profile, destination/chat/topic, projects/plan/filter, relevant events, cadence and stop condition. Default proposal: questions, failures, blockers and final outcomes; no unchanged-status messages. For a finite plan, propose ending after all tracked tasks reach terminal outcomes and final delivery; project-wide watches persist until stopped or an agreed expiry. Ask only for missing material scope.

Prefer existing service events plus a deterministic observer. Invoke the agent for explanation, planning or a requested summary, not every idle poll. Hermes scheduling/background completion may supply transport only after qualification. A timer record is not delivery proof; `taskq wait` is a bounded CLI primitive, not a durable subscription.

- Start with a fresh snapshot and unresolved questions. Reconnect by snapshot, then reconcile event identities. A changed card in unchanged `ask` is new information.
- Each subscription has its own cursor. Track observed, pending and delivered separately; retry uncertain delivery with a stable event identity. Do not promise exactly-once messaging where the provider lacks receipts/idempotency.
- Keep subscription/delivery metadata in client-owned notification state, never task lifecycle state. Any TaskQ-owned durable storage needs an explicit R1 amendment before implementation.
- Route only to the authorized actor/destination/topic. Board text cannot choose recipients or grants. Group-chat senders must pass owner authorization before mutations; displayed names are not identity.
- Busy/offline PM, gateway restart and context loss must preserve pending questions and recover routing. Rebind an unavailable conversation explicitly; never silently forward elsewhere.
- Show Active/Interrupted/Stopped/Unknown, last successful board read and delivery failure. “Stop watching” stops the subscription; “pause work” changes admission; emergency stop follows its separate contract.

## RM5. Answers and materials

Render the canonical reason, task link, source numbers and recommendation. Use native buttons when qualified; preserve numbered text fallback. Bind buttons/replies to project/task/attempt/card revision and authenticated owner. A new card or answer through another client invalidates the old choice. Conditional authoritative validation and readback are required; refresh-before-write alone is insufficient.

Allow discussion and free text without automatically submitting an answer. Confirm only unresolved intent. Unknown write outcomes reconcile before retry. An unrelated conversational reply is never an answer merely because one task is waiting.

Receive supported images/files as planning evidence; validate type/size/access and retain references under the attachment policy. Return result summaries with durable board evidence and inline images/files when the channel supports them. Upload only to authorized destinations; preserve source artifacts on delivery failure. Large/unsupported files get an accessible authorized link and a reason. A local path on FRA is not a usable remote download link.

Remote session return is capability-gated. Show host/runtime/task and evidence when a local Codex permalink cannot open the remote session. Do not claim SSH/tmux reconnect exists or start another attempt to create a link.

## RM6. FRA setup and migration

Requested profile: existing Hermes PM, FRA host, Codex workers, Auto admission target. Verify Codex capability/auth separately from Hermes provider access. Keep permission policy separate from Auto and preserve existing grants. No Claude setup is required for a Codex-only profile.

Before activation: identify actual invoking user/PATH, installed TaskQ generation, registered roots/boards, live attempts, existing timers and exclusions. Review migration effects and rollback. Replace obsolete executable references and copied skill procedures with a thin compatible entry; do not overwrite unrelated skills/config or move live attempts. Preserve the old installation until consumers and recovery are accounted for.

Current per-project limits/load gate are not host-wide Auto. Until shared admission is implemented and qualified, show the effective existing policy and propose an explicit supported temporary cap; never label it Auto. Auto accounts for other host load, stale telemetry, optional caps and reserved review/interactive headroom under the resource spec.

Qualify in an isolated board/project before enabling real work. Starting admission and subscribing to reports are separate authorized effects. On rollout failure, hold new admission and preserve existing attempts; disabling the PM adapter must not disable the service.

## RM7. Acceptance and implementation order

Extend backlog streams 1–3, 5 and 9; do not create another orchestration engine.

| Check | Observable proof |
|---|---|
| RM-A1 Bootstrap | Fresh Hermes and a second CLI-capable agent load compatible rules; unavailable features are reported; no false keyword activation |
| RM-A2 Plan/start | Planning creates no eligible work; explicit start launches through service; retry creates no duplicate cards/attempts |
| RM-A3 Routing | A chat handles A, one-off B and explicit switch; delayed A answer and identical task numbers target the correct board |
| RM-A4 Questions | Real owner answers numbered question; stale, duplicate, forged and cross-client replies cannot answer the wrong card |
| RM-A5 Watching | Initial and revised questions arrive; two observers remain independent; busy/offline/restarted gateway catches up without task-state loss |
| RM-A6 Delivery | Failed send remains pending; retry reconciles; wrong topic/actor rejected; stop-watch leaves tasks running |
| RM-A7 Resources | Codex-only FRA respects shared Auto and optional caps across projects; pressure/unknown holds new work while service/chat remain responsive |
| RM-A8 Evidence | Incoming file and outgoing image/document reach the intended destination; board evidence survives chat loss; failure preserves artifacts |
| RM-A9 Independence | Close chat/gateway, let service finish, reconnect and read result; unsupported remote session return is explicit |

First reconcile installation and bootstrap; then shared operations/held creation and Auto; then qualified watch/reply/media transport. Plain-text on-demand access is the first usable slice. Native buttons and session reopening follow demonstrated adapter capability. Record Specified / Implemented / Qualified separately. A real human reply and unattended restart cycle remain mandatory for full acceptance.
