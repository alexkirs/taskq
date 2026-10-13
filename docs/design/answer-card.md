# Shared answer card

Accepted target under [taskq.md § 2](../../taskq.md#ask-card). TaskQ owns the component and its conformance fixtures; DashQ consumes it. This is not a shipped UI claim. Apply shared [operator rules](https://github.com/taskqorg/specq/blob/main/design/operator-ui.md) and [AI-assisted controls](https://github.com/taskqorg/specq/blob/main/design/workflow.md#ai-assisted-controls).

## Ownership and delivery

One small browser-compatible component implements task question, source-numbered choices, recommendation, free text and receipt states. TaskQ tray and DashQ supply adapters for their existing bridges; the component owns no credentials, network client, board parser or task state machine. No UI framework, service or general plugin system.

Keep source, styles/tokens and fixtures in TaskQ. Publish versioned static assets with the TaskQ release and checksum; DashQ pins their version/hash and serves them locally under its CSP. Vendored generated assets are permitted only with provenance and checksum, never independent edits. No runtime CDN, iframe of the tray, npm registry or moving-main dependency. A compatible component update must pass both consumer fixtures.

## Card and interaction

- Consume TaskQ's canonical parsed card/snapshot: board identity, task identity/link, state, question/card revision, original text/evidence, ordered choices and recommendation. Parse with TaskQ's own semantics, never a second parser in DashQ. Unparseable cards remain readable with their board link and no write controls.
- Keep state, linked issue number and title one flowing block. Reveal the question beside its task, with no extra conversation screen. Preserve Done/Problem/evidence information; don't repeat boilerplate answering instructions when controls convey them.
- Preserve choice IDs, order, text and recommendation. `★` has accessible name Recommended. Never sort the recommended choice first or renumber it.
- A normal choice submits that choice directly. A separate text field submits literal free text with TaskQ's current limits. Never infer a numbered choice from free text. No extra confirmation for an ordinary answer.
- Consequential operations, including a choice that TaskQ interprets as close/publication, stay visible in source order but unavailable in the dashboard with a reason and a board link. Do not silently remove or reinterpret them.
- Shared states: actionable, sending, accepted, stale, rejected, delivery unknown and read-only. Show one local receipt beside the question; preserve row, scroll, focus and typed draft through refresh. Acceptance is one board operation, not worker resumption, task completion or repaired service.
- Bind a draft to board/task/question revision. A changed question disables the old submission; show the new question and retain the old draft without auto-submitting or silently rebinding it. Require an explicit review of the new question before another send.
- One operation ID binds one immutable answer payload and expected revision. Double click and uncertain-delivery reconciliation retain that ID; changed text/choice is a new operation. Disable duplicate controls while sending. Unknown delivery never becomes accepted on timeout or triggers a new-ID retry.

## Adapters and evidence

The adapter supplies answer capability/reason and invokes the canonical TaskQ dispatcher with actor, expected revision and operation ID. The component never emits board comments or decides authorization. An accepted receipt comes from TaskQ readback; failures retain producer reason/next action. Browser access and observation enrollment alone do not grant write permission.

Use the same fixtures in TaskQ and DashQ: choice and free text; long question; malformed card; review decision; read-only; duplicate click/lost response; stale revision; permission revoked; sending refresh; denied/unknown/accepted receipt. Check keyboard use, focus/drafts, 16/20 px, both themes and narrow layout. Pure UI tests cannot certify a real board write.

## Accepted reference

[Interactive reference](answer-card-reference.html) uses the existing TaskQ design example, with synthetic task #841 and counts. It approves compact placement and ordinary choice/free-text/receipt interaction, not its mocked transport or invented state. It does not replace the failure-state fixtures above.

Reference SHA-256: `4b857925b7a606f1766b84f2b0e79b36fb26bc1203d8a64a51ccdc29a321c556`.
