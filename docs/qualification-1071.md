# specq compliance audit of working agents (#1071)

Audit date 2026-10-14, about 16:20 UTC. Planning and audit only: no deployment, grant, restart or app change. Contract: [taskq.md § 8 Rule delivery](../taskq.md), pin `677b6db` of [specq](https://github.com/taskqorg/specq). Current specq `main` is one commit ahead (#25: `engineering/README.md`, `principles.md`, `profiling.md`); the four delivered bodies are unchanged, so the pin is current for them.

Method: the four pinned bodies were fetched from their immutable URLs and searched verbatim in actual model inputs (Claude transcripts, Codex rollouts). Board facts come from host heartbeat issues and the task log. The audit worker ran on `mac`; under the brief's folder rule it read only this host's `~/.claude` and `~/.codex`. Windows and Linux inputs were not read and stay **unknown** below. The board API budget was exhausted during the audit; no further board reads were made.

## A1. Installations

| Host (heartbeat) | Version | Class | Live attempts | Evidence |
|---|---|---|---|---|
| `mac` | 4.2.0-dev.1791987853 (built 14:24 UTC, after #962) | Working, six projects, focus `all` | #1047, #1051, #1071 | Claude 2.1.299, Codex CLI 0.159.3; access legacy (no grant): Claude `dontAsk` in TaskQ Seatbelt, Codex `workspace-write` + network |
| `win` | 4.2.0-dev.1791987849 (14:24 UTC; before #1057 merged 16:14) | Working | #1032, #1049, #1050, #1052, #1058 | Client versions, grants and actual inputs unknown from this audit |
| `hermes-fra1` | 4.2.0-dev.1791987850 | Working service, `focus off`: admits nothing | none | Codex 0.162.0 qualified in isolated fixtures only (#997, Linux constrained acceptance) |
| `wsl` | 4.2.0-dev.1791994244 (16:10 UTC) | Qualification candidate (#1050), heartbeat issue #1065 | none | No `focus off` line, so it admits by its focus; focus labels unknown |
| `macq` | 4.1.11 | Qualification candidate (#1051 update probe, fake codex child #1069) | none | 4.1.11 has no rule delivery, no grants and no focus line in its heartbeat; focus unknown |

Delivered revision: every TaskQ brief observed on `mac` names `specq@677b6db` with source paths. The 4.2.0-dev hosts were built after #962, so their adapter delivers the same pin (model: `runtime.rs` `rules_reach_every_runtime_role_and_resume`). 4.1.11 delivers no rules.

## A2. Full rule bodies in actual inputs (mac, 2026-10-14)

| Client / role | Start | Continuation | Result |
|---|---|---|---|
| Claude CLI worker, reviewer, operator | 126 sessions, all briefs 4/4 bodies | Service resumes: 4/4 in every observed resume (150 brief turns, 0 partial) | Delivered |
| Claude CLI, after context compaction | — | 4 rule-carrying sessions compacted (for example #1011 worker 06:58, a worker at 16:03). The compact summary replaces the brief: 0/4 bodies until the next service resume | **Delivery failure** inside one long turn |
| Codex CLI worker, reviewer, operator | 81 service-launched rollouts, all 4/4 | Resumes 4/4; one compacted worker kept all four bodies in `replacement_history` | Delivered |
| codex-app | Handshake carries no rules (expected); queued brief 4/4 (`app-7033…` rollout, #966/#989 chain) | Queued text 4/4 | Delivered; handshake alone is not proof |
| PM sessions (Codex Desktop, Claude chat) | Not a TaskQ launch: no bodies, only AGENTS.md links | — | Out of § 8 scope; specq reaches the PM only as links |
| Windows, Linux working inputs | unknown | unknown | Not read by this audit |

Only four specq files are delivered. `principles.md`, `caveman/rules/core.md`, `documentation.md`, `spec-first.md` and `permission-authority.md` reach agents as links in taskq.md/AGENTS.md; their loading is unverified.

## A3. Behavioral evidence map

`Live` = recorded actual agent behavior on that path; `Prod` = working queue evidence from this audit.

| Client | macOS | Windows | Linux |
|---|---|---|---|
| Claude CLI | Live #1001 (qualification); Prod: delivery verified, behavior mixed (A4) | Live #996 (qualification); Prod unknown | Unsupported: not installed on FRA |
| Codex CLI | Live #961; Prod: delivery verified | Live #965; #1057 candidate in isolated install; Prod unknown | Live #997 and constrained matrix (fixtures); Prod none (`focus off`) |
| codex-app | Live #966, #989 | Live #989 handshake/queue; Prod unknown | Unsupported |
| Claude Desktop | Unsupported: no TaskQ adapter; manual PM use unqualified | Unsupported | Unsupported |

No row shows that rule delivery causes compliance in general; reviewer verdicts below are the only production behavior evidence.

## A4. Suspected noncompliance

Class: **D** delivery failure, **B** behavioral violation (rules delivered), **M** missing evidence.

| # | Instruction | Actual action | Consequence | Class |
|---|---|---|---|---|
| 1 | § 8: rules apply to the turn, including continuations | Claude compaction summarized the brief away (4 sessions) | Rest of the turn runs without verbatim rules | D |
| 2 | verification.md: report what observations establish | #1051 claimed U-A6 Pass without temporal evidence; #1047 claimed native banner behavior without evidence | Reviewer requeues; rework | B |
| 3 | verification.md: defect fix needs before/after on the same workload | #1011, #1032, #1043 handed in without same-workload after-counts | Three requeues of the board-budget chain | B (repeated: friction) |
| 4 | taskq.md R8 / § 2 model: lifecycle change needs a golden trace | #1058 added only a unit test | Requeue | B |
| 5 | § 7 rule 4: changed test needs an amended rule | #1024 changed an old-revision rule without taskq.md | Requeue | B |
| 6 | agent-workflow.md: one writer, frozen candidate | #993 branch amended by another writer during validation | Requeue, candidate not frozen | B |
| 7 | § 7 rule 6: no local paths in public text | #1034 `gone` text carried a worker's absolute Windows paths | Public leak of local paths; addressed in #1034 | B (worker), D (adapter) |
| 8 | Brief folder rule | This audit worker ran `git fetch`/`log` in a sibling specq checkout | Ref update outside the allowed folders; no file content changed | B |
| 9 | Task scope: no Full | #1068 attempts failed twice: `--bg with bypassPermissions requires accepting the disclaimer` | A Full grant exists on some working host (host unknown); two `gone`, retries used | M (grant origin), D (launch) |
| 10 | § 3 capacity/focus on production board | `macq` (4.1.11) and `wsl` candidates beat on the production board with unknown focus | A rule-free 4.1.11 service could claim an untargeted ready task | M |

Rows 2–7 were caught by independent reviewers that received the same rules: that is evidence the review rules apply, not that workers comply.

## A5. Windows candidate (#1057) blockers for production

1. The working `win` service predates #1057; adopting it needs an update plus the private `windows-native/` bundle (patched GNU `codex.exe` + official MSVC code-mode host, pinned hashes). Neither is distributed; a Codex auto-update changes the hash and refuses admission.
2. Only standalone Codex CLI, Workspace grant with network, start and resume were qualified. The working queue chain (claim, `gh`/board token, git push, PR, `close`) inside the boundary is unqualified.
3. Windows Claude and codex-app have no enforcement path; they run legacy flags or Full. #1068 shows a Full grant already exists on some host.
4. Restricted, network-off, Elevated/MXC and hard crash recovery are unqualified; Elevated setup is blocked by a busy app `node_repl.exe`.
5. No grant migration plan for the working `worker-policy.json` (exists, contents unread), and the rollback floor means 4.1.11 hosts would delete the store.

Safe adoption without Full or wider grants = 1 + 2 on one owned project with an explicit project Workspace grant, other runtimes unchanged.

## A6. Plan (owner decisions marked ◆)

| P | Task | Depends on | First action | Bounded acceptance | Owner |
|---|---|---|---|---|---|
| 1 | Re-deliver rules after Claude compaction | — | ◆ choose route: `--append-system-prompt-file` on spawn (persists in the system prompt; resume must not copy the session) or a re-read line in every resume | A compacted Claude worker transcript shows 4/4 bodies after compaction; one live run on mac | worker; owner picks route |
| 2 | Read Windows working inputs | — | Run the same transcript check on `win` for current Claude/Codex attempts (read-only) | Table row with counts per role and continuation; unknown rows named | operator on win |
| 3 | Qualification hosts off the production board | — | Read back `macq`/`wsl` focus; ◆ require `host-<candidate>` focus or a separate board for candidates | Both heartbeats show `focus off` or candidate-only labels; no claim of untargeted tasks | owner, operator |
| 4 | Find and decide the Full grant | — | `service permissions` read on each working host | Every Full grant listed with `by`/`at`; ◆ keep or remove | owner |
| 5 | Evidence-before-claim pattern | — | Reuse reviewers; add one line to the worker brief only if specq lacks it (it does not: verification.md) — no new rule | Next five reviewed results: requeue rate for missing evidence recorded | PM |
| 6 | Windows Codex Workspace on one working project | 2, 4 | ◆ approve private bundle provenance and an explicit project grant | Existing matrix row Q2/Q5 plus one real queue task: claim, push, PR, close inside the boundary | owner, worker on win |
| 7 | Deliver the remaining specq files | — | ◆ decide whether principles/core/documentation join the pin (size vs. 64 KiB per file) | taskq.md § 8 names the set; transcripts show them | owner |

Start with P1 and P3 in parallel: P1 closes the only observed delivery failure; P3 removes the rule-free launch path.

## A7. Response to confirmed noncompliance (proposal, not applied)

- Delivery failure (missing, partial or stale rules): refuse the next launch or resume, as § 8 already does for fetch failure. Extend to: a service below the rule-delivery version must not admit on a production board (◆ owner: add a minimum service version to admission, or a board guard).
- Continuation loss (compaction): repair, not refusal; fix at the next spawn/resume. Running turns continue (R11).
- Behavioral violation: the reviewer requeues as today; repeated cases go through learn-from-friction. No automatic refusal of a runtime or host.
- Current work stays intact: no stop, restart or grant change for live attempts; every change applies to new starts and resumes only (§ 8 Access, Rule delivery).

## A8. Test reuse

Reuse `live_operator_matrix`, `live_linux_boundary_matrix`, `live_windows_boundary_matrix` and the Q1–Q3 worker/reviewer scenario only for changed chains: P1 (Claude continuation after compaction) and P6 (Windows working queue) are the only new live runs. The transcript check above is a development instrument; it is not added to CI. No new unit tests, per-commit paid probes or duplicated prose rules are proposed.
