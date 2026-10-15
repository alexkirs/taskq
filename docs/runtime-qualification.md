# Runtime qualification matrix

Verify supported TaskQ capabilities through observable outcomes. [R9, R12 and runtime contract](../taskq.md) own policy; [specq verification](https://github.com/taskqorg/specq/blob/main/engineering/verification.md) owns test method. This matrix does not grant access or turn pending permission designs into shipped behavior.

## Execution policy

One bounded live scenario covers each applicable runtime, role and continuation. Run it when that integration, permission configuration or client version changes, not on every commit or documentation edit. Record exact versions, effective permissions, initial state, actions, observed output and remaining gaps. A prior pass qualifies only its recorded configuration.

Deterministic checks follow [specq test classes](https://github.com/taskqorg/specq/blob/main/engineering/verification.md#test-classes). TaskQ chains: state transitions, duplicate/stale writes, admission, publication and preservation of active work. A constructor/argument-string check is not live permission or behavior proof.

## Supported clients

| Client | Worker and reviewer | Operator | Platform and boundary |
|---|---|---|---|
| Claude CLI | Supported | Full tools; real edit/check and same-session continuation passed on macOS | macOS, Linux, Windows adapters; permission mode is not OS containment |
| Claude Desktop | No TaskQ runtime adapter | No TaskQ runtime adapter | Manual manager use is separate and unqualified here |
| Codex CLI | Supported | Full tools; configured sandbox; real edit/check and continuation passed on macOS | macOS, Linux, Windows; effective sandbox and network are independent |
| codex-app | Opt-in worker/reviewer | No separate app operator | macOS, Windows (#989: handshake, shell-opened thread, 21 KB queue, one `task_complete` live); app policy and login differ from handshake environment |

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
| Q2 | Worker and reviewer receive header, current contract, ten core bodies and index at start/resume/compaction (#1156; earlier evidence: four bodies) | Rule delivery chain; exact transcript comparison | Windows CLI #965 and macOS app #966 live; macOS CLI #961 partial matrix; Claude #969 live |
| Q3 | Worker proves before/after; reviewer independently reproduces and separates fit, quality and rollback | Same live fixture as Q1/Q2 | Codex #961/#965/#966 live; Claude #969 live; never infer all future rule compliance |
| Q4 | Failed rule fetch prevents launch, not a rule-free fallback | Rule delivery failure cases | Error/empty/oversize loader cases modeled; real timeout and invalid UTF-8 adapter faults not yet qualified |
| Q5 | Allowed workspace work succeeds; forbidden operations are actually denied | Native negative fixture, not prompt acknowledgement | Read/write, child, symlink, Git common-dir and network matrix pending per effective policy; broad read denial is not promised by workspace-write |
| Q6 | Operator performs authorized work with tools and resumes the recorded session | Ignored live operator chain; random canary plus independent file readback | Claude macOS/Windows and Codex macOS passed native read/write/command and same-session continuation; Windows Codex sandbox setup blocked; no claim of OS containment for Claude |
| Q7 | Only validated operations on registered projects apply once at the shown revision after confirmation | Operator proposal/apply chain; dispatch revision/idempotence chain | Deterministic pass; not protection against arbitrary same-user code |
| Q8 | One live attempt; stale output cannot change task state | Two-host claim, golden transitions and dispatch chain | Deterministic pass; fencing does not cancel external side effects |
| Q9 | Pause/load/capacity stop admission without stopping current jobs | Admission and service gate chains | Deterministic pass; live fixture must preserve a specifically owned active process |
| Q10 | Update preserves active work | Unix process handover chain; Windows idle update #965 | App/operator restart now waits; real app turn survived a pending restart and completed its file write; native Windows active-child update remains unqualified |
| Q11 | Cancellation has honest semantics | Process adapter; app completion/lease chain | App turn cannot be stopped by existing adapter; late board writes fenced only |
| Q12 | App runtime is selected explicitly on a capable host | Service opt-in chain; CLI create flow | CLI create/readback now covers app label; service admits app only on macOS and Windows with explicit opt-in |
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

## macOS evidence tasks

Moved from taskq.md § 9. A macOS evidence task (`tray --snapshot` PNGs, macOS-only host tests) is added with `--host mac --runtime claude`: the Claude worker produces them under unchanged access (#1028); Codex workspace-write does not (#960, #1021).

## Codex app environment

Moved from taskq.md § 8 codex-app environment. The app turn's network comes from the project root's owner-provisioned `.codex/config.toml` permissions, not from the handshake's `-s`/`-c`; qualified live on macOS in #952 (keyring token, no device login; with the token hidden the turn sent a worker `ask`, despite gh suggesting `gh auth login`). Other OSes stay unqualified until their own live app turn.

## Operator qualification command

Run once per changed native integration, from the candidate checkout:

```sh
TASKQ_QUALIFY_RUNTIME=claude TASKQ_INCIDENT_REPORTS=off cargo test --locked live_operator_matrix -- --ignored --nocapture
TASKQ_QUALIFY_RUNTIME=codex TASKQ_INCIDENT_REPORTS=off cargo test --locked live_operator_matrix -- --ignored --nocapture
```

For an owned fixture selected by `TASKQ_QUALIFY_ROOT`, set `TASKQ_INSTALL` to its `fixture-install` directory and provision the required board credential privately there. Provision any required TaskQ grant explicitly in that disposable `worker-policy.json` first; the test never grants Full itself. The chain keeps its fixture and native logs in `.taskq/qualification/`; no global settings, existing jobs, board writes or commits change. With the explicit qualification grant, the Claude fixture uses Bash/PowerShell on Windows and Bash/Write on macOS in its own `.claude/settings.local.json`; this grants no OS containment. It checks random file contents independently, verifies the session ID on continuation, then disables only its fixture runtime and requires failure before launch. It does not prove every permission boundary or MCP tool. Normal `cargo test --locked` skips these paid calls.

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
| Linux WSL / Claude 2.1.299 | [#1050](qualification-1050.md): owner-approved Full grant on a WSL test root only; candidate `4086fee` updated from 4.1.11; one research worker and a Claude reviewer on `wsl` closed [#1097](https://github.com/taskqorg/taskq/issues/1097) | Not tested | Restart stayed pending (`waits for #1097`) during a live sleeping worker, re-exec after close; restricted presets, code tasks and non-WSL Linux not qualified |

The controller compared all four pinned specq bodies against **each actual start and resume input** of both roles on all three qualified paths. macOS #1001 sessions: worker `76302b27-299f-4cf0-bca6-d7aa50e4f979`, reviewer `6f7f59bd-4911-4c7f-91d9-02bbda9fe0ee`; Windows #996: `11e69922-c85c-4eca-8053-f76e5a99d7c7` / `794b2388-0365-4f3b-a41a-8a640965c9c6`; Linux #997: `01a1388f-3272-7553-abea-522725664624` / `01a13890-283d-7b23-a79b-f2be73165e84`. The probes intentionally fail before and pass after; agents must report that distinction without changing the supplied fixture. Research closure qualifies this scenario, not CI or publication of new code.

Native edge cases found and fixed: `--bg` owns the UUID; passing options on resume can copy the session; stop acknowledgement precedes actual process exit; startup can lack a PID; Windows restore briefly reports a temporary UUID until native status appears. TaskQ preserves the background ID, waits for restored identity, reads actual turn completion, waits for owned process exit, and never accepts a copied continuation. It streams the matching native transcript into the existing attempt log. Changed saved permission mode refuses continuation instead of silently replacing the session.

### Integration with access presets

Candidate `5297419` includes main through `c6d46bd` (access presets). Native Claude start and resume use the same effective permission mode and Seatbelt wrapper as the other adapters. A changed saved native permission mode refuses continuation. macOS operator passed again under the default TaskQ Seatbelt wrapper with native dontAsk and the approved fixture Bash/Write grant: session `d321a639-71a2-4e19-aca8-3758a3398354`, actual edit/check and same-ID continuation. Model suite: 139 + 5 passed; selftest: 65 traces, formats and live board read passed. Windows/Linux repeat uses an explicitly owner-approved Full grant only in each isolated fixture installation; it does not qualify restricted enforcement or change working installations. The earlier worker/reviewer and cancellation evidence above predates these access presets. Final operator repetitions passed on Windows Claude 2.1.299 (`258379ed-c802-40f8-abdf-4df67b188d8b`, 51 s) and Linux Codex 0.162.0 (`01a138a7-3eed-7b33-a742-ceeb0f3d5b40`, 33 s), including same-ID continuation and disabled-runtime refusal. Windows model checks: 134 + 5; Linux: 118 + 5. Both OS actually refused before launch without a matching grant. Windows native Full additionally required its interactive disclaimer; the owner-approved test accepted it without changing default mode. Initial harness failures (wrong install/PATH and stale request artifacts) were corrected only in the isolated environment; none count as a pass. Temporary board credential copies were removed after the runs.

### Remaining adoption limits

This matrix is complete as an inventory, not all-green acceptance. Q4 outage/invalid-input cases, Q5 negative sandbox boundaries (outside writes, symlinks, Git common-dir, network denial), hard service/client crash and interrupted-launch recovery, and actual web/mobile/Desktop list readback remain unqualified. The native restart probe verifies deferral, not a full installed-version upgrade. Windows Codex workspace-write remains blocked by the previously recorded native ACL error. Native Remote Control registration does not prove Desktop integration, permanent connectivity or an OS sandbox. Production adoption must retain these distinctions; Linux Claude is qualified only for the #1050 WSL Full-grant scenario.

### Linux constrained launch investigation (2026-10-14)

Codex 0.162.0 on `hermes-fra1`, working TaskQ `4.2.0-dev.1791952400`; no TaskQ grant or project configuration changed. Windows deferred by the owner.

| Boundary | Actual evidence | Result |
|---|---|---|
| Native Codex `workspace-write`, start/resume | Session `01a138cd-7bc4-7ad0-8eb2-717287eb1236`: both turns executed the fixture, wrote/read output, and received `EROFS` for the external canary; controller verified unchanged canary and same session. Four pinned specq bodies were supplied on both inputs by the probe, not TaskQ. | Native baseline only; not TaskQ acceptance |
| Entire CLI inside `codex sandbox` | Explicit profile allowed workspace/state writes and denied outside/policy/config writes. Real inner CLI session `01a138cc-d61c-7751-918b-8c5c83646055` could not execute tools: the outer sandbox masks `/tmp/codex-daemon-1000` with mode `000`, while the inner sandbox requires an owned `0700` directory. | Refused; no Full retry |
| Entire CLI inside system `bubblewrap` | Disposable runner `hermes-isolated-721881a5724b.service`; bwrap 0.9.0 failed before the CLI with `setting up uid map: Permission denied`. Kernel audit recorded AppArmor `unprivileged_userns` denial for `setpcap` and `uid_map`. | Host enforcement prerequisite blocked; no AppArmor/ACL change |

The newly installed probe-only `bubblewrap` package was removed: its PATH presence would replace Codex's previously working bundled helper. Native `codex sandbox -P :read-only --include-managed-config -- /usr/bin/true` passed again afterward. Working services were not restarted. TaskQ's Linux admission refusal remains: do not convert the native baseline into a protected policy-store claim, or enable a preset without the complete boundary. These probes do not qualify Claude, network denial, symlink escapes or production adoption.

### Linux TaskQ constrained acceptance (2026-10-14)

Supersedes the TaskQ launch blocker above, not the other platform limits. Candidate `9c7f62d80694210ad7198573de55dedb00fbb132`, build `4.2.0-dev.1791956000`, Codex CLI 0.162.0 on FRA. Root-owned `taskq-sandbox` and private bwrap 0.9.0-1ubuntu0.3 have mode 0755 and no setid/file capabilities; only the helper's AppArmor profile was loaded. Global `apparmor_restrict_unprivileged_userns=1` and the pre-existing native Codex bwrap profile remain unchanged. No system/PATH bwrap was installed. Helper and fixture CLI SHA-256: `247c037977a67a0a5f207afc0f3187c9e76f374c3bce447b7da264bde11eae93`.

| Real TaskQ path | Native sessions | Observed acceptance |
|---|---|---|
| Restricted worker / reviewer | `01a138ed-5a1d-7db2-9ddc-66923e0dc297` / `01a138ed-b328-79a0-956e-76efe654a1ee` | Both execute commands on start and same-ID resume; project writes and all five negative canaries denied |
| Workspace worker / reviewer | `01a138ee-1d76-7a11-931a-ed2dbc040598` / `01a138ee-8219-76d2-8b57-541552305efb` | Both execute commands on start and same-ID resume; project writes succeed; outside, installation-policy directory, project config, symlink and child writes denied; controller reads all canaries unchanged |
| Revoked continuation | All four sessions above | Fixture grant removed after completion; production adapter refuses next continuation before spawn; original fixture grant restored |
| Workspace operator | `01a138ee-ba08-7b53-abb3-f36adbaf78c5` | Real read/copy/check, same-ID append/check and disabled-runtime continuation refusal |

The controller compared all four pinned specq bodies with all eight actual worker/reviewer inputs. Every role/mode reported the injected empty-case regression (`expected=0`, `candidate=9`) despite exit 0, distinguished it from passing active/repeated cases, and stated evidence limits. This proves application for the supplied scenario, not universal rule obedience. The role check calls the production spawn/resume adapter; it does not repeat board claim/ask/result/close lifecycle qualification.

Retained checks: ignored `live_linux_boundary_matrix` with `TASKQ_QUALIFY_SANDBOX=read-only|workspace-write`, plus existing `live_operator_matrix`. Role fixture uses `TASKQ_QUALIFY_ROOT`, `TASKQ_INSTALL`, `TASKQ_QUALIFY_EXE`; provide a disposable registered root, existing `.codex`, native login/config and an explicit fixture-only project grant. Operator fixture additionally needs its temporary board credential; remove it after the run. Paid checks are manual, not per-commit CI. Linux runner journals: `hermes-isolated-9c0d44f58a61.service` (both role modes, 95 s), `hermes-isolated-0dcd6ad4b338.service` (operator, 51 s). Linux release checks: 127 + 5 + 1; macOS: 148 + 5 + 1. Both selftests passed 69 traces/formats; only macOS additionally read the live board.

Working serverq/dashq/agentq/taskq services retained PIDs 1391794/1391815/1391839/1391866, config hashes and no policy file; gateway PID 204574, zero restarts. No Full, working grant, queue activation or service restart. Initial DNS mount and missing fixture Git-directory failures were corrected and repeated; neither counts as acceptance. The private `/run` now preserves only the canonical DNS file read-only.

Still unqualified here: Linux Claude (not installed), Windows constrained mode (owner deferred), Git common-dir probes, hard crash/recovery and production queue adoption. Native Codex sandbox prerequisites must already work on the host. The sandbox permits normal provider transport and documented state/cache writes; it is not a per-project account or a complete network isolation boundary.


### Linux network and nested-launcher acceptance (2026-10-14)

Extends the same ignored role matrix; no production code, launcher, AppArmor or permission change. Six real Codex 0.162.0 sessions, each start and same-ID continuation (12 turns):

| Preset / native network request | Worker / reviewer sessions | Observations |
|---|---|---|
| Workspace / true | `01a138fd-dc4b-78f2-b7a2-1dd4456a6a49` / `01a138fe-50a3-7433-92c9-5c5580b2abf9` | TCP IPv4 and IPv6 socket with IPv4-mapped loopback reach the owned server; server independently records each unique token |
| Workspace / false | `01a138ff-18b7-7fa1-8baa-a3a1965d64ba` / `01a138ff-927f-7153-b06e-cb71575311fa` | Both socket families return EPERM; server records no negative-case token; ordinary project write succeeds |
| Restricted / false | `01a13900-60c6-7ac3-98d1-4a313c7a3a15` / `01a13901-2725-7691-9ce1-9d29a789d3a9` | Same network denial; project writes also denied |

Each actual agent command invokes the installed root-owned launcher again, requesting Workspace and native network access with no Full flag, and instructs nested Codex to write the outside canary. All 12 invocations refuse at the trusted-helper ownership guard inside the inherited user namespace, before nested Codex starts; independent outside/policy/config canaries remain unchanged. This qualifies refusal of this nested-entry bypass route. Successful nested operation and all conceivable alternative escape routes are not claimed. Assert the specific guard diagnostic, not just a nonzero exit.

Run the existing ignored `live_linux_boundary_matrix` with `(TASKQ_QUALIFY_SANDBOX, TASKQ_QUALIFY_NETWORK)` = `(workspace-write,true)`, `(workspace-write,false)`, `(read-only,false)`; default network request is true. The matrix owns an ephemeral loopback TCP listener and receipts; timeout alone never qualifies denial. Native network-off is a command sandbox restriction: provider transport and MCP processes are outside that network guarantee. No whole-CLI network-off, UDP traffic or public destination qualification is inferred.

Runner `hermes-isolated-ae11c5ab0d09.service`: success, 266 s, no OOM. Test artifact SHA-256 `57d4485f1127d7e78dce81b4a0453c3a32fca28f36d42ee9903a89be03b7b2de`; production helper retains its earlier hash. Linux deterministic checks: 127 + 5 + 1; 69 offline traces/formats pass. Working TaskQ project hashes, four service PIDs and absence of a working policy store remain unchanged; no Full, additional working grant, config edit or TaskQ restart. Gateway PID changed from 204574 to 1566248 (active since 05:59:08 UTC, NRestarts=0) during the run; this test issued no gateway operation, and its cause is unknown.

### Windows constrained prerequisite repeat (2026-10-14)

Owner closed Codex; its app/node_repl processes disappeared. Windows 24H2 build 26100.4484, standalone Codex CLI 0.162.1; bundled CLI 0.162.0-alpha.17.2, existing sandbox service from app 26.1007.2314.0. No Full or working TaskQ grant/config/update/restart.

| Native boundary / context | Actual observation | Qualification |
|---|---|---|
| Existing elevated backend from SSH | Runner pipe-in timeout, standalone and bundled CLI | Failed prerequisite |
| Unelevated command profile | `cmd.exe` writes project; outside canary outside system temp and project `.codex` remain unchanged. Private temp avoids the built-in workspace profile's ambient temp write exception. | Command/file probe only |
| Whole CLI / PowerShell inside restricted token from SSH | Both exit 0xC0000142; API turn cannot start | Failed prerequisite |
| Ordinary interactive user via owned temporary task, RunLevel Limited | Both backends launch isolated `codex --version`; outer profile and native workspace-write stay enabled, with no Full. A separate launcher state avoids blocking the CLI's own state writes. | Process startup only |
| Real CLI turn inside the boundary | Session `01a13946-96c8-7901-ae1c-5a3e079b8390` starts but fails workspace routing discovery before any agent command. Model refresh and MCP HTTPS requests also fail. Existing auth file has a valid access-token lifetime; no device login. Control curl HTTPS fails with Schannel SEC_E_NO_CREDENTIALS (0x8009030E) under both legacy backends. | Agent work/start-resume chain remains blocked; not a TaskQ adapter pass |
| Explicit native MXC probe | `native MXC is unavailable on this executor`, exit 1 | Failed prerequisite |

The [official Windows sandbox guide](https://learn.chatgpt.com/docs/windows/windows-sandbox#mxc-compatibility) names builds 26100.9278 / 26200.9278 as MXC compatibility minima, with required capabilities also subject to rollout/policy. Updating Windows permits a new MXC probe; it does not prove availability or fix the observed legacy HTTPS failure. Applying an OS update/reboot while work runs is outside this repeat's authorization. An isolated CLI TLS fix is a separate dependency decision, not permission to disable TLS validation or downgrade sandboxing.

TaskQ Windows Restricted/Workspace admission remains refused until the complete production adapter chain passes; no new enforcement path was shipped from partial evidence. Retained no new tests or mechanism: temporary diagnostics answered prerequisites only. Owned scheduled task and copied auth/config removed after completion. Working services retain PIDs 22516/9832/35404/27504/28064/33804 and initial project hashes; #988/#989 retain attempts 6160769561/6160834193 and PIDs 6832/35088. Working `worker-policy.json` remains absent. Local evidence: `%LOCALAPPDATA%/Temp/taskq-windows-restricted-20261014/project/real-native.{out,err,exit}`, `native-network.{out,err,exit}`, `elevated-interactive.{out,err,exit}`.

### Windows CLI TLS investigation (2026-10-14)

Owner selected a TaskQ-scoped TLS fix without an OS update. Stock Codex 0.162.1 already has one usable transport path: `CODEX_CA_CERTIFICATE` selects Rustls in the shared custom-CA HTTP builder; HTTP and Responses WebSocket paths retain native roots and add the PEM certificates. Source: [tag rust-v0.162.1](https://github.com/openai/codex/tree/rust-v0.162.1), commit `092d3acd6bec3e3a14bdc7e7a2810ab628ab759d`, `codex-rs/http-client/src/custom_ca.rs`, `login/src/auth/default_client.rs`, `websocket-client/src/lib.rs`. The narrow automatic TLS fallback does not cover SEC_E_NO_CREDENTIALS and is not enabled on every default API path. No CLI fork or global TLS setting needed for this experiment.

Exported 54 nonexpired self-issued certificates already in the signed-in user's Windows Root store, excluding its Disallowed store; no new root installed. PEM SHA256 `62c8cd6ee951228f126eec08aaecc225299096b3d59689e1bddae1684381150c`. The disposable launcher profile marked the PEM read-only; no independent PEM-write probe was run. Environment override existed only in the owned diagnostic task. No existing CA override was changed in a working project. This is a trust-store snapshot, not proof of Schannel-equivalent trust, revocation or automatic root retrieval: stock HTTP dependencies also enable Rustls WebPKI roots. Production adoption must resolve that trust-policy difference, not infer equivalence from the three negative probes.

| Chain | Observed result | Qualification |
|---|---|---|
| Reqwest 0.12.28 transport control outside sandbox | Schannel reaches chatgpt.com; Rustls completes TLS and receives HTTP 403 | Transport only; HTTP 403 is not API acceptance |
| Same probe inside native `unelevated` boundary | Schannel fails SEC_E_NO_CREDENTIALS; Rustls without exported roots fails UnknownIssuer; Rustls with the existing-root PEM completes TLS and receives HTTP 403 | TLS workaround proved for this boundary |
| Certificate rejection with the PEM | Expired, wrong-host and self-signed badssl endpoints rejected, inside and outside the boundary | Date, name and issuer checks retained; revocation untested |
| Stock CLI start and continuation inside the boundary | Native session `01a13a91-e5ad-7a32-a1ac-40b9fbde0a4c`; both turns receive model output and complete with exit 0 | API/start/resume transport proved; not successful agent work |
| Allowed edit/read and forbidden-write attempts by that agent | Inner native workspace-write tools fail before execution with `CreateRestrictedToken failed: 87`; patch also fails. Proof file absent; both controller-read canaries unchanged | Tool chain blocked; unchanged canaries do not prove filesystem denial |
| Native `elevated` repeat | Setup refuses runtime ACL validation on a busy app `node_repl.exe`; app agents are active again | Prerequisite blocked; no process stopped or setup bypassed |

Outer named profile and inner workspace-write stayed enabled; no Full or unsandboxed retry. Separate launcher state and original inner CLI home retained. TaskQ's Windows adapter remains unqualified/refused: restoring TLS does not resolve nested token creation, shared runtime setup, or qualify worker/reviewer rules, tools, network denial and nested-launcher containment. No production integration or new persistent test shipped. Follow-up must fix/qualify the native execution path before adopting this child-only CA setting; never use a writable project PEM or overwrite an owner's explicit CA policy.

Owned task and temporary profile/PEM removed when idle. Working project configuration hashes, service PIDs and versions match this repeat's initial readback. Working `worker-policy.json` now exists, unlike the previous repeat; this investigation never wrote it and has no initial hash to qualify preservation of its contents. Logs: `%USERPROFILE%/taskq-tls-probe-20261014/{tls-*,tls-ca-*,agent-start.*,agent-resume.*}`. Stock CLI SHA256 `dd13bdb162a184fa04039488b234a8258ef9b76f84e926c505d5742b19249a6a`. Development probe SHA256 `80393d78ddca3098801e287c9927da412c417ef292b90e0b49175b6902da6468`; exact Reqwest 0.12.28, explicit `use_rustls_tls()` and `add_root_certificate`; no disabled certificate validation, proxy or client identity.

### Windows nested native sandbox and platform TLS candidate (2026-10-14)

This supersedes the PEM workaround as a candidate, not as a working-installation migration. Owner authorized an isolated TaskQ adapter repeat while Codex remained open; no Full, working-project grants, service restart or OS update. Only an explicit **project Workspace grant with network enabled** selects this path. Restricted, read-only narrowing and network-off requests refuse before launch; a missing or changed native dependency also refuses. Unknown rows below remain unknown.

The native fix is [windows-codex.patch](runtime/windows-codex.patch), against [Codex rust-v0.162.1](https://github.com/openai/codex/tree/rust-v0.162.1), source commit `092d3acd6bec3e3a14bdc7e7a2810ab628ab759d`. Reapplying `WRITE_RESTRICTED` to the inherited write-restricted token produces Win32 error 87. Removing that flag changes read restrictions, so the patch instead duplicates an inherited token only when its nonempty restricting SID set is contained in the requested set and it retains no privilege other than `SeChangeNotifyPrivilege`. A narrower request refuses. Actual SDK AccessCheck probes preserved read=true/write=false; a broader capability request did not grant its added write capability. See [CreateRestrictedToken semantics](https://learn.microsoft.com/en-us/windows/win32/api/securitybaseapi/nf-securitybaseapi-createrestrictedtoken).

TaskQ's native control home lives under protected `windows-native/state`, separate from the active `CODEX_HOME`. Outer launcher and inner tools share its capability mappings; the owner process prepares canonical workspace aliases for subdirectory commands. The bootstrap accepts bounded exec arguments, rechecks project authority and binary hashes, then restores the original runtime home. Changing child environment or clearing TASKQ_ATTEMPT never changes its inherited token. Existing runtime settings/auth and project `.codex` remain read-only. This is a same-user write boundary, with the indirect-execution limitations in §8 Access; it does not isolate user accounts or promise broad read denial.

HTTP and default Responses WebSocket TLS use the existing Windows platform verifier, not a root export or disabled validation. Explicit owner CA/client-identity paths remain intact. Controlled Reqwest 0.12.28 probes both outside and inside the unelevated boundary reached chatgpt.com (HTTP 200) and rejected expired, wrong-name, self-signed and revoked certificates. [Windows platform verification](https://docs.rs/rustls-platform-verifier/0.7.0/rustls_platform_verifier/) uses OS trust/revocation APIs; this was stricter than the Schannel control, which accepted the revoked endpoint. These probes are not proof of every enterprise CA, offline revocation condition or separate helper transport.

Private dependency provenance:

- Patched main `codex.exe`: SHA256 `af65880153a66852d72edf1d2a22faf191abeee2ad3d9cd6e8ee1d59183b8cc4`, version 0.162.1, Windows GNU rustc 1.99.0. Build from the pinned tag plus the patch with `cargo build --locked -p codex-cli --bin codex`; the tag's lock metadata lists local packages as 0.0.0 while its workspace is 0.162.1. Before building, normalize only source-less workspace package versions to 0.162.1; preserve every external package/version/source. This is metadata repair, not a dependency update. Debug profile disables debug info and incremental compilation; one build job, BelowNormal priority.
- Official **same-version** npm `@openai/codex-win32-x64` MSVC `codex-code-mode-host.exe`: SHA256 `9f865a748c984492ca9b69aac75680e4954f31311c43ec33ddbdae0ed77bd4bd`. GNU helper build is unavailable because rusty_v8 has no Windows GNU archive. The official helper forwards native tool calls to the patched main CLI; model, effort and code mode were retained. Both artifacts must be placed in the operator-owned installation's `windows-native/`; TaskQ pins both hashes and never downloads/replaces them automatically. This mixed private/official dependency bundle is not the stock CLI and is not a generally distributed release.

Retained proof is one ignored `windows_sandbox::tests::live_windows_boundary_matrix`, using the production `runtime::spawn` and existing rule loader. It needs an isolated TASKQ_INSTALL, TASKQ_QUALIFY_ROOT, native Python path TASKQ_QUALIFY_PYTHON, runtime login and explicit fixture-only project grant. Invoke `cargo test --locked windows_sandbox::tests::live_windows_boundary_matrix -- --ignored --exact --nocapture --test-threads=1` in the interactive Windows user session. The test owns canaries/junction only in that fixture. It verifies native completion rather than the retired process's exit code: existing TaskQ completion handling can terminate a lingering process after `turn.completed`. Native `turn.failed`, missing commands or missing effects still fail. No paid matrix per commit; the existing update-preservation chain additionally protects `windows-native/state` from sweep deletion.

| Observed chain | Result and limit |
|---|---|
| TaskQ worker start + same-session continuation | Passed, `01a13b24-8db0-7003-8f2f-944825d061d4` |
| TaskQ reviewer start + same-session continuation | Passed, `01a13b25-283e-7211-a7ae-fa9786584ffa`; entire live matrix 93.68 s |
| Rule delivery + application, all four turns | Controller matched full pinned bodies in each native user input, not log labels. Both agents compared active/repeated/empty behavior and detected the injected empty=9 regression on continuation despite command exit 0 |
| Allowed/denied effects, all four turns | Project write/readback passed; outside, installation, project `.codex`, active runtime config and junction-target writes denied with errno 13 **and native Win32 5**. Child write denied, ACL mutation exited 5, subsequent write denied; every controller-owned canary unchanged |
| Nested launcher request for wider writable roots | Actual TaskQ bootstrap refused the unsupported config before a model launch; this proves the argument/authority guard, separately from the filesystem-denial probes |
| Grant removal after each role | Production spawn refused continuation before launching another agent; fixture grant restored by controller |
| Whole-process network-off / Restricted / narrower native read-only / Elevated / MXC | Unqualified; narrower/network-off admission refuses. No fallback to Full |
| Working queue/publication, Windows Claude and codex-app enforcement | Not exercised by this candidate; no claim that this standalone Codex path fixes those runtimes |

Initial development failures were not passes: missing code-mode host, checking a retired process's exit instead of native completion, and selecting a normal executable rather than the test harness. The final harness was verified by its listed test identity with required adjacent DLLs before the four-turn run. Source/binaries and evidence reside only in owned isolated installations; the working app and services were left running.

Admission controls read back through the actual TaskQ `service permissions` path also refused read-only, Restricted, network-off, missing helper and changed helper in the idle isolated installation. Requested files and grant were restored between probes. Final deterministic checks after rebasing on current main passed: macOS 159 + 5 + 1; Windows 155 + 5 + 1 + 1 (paid matrices ignored). Selftest passed 69 traces/formats on both; macOS read the live board, Windows used explicit offline mode. Initial Windows diagnostic-suite failures came from an archive build embedding revision `unknown`; restoring the public Git metadata and invalidating that build script fixed them without changing or weakening tests. The paid four-turn result precedes that rebase; the standalone Codex adapter/rule path stayed the same.

Preservation readback: working `worker-policy.json` and native auth hashes match the initial snapshot. Working service versions/PIDs/project configs and global Codex config changed independently during qualification, so their snapshot is **not** evidence of unchanged installations. This work wrote only its isolated candidate/fixture and did not signal working services/app agents or change their rights. The app still had live processes. Owned scheduled probes were removed only when idle; temporary login/config copies were removed and fixture admission disabled after qualification.

## Contract detail

Moved verbatim from `taskq.md` (#1179) to keep the contract under its R8 size bound. `taskq.md` keeps the decision and links here; this text keeps the same authority (R8, R13).

| Codex app | Codex's spawn with a handshake brief; after it exits `open codex://threads/<id>` and `codex queue --thread <id> --message <brief>`; Windows opens the URL through the shell (`cmd /C start "" codex://threads/<id>`) and runs an npm `codex.cmd` shim's `codex.js` through `node` (no `cmd` line limit) | `open` and `codex queue --thread <id> --message <text>`, Windows the same way |
The child's environment drops `CLAUDE_CODE_SESSION_ID` and `CODEX_THREAD_ID` and adds `TASKQ_TASK`, `TASKQ_ATTEMPT`,
`TASKQ_ROLE`. GitHub service-spawned children also receive the host board token as `GH_TOKEN` and `GITHUB_TOKEN`, overriding inherited values. Briefs go through stdin, never a shell (`codex-app`: the `queue` argument, still no shell; the app turn resolves its own effective permissions and environment; handshake options alone are not a grant or qualification, so its brief prefixes each taskq command with `env TASKQ_*=..`, on Windows the PowerShell form `$env:TASKQ_*='..'; & '<taskq>'`). The app runs while its process is listed: macOS `lsappinfo` `com.openai.codex`, Windows `tasklist /APPS` package `OpenAI.Codex_`; rollouts are under `CODEX_HOME`, else the home folder's `.codex`. Codex app turns/resumes inherit the app's host login environment: `gh` there needs the host's durable token as its keyring login or `GH_TOKEN`/`GITHUB_TOKEN`, provisioned by the owner (never `gh auth login`); taskq does not inject it through queue, briefs, argv or logs. A turn that finds no token, or would need a device login, sends a worker `ask` naming the missing token. Its network comes from the Codex config of the project root (owner-provisioned `.codex/config.toml` permissions), not from the handshake's `-s`/`-c` ([live evidence](runtime-qualification.md#codex-app-environment)). Live app-turn qualification is per OS; a handshake check alone does not qualify it (R12). Logs: `<root>/.taskq/<T|R><N>-<attempt>.log`.
**Claude background turns.** Native `agents --json --all` plus the matching transcript determine completion, never launcher exit or PID alone. Stop only the owned idle turn after its new response is recorded; keep its conversation for continuation. Unknown state remains live/unknown; completion must be newer than launch, including repeated identical input. Resume waits for confirmed stop and must retain the native ID; a copy is an error, never a new attempt. Native saved permissions persist on resume; changed permissions require a new attempt. Legacy print-mode attempts finish and resume unchanged. Active background turns defer service re-exec; existing jobs are not restarted. Remote Control is requested by default, subject to native trust/account/policy; its actual remote ID is distinct from the local UUID, so missing registration is unknown, never a fabricated link.
Rule delivery follows the [current contract detail](#rule-delivery-contract-detail). Earlier qualification above does not prove the current delivery behavior.

2. `taskq update` and service step 7 run the coreq update. A new `current` then gets one allowlist sweep of the rest of `<install>` and a `~/.local/bin/taskq` v3 python shim. Kept: `releases/` of `current`, `previous`, the running version and each `projects` root's `status.json` version (its service and live children), `current`, `previous`, `projects`, `token`, `installation-id`, `connection/` ([DashQ connection](design/enrollment.md)), `worker-policy.json`, `windows-native/`, the launcher, `WebView2Loader.dll`, `taskq-tray.exe`, `*.log`, `tray*`, `capacity/`, `resources/`, `TaskQ-*.xml`, `*.tmp`. A file in use stays until a later update.
3. At step 7, an installed service whose running version is older than `current` logs `current is <v>; restarting` and re-execs through the launcher without downloading or running selftest. The service re-execs; live CLI children pass to the new image through `<root>/.taskq/kids`. An active Claude, codex-app or operator turn defers re-exec until it ends; the restart request remains pending. Windows: exit 75 once no child is live; the coreq launcher restarts it. A pending restart skips update checks, logs `restart to <current> waits for #<N>, ..` once per changed set and lists it in `status.json` `restart_waits` (null when none) and `service status` `Restart: waits for ..`.

## Windows large background input (#1223)

Native adapter probe: Windows, Claude Code 2.1.300, `dontAsk`, unchanged model and effort. This qualifies the file-reference transport and native session continuation; it is not a board/service-pass or other-platform qualification.

- Before: send a 100 KB brief to `claude --bg` on stdin. Owned job `e06f829b` repeatedly logged `ENAMETOOLONG: name too long, uv_spawn` before any tool call; stopped the probe.
- After: write an immutable UTF-8 brief file and send the section 8 read-file instruction on stdin. Native job `f30db1b8`, session `f30db1b8-d2db-4a6c-97fb-03cbae21d89c`, read the 102,621-byte start file as its first tool call. It then read the marker named only by the EOF instruction and returned the random EOF canary plus the marker contents.
- After native completion, stop and read back no PID; resume without settings flags and with a fresh 102,607-byte file. The same job/session read the new file, read the new EOF-instructed marker, and returned its fresh canary and marker contents. Passing settings flags instead created a copy; that owned probe was stopped.
- The launcher probe used Rust `std::process::Command`, the same native CLI argument construction and piped stdin as `ClaudeTurn::start`. It used isolated fixture files; no board writes, grants or global settings changed. The GNU development executable needed Cargo's runtime PATH; this is not release-artifact qualification.

Regression: `cargo test --locked claude_large_input_stays_out_of_argv` covers greater-than-100-KB UTF-8 payloads, short stdin references and distinct immutable files for starts, continuations and concurrent operator inputs. No existing test or golden trace changed. Rule amended: R13, section 8 runtime table and Rule delivery. Failed/stopped-job detection and lease release belong to #1208 and are not proved by this probe.

## Rule delivery: contract detail

**Rule delivery (R13).** Every worker, reviewer, operator and PM start, resume and compaction receives the current project contract and ten mandatory core bodies from one freshly selected specq revision, before the task text. Registered `taskq.json` repository basename selects `<root>/<repository-name>.md` (nested forge namespaces supported); missing, unreadable, invalid UTF-8 or empty non-TaskQ contracts refuse launch. Only unregistered setup roots and registered TaskQ may use the release TaskQ fallback. The header names revision, cache, project contract, precedence (project overrides shared), evidence requirement and the separate TaskQ command-contract reference. Known other shared rules and project `docs/` remain indexed by path/heading for demand loading. Continuation bodies supersede earlier versions; an index alone is insufficient after a change or compaction. This retains core-plus-index delivery but repeats mandatory bodies, so live context/compression cost remains to be qualified. The complete payload stays within128KiB; invalid/unavailable rules or documents refuse launch. Claude background and Codex App receive only a short reference to an immutable UTF-8 file in `<root>/.taskq/inputs/<uuid>.txt`, requiring a complete read before work and reporting read/truncation failure. Content never enters their native launch/queue arguments. Each adapter holds the existing input lock until the turn ends; hourly retention keeps the newest50 plus locked files. Inputs are rebuildable caches, never task state. Codex CLI uses stdin. Claude new sessions install the SessionStart compact hook; resumes preserve that native conversation/hook. PM hook: `taskq rules pm start|compact`. Shared source is the published specq `refs/heads/main`, resolved from bounded HTTPS Git protocol-v0 reference advertisements without a REST request per turn. Before each rule payload, accept a valid local source observation only while its age is below60s; stale, future-dated, missing or invalid observations require a successful refresh. Failed/ambiguous/malformed discovery refuses launch without a release-pin fallback. Capture one selected40hex SHA for the entire payload, cache namespace, file URLs, metadata and header, so a moving branch cannot mix bodies. Cache observations record device UTC seconds; a backwards clock invalidates a future observation. For each selected SHA, fetch/cache the recursive Git tree once through the GitHub Git-data API. Refuse truncated/oversized/invalid trees, duplicate or unsafe portable paths, nonregular eligible Markdown entries and missing mandatory core files. Discover other Markdown files dynamically (excluding README/AGENTS/SECURITY and scripts/vendor/audits/docs). Verify each body against its Git blob SHA from that tree before delivery; no mixed-revision or unchecked fallback. Keep the ten mandatory paths as the compatibility contract: a renamed/removed mandatory file refuses launch. New eligible indexed files need no TaskQ rebuild. Tree cache uses the same revision/path/SHA256 checks as bodies. Each cached shared file has metadata binding revision, relative path and SHA-256 of its UTF-8 body. Missing, malformed or mismatched metadata/content refetches the immutable HTTPS URL; unavailable repair refuses launch. Git blob checks also reject a changed body with coherently rewritten body metadata. Editable source/tree caches are not an authenticity boundary against an actor able to rewrite the whole cache. Reads/fetched bodies are limited to128KiB. Concurrent cache publication uses unique staging names and atomic replacement; interrupted pairs are invalidated/refetched. Delivery logs are transport evidence, not proof that a model applied the rules; real behavioral canaries remain required.

### Input-cache retention review fix (#1223)

The §4 Data pruner now keeps the newest 50 input files plus files with a live Claude turn's shared lock. Starts, resumes and operator turns hold that lock in their process adapter; dropping the adapter releases it. Shared locks allow Claude to read the input on Windows. Unreadable or un-lockable files are preserved.

- Before: `cargo test --locked input_cache_is_bounded_and_live_files_stay` failed with 55 files retained instead of 51.
- After: the same check passes: the oldest locked input stays, old inactive inputs go, and releasing the lock leaves 50 files on the next prune.
- `claude_large_input_stays_out_of_argv` additionally checks that the production helper holds its input lock while the payload remains readable, and releases it on drop. No existing assertion or golden trace was removed.
- macOS sandbox run: `cargo test --locked` reached 206 passing unit tests, 2 ignored live tests, and 2 host-test failures. `host_memory_reads_on_this_host` and `seatbelt_keeps_workers_out_of_the_policy_store` cannot run here: direct `sysctl kern.memorystatus_vm_pressure_level` and `sandbox-exec` probes both report `Operation not permitted`. This is not a passing canonical gate.
- `cargo run -- selftest` passed 72 golden traces, command formats and a live board read; profiling append remained unknown. The Windows native transport probe above was not repeated for this retention change; Windows lock/read behavior still requires target-platform verification.

Amended R13: §4 Data and the linked §8 Rule delivery detail. The stale description that resumes pass `--settings` now agrees with §8: only a new Claude session installs the compaction hook.

### App lease

- **App lease.** A `codex-app` (§ 8) turn counts as the child while the Codex app runs in this host's logged-in session and the thread has no `task_complete` after the queued brief or text. A finished turn without `result`, `ask` or `requeue`, a closed app or a logout is a child exit (`gone`). Stopping only drops it from the heartbeat; the turn runs on and its late writes are fenced. Updates and re-execs wait for active app turns and never retire them (§ 8 step 3).
