# Message format implementation handoff

Decision source: [taskq.md](../../taskq.md), R6/R7 and sections 1, 2, 4, 5, 7.
This is the implementation map for #773; no code changes ship with #772.

| Surface | Existing code | Work needed |
|---|---|---|
| Task body | `src/main.rs::command`, `add` | Validate title, Goal, Acceptance and Scope before board writes; preserve section order, stable A-numbers and dependency lines. |
| Ask card | `command`, `ask`, `numbered` | Validate literal Done/Problem input, option count and recommendation; place evidence before options and append the task-specific answer hint. |
| Result | `command`, `result` | Render Changed, one Checks line per supplied check, then Open; add the specified optional `--recommend`/`--link` support and append a full ask card only for a decision. Require Done/Problem in that case. |
| Close | `close_one`, `command`, `close` | Validate the labelled verdict; preserve every Acceptance outcome and exact-SHA CI evidence, then retain publication acknowledgement/readback. |
| Other task writes | `command`, `answer/steer/requeue/later/failed`, `print_task`, `answer_picks` | Validate newly supplied text, use the reason/instruction templates and shared success/error wording; preserve multi-item outcomes and fencing. |
| Status | `report`, `cell` | Extract Problem for the brief reason instead of flattening the entire card; cap visible titles/reasons/options with issue links, retain answer tokens and all rows. Keep Markdown escaping. |
| Service and other CLI | `service`, `run`, `wait`, `platform.rs` | Apply acknowledgements, error/Next lines and per-item diagnostics; retain raw service status JSON and original inbox/log output as diagnostic exceptions. Keep service focus/list field lines readable. |
| Tray/menu | `src/tray.rs`; approved menu renderer | Share R6 count names/order, short task/action labels, explicit stale/unknown reasons and full accessible text. Preserve approved navigation and platform constraints. |

Use one template/limit validator across CLI producers, without changing `core::fold`, states, labels, trusted-author rules or answer delivery. Do not rewrite old comments or truncate authored text. Add focused boundary/ordering tests and update affected renderer assertions only with the amended R6/R7 contract; keep lifecycle golden traces intact. Verify tray presentation on each implemented platform separately (R12).

Audit coverage:

- Section 1: body order, title/section limits and imperative style specified.
- Section 2: ask and result-card order, bounds, evidence, recommendation and answer hint specified; #771's merged hand-typed syntax preserved. Machine log headers, spawn/gone and heartbeats remain exact protocol text, not phone summaries.
- R6: all status groups retained; per-cell limits and linked full text specified. No total report limit: hiding tasks/options would hide decisions. Dependency lines remain one per dependency for the same reason.
- Section 4: tray/service labels, counts, task rows, stale/unknown reasons and lengths specified. Existing approved menu navigation and accessibility remain authoritative; this task does not redesign the menu.
- Section 5: task acknowledgements, refusal/error/action lines, free text, wait, batch outcomes, service fields, update/cleanup/setup/selftest and help covered. Raw inbox, log and status JSON remain uncapped to preserve diagnostic evidence. Exact commands, SHAs, paths and link targets are exempt from character caps to preserve verifiability.
- Section 7: result/close field order, line/character bounds, Acceptance and CI outcomes, open items and verbatim PM relay specified. Research answer bodies use the result template; detailed evidence may be linked.

These are approved design targets pending implementation, not claims that current CLI/tray output meets them. R6 and R7 are amended. No tests, golden traces or lifecycle rules changed.
