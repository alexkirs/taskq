# Shared answer card

Accepted target under [taskq.md § 2](../../taskq.md#ask-card). TaskQ owns the component and its conformance fixtures; DashQ consumes it. This is not a shipped UI claim. Apply shared [operator rules](https://github.com/taskqorg/specq/blob/main/design/operator-ui.md) and [AI-assisted controls](https://github.com/taskqorg/specq/blob/main/design/workflow.md#ai-assisted-controls).

Follow [shared-block inheritance B1–B4](shared-blocks.md). Compact placement does not authorize independent icons, tokens, counter layouts or simplified editors; the parent package governs visual fidelity.

## Ownership and delivery

One small browser-compatible component implements task question, source-numbered choices, recommendation, free text and receipt states. TaskQ tray and DashQ supply adapters for their existing bridges; the component owns no credentials, network client, board parser or task state machine. No UI framework, service or general plugin system.

Keep source, styles/tokens and fixtures in TaskQ. Publish versioned static assets with the TaskQ release and checksum; until a v* release carries them, the pin is a TaskQ commit plus SHA-256 (§ Implementation, #937 owner answer). DashQ pins that version/hash and serves them locally under its CSP. Vendored generated assets are permitted only with provenance and checksum, never independent edits. No runtime CDN, iframe of the tray, npm registry or moving-main dependency. A compatible component update must pass both consumer fixtures.

## Card and interaction

- Consume TaskQ's canonical parsed card/snapshot: board identity, task identity/link, state, question/card revision, original text/evidence, ordered choices and recommendation. Parse with TaskQ's own semantics, never a second parser in DashQ. Unparseable cards remain readable with their board link and no write controls, except the explicitly enabled tray-only fallback below.
- Keep state, linked issue number and title one flowing block. Reveal the question beside its task, with no extra conversation screen. Preserve Done/Problem/evidence information; don't repeat boilerplate answering instructions when controls convey them.
- Preserve choice IDs, order, text and recommendation. `★` has accessible name Recommended. Never sort the recommended choice first or renumber it.
- A normal choice submits that choice directly. A separate text field submits literal free text with TaskQ's current limits. Never infer a numbered choice from free text. No extra confirmation for an ordinary answer.
- Consequential operations, including a choice that TaskQ interprets as close/publication, stay visible in source order but unavailable in the dashboard with a reason and a board link. Do not silently remove or reinterpret them.
- Shared states: actionable, sending, accepted, stale, rejected, delivery unknown and read-only. Show one local receipt beside the question; preserve row, scroll, focus and typed draft through refresh. Acceptance is one board operation, not worker resumption, task completion or repaired service.
- Bind a draft to board/task/question revision. A changed question disables the old submission; show the new question and retain the old draft without auto-submitting or silently rebinding it. Require an explicit review of the new question before another send.
- One operation ID binds one immutable answer payload and expected revision. Double click and uncertain-delivery reconciliation retain that ID; changed text/choice is a new operation. Disable duplicate controls while sending. Unknown delivery never becomes accepted on timeout or triggers a new-ID retry.

Tray-only fallback (R6, #1021): `capability.manage` enables requeue (one-line reason, at most 232 characters), Later and confirmed Close only for `ask` with no canonical choices. Other consumers receive no new authority. Requeue sends `Reason: <literal reason>`; Close cancels without publication through `later` then `close`, with distinct stable operation IDs and the first readback revision fencing the second command. A failed/unknown step stops; a partial result names the parked task. Tray mounts cards only for ask, and retains operation receipts separately after a state change. Numbered close choices remain unavailable.

## Adapters and evidence

The adapter supplies answer capability/reason and invokes the canonical TaskQ dispatcher with actor, expected revision and operation ID. The component never emits board comments or decides authorization. An accepted receipt comes from TaskQ readback; failures retain producer reason/next action. Browser access and observation enrollment alone do not grant write permission.

Use the same fixtures in TaskQ and DashQ: choice and free text; long question; malformed card; review decision; read-only; duplicate click/lost response; stale revision; permission revoked; sending refresh; denied/unknown/accepted receipt. Check keyboard use, focus/drafts, 16/20 px, both themes and narrow layout. Pure UI tests cannot certify a real board write.

## Implementation

- Canonical fields: each `status --json` task row with a card adds `question`: `parsed`, original `text`, `done`, `problem`, `evidence` (label, URL), `choices` (`id`, `text`, `recommended`, `consequential` for an option starting with `close`) and free-text `limits`. `parsed` is false without Problem, 2-4 options and one `★`.
- Component: [src/answer-card.js](../../src/answer-card.js) (`TaskqAnswerCard` global or CommonJS) and [src/answer-card.css](../../src/answer-card.css). `mount(el, adapter).update([{board, row}])` keeps drafts, focus and scroll; the adapter gives `capability(board, row)` = `{answer, reason, manage?}` (manage is tray-only) and `send(board, op)` resolving to `{state, reason, next}`, with `op` = `{id, task, revision, choice | text}`; tray fallback adds `action: requeue|later|close` and Close requires `confirmed: true`. Pure `status`, `step` and `render` hold all behavior.
- Tray adapter: [src/tray.html](../../src/tray.html) embeds the unchanged assets beside task rows and keeps card instances through refresh/navigation. The existing IPC bridge in [src/tray.rs](../../src/tray.rs) validates registered project/board identity and invokes the owner CLI dispatcher with the exact operation ID and revision; only its readback receipt accepts an answer. A missing receipt stays unknown and reconciles with the same payload/ID. `node tests/tray-answer.test.cjs` checks this adapter boundary.
- Conformance: [tests/fixtures/answer-card.json](../../tests/fixtures/answer-card.json); `cargo test` checks its `question` fields against TaskQ's parse, and each consumer runs `replay(case, capability)` over it (`node tests/answer-card.test.cjs`). `tests/answer-card.browser.cjs` checks 16/20 px, both themes, 280/480 px, refresh and keyboard with Playwright.
- Release: `taskq-<version>-answer-card.js`, `.css` and `-fixtures.json` ship in each release, listed in `SHA256SUMS` with build provenance. DashQ pin (no v* release carries the assets yet, latest v4.1.11): vendor these three files from TaskQ commit `8715e1d2a63f725a290549234c720204d8234413` (#942, last change to all three) with that commit and these checksums as provenance; `git show <commit>:<file> | shasum -a 256` reproduces them. The historical pin above does not include the tray-only #1021 fallback. Checksums of the current source, checked by the node test:

`src/answer-card.js` SHA-256: `35662bb6d71975e54345097b51aa871db7c8b81c6a137faa1c1c2d72b24b8b92`
`src/answer-card.css` SHA-256: `b33ea8d5ba6ec70931ecc2cac745bf1808181b4939019bf478f09cdd7791a796`  
`tests/fixtures/answer-card.json` SHA-256: `dd98bda612d83617685c504637dd6a995180f88a3c0a8944d067633422eee0a5`

Parent source: TaskQ `531f68e2c6f971eb56c392e623e90721e674f346` supplies the JS/CSS/fixtures; design inheritance is pinned to `cf46e637d45f971dc83f1c1ea5f1e9a119a74256` (#943, K-A8 right alignment). The tray places the shared card beside its existing row without restyling it. This adapter fixes same-ID reconciliation when refresh removes an answered question; the updated JS/fixture hashes above identify that shared fix.

B4 parent-to-child map, design `cf46e637`:

| Parent | Child | Placement differences | Evidence |
|---|---|---|---|
| [visual-style.md](visual-style.md) palette | `.qac` tokens and the tray body tokens | none | `tests/answer-card.parent.cjs` computed colors |
| `question()` in [plans-and-decisions.html](plans-and-decisions.html), `qcard` CSS of [workspace-preview.html](workspace-preview.html) | `render()` and `src/answer-card.css` | Head flows `? #N title` per M3; host/runtime stay on the tray row above; fixture state preview omitted | divider, mark, choice fill/shape/numbers, wrap, Other answer and Checked & context disclosures equal |
| Q1 states | `STATES` receipts | Recorded is "Answer accepted on the board" after readback | fixtures, `tests/tray-answer.browser.cjs` |
| K-A8 project strip | tray project header | unchanged tray counters | right edge, order, no overflow at counts 0/1/12/100 |

Deferred: Discuss with AI stays with the AI workspace route. The [#938 project-block extraction](project-blocks.md) fixes zero capsules (K-A2) and merged Doing/Review (K-A1); its release and native qualification remain separate. `PLAYWRIGHT_CHANNEL=chrome node tests/answer-card.parent.cjs` compares both themes, 16/20 px and 320/360 px; it is an instrument, not native qualification.

Delivery of #920 is the adapter PR first, per the owner's answer on the task. The commit/checksum pin above is #937's release evidence; native tray interaction is qualified on the next real ask card answered from the macOS tray (#937 owner answer), not on a synthetic task. Browser and dispatcher tests do not certify native interaction, a real board answer or worker resumption; README screenshot freshness remains a separate native capture gate.

## Accepted reference

[Interactive reference](answer-card-reference.html) uses the existing TaskQ design example, with synthetic task #841 and counts. It approves compact placement and ordinary choice/free-text/receipt interaction, not its mocked transport, invented state or approximate icons/tokens/counters. Shared-block B1–B4 takes precedence over those demo details. It does not replace the failure-state fixtures above.

Reference SHA-256: `4b857925b7a606f1766b84f2b0e79b36fb26bc1203d8a64a51ccdc29a321c556`.
