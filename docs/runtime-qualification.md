# Runtime qualification matrix

Verify supported TaskQ capabilities through observable outcomes. [R9, R12 and runtime contract](../taskq.md) own policy; [specq verification](https://github.com/taskqorg/specq/blob/main/engineering/verification.md) owns test method. This matrix does not grant access or turn pending permission designs into shipped behavior.

## Execution policy

One bounded live scenario covers each applicable runtime, role and continuation. Run it when that integration, permission configuration or client version changes, not on every commit or documentation edit. Record exact versions, effective permissions, initial state, actions, observed output and remaining gaps. A prior pass qualifies only its recorded configuration.

Keep cheap deterministic checks only for a distinct user-visible chain or safety invariant: state transitions, duplicate/stale writes, admission, publication and preservation of active work. Use data cases in those chains. Remove development-only and redundant assertions after their question is answered. A constructor/argument-string check is not live permission or behavior proof. Never delete the only check protecting a supported behavior merely to lower the count.

## Supported clients

| Client | Worker and reviewer | Operator | Platform and boundary |
|---|---|---|---|
| Claude CLI | Supported | Full tools; real edit/check and same-session continuation passed on macOS | macOS, Linux, Windows adapters; permission mode is not OS containment |
| Claude Desktop | No TaskQ runtime adapter | No TaskQ runtime adapter | Manual manager use is separate and unqualified here |
| Codex CLI | Supported | Full tools; configured sandbox; real edit/check and continuation passed on macOS | macOS, Linux, Windows; effective sandbox and network are independent |
| codex-app | Opt-in worker/reviewer | No separate app operator | macOS only; app policy and login differ from handshake environment |

## Required platform runs

Run the same bounded scenario on macOS, Windows and Linux for Codex CLI. Claude CLI requires macOS and Windows now; Linux remains a required qualification row before enabling Claude there. A missing binary/login is blocked, never a pass. Claude Desktop has no adapter; Remote Control is a separate native CLI capability and does not qualify a Desktop runtime.

| Platform | Codex CLI | Claude CLI | codex-app |
|---|---|---|---|
| macOS | Worker, reviewer, operator; start and resume | Same roles; native background lifecycle and Remote Control | Explicit worker/reviewer only |
| Windows | Same CLI roles; native permissions and process lifetime | Same roles; native background lifecycle and Remote Control | Unsupported |
| Linux | Same CLI roles; sandbox/network and process lifetime | Required before adoption; currently not installed on FRA | Unsupported |

## Feature matrix

Model = deterministic logic/adapter check. Live = actual agent/session evidence. Unknown = not qualified. Unsupported = no adapter; never show it as a pass.

| ID | User-visible capability | Existing instrument | Actual evidence and remaining gap |
|---|---|---|---|
| Q1 | Worker asks an ambiguous choice before editing; answer resumes the same session | Service answer chain; real start/resume | Codex CLI #961/#965 and app #966 live; Claude #969 live |
| Q2 | Worker and reviewer receive all four pinned rule bodies at start and resume | Rule delivery chain; exact transcript comparison | Windows CLI #965 and macOS app #966 live; macOS CLI #961 partial matrix; Claude #969 live |
| Q3 | Worker proves before/after; reviewer independently reproduces and separates fit, quality and rollback | Same live fixture as Q1/Q2 | Codex #961/#965/#966 live; Claude #969 live; never infer all future rule compliance |
| Q4 | Failed rule fetch prevents launch, not a rule-free fallback | Rule delivery failure cases | Error/empty/oversize loader cases modeled; real timeout and invalid UTF-8 adapter faults not yet qualified |
| Q5 | Allowed workspace work succeeds; forbidden operations are actually denied | Native negative fixture, not prompt acknowledgement | Read/write, child, symlink, Git common-dir and network matrix pending per effective policy; broad read denial is not promised by workspace-write |
| Q6 | Operator performs authorized work with tools and resumes the recorded session | Ignored live operator chain; random canary plus independent file readback | Claude macOS/Windows and Codex macOS passed native read/write/command and same-session continuation; Windows Codex sandbox setup blocked; no claim of OS containment for Claude |
| Q7 | Only validated operations on registered projects apply once at the shown revision after confirmation | Operator proposal/apply chain; dispatch revision/idempotence chain | Deterministic pass; not protection against arbitrary same-user code |
| Q8 | One live attempt; stale output cannot change task state | Two-host claim, golden transitions and dispatch chain | Deterministic pass; fencing does not cancel external side effects |
| Q9 | Pause/load/capacity stop admission without stopping current jobs | Admission and service gate chains | Deterministic pass; live fixture must preserve a specifically owned active process |
| Q10 | Update preserves active work | Unix process handover chain; Windows idle update #965 | App/operator restart now waits; real app turn survived a pending restart and completed its file write; native Windows active-child update remains unqualified |
| Q11 | Cancellation has honest semantics | Process adapter; app completion/lease chain | App turn cannot be stopped by existing adapter; late board writes fenced only |
| Q12 | App runtime is selected explicitly on a capable host | Service opt-in chain; CLI create flow | CLI create/readback now covers app label; service admits app only on macOS with explicit opt-in |
| Q13 | Publication checks exact candidate and preserves dirty work | Publication and cleanup chains | Research ancestry is intentionally separate from new-code CI; rule adherence is independently reviewed |
| Q15 | Native Claude background lifecycle | Same bounded live chain, not a new per-commit test | Verify stdin delivery, native-assigned ID, turn completion, stop readback, same-ID resume without copying, Remote Control registration and interrupted-launch reconciliation on each platform |
| Q14 | Missing board credential fails without logging in or exposing it | GitHub credential chain; actual task commands | Codex CLI/app live board writes in prior qualification; Claude #969 live |

Prior live records: [#961](https://github.com/taskqorg/taskq/issues/961), [#965](https://github.com/taskqorg/taskq/issues/965), [#966](https://github.com/taskqorg/taskq/issues/966), [Claude #969](https://github.com/taskqorg/taskq/issues/969#issuecomment-6159775848). Preserve failed attempts as evidence; a retry supersedes only the specific failed claim.

## Bounded live scenario

1. Record candidate/client/OS and effective configuration. Use only an isolated fixture and an explicitly owned test task; leave existing jobs, grants and global configuration untouched.
2. Start worker with an ambiguous in-memory pause fixture. It must ask before editing. Answer keep-active, resume the same session, verify exact canonical rule bodies in both actual inputs, and inspect tool output for expected baseline failure and corrected active/repeat/empty success.
3. Start independent reviewer, execute the published fixture, inspect its actual rule input, ask before acceptance, resume the same reviewer and inspect the continuation before closing. A self-report or handshake is insufficient.
4. For each changed permission boundary, controller creates disposable canaries with unknown content, agent attempts the approved positive/negative operations, controller checks actual results and unchanged forbidden targets. Use native denied operations, not a request to obey prose. Never inspect real private files or credentials. Report allowed reads separately from denied writes.
5. For operator, inspect actual tools and execute only authorized fixture read/write/check. Require actual read/write/check results and same native session on continuation; disabling that runtime must prevent continuation without fallback. Proposed JSON still passes the existing validation/confirmation/readback chain.
6. For lifecycle changes, preserve an owned long-running fixture across pause/update; distinguish process exit, app-turn exit, lease and board state. Do not cancel unrelated work to manufacture an idle host.

## Current limitations

The shipped access presets in taskq.md § 8 supersede the earlier permission proposal: macOS enforces a TaskQ Seatbelt profile, while Windows/Linux currently require an explicit Full grant; missing grants or unknown restricted enforcement refuse launch. Full has no OS containment. Earlier passes below qualify their recorded candidate and permission configuration, not a later policy revision. No prompt, project configuration or 0600 file is claimed as an immutable host ceiling. No new grant broker, background monitor or duplicate test framework is introduced by this matrix.

## Operator qualification command

Run once per changed native integration, from the candidate checkout:

```sh
TASKQ_QUALIFY_RUNTIME=claude TASKQ_INCIDENT_REPORTS=off cargo test --locked live_operator_matrix -- --ignored --nocapture
TASKQ_QUALIFY_RUNTIME=codex TASKQ_INCIDENT_REPORTS=off cargo test --locked live_operator_matrix -- --ignored --nocapture
```

Provision any required TaskQ grant explicitly in the disposable `fixture-install/worker-policy.json` first; the test never grants Full itself. The chain keeps its fixture and native logs in `.taskq/qualification/`; no global settings, existing jobs, board writes or commits change. With the explicit qualification grant, the Claude fixture uses Bash/PowerShell on Windows and Bash/Write on macOS in its own `.claude/settings.local.json`; this grants no OS containment. It checks random file contents independently, verifies the session ID on continuation, then disables only its fixture runtime and requires failure before launch. It does not prove every permission boundary or MCP tool. Normal `cargo test --locked` skips these paid calls.

Claude worker/reviewer #969 includes a rejected first result (missing reproducible fixture), a repaired worker result, independent reviewer execution and resumed reviewer input. Controller compared all four pinned bodies against actual worker/reviewer start and continuation messages. The rejection is evidence that review applied the rules; closing the issue alone is not that evidence.

## Recorded candidate evidence (2026-10-14)

| Path | Client / session | Observed result |
|---|---|---|
| macOS operator, Claude | Claude Code 2.1.296; `4caa4089-e70f-4b7f-a152-7e734f214af6` | Read/copy/command check; append/check on the same session; host-disabled continuation refused before spawn. Four pinned bodies compared in both inputs. |
| macOS operator, Codex | CLI 0.159.3; `01a1385b-5d56-7381-b88a-b6c4f26e82fd` | Same chain passed. Actual policy workspace-write + network enabled; four pinned bodies compared in both inputs. |
| macOS app, deferred restart | CLI handshake 0.159.3 / app daemon 0.162.1; `01a1385f-473e-7260-98b3-0fe740288ab2` | Actual app tool wrote a start marker, waited, then wrote the end marker. Calling the real reexec guard mid-turn kept restart pending and preserved the turn through completion. Input contained all four pinned bodies; network remained disabled. No app restart. |
| Windows operator, Codex | CLI 0.162.1; `01a1385d-216b-7a52-a1e0-fb21bc9529cf` | Blocked before command execution: native sandbox setup could not validate read/execute ACLs on an in-use `node_repl.exe` (OS error 32). No sandbox downgrade and no interruption of that process. |
| Windows operator, Claude, denied | Claude Code 2.1.296; `53a5aa18-cb00-4676-b82d-c032d201844a` | Read succeeded; native dontAsk denied Bash/PowerShell. Agent reported the refusal, left output absent and did not bypass it. This failed attempt remains recorded; the explicitly approved fixture-only grant was then qualified below. |

| Windows operator, Claude, granted | Claude Code 2.1.296; `94d56415-6922-4de5-97a3-30339954110a` | With owner-approved Bash/PowerShell permission only in the disposable project: read/copy/command, same-session append/check, disabled-runtime refusal all passed. Controller compared all four pinned bodies in both actual inputs. |

The one-off app restart probe was removed after answering its development question; the existing process handover chain retains the regression case. The two redundant command/retro-constructor tests were removed; missing-log behavior moved into the existing retro chain. The ignored operator chain is the sole new retained live test. Windows deterministic checks passed (122 binary + 5 policy); macOS passed (125 binary + 5 policy). Offline selftest passed 53 golden traces and command formats; offline mode does not qualify board connectivity.

## Claude Remote Control probe (2026-10-14)

The owner requested Remote Control by default so local CLI sessions can appear in Claude. Native user setting `remoteControlAtStartup=true` was already present on macOS; it was enabled and read back on Windows without restarting existing sessions. Project settings cannot enable this setting; see [native Remote Control](https://code.claude.com/docs/en/remote-control#enable-remote-control-for-all-sessions).

- Claude 2.1.296 interactive `--remote-control`: connected, returned a remote session URL and wrote a `bridge-session` record to its actual transcript.
- Native background `claude --bg --remote-control` on 2.1.299: returned an owned background ID, answered the qualification prompt, and its native logs reported Remote Control active with a remote URL. The native updater advanced the installed CLI between these probes.
- Without any Remote Control flag, both an interactive launch and `claude --bg` on 2.1.299 auto-connected from the user setting. Their native `/remote-control` status panels confirmed existing remote sessions and URLs. Auto-connect did not print the same announcement or transcript record as the explicit flag, so absence of either alone cannot prove disconnection.
- TaskQ-style `-p --output-format stream-json` on 2.1.296: both explicit `--remote-control` and per-run `remoteControlAtStartup=true` completed the prompt but created no `bridge-session` record. Repeating the explicit flag on 2.1.299 also produced zero bridge-session records. Exit zero did not prove remote registration.

No Remote Control flag was added to TaskQ's print-mode adapter. Native interactive/background connection is qualified; The initial probe did not qualify TaskQ integration. The native adapter qualification below supersedes that limitation; web/app visibility remains separate from native registration.

## Native background acceptance (2026-10-14)

One live chain, three real hosts; normal CI still skips paid agent runs. Temporary role/lifetime probes were removed after collecting evidence; the existing operator chain and service continuation/restart chains retain the regression coverage. `TASKQ_QUALIFY_ROOT` selects an explicitly owned, already trusted qualification directory; archive its previous request logs before a repeat. Native trust/account failures are prerequisites, not permission to bypass them.

| Host / client | Worker and independent reviewer | Operator | Native lifetime / Remote Control |
|---|---|---|---|
| macOS / Claude 2.1.299 | [#995](https://github.com/taskqorg/taskq/issues/995), then [#1001](https://github.com/taskqorg/taskq/issues/1001) with answer after the asking turn stopped: both roles start, ask, retain the attempt and native session, resume and execute the supplied expected-failure/passing probes | Read/copy/command, same-ID append/check, disabled-runtime refusal passed | Registered native remote URLs; restart stayed pending during a real sleeping command; native stop prevented its later marker write |
| Windows / Claude 2.1.299 | [#996](https://github.com/taskqorg/taskq/issues/996): both roles start, ask, resume and independently execute probes; native `py` absent, Python 3.12.10 used and reported | Same operator chain passed with the approved fixture-only grants | Registered native remote URLs; same deferred-restart/cancellation marker scenario passed using `python` |
| Linux FRA / Codex 0.162.0 | [#997](https://github.com/taskqorg/taskq/issues/997): both roles start, ask, resume the same attempt/session, execute probes, and publish separate task-fit/quality evidence | Same chain passed with workspace-write and network enabled, including disabled-runtime refusal | Existing CLI process adapter; no Claude Remote Control claim |
| Linux / Claude | Blocked: no installed client/login on the qualification host | Blocked | Required before adopting Claude on Linux; do not install or transfer credentials merely to fill a cell |

The controller compared all four pinned specq bodies against **each actual start and resume input** of both roles on all three qualified paths. macOS #1001 sessions: worker `76302b27-299f-4cf0-bca6-d7aa50e4f979`, reviewer `6f7f59bd-4911-4c7f-91d9-02bbda9fe0ee`; Windows #996: `11e69922-c85c-4eca-8053-f76e5a99d7c7` / `794b2388-0365-4f3b-a41a-8a640965c9c6`; Linux #997: `01a1388f-3272-7553-abea-522725664624` / `01a13890-283d-7b23-a79b-f2be73165e84`. The probes intentionally fail before and pass after; agents must report that distinction without changing the supplied fixture. Research closure qualifies this scenario, not CI or publication of new code.

Native edge cases found and fixed: `--bg` owns the UUID; passing options on resume can copy the session; stop acknowledgement precedes actual process exit; startup can lack a PID; Windows restore briefly reports a temporary UUID until native status appears. TaskQ preserves the background ID, waits for restored identity, reads actual turn completion, waits for owned process exit, and never accepts a copied continuation. It streams the matching native transcript into the existing attempt log. Changed saved permission mode refuses continuation instead of silently replacing the session.

### Integration with access presets

Candidate `5297419` includes main through `c6d46bd` (access presets). Native Claude start and resume use the same effective permission mode and Seatbelt wrapper as the other adapters. A changed saved native permission mode refuses continuation. macOS operator passed again under the default TaskQ Seatbelt wrapper with native dontAsk and the approved fixture Bash/Write grant: session `d321a639-71a2-4e19-aca8-3758a3398354`, actual edit/check and same-ID continuation. Model suite: 139 + 5 passed; selftest: 65 traces, formats and live board read passed. Windows/Linux repeat uses an explicitly owner-approved Full grant only in each isolated fixture installation; it does not qualify restricted enforcement or change working installations. The earlier worker/reviewer and cancellation evidence above predates these access presets.

### Remaining adoption limits

This matrix is complete as an inventory, not all-green acceptance. Q4 outage/invalid-input cases, Q5 negative sandbox boundaries (outside writes, symlinks, Git common-dir, network denial), hard service/client crash and interrupted-launch recovery, and actual web/mobile/Desktop list readback remain unqualified. The native restart probe verifies deferral, not a full installed-version upgrade. Windows Codex workspace-write remains blocked by the previously recorded native ACL error. Native Remote Control registration does not prove Desktop integration, permanent connectivity or an OS sandbox. Production adoption must retain these distinctions; Linux currently qualifies Codex only.
