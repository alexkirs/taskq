# Tasks from findings

Accepted product target under [Product](../../taskq.md#product). Extends [shared blocks](shared-blocks.md); current queue transitions remain governed by taskq.md. This is not evidence of a released creation adapter.

## F1. Review and create

A consumer opens the TaskQ-owned task composer with an editable title, goal, evidence and acceptance. Preselect a project only from an explicit owner mapping; show its provenance and permit an override. Never infer ownership from the producer name. Use canonical project rows, counters, provider assets, task rows and answer cards; preserve their independent navigation and source choice identities.

Show one concrete task and its destination before submission. The default is **Create task only**, held without execution. **Create task & start** explicitly admits the reviewed work under existing focus, permissions and limits. Creating or starting an investigation grants no broader repair, deletion, restart or spending authority. Consequential proposals retain the existing review boundary. Preserve drafts on cancel, denied access, stale input and unknown outcomes.

Attach bounded, sanitized source evidence and a stable reference (producer installation, rule and episode). Keep paths, raw logs, credentials and arbitrary source instructions out of automatic attachments. Treat evidence as data, not executable instructions. Source links require authorization. Edits must preserve the reference used for reconciliation.

## F2. Canonical operation

TaskQ owns create/optional-start validation and dispatch, using existing board operations and lifecycle rather than another state machine. Extend the existing operation ID/receipt contract to creation. Bind one immutable request to actor, board, content and source episode; a repeated ID with changed content is rejected. An accepted receipt identifies the created issue only after authoritative board readback. A partial create/start outcome returns the existing issue and separate admission result; do not create another issue.

Unknown means reconcile that same operation read-only; never blindly create again. If a linked task for the episode exists, open it instead of offering duplicate creation. Cross-client recovery and concurrent submissions must reconcile against board-owned evidence, not only consumer memory; R1 remains binding. No new TaskQ database or machine block in issues. If the forge cannot prove absence after an ambiguous write, keep the operation unknown rather than risking a duplicate.

Observation enrollment and answer permission do not authorize creation/start. Consumers require explicit action capability and board authorization. Missing write capability leaves the review readable with a precise reason. Disabled or unavailable controls must not imply a write succeeded.

## F3. Work and recovery are independent

TaskQ reports task state from its canonical snapshot. A closed task does not prove the observed condition recovered, nor does a source recovery automatically close or reopen a task. The source owns verification; DashQ's [finding handoff](https://github.com/taskqorg/dashq/blob/main/docs/design/finding-tasks.md) owns placement, freshness and incident correlation. Answers reuse the shared card; an answer receipt does not prove worker resumption.

## F4. Component and acceptance

Extract the reviewed composer/review primitives into TaskQ's versioned shared assets; DashQ supplies placement and its authorized adapter. The accepted handoff form is a new TaskQ child, not an already shipped component. Parent design is `75757d5d843159726a2ea44ca1615f9cf7fd18fd`; qualify any successor. The [DashQ reference](https://github.com/taskqorg/dashq/blob/main/docs/design/finding-task-reference.html) includes the provenance manifest and fixture-only adaptations.

- A1: project selection/override, title/goal edits, held default, explicit start and cancellation preserve the reviewed target and scope.
- A2: accepted, denied, lost response, concurrent duplicate and create-success/start-failure reconcile to at most one issue. Replay after restart uses the same operation; changed payload is rejected.
- A3: shared row/card/counters match the parent at 16/20 px, narrow width, both themes and long titles; answer drafts survive refresh.
- A4: real forge create/readback, authorization and failure recovery must qualify before release. Browser simulations prove neither board writes nor execution.

Delivery: #948 canonical create/recovery; #949 shared component depends on #948 and #938. DashQ #53 consumes the released component and operation contract; observation work stays independent.

## Local CLI delivery

The owner-approved #948 scope is the authenticated local CLI on its configured board. Remote consumer create/start grants and the shared composer remain separate prerequisites. The binding syntax, claim and receipt rules live in [taskq.md §1](../../taskq.md#1-tasks).

Read the destination from `taskq status --json` (`board.url`), then submit the reviewed task:

```text
taskq finding "Investigate repeated failures" --board https://github.com/owner/project/issues --source installation/rule/episode --goal "Explain the observed failure." --acceptance "Report the cause with evidence." --evidence "Three failed checks in this episode." --json
```

The default creates held work. Add `--start` only when the reviewed request explicitly authorizes admission; its receipt is separate from create. Keep that choice and all request fields unchanged on replay. To start a task created with the default choice later, use `taskq start N --op-id ID`. The source IDs are opaque identifiers, not paths or links; evidence is reviewed, sanitized text supplied by the owner, never collected by this command. Preserve the draft in the calling client.

The same board/source derives the same operation ID across local clients and restarts. An unknown claim/create reconciles read-only and may remain unknown permanently. No consumer enrollment or answer grant authorizes this CLI. Real-forge ordering, create/readback and failure qualification are still required before release; deterministic adapter checks do not establish that qualification.

## Shared composer delivery (#949)

`src/finding-composer.js` and `.css` implement the shared review child. Load the pinned project-blocks and answer-card JS/CSS first, then call `TaskqFindingComposer.mount(element, adapter).update({finding, projects, linked?})`. The host supplies its inherited font, color scheme and available width; it never restyles the three components' internals. This extraction adds no tray entry, remote write grant or transport.

`finding` is `{source, revision, mapping?, title, goal, acceptance, sanitizedEvidence}`. `source` is the CLI's immutable installation/rule/episode identity; `revision` changes when source evidence changes. Acceptance is one item per line. Only `mapping: {project, provenance}` from explicit owner mapping preselects a project; otherwise the owner chooses. Project objects use [project-blocks](project-blocks.md). A supplied `linked: {project, row}` must be the adapter's authoritative correlation for this source; it replaces creation with the canonical task row and answer card. Source recovery is never inferred from its state.

The adapter provides:

- `capability(project) -> {create, start, reason}` and `send(request) -> Promise<{create, start}>`. Request is `{board, source, title, goal, acceptance: string[], evidence: string[], start}`; receipts are the canonical finding CLI receipts. The adapter revalidates actor, exact board, capability and content at dispatch. `start` is false by default; a missing grant leaves review readable with a reason. A browser flag is never authorization.
- `reconcile(request)` is **read-only** and receives the same frozen request after unknown delivery. Never implement this by blindly replaying a writing CLI operation: `finding --start` can retry admission. If a safe read-only adapter is unavailable, omit it; the control stays unavailable with an explicit reason. A partial start receipt keeps the created issue. New remote grants and real-forge qualification remain #948's separate release prerequisites.
- `navigation: {project, tasks, open}` uses the existing independent project/counter/link routes. `answer` is the unchanged [answer-card adapter](answer-card.md#implementation).
- Optional `saveDraft(source, draft)`, `loadDraft(source)` and `cancel()` let the authorized host retain its own draft across closing/reloading; without persistence, retention lasts for the mounted component. Save the full immutable request and receipts, never only edited text. Restored in-flight requests become unknown and cannot be submitted again. These drafts are not task truth; cross-client deduplication remains board-owned.

Only `sanitizedEvidence` enters the editable evidence field; extra source properties, paths, logs and source links are not automatically attached. The producer must sanitize this bounded text before supplying it. The owner reviews destination/scope/evidence before sending; edits reset that acknowledgement. Escaping prevents markup execution, not secret detection. A source revision change preserves edits and exposes the new sanitized observation for explicit review. After submission, payload fields stay frozen, including on denial: the board may already hold the immutable claim. Reconciliation never changes that payload or submits a second create.

### Provenance and acceptance

The [manifest](finding-composer.sources.json) records component version `1`, design parent `75757d5d843159726a2ea44ca1615f9cf7fd18fd`, the immutable DashQ form reference and hashes of actual composer/shared assets. Until a v* release carries these assets, use the delivery commit plus the manifest's SHA-256, following the existing answer-card pin rule; schema/component version alone is not a release pin. Release staging adds `taskq-<release>-finding-composer.{js,css}`, `-fixtures.json` and `-sources.json` to the existing checksum/attestation path. Consumers must pin all shared dependencies as well; no moving-main or CDN imports.

| Parent | Child | Permitted adaptation |
|---|---|---|
| #938 project-blocks summary/counters | Exact `TaskqProjectBlocks.summary()` | Separate destination radio; project, Board and counter routes remain independent |
| Accepted finding form `editor()` | Title, scope, evidence, acceptance and after-confirmation review | Editable bounded evidence/acceptance, full-height text fields, explicit review acknowledgement, capability reasons and canonical receipts replace demo simulation |
| #938 task row / #920 answer card | Exact `task()` / persistent `mount().update()` | Placement beneath the review; linked task prevents duplicate creation; answer drafts/focus survive refresh |

`node tests/finding-composer.test.cjs` is the CI chain check for bounded reviewed payloads, ownership, sanitized-field allowlisting, receipt validity and asset provenance. `node tests/finding-composer.browser.cjs <output.html>` creates a local self-checking page: open it in Chrome and require all eight PASS results (16/20 px, 320/360 px, light/dark). It compares embedded project styles with the pinned standalone component and exercises independent navigation, override/held default, cancellation, stale/denied input, duplicate clicks, remount/reload, same-request reconciliation, partial start and answer draft/focus retention. Inspect the remaining long-content forms visually. Browser fixture acceptance proves neither a real forge write, native tray integration, release publication nor consumer adoption.
