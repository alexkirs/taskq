# Wiki editorial inventory (#205)

Draft for root and user review. Nothing here is published. The proposed Wiki diff is in [samples](wiki-editorial-samples.md).

Read on 2026-10-08:

- Wiki `9ccb572258c14c0252ba74e2db6d1f288a5f12fb` (pages: Home, Required-settings, Known-issues, Cleanup-schedule, Atomic-reservation-before-worker-launch).
- Repository `ec0cd349946f89081a701283742b0db735162e94` (`main`, after PR #199).
- Gradus Caveman `62579538f05fb6b69a12449c1ebad9567d1fdecc`: `rules/core.md`, `rules/documentation.md`, `rules/notifications.md`, `examples/messages.md`.

Revision links use `https://github.com/alexkirs/taskq/wiki/<Page>/<sha>`. They stay readable after a page changes.

## Status check

Checked with `gh issue view` and `gh pr view 199` on 2026-10-08.

- #195: closed 2026-10-07; shipped `c5378c1`, `b54aff6`. Home still says "implementation pending": stale.
- #191: open, `q-review`. Code merged to `main` by PR #199 (`ec0cd34`, 2026-10-07T21:50Z). `taskq preflight --json` pins its source to Home `05cf6aa`.
- #186: open, `q-later`. Stage 1 page "Atomic reservation": accepted, implementation pending.
- #197: open, `q-review`. Page "Cleanup schedule": accepted, integration pending.
- #187: open, `q-later`. `taskq help` does not exist.
- #176, #177, #185: open, `q-review`.
- #198: open, `q-doing`. No #198 Wiki page exists at `9ccb572`.
- #206: open, `q-later`. It owns Gradus style integration.

## Findings and destinations

Each removal keeps the text reachable by a revision link and the issue.

- **Home 9-48** (four full specifications: #195, #191, #186, #187). Home must hold start and navigation only. Destination: draft Specifications index, one entry per specification. Each entry links the issue and the accepted revision.
- **Home 4, 7** (#197 and #186 stage 1 links). Destination: Specifications index. Both pages stay unchanged.
- **Home 41-43** (writing rule). Competes with Gradus Caveman. Destination: Home links Gradus. The old text stays at Home `d3cc01c`.
- **Required-settings 3-32** (#187 short start, scenario table, help outline). Not a setting. Destination: #187 entry in the index, with links to Home and Required-settings `d3cc01c`.
- **Required-settings 46, 48** (proposed PM modes under "Codex workers"). Not a setting. Destination: one line in the new PM section, links to #185, #186 and `docs/external-pm-tick.md`.
- **Required-settings 47** (local-command ACK). Confirmed requirement under the wrong heading. Destination: new section "PM: local-command ACK before worker launch".
- **Known-issues 12** (#159 investigation log in a symptom list). #159 is closed. Destination: #159 and Known-issues `1d9ce23`. The page keeps symptom and action.

Known-issues 3-11 and 13-16, Cleanup-schedule and Atomic-reservation: no change.

## Writing style authority

- Authority: Gradus Caveman, pinned at `62579538f05fb6b69a12449c1ebad9567d1fdecc`. TaskQ links it; it does not copy or redefine it.
- Persistent docs use `rules/documentation.md`: grammatical, plain English.
- #206 owns integration into contracts, briefs and templates. This task changes none of them.
- The earlier proposal of a "Writing" section in `taskq/contracts/taskq.md` is withdrawn.

## Repository sources (read-only, no change)

- `taskq/contracts/taskq-manager.md` § 3 "Reply to the owner": links only, Workers table and Board line once per pass.
- `taskq/tick.py` `TICK_PROMPT` v2: reply in the owner's language; one or two lines when nothing changed.
- `taskq/worker.py` brief: no style rule.
- Report schema v1 (`taskq preflight --json`, `report_contract.template`): required report and worker fields.

## Risks and unknowns

- The Wiki can change before publication. Recheck the base revision, then rebuild the diff if it moved.
- The Specifications page must be published before Home links it. GitHub answers 200 for a missing Wiki page, so a link check does not catch this.
- The Gradus link pins one revision. #206 decides whether to follow a newer revision.
