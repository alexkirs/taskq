# Wiki editorial inventory (#205)

Draft for user review. Read-only inventory; no Wiki, contract or template change. Bulk migration waits for review.

Read on 2026-10-08: Wiki `d3cc01c131d9d85b565d7c88f81757c03caa6bbb`, repository `2609a3cc8a3a7f7f52620ee5395e30588da792ff`.
Wiki revision links: `https://github.com/alexkirs/taskq/wiki/<Page>/<sha>`.

## Sources

| Source | Version | Lines | Content |
|---|---|---|---|
| Wiki Home | `d3cc01c` | 1-4 | Entry and navigation. Keep. |
| | | 6-13 | #195 specification. |
| | | 15-23 | #191 specification. |
| | | 25-34 | #186 critical path. |
| | | 36-45 | #187 writing rule and help proposal. |
| Wiki Required-settings | `d3cc01c` | 3-32 | #187 short start, scenario table, help outline. |
| | | 34-44 | Claude and Codex settings. Keep. |
| | | 46-48 | #188 PM mode notes under "Codex workers". |
| | | 50-51 | `limit.*` notes. Keep. |
| Wiki Known-issues | `1d9ce23` | 3-11, 13-16 | Symptom and action bullets. Keep. |
| | | 12 | #159 Windows investigation log. |
| `taskq/contracts/taskq-manager.md` | `2609a3c` | 689-694 | "Reply to the owner": links, Workers table, Board line. |
| `taskq/contracts/taskq.md` | `2609a3c` | all | Worker contract. No writing rule. |
| `taskq/worker.py` | `2609a3c` | 17-41 | Generated worker brief. No writing rule. |
| `taskq/tick.py` | `2609a3c` | 75-80 | `TICK_PROMPT` v2: "Reply in the owner's language, one or two lines when nothing changed." |
| `docs/wiki-sot-process-proposal.md` | `2609a3c` | 7, 30 | #189: Home links short spec pages; Required-settings holds settings only. |
| `docs/pages-gate-qualification.md` | `2609a3c` | all | #195 qualification record. |
| `docs/external-pm-tick.md` | `2609a3c` | all | External PM and TICK guide, #181 evidence. |

## Status check

Checked with `gh issue view` on 2026-10-08.

| Item | Wiki says | Actual | Action |
|---|---|---|---|
| #195 | "Accepted specification; implementation pending" (Home 8) | Closed 2026-10-07T19:53Z. Shipped `c5378c1`, `b54aff6`. | Mark implemented. Wiki is stale. |
| #191 | Accepted, implementation pending | Open, `q-review`. No commit on `main`. | Keep "accepted, in review". |
| #186 | Accepted, qualification pending | Open, `q-later`. | Keep "accepted, later". |
| #187 | Proposed help command | Open, `q-later`. `taskq help` does not exist (exit 2 at `2609a3c`). | Keep "proposed". |
| #185, #176, #177 | Pending qualification | Open, `q-review`. | Keep. |

## Duplicates and relocation

Relocate first, then remove. Each removed block stays reachable by a Wiki revision link and its issue.

| Block | Problem | Destination | Main page keeps |
|---|---|---|---|
| Home 6-13 (#195) | Stale status. Execution detail duplicates `docs/pages-gate-qualification.md`. | #195 and the qualification doc. Text at Home `18038ed`. | One status row. |
| Home 15-23 (#191) | Full spec on the entry page. | Own Wiki page, or #191 issue. Text at Home `05cf6aa`. | One status row. |
| Home 25-34 (#186) | Full spec on the entry page. | Own Wiki page, or #186 issue. Text at Home `6b9027b`. | One status row. |
| Home 36-41 (rule) | Second writing rule. Differs from the #205 wording. | Single authority, see below. | One link. |
| Home 42-45, Required-settings 3-32 (#187) | Onboarding proposal in the settings page. #187 owns it. | #187 issue. Text at Required-settings `d3cc01c`. | One link to #187. |
| Required-settings 46, 48 | PM mode proposals under "Codex workers". Not a setting. | `docs/external-pm-tick.md`, #185, #186. | One line, status "proposed". |
| Required-settings 47 | Confirmed preflight ACK requirement. Wrong heading. | Same page, new "PM" heading. | Short bullet. |
| Known-issues 12 (#159) | Investigation log in a symptom list. | #159 issue. | Symptom, action, link. |

No other duplicate source found. The tick reply rule exists in manager contract § 3 and `TICK_PROMPT`; a test keeps `TICK_PROMPT` equal to § 2.

## Single authority for the writing rule

Proposed text (from #205):

> Action -> command -> result. State the material blocker, risk or unknown when needed. Short sentences. Clear words. Normal spaces. No filler or repeated context. Terse Russian and English must remain understandable.

Recommendation: the authority is a new short section "Writing" in `taskq/contracts/taskq.md`.
Reason: workers and PMs both read it offline through `taskq contract`.

Links, no copies:

- Wiki Home: one line linking the contract section.
- `taskq-manager.md` § 3 "Reply to the owner": one line linking the section.
- `worker.py` brief: one line naming the section.
- `TICK_PROMPT`: no change. It already points to § 3.

Decision for the user: #189 made the Wiki the normative source. This recommendation puts the rule in the packaged contract instead. Alternative: the Wiki holds it, and the contract links the Wiki. Then offline workers cannot read the rule.

## Boundaries

- #187: onboarding and two-level help. This task does not write help text.
- #198: context hygiene.
- One PM or separate TICK: later architecture. Not touched.
- Old chat and issue messages stay as written.

## Unknowns

- Wiki has no link checker in CI. A link check runs manually at migration.
- `contract_news` hashes only `taskq-manager.md` (`taskq/tick.py` 89-90). A running PM is not told about a `taskq.md` change.
