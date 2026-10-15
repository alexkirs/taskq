# Contract audit before injection

Audit of `taskq.md` (task #1158) before #1156 injects the whole contract into every attempt.
It applies specq [Documentation](https://github.com/taskqorg/specq/blob/main/caveman/rules/documentation.md) ("prefer a link to one canonical rule over copied policy") and [spec first](https://github.com/taskqorg/specq/blob/main/engineering/spec-first.md) ("link shared rules; do not copy them", "keep design targets distinct from shipped behavior").
This page proposes edits only. It changes no contract text; each edit task below follows R13 and edits `taskq.md` itself.

## Measured state

- `taskq.md` at the audited commit `9c0f93f`: 300 lines, 98,270 bytes with LF line ends (about 25k tokens). The next `main` commit (#1157, `prefer-*` labels) adds 570 bytes to § 1, § 4 Filter, § 5 and § 9; the verdicts below do not change.
- `docs/`: 70 Markdown files; an index of path plus first heading is 4,530 bytes.
- specq `main`: 35 Markdown files, about 91 KB (#1156 measurement); four are injected today.
- Eight lines hold 25 KB, a quarter of the file: Product line 82, § 4 Tray, AI requests, Incident reports and Log and board budget, § 8 Access, § 8 codex-app environment and § 8 step 5.

## Constraints on every edit

- **§ 2 table.** `every_table_row_has_a_golden_trace` (`src/core.rs`) parses the rows after `| State | Entry |`. Keep the table in `taskq.md`, keep its header text, and keep every `(state, entry)` pair covered by `tests/golden/fold.json`.
- **300-line test.** The same test asserts at most 300 lines and no `YYYY-MM-DD` date. Line count is not the size problem: long lines are. A byte bound is a separate follow-up (task 7).
- **Names that code and docs cite.** R-numbers keep their numbers (Change rule). Code comments cite `§ 3`, `§ 4 Data`, `§ 4 Log and board budget`, `§ 8 Access`, `§ 8 Folders`, `§ 9`; `AGENTS.md` links `#executable-behavioral-model` and `#shared-project-bootstrap-600`. Keep section numbers and these headings, or update the citations in the same diff.
- **Contract hash.** `src/incident.rs` hashes `taskq.md` into the incident fingerprint, so every edit changes it. This is expected and needs no action.
- **Spec first.** Shipped decisions stay in `taskq.md`. Only three kinds of text leave it: copies of specq rules (replaced by a link), design targets that are not shipped (moved to their design page), and implementation or evidence detail that a design or qualification page already owns.

## Verdict per section

Sizes are UTF-8 bytes of the section including its heading. "Target" is the size after the proposed edit.

| Section | Before | Target | Verdict | Reason |
|---|---:|---:|---|---|
| Title and scope | 328 | 300 | keep | Purpose and version in three lines. |
| Change rule | 349 | 250 | compress | "At most 300 lines" repeats R8; keep it in one place. |
| R1 Board is the only state | 898 | 500 | compress | The list of local non-task files repeats § 3/§ 4 Data; refer to § 4 Data. |
| R2 One task, one attempt | 399 | 400 | keep | Short; links shared task preparation. |
| R3 Roles | 1,193 | 900 | compress | Operator line restates approval and evidence rules from agent-workflow and verification. |
| R4 Comments move the queue | 263 | 260 | keep | Core mechanism, already minimal. |
| R5 Completion on the task | 265 | 265 | keep | Already a link to verification. |
| R6 Human report | 2,316 | 1,500 | compress | Tray/browser reuse sentence belongs to `project-blocks.md`; report order and limits stay. |
| R7 Style | 1,028 | 450 | compress | Lines 47–49 restate specq Core and Change communication; line 44 is a design-target note that R13 covers. |
| R8 Contract is SoT | 305 | 200 | compress | Restates spec-first; keep only the golden-trace and same-release rule. |
| R9 Model, effort, permissions | 998 | 850 | compress | "Tools are capability, not authority" restates permission-authority; owner keys and legacy flags stay. |
| R10 Multi-project | 244 | 244 | keep | Project-only rule, minimal. |
| R11 Retire sessions | 581 | 580 | keep | Project-only lifecycle rule. |
| R12 Unverified means unknown | 1,484 | 600 | compress | Browser-launch and screenshot policy is test infrastructure detail for `ui-acceptance.md`. |
| R13 Spec first | 1,029 | 500 | compress | Security-report text copies `SECURITY.md` and specq security; one link suffices. |
| Product | 7,171 | 2,000 | link-out | Lines 71–73 and 82–84 are accepted design targets, not shipped behavior; README and look decisions stay. |
| § 1 Tasks | 5,441 | 3,800 | compress | Create claim and Finding CLI are shipped but wordy; keep the rules, cut restated reasoning. |
| § 2 Log and states | 5,434 | 5,000 | keep | Behavioral truth and the golden-trace table (constraint); compress bullets only. |
| § 2 Ask card | 1,912 | 1,200 | compress | The shared UI target and tray exception (line 138) belong to `answer-card.md`. |
| § 2 Executable behavioral model | 452 | 350 | keep | Anchor cited by `AGENTS.md`; already a link plus TaskQ sources. |
| § 3 Attempts, lease, capacity | 5,111 | 3,500 | compress | OS API names of the load gate belong to `capacity.md`; admission and hold rules stay. |
| § 4 Service (loop and bullets) | 23,955 | 9,000 | compress | Tray and AI request bullets repeat `menu.md` M3–M14; incident detail repeats `incidents.md`. |
| § 4 Profiling | 2,650 | 800 | compress | Store, budget and report rules copy specq profiling; keep the six operations, lock and `taskq profile`. |
| § 5 Commands | 7,951 | 6,000 | compress | Table stays; output bullets restate Core (preserve evidence, verbatim strings). |
| § 6 Setup | 646 | 300 | compress | The setup target is not shipped; one link to `setup-and-project-locations.md`. |
| § 6 Shared project bootstrap | 742 | 600 | keep | TaskQ's own bootstrap rule; anchor cited by `AGENTS.md`. |
| § 7 Worker, reviewer, publication | 2,108 | 1,700 | compress | Rule 1 restates agent-workflow Prepare and Approval boundaries. |
| § 7 Result and close verdicts | 4,075 | 2,700 | compress | CI-poll rule appears three times (§ 4, § 7 rule 5, reviewer); review axes restate agent-workflow Review. |
| § 7 Recipes and owner gates | 1,112 | 1,000 | keep | Shipped and project-specific. |
| § 8 Runtimes and deployment | 13,160 | 6,500 | compress | Platform sandbox qualification, rollback-floor procedure and codex-app qualification history belong to permission and qualification pages. |
| § 9 Config and cutover | 4,645 | 3,500 | compress | Evidence-task guidance and label-schema history (#769, #776) are history, not config. |
| **Total** (with the 25-byte Principles heading) | **98,270** | **≈ 56,000** | | About 14k tokens, a 43 % cut. |

## Text that restates specq

Each passage is replaced by a link to the named file, keeping only TaskQ values.

| taskq.md passage | Copies |
|---|---|
| R7 lines 47–49: validate input, never cut evidence, historical entries fold unchanged, compact displays never shorten stored text | `caveman/rules/core.md` (Write, Review); `caveman/rules/changes.md` (verbatim diagnostics) |
| R8: contract and code in one deliverable | `engineering/spec-first.md` |
| R3 Operator and R9 last sentence: tools are capability, not authority; explicit authorization; verified results | `engineering/permission-authority.md`; `engineering/agent-workflow.md` (Approval boundaries) |
| R12 "Do not duplicate matrix evidence"; development instruments are not gates | `engineering/verification.md` (Test classes) |
| R13 security reports and response target | `engineering/security.md` (SECURITY.md rule) plus the project's `SECURITY.md` |
| Product line 71: numbered choices keep source numbers across interfaces | `design/workflow.md` (AI-assisted controls) |
| Product "Tone" line | R7 itself (internal duplicate of the specq Core link) |
| § 4 "Owner-facing service/tray text": unknown values carry a reason; keep full accessible text | `engineering/verification.md` (Unknown is never healthy); `design/operator-ui.md` (D8) |
| § 4 Data: every growing file has a bound and a pruner; corrupt files kept as unknown | `engineering/data.md` (Files); keep the TaskQ bounds |
| § 4 Profiling: store reuse, budgets unknown until baseline, report links | `engineering/profiling.md` (Store, Budgets and unknowns, Report) |
| § 5 output bullets: diagnostics keep exact commands and honest unknowns; preserve evidence | `caveman/rules/core.md`; `caveman/rules/changes.md` |
| § 7 rule 1: ask before code on conflict, untestable acceptance or oversize work | `engineering/agent-workflow.md` (Prepare the task) |
| § 7 reviewer: task fit and implementation quality separately | `engineering/agent-workflow.md` (Review) |
| § 8 step 1: checksums and provenance attestation | `engineering/release-contract.md`, `engineering/security.md`; keep the TaskQ command |

## Design targets and detail that belong in docs/

| taskq.md passage | Kind | Home |
|---|---|---|
| Product line 71: product principles summary | target | `docs/product-principles.md` |
| Product line 72: existing-agent entry, remote PM, Hermes/FRA | target | `docs/design/agent-entry.md`, `docs/design/remote-manager.md` |
| Product line 73: decision register, dynamic resources, shared resource controls, plans | target | `docs/design/decision-register.md`, `docs/design/dynamic-resources.md`, `docs/design/shared-resource-controls.md` |
| Product line 82: finding handoff, operator UI, menu, counters, rendering, emergency UI, agent settings, delivery UI, plans, use cases | target | `docs/design/README.md` as the target index; each named page |
| Product lines 83–84: custom tray icon, board specification | target | `docs/design/custom-icon.md`, `docs/design/board.md` |
| R7 line 44: owner-text templates are a design target | target | covered by R13 and `docs/design/README.md` |
| R12: Playwright, `PLAYWRIGHT_CHANNEL`, Chrome crash dialog, screenshot routes | detail | `docs/design/ui-acceptance.md` (native matrix) |
| § 2 Ask card line 138: shared answer card and tray exception | target | `docs/design/answer-card.md` |
| § 3 Load gate: Windows, macOS and Linux reading APIs | detail | `docs/design/capacity.md` |
| § 4 Pause: emergency targets (tray control, graceful/force stop, selective resume) | target | `docs/design/emergency-recovery.md` |
| § 4 Tray: popover routes, header Pin/Top, history bins, Resources window, notification modes, Windows/macOS bundle detail | detail | `docs/design/menu.md` M3, M6, M13, M14 |
| § 4 AI requests: clarify chain layout, Reply button, draft and history, Add project dialogs | detail | `docs/design/menu.md` M7, M10; `docs/design/ai-workspace.md` |
| § 4 Incident reports: payload allowlist, dedup bounds, route detail | detail | `docs/design/incidents.md` |
| § 4 Log and board budget: header parsing, per-op counting format | detail | `docs/budget-1032.md` |
| § 4 `status.json` key list | detail | code (`src/service.rs`) with a one-line pointer |
| § 6 setup target | target | `docs/design/setup-and-project-locations.md` |
| § 8 Access: Linux and Windows sandbox qualification, rollback floor procedure | detail | `docs/design/worker-permissions.md`, `docs/design/linux-sandbox.md`, `docs/runtime-qualification.md` |
| § 8 codex-app environment: #952 live qualification | evidence | `docs/runtime-qualification.md` |
| § 8 step 5: macOS signing identity procedure | detail | `docs/design/worker-permissions.md` or a new release page |
| § 9 evidence-task guidance (#1028, #960, #1021) and label-schema history (#769, #776) | evidence | `docs/runtime-qualification.md`; git and issues |

## Proposal for #1156

- **Injected contract:** about 56 KB after tasks 1–6 (from 98.3 KB). A hard bound of 60 KB, tested next to the 300-line test, keeps it there.
- **Docs index:** about 4 KB, one line per Markdown file under `docs/`, excluding `evidence/` folders (they are reached from their owner pages):

  ```text
  - docs/<path>.md: <first heading>
  ```

  Example lines:

  ```text
  - docs/design/menu.md: TaskQ operator menu
  - docs/design/worker-permissions.md: Worker permissions
  - docs/runtime-qualification.md: Runtime qualification matrix
  ```

  Every first heading must be English (task 7: `docs/design/board.md` starts with a Russian heading).
- **Total cap:** the specq share depends on specq#35 and specq#36. With specq at today's 91 KB, contract 60 KB plus index 6 KB plus specq gives about 157 KB; a total cap of 160 KiB in § 9 fits. Set the final number after specq#35 lands.

## Edit tasks, in order

Each task edits `taskq.md` first (R13), moves text only to a page that already owns the topic, and passes `cargo test --locked` and `taskq selftest` with the constraints above.

1. **Product link-out.** Replace Product lines 71–73 and 82–84 with one line pointing to `docs/design/README.md` as the target index; keep README, header, setup phrase, screenshots, formatting and last-line decisions. Saves about 5 KB.
2. **§ 4 Tray and AI requests.** Keep shipped decisions (one tray per host, files read, confirm and apply rules, op allowlist, request log); move layout and platform detail to `menu.md` M3–M14 and `ai-workspace.md`. Saves about 8 KB.
3. **§ 8 Access and codex-app.** Keep presets, authority and refusal rules; move platform qualification, rollback-floor procedure and signing procedure to the permission and qualification pages. Saves about 5 KB.
4. **specq restatements.** Replace every passage in the restatement table with its link. Saves about 4 KB.
5. **Internal duplicates.** One home each for the 300-line rule, the CI-poll rule and the Ask/Result limits. Saves about 1 KB.
6. **Remaining compression.** § 1 claims, § 3 load gate APIs, § 4 incidents, budget, profiling and `status.json` keys, § 9 history. Saves about 20 KB.
7. **Guards.** Add a byte bound to `every_table_row_has_a_golden_trace`; give `docs/design/board.md` an English first heading. After tasks 1–6 the contract was 83 KB, so the owner set the bound at 85 KiB (#1166); a follow-up compresses the contract and lowers it to 60 KiB.
8. **#1156** injects the result with the index shape above.
