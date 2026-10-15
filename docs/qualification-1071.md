# specq compliance audit of working agents (#1071)

Audit 2026-10-14. Planning and audit only: no deployment, grant, restart or app change. Contract: [taskq.md § 8 Rule delivery](../taskq.md), pin `677b6db` of [specq](https://github.com/taskqorg/specq). specq `main` is one commit ahead (#25: `engineering/README.md`, `principles.md`, `profiling.md`); the four delivered bodies are unchanged, so the pin is current for them.

Method: the four pinned bodies were fetched from their immutable URLs and searched verbatim in actual model inputs (Claude transcripts, Codex rollouts) since 2026-10-01. Attempt 6172932304 read `mac`; attempt 6173092265 read `win`. Each attempt read only its own host's `~/.claude` and `~/.codex`. Board facts come from heartbeats, `status.json` and task logs. A claim about a host or attempt whose inputs were not read is **unknown**, not inferred from the build.

## A1. Installations

| Host | Version | Class | Clients and access | Delivery proof |
|---|---|---|---|---|
| `mac` | 4.2.0-dev.1791987853 | Working, six projects, focus `all` | Claude 2.1.299, Codex 0.159.3; legacy (no grant): Claude `dontAsk` in TaskQ Seatbelt, Codex `workspace-write` + network | Read: 126 Claude sessions and 81 Codex rollouts, all briefs 4/4 (attempt IDs not recorded by that scan) |
| `win` | 4.2.0-dev.1791987849 (before #1057) | Working, six projects, focus `all`, 6 live Claude children | Claude 2.1.299, Codex CLI 0.162.1: **Full** grant rev 4 for both, recorded `by alexk` 2026-10-14 13:22 UTC, `runtime sandbox off, uncontained`; codex-app refused (no grant, legacy) | Read: A2 table, attempt-level |
| `hermes-fra1` | 4.2.0-dev.1791987850 | Working service, `focus off`: admits nothing | Codex 0.162.0 qualified in isolated fixtures (#997) | No TaskQ launches, so nothing to deliver; inputs not read |
| `wsl` | 4.2.0-dev.1791994244 | Qualification candidate (#1050, heartbeat #1065) | Full grant set during the owner-approved #1050 test (#1068 log); Claude login lost mid-test | Unknown |
| `macq` | 4.1.11 | Qualification candidate (#1051, #1069) | 4.1.11 has no rule delivery and no grants | None by design; focus unknown |

Delivered revision on `mac` and `win`: every observed brief names `specq@677b6db` with source paths. Other hosts: unknown.

## A2. Full rule bodies in actual inputs

`win` inputs (`~/.claude/projects`, `~/.codex/sessions`, since 2026-10-01). Start = first brief of a session; continuation = later brief in the same session (service resume).

| Client / role | Start | Continuation | Result |
|---|---|---|---|
| Claude worker | 53 briefs, 34 tasks (#1032, #1049, #1050, #1052, #1058, #1061, #1071 ..) | 12, 10 tasks | 4/4 in every brief |
| Claude reviewer | 20, 15 tasks (#970 .. #1034) | 1 (#996) | 4/4 |
| Claude operator | 9 | 3 | 4/4 |
| Codex CLI worker / reviewer / operator | 18 / 32 / 1 (all 52 rollouts `codex_exec`) | 11 / 4 / 0 | 4/4; incl. #1071 reviewer attempt 6173058617 |
| codex-app | 0 TaskQ rollouts | — | No production evidence: launches refused by the access gate |
| Claude after compaction | — | 7 auto compactions in worker sessions (#970, #971, #1009, #1011, #1049, #1050, #1052) | **0/4 bodies**: the summary replaces the brief and no brief follows in that turn |
| Codex after compaction | — | 0 compactions on `win` | Unknown on `win`; `mac`: 1 compacted worker kept 4/4 in `replacement_history` |

Attempt-level examples with 4/4 at start: #993 6160419622 (Codex), #970 6160135252, #1034 6164288491, #1032 6172023854, #1049 6172586319, #1050 6172609729, #1052 6172627345, #1058 6172879386 (Claude).

`mac` (attempt 6172932304): Claude 126 sessions and 150 resume turns 4/4; 4 compactions 0/4; Codex 81 rollouts 4/4; codex-app queued brief 4/4 (handshake carries no rules, expected). PM sessions are not TaskQ launches and get only AGENTS.md links. Linux: unknown.

### What a Claude agent holds after compaction (`win`, Claude 2.1.299)

- The compact summary paraphrases the brief. Example #1009: "TaskQ shared rules (specq@677b6dbb…: agent-workflow, use-cases, verification, changes.md)". The names survive, the bodies do not.
- Claude re-attaches project instructions after the boundary: `C:\…\taskq\CLAUDE.md` → `AGENTS.md` of the **project-root checkout** (see A4 row 1). Their text has no specq link.
- SessionStart hooks re-run after auto compaction inside a TaskQ worker turn: in #1049 (attempt 6172586319) the user-level caveman hook text reappeared verbatim after the boundary. A hook is therefore an observed in-turn re-delivery path; a service resume is not (it comes only after the turn).
- Bounded probe (7 Haiku calls, temp dir): a rule in `--append-system-prompt-file` and a rule in the first user message both survived a forced `/compact` in a two-turn session; the summary copied each rule. The probe is too small to show verbatim survival in a 160k-token session, and the system prompt is not logged, so that route cannot be read back from transcripts.

### Behavior after compaction (`win`)

| Task / attempt | After compaction | Reviewer verdict | Reading |
|---|---|---|---|
| #970 6160135252 | result 13 min later | requeue only for a merge conflict; "No other gap found" | Compliant by review |
| #1049 6172586319 | result 10 min later | requeue: "Evidence mixes builds; #1053 proof is pending … correct #1053 claim" | Violates verification.md "Verify the exact candidate"; cause (compaction or not) unproved |
| #1052 6172627345 | result 15 min later | requeue: residual paraphrase in `ui-acceptance.md` | Task-fit gap, no specq violation named |
| #1050 6172609729 | ask 14 min later | not reviewed (ask) | No evidence |
| #971, #1009, #1011 | no verdict command in the compacted session | — | No evidence |

Four post-compaction verdicts are too few to attribute a violation to compaction. The delivery gap is confirmed; its behavioral cost is unknown.

Only four specq files are delivered. `principles.md`, `caveman/rules/core.md`, `documentation.md`, `spec-first.md` and `permission-authority.md` reach agents as links; their loading is unverified.

## A3. Behavioral evidence map

`Live` = recorded qualification behavior; `Prod` = working-queue evidence from this audit.

| Client | macOS | Windows | Linux |
|---|---|---|---|
| Claude CLI | Live #1001; Prod: start/resume delivered, compaction drops bodies, behavior mixed (A4) | Live #996; Prod: start/resume delivered (attempt-level), compaction drops bodies, behavior mixed (A4); runs under Full | Unsupported on FRA (not installed); `wsl` candidate unverified (login lost) |
| Codex CLI | Live #961; Prod: delivered | Live #965; #1057 candidate isolated only; Prod: delivered, runs under Full | Live #997, constrained matrix in fixtures; Prod none (`focus off`) |
| codex-app | Live #966, #989 | Live #989 handshake/queue; Prod: none (refused, no grant) | Unsupported |
| Claude Desktop | Unsupported: no TaskQ adapter | Unsupported | Unsupported |

Delivery is not compliance. Reviewer verdicts are the only production behavior evidence.

## A4. Suspected noncompliance

Class: **D** delivery failure, **B** behavioral violation with rules delivered to that attempt, **M** missing evidence. "Delivery" names the proof for that attempt.

| # | Instruction | Actual action (attempt) | Consequence | Delivery | Class |
|---|---|---|---|---|---|
| 1 | Brief: "Read `<root>/taskq.md` first and do only what it allows (R13)" | `runtime.rs` `contract()` names the project-root file. On `win` the root checkout is at `3e7b8a8` (2026-10-12), 190 commits behind `origin/main`: no § 8 Rule delivery, no operator role; its AGENTS.md has no specq line. Claude loads that AGENTS.md at start and after compaction. Claude agents read the root taskq.md 12 times since 10-13 (#970, #971, #974, #989, #993, #1009, #1011, #1016, #1032) | Agents are told to obey a stale contract while their worktree holds the current one. No requeue traced to it yet | Mechanism read on `win`; `mac` root freshness unknown | D |
| 2 | § 8: rules apply "including continuations" | Claude auto compaction removed the bodies in 11 worker sessions (7 `win`, 4 `mac`) | Rest of the turn runs on a summary | Read | D |
| 3 | verification.md "Verify the exact candidate" | #1049 6172586319: result mixed builds, unproved #1053 claim | Requeue | 4/4 at start, 0/4 after compaction | B (after D) |
| 4 | verification.md "Report what observations establish" | #1051 6172595443 claimed U-A6 Pass without temporal shift/rebin evidence; #1047 6172561941 claimed native banner behavior without evidence | Requeues, rework | `mac`, attempt not in `win` inputs; mac scan has no attempt IDs | M (delivery), B if delivered |
| 5 | verification.md: defect fix proves before/after on the same workload | #1011 6162686592, #1032 6171268780, #1043 6171947611 handed in without same-workload after-counts | Three requeues of one chain (friction) | `mac` attempts, not attempt-proved | M, B if delivered |
| 6 | taskq.md R8 / § 2: lifecycle change needs a golden trace | #1058 6172789875 added only a unit test | Requeue | `mac`, not attempt-proved | M, B if delivered |
| 7 | § 7: a changed test needs an amended rule | #1024 6162477380 changed an old-revision rule without taskq.md | Requeue | not attempt-proved | M, B if delivered |
| 8 | taskq.md § 7 rule 6: no local paths in public text | #1034 6164288491 implemented `gone` reasons that post raw chat; reviewer found a worker's `/C:/Users/…` links | Requeue before merge | 4/4 (`win`) | B |
| 9 | agent-workflow.md: one writer, frozen candidate | #993: branch amended `ed3884b`→`c8ce356` during review 6160680751 | Requeue | Writer identity unknown | M |
| 10 | Brief folder rule: touch only this project, `~/.claude`, `~/.codex`, OS temp | 6172932304 ran `git fetch`/`log` in a sibling specq checkout; this attempt 6173092265 ran `git fetch` in sibling project roots (agentq fetched; four refused by Git ownership checks) | Ref updates outside the allowed folders; no file content changed | 4/4 (this brief) | B |
| 11 | § 3 capacity/focus on the production board | `macq` (4.1.11) and `wsl` beat on the production board with unknown focus | A rule-free 4.1.11 service could claim an untargeted task; `MIN_SERVICE` guards only one install | — | M |

Rows 3–9 were caught by independent reviewers that received the same rules: evidence that the review rules apply, not that workers comply.

## A5. Windows candidate (#1057) blockers for production

1. The working `win` service predates #1057. Adoption needs an update plus the private `windows-native/` bundle (patched GNU `codex.exe` + official MSVC code-mode host, pinned hashes). Neither is distributed; a Codex auto-update changes the hash and refuses admission.
2. Only standalone Codex CLI, Workspace grant with network, start and resume were qualified. The queue chain (claim, board token, git push, PR, `close`) inside the boundary is unqualified.
3. `win` already runs Claude and Codex under a host Full grant. Adopting #1057 narrows Codex; Claude has no Windows enforcement path, so it stays Full or returns to legacy `dontAsk` (owner choice).
4. Restricted, network-off, Elevated/MXC and hard crash recovery are unqualified; Elevated setup is blocked by a busy app `node_repl.exe`.
5. A grant change applies to new starts only (§ 8); live Full children keep running until their attempt ends. 4.1.11 hosts would delete the store (rollback floor).

Safe adoption without Full or wider grants = 1 + 2 on one owned project with an explicit project Workspace grant for Codex; Claude unchanged until decided.

## A6. Plan (owner decisions marked ◆)

| P | Task | Depends on | First action | Bounded acceptance | Owner |
|---|---|---|---|---|---|
| 0 | Brief names the current contract | — | ◆ choose: brief names `taskq.md` of the attempt's worktree from `origin/main`, or the service refuses launch when the root checkout is behind `origin/main` | A new `win` attempt's transcript shows the agent reading the current contract; the root checkout is not modified by TaskQ | owner picks; worker |
| 1 | Re-deliver rules inside a compacted Claude turn | — | ◆ choose: per-spawn SessionStart `compact` hook that prints the pinned bodies (observed path, readable in transcripts) or `--append-system-prompt-file` (not readable back). Resume re-delivery alone misses in-turn compaction | One long `win` or `mac` worker turn: the transcript after `compact_boundary` holds 4/4 bodies before the next tool call | worker; owner picks |
| 2 | Inventory hosts not read | — | Run the same transcript and root-freshness check on `mac` (root checkouts) and `wsl` | Rows with counts per role and continuation; unread rows stay unknown | operator per host |
| 3 | Qualification hosts off the production board | — | Read back `macq`/`wsl` focus; ◆ candidate-only focus or a separate board | Both heartbeats show `focus off` or candidate labels; no claim of untargeted tasks | owner, operator |
| 4 | Full access at planning time — resolved by #1213 | — | Apply [§ 8 Planning Full access (R9)](../taskq.md#8-runtimes-and-deployment) to proven program requirements | Planner selects the existing Full preset with failure evidence; operator-owned grant remains required. No grant changed or host qualified by this spec update | planner; owner grants |
| 5 | Evidence-before-claim | — | Reuse reviewers; specq verification.md already holds the rule, so no new prose | Next five reviewed results: requeue rate for missing evidence recorded | PM |
| 6 | Windows Codex Workspace on one working project | 0, 4 | ◆ approve private bundle provenance and an explicit project grant | Matrix rows Q2/Q5 plus one real queue task: claim, push, PR, close inside the boundary | owner, worker on `win` |
| 7 | Deliver the remaining specq files | 1 | ◆ decide whether principles/core/documentation join the pin (64 KiB per file) | taskq.md § 8 names the set; transcripts show them | owner |

Start with P0 and P1 in parallel: both are confirmed delivery failures with a single readable acceptance check. P3 removes the rule-free launch path.

## A7. Response to confirmed noncompliance (proposal, not applied)

- Stale contract (P0): refuse new launches only when the named contract is behind `origin/main` and cannot be replaced by the worktree copy; never edit or reset the owner's root checkout.
- Missing, partial or stale rules: refuse the next launch or resume, as § 8 already does for a fetch failure. Extend to the board: ◆ a service below the rule-delivery version must not admit on a production board (`MIN_SERVICE` today covers only services of one install).
- Compaction loss (P1): repair, not refusal; the hook re-delivers inside the running turn. Running turns continue (R11).
- Behavioral violation: the reviewer requeues as today; repeated cases go through learn-from-friction. No automatic refusal of a runtime or host.
- Current work stays intact: no stop, restart or grant change for live attempts; every change applies to new starts and resumes only (§ 8).

## A8. Test reuse

Reuse `live_operator_matrix`, `live_linux_boundary_matrix`, `live_windows_boundary_matrix` and the Q1–Q3 worker/reviewer scenario only for changed chains: P1 (Claude in-turn compaction) and P6 (Windows working queue) are the only new live runs; P0 needs one real attempt, not a matrix. The transcript check is a development instrument, not CI. No new unit tests, per-commit paid probes or duplicated prose rules are proposed.
