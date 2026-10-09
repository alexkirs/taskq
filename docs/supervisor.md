# Supervisor: research and design (#524)

Owner decision 2026-10-09 ([#524 comment](https://github.com/alexkirs/taskq/issues/524#issuecomment-6072543589)):
one supervisor per task, full lifecycle, on every runtime (Claude, Codex, DOT).

1. The manager takes requests, sets priority, relays owner questions and gets a short outcome; code, test logs and
   retries stay out of its context.
2. One supervisor owns one task: launches and follows its worker, reviews the diff and the exact-head CI, requeues
   with fixes or merges and closes, then reports in one line.
3. The worker implements; the supervisor never edits the implementation.
4. Supervisor runtime follows the manager (Codex manager: Codex supervisor; Claude manager: Claude supervisor). The
   worker's runtime is chosen separately.
5. State and both session ids live on the board. One controller owns launch and close, so manager, tick and
   supervisor do not race.

Not accepted: a permanent global supervisor, and a fresh reviewer started only after the result (the earlier draft
of this note). The contract is `taskq.md` (R2, R3, R11, § 3, § 4, § 7); this note never overrides it. Numbers marked
*estimate* were not measured (R12).

Status: shipped by #525; live proof is #526. Owner clarification 2026-10-09
([#525 comment](https://github.com/alexkirs/taskq/issues/525#issuecomment-6073170182)): one setup and start, then the
approved queue runs by itself; the runtime adapter wakes the supervisor and starts the next task, with no
user-managed sender, timer or lifetime extension, and permissions, models and effort unchanged.

## 1. Variants studied

- **(a) Persistent supervisor per task** (#243, removed by #290): the tick spawns `S<N>`; `S<N>` spawns `T<N>`
  itself, is woken on each event, reviews, closes or rejects, lives until close.
- **(b) Short-lived reviewer per review**: a fresh session per `review`, no launch, no observe. Rejected by the
  owner: it does not launch or follow the worker.
- **(c) Hybrids.** (c1) reviewer resumed on the next review; (c2) persistent supervisor, worker spawned by the
  tick on the supervisor's order. **Chosen: (c2)**, the only variant that keeps the full lifecycle without
  starting a session from inside a sandbox (§ 3, #270).

| | Claude | Codex (local `codex exec`) | DOT (Codex cloud manager, local Codex child) |
|---|---|---|---|
| (a) supervisor spawns worker | Works: `claude --bg` from Bash under `dontAsk`. Wake by `stop` + `--resume` forks a new id (#284). | Blocked: `workspace-write` cannot start `codex` nor see other pids (#270, #502, #269 run 4b). Only `danger-full-access` worked; R9 forbids taskq changing it. | As Codex. |
| (b) reviewer per review | Works, but no launch/observe: rejected. | Same. | Same. |
| (c1) resumed reviewer | Fork on resume (#284). | `exec resume` keeps the id. | As Codex. |
| (c2) supervisor orders, pass spawns | Works; wake by its own background `taskq wait --task N` while its process runs; once it ended, the pass resumes it and records the new id (§ 4). | Works within `workspace-write` (board writes need only `gh` and network); wake by `exec resume` from the pass, same id. Its orders run at its turn's end (`tick --after`). | As Codex; the cloud manager only receives the one line. |

## 2. Old bugs, root cause, workaround

| Bug | What happened | Root cause | Workaround in (c2) |
|---|---|---|---|
| #284 | Waking a stopped Claude supervisor (`claude --bg --resume <id>`) started a copy under a new id; taskq refused it ("supervised by claude:fa18ac97: only that session decides"); supervisors renamed themselves and started helpers. | Claude resumes a stopped session as a new session (copied transcript, new id); authority was bound to the old id. | A running Claude supervisor wakes on its own background `taskq wait --task N`, same id. One whose process ended is resumed by the pass, which records the new id as `supervisor` (comment `nudge`, `supervisor <new> replaces <old>`): the gate follows the board, the old id is refused, gets `stop` from `wait --task` and is retired once stopped. Names come from the spawn (`--name`, 3f2fbc8); the brief forbids starting sessions. |
| #291 | A worker resumed after a reject became a copy that pushed but could not `take`. | Same fork, on the worker. | Rework is a new worker that continues branch `taskq-<N>` (R11, step 4); a worker is never resumed for rework. |
| #270 | Every Codex supervisor failed to spawn its worker ("no Codex app server on this machine"); the worker resume after a reject ran sandboxed too; tests archived live threads. | `workspace-write` blocks the app-server socket and process starts outside the sandbox; only `danger-full-access` worked. Tests reached the live app server (25cb09f). | The supervisor only writes its order (`taskq run N`, rework `requeue`) to the board; the pass outside the sandbox spawns. The app-server client is gone (#290); tests never reach live runtimes. |
| #207 | DOT-created Codex sessions stayed in the active UI; archive routes unqualified. | Cloud threads and app-held threads belong to another process; `thread/archive` fails "active writer" (#158). | `S<N>` and `T<N>` are local headless `codex exec` threads with `.taskq/<name>.pid`, retired by recorded id. DOT only runs the manager. |
| #290 | The supervisor was deleted in the single-file cutover. | Cost: typed supervisor, handoff races (#240 P1), fork adoption (#284, #291), sandboxed access (#270), wake logic, an authority model; about a third of the old code. | Fork recorded by the pass, not adopted by the session; no sandboxed spawn, one id gate on four commands, one block field, one brief. |
| #502 | A tick inside a sandboxed Codex session called live workers gone and requeued them. | The sandbox cannot see other processes; `alive` returned False. | No pass runs in the sandbox. Each `codex exec` turn gets a detached `taskq tick --quiet --after <pid>` started by the unsandboxed pass that started the turn; it waits for the turn to end and then runs one pass outside the sandbox. |
| #510 | A Codex manager is not woken when a background `taskq wait` ends. | Codex has no wake on a finished background command. | A Codex supervisor ends its turn and is woken by `exec resume` from the pass (a local rollout exists: `codex exec` made it). Its process exits at every turn end, so the pass reads idle from the log and rollout, not from the pid (§ 3a). The manager gets the one line through its sender (R4, #522 unchanged). |

## 3. Per runtime (c2)

| | Claude supervisor (Claude manager) | Codex supervisor (Codex or DOT manager) |
|---|---|---|
| Spawned | Pass step 3: `claude --bg --name "S<N> <ORCH> <title> (<machine>)"`, worker tools and `permission_mode` | Pass step 3: `codex exec --json` with the `codex` options, detached; `.taskq/S<N>.log`, `.taskq/S<N>.pid` |
| Launches worker | `taskq run N`; the event pass spawns `T<N>` at once | `taskq run N`; no pass in the sandbox: the pass at its turn's end spawns `T<N>` |
| Woken | Its background `taskq wait --task N` (Bash `run_in_background`) ends: same session, same id | Pass step 4: `exec resume <thread> '<event> #N: read your issue'` when no turn runs; events during a turn go at its end (`tick --after`) |
| Reaches the board | `taskq` and `gh`/`glab` through Bash | Same, network on in the sandbox (as workers' `result`) |
| Worker dies | Pass comments `gone`, wakes it; it requeues (rework) or asks; second death: `ask` | Same |
| Idle (expected) | Listed with no pid, not `failed`: resumed on the next event, new id recorded. Listed with a pid: running, its wait wakes it (§ 5.2) | Pid exited, last turn `turn.completed` in `.taskq/S<N>.log`, local rollout: never respawned (§ 3a) |
| Supervisor dies | Not listed, or `failed` with no pid. First death: respawn `S<N>`, replace `supervisor`, retire the old id; the new one adopts the live worker from the board. Second: `ask` with `tail` | Last turn `turn.failed`/`error`/none, or no local rollout. First death: one `exec resume` of the same thread when the rollout is local, else respawn as Claude. Second: `ask` with `tail` |
| CI red or merge refused | `close` sends the task back (`taskq.md` § 6); supervisor requeues with the fixes; third rework: `ask` | Same |
| Retire | Ends its turn after `close` (never stopped by it); `close` stops `T<N>`, the next pass retires `S<N>` by recorded id once stopped (R11) | `close` in the sandbox cannot kill or archive; the pass at its turn's end retires both, stopped only |
| Tokens (*estimate*) | One context per task: system + tools + `taskq.md` (~10k) + issue, then per review the diff and CI; about 50k-150k input per task, cached within a turn, re-read cold after 5 min idle on each wake | Similar; larger Codex system prompt; each `exec resume` re-reads the thread |

Manager saving (measured in #503): before compaction a manager tick cost 4.47M input tokens at 650k context, after
0.44M at 100k. Diffs and CI logs read during reviews feed that growth; with (c2) the manager reads one line per task.
Total cost rises by the supervisor sessions; no saving is claimed until #526 measures both (owner: count total cost,
not only manager context).

### 3a. Idle or dead

`alive` (§ 8 of `taskq.md`) is pid-running for Codex. A Codex supervisor ends its turn by design and waits for
`exec resume`, so pid exited is its normal idle state; reading it as death would respawn or `ask` after every turn.
The pass decides from evidence on disk only:

| Evidence (Codex) | State | Pass does |
|---|---|---|
| pid in `.taskq/S<N>.pid` running | running | an event waits for the pass at its turn's end |
| pid exited, last turn after the last `turn.started` ended `turn.completed`, rollout in `$CODEX_HOME/sessions` (the #522 `rollout` check) | idle | `exec resume` on the next event, same thread; nothing else |
| no pid file; last turn `turn.failed`, `error` or no terminal event (killed); no local rollout | dead | comment `gone` with the evidence; first death: one `exec resume` of the same thread if the rollout is local, else respawn; second: `ask` with `tail` |

These event names were read in this machine's `.taskq/T*.log` (`thread.started`, `turn.started`, `turn.completed`,
`turn.failed`, `error`). For Claude, from `claude agents --json --all`: listed with a pid is running, listed with no pid and not `failed`
is idle (resumed on the next event), not listed or `failed` is dead. Death counts reset at each `result` or `answer`, as #393.

## 4. One controller

- The board's `supervisor` field names the controller. `run`, `close` and a rework `requeue` of a supervised task
  come from that session; another agent session is refused, except the task's manager (its `pm`, #532)
  on the owner's word, and the owner's shell. Their `requeue`, `later` or `close` drops the supervisor.
- The pass never judges: it spawns, wakes, checks liveness and retires, and only on the board's word (the order,
  the event, the recorded id). The dispatch lock (#357) keeps passes from racing on one machine.
- The gate is safe because the board always names the current id: Codex `exec resume` keeps the thread id; a
  Claude resume makes a new one, and the pass that made it records it before anyone acts on it. #284 failed because
  the session itself had to adopt the new id.
- Adoption is bounded and stays with one controller: only the pass resumes, and only an idle supervisor (a running
  one wakes on its own wait, never `send`-ed). One resume per event batch (`.taskq/S<N>.seen`), under the dispatch
  lock, so duplicate events make no second copy. A new supervisor (respawn after a death) adopts the live worker from
  the board, never by name. The replaced id is refused by the gate, gets `stop` from `wait --task N` and is retired
  by recorded id once stopped (R11); a second death is an `ask`, no further resume or respawn.

## 5. Unsupported topologies (blockers, not bypassed)

1. **The first pass.** Every `S<N>`/`T<N>` turn is started by a pass, and that pass leaves the watcher for the
   turn's end, so the chain runs by itself once started. The first pass needs a process outside the Codex sandbox:
   `taskq pm` or an event command in a Claude or shell session, or the owner's start. A Codex manager whose own
   commands are all sandboxed starts no chain; that is a setup blocker reported as such, not bypassed (R9).
2. **Claude `--bg` liveness is unverified (R12).** Whether a `claude --bg` job keeps its pid while its background
   `taskq wait --task N` runs is not proved. Either way it is woken (by its wait, or by the pass's resume with the
   new id recorded); a listed job with a pid but no running wait would miss events until its next turn. #526
   records it.
3. **Manager not reachable on its runtime.** The supervisor never messages the manager; its one line goes to the
   board and the manager's `taskq wait` or a sender carries it (R4). A Codex app or DOT cloud thread with no local rollout has no
   `exec resume` route; it gets the line only through an independent Codex app sender or when next talked to (#522).
   A Claude sender reaches only Claude sessions.
4. **No manager on the task.** Supervisor runtime and machine follow the task's own manager, its `pm` on the board
   (#532), never `.taskq/pm.json`; a task with none starts no supervisor and waits (the report says `blocked (no manager)`)
   until a manager adopts it (`taskq pm --adopt N`). A Codex and a Claude manager share one checkout without taking
   each other's tasks, gate or `wait` events. The dispatch lock serializes one checkout only; across checkouts or
   machines the board has no compare-and-swap (`taskq.md` R4).
5. **Cross-machine.** A Codex supervisor is woken only by a pass on its own machine; a pass elsewhere shows its bare
   id (R6) and cannot wake or retire it.

## 6. Transition

Every task with no `supervisor` field (started before #525 shipped, #525 and #526 among them) keeps the unsupervised
path to its end: the pass spawns and follows `T<N>`, `wait` prints `review #N`, the manager reviews the diff and the
exact-head CI and closes or requeues it (`taskq.md` § 7 Unsupervised review). Every new task gets its supervisor at
step 3. The #522 sender (routes, rollout rule, one-blocker stop) is unchanged; only the set of
events `wait` prints changes.

## 7. Follow-ups

- #525 shipped § 3 and § 4: `supervisor` block field, `run`, the id gate, pass step 4, `wait --task N`, `pm.json`
  runtime and id (replaced by the task's `pm` in #532), the idle/dead state of § 3a, the Codex turn-end pass, retire by recorded id (R11); tests on fakes
  only.
- #526 live check (`--deps 525`): Claude manager and Codex manager, each a full cycle with a controlled CI failure,
  rework, exact-SHA merge, close and retirement of both sessions; a Codex supervisor idle across turns is not
  respawned, a killed one is recovered once; proves or refutes § 5.2, records the Claude idle `state` and the turn-end pass live; measures
  total cost and manager context.

## 8. Prepublication evidence (#533)

The accepted [#530 methodology](https://github.com/alexkirs/taskq/issues/530#issuecomment-6073516046)
is applied in `taskq.md` R5/R8/R12/R13, § 5/§ 6/§ 7/§ 10. That contract is the testing/publication SoT.
The worker transfers a candidate in both modes; the accepting supervisor reviews exact-SHA checks and applicable
isolated live qualification before `close` publishes. Direct mode uses the existing `taskq-N` branch without a PR;
close fast-forward pushes the accepted immutable SHA after remote-head and CI checks. PR exact-head merging stays.
Legacy already-published results and research answers close without a new publication, and prove no unobserved
live behavior. Existing claims are not rebound. No suite migration or new paid benchmark is part of #533.


## 9. Executable PM/ARM handoff investigation (#595)

Status: **incomplete; live qualification BLOCKED**, 2026-10-09 18:46 UTC
(2026-10-10 in the owner's timezone). This is a bounded research record, not a new role contract.
The owner activated #595, per-project Claude 0 / Codex 3 for taskq and csgo, and the independently
reviewed publication lifecycle. The original intake-only/later restriction is superseded. No additional
project, permission, model, effort, credential or replacement sender was authorized here.

### Observations and scope

- Fresh issue read: #595 is open `q-doing`, with distinct recorded manager, supervisor and worker,
  `order: null`, `result: null`. Its history records intake, activation, supervisor order and worker spawn.
  Acceptance 6's original `q-later`/null claim/supervisor assertion describes intake, not the activated state.
- The supplied screenshot/user report is evidence of a PM-loaded/no-workers/no-ARM interaction with
  pending decisions. It does not distinguish prompt omission from pending authorization. Claude's
  reported success remains user-reported, not an independently observed baseline.
- This worker is a local Codex CLI session with `CODEX_SANDBOX=seatbelt`; that observation does not
  identify the owner's app/DOT session. CLI version: `codex-cli 0.159.3`; installed Claude CLI:
  `2.1.295 (Claude Code)`. The isolated CLI launch inherited configured `gpt-6.1-sol`, effort `low`;
  no model turn started, so effective model/effort are not independently confirmed. Other contexts'
  versions/models/effort are unknown. No setting was changed.
- Available execution tools include shell execution and continuation of a running shell process.
  The supplied callable tool inventory has no `send_message_to_thread`. Collaboration messaging
  is not an independent app sender and is not an ancestor-wake route. No existing app sender or
  DOT cloud-parent/execution-child session was accessible for qualification. Their actual state is unknown.
- The executing checkout's limits were read as Claude 0 / Codex 3; the branch's tracked fixture
  configuration still has the repository defaults. Neither was edited. csgo was not accessed or changed.
  No real task was adopted/rebound, parked state altered, monitor replaced, wait consumed, runtime
  resumed, production tick launched, or publication/deployment performed by the experiment.

The initial candidate branch was based on `0d51c8b17840f1afa1443b6dfc74f7acfcfaedf5`.
Current prompts are reproducible from that revision's `cmd_pm`, `cmd_arm` and the contract sections
loaded by `cmd_pm`; do not duplicate the whole contract here. PM includes Principles and Manager,
then the self-arm instructions. The current ARM prefix already says:

> Explicit owner arm: execute the proven route, not just this prompt. Reuse the existing monitor and targeted wait;
> repeated arm must not create duplicates. Keep paused projects paused. Prove an idle-manager wake and the next wait;
> printed output is not proof.

This text and the contract already require action. Stronger verbs alone are not evidence that a supported
route exists or that a pending approval disappeared.

### Fixed input, exact candidate overlays and matrix

Within each topology keep the user inputs exactly `taskq pm`, then `taskq arm tick`, with the same
explicit project/scope request. For the only attempted cell that scope was:
`Isolated #595 empty-board project only; no production board, publication or deployment.`
PM identity must be the genuine session selected in that cell; never reuse the production worker's
identity as an experiment manager. Keep the runtime's existing model, effort, permissions and limits.

Three variants are defined, with one control and at most one run per candidate/command/topology.
Stop a blocked step; do not launch later variants through the same unavailable route.
These overlays are proposed tool-output text, not extra user clarification or adopted contract changes:

**Control C:** unmodified output from `taskq pm` / `taskq arm tick` at the baseline revision above.
The failed CLI preflight used candidate files (including the id/link clarification); initialization failed
before any role was rendered, so this is not a completed baseline/candidate prompt comparison.

**Imperative I:** prepend to the same current output, respectively:

```text
PM: Execute this manager workflow now for the owner's explicit projects and authorized scope.
Run the initial taskq tick through the supported execution session, handle its report, then perform
this role's next supported wait. Report the exact blocked step if approval, access or transport is missing.
```

```text
ARM: Execute the authorized arming workflow now. Inspect and reuse this PM/project's existing
monitor and targeted wait; preserve paused monitoring. Use the supported route below. Record
manager receipt, its tick handling and the next targeted wait before reporting armed. Stop at a blocker.
```

**Handoff H:** prepend the common PM/ARM sentence and exactly one verified-context paragraph below:

```text
PM: Take this manager role and execute its next authorized step now for the explicit project scope.
ARM: Reuse this PM/project's monitor and execute its next supported wait/wake step now.
```

```text
Claude Code: Run the initial authorized tick here, then reuse or start exactly one background
wait using the available run_in_background tool. On its completion handle the tick here and wait again.
If that wake capability is unavailable, stop and report it.
```

```text
Direct Codex app or CLI: Keep the PM and foreground wait/tick loop in this session. Do not delegate
ordinary direct-session monitoring. Between turns use only a separately supported, reachable route:
local CLI resume requires the target's local rollout; app messaging requires an existing independent
app sender with a supported tool. A background command finishing is not an idle-thread wake.
```

```text
Verified DOT cloud parent: Hand the explicit project scope and intended PM identity to your existing
authorized local execution child. Have that child run taskq pm and execute the returned workflow there.
Route ARM to the session that owns that project's existing monitor. Require its canonical report and
receipt/next-wait evidence; relay the completed report unchanged. Stop if that child or route is unavailable.
Do not rebind tasks to the child, replace a sender or infer parent wake from child command completion.
```

Select a paragraph from observed topology and actual tool capabilities, not an environment-name guess.
For app and CLI the direct-session instruction is intentionally the same; between-turn transport differs.
These are hypotheses, not successful prompt wording. No live-winning wording has been established.

| Context | C PM / ARM | I PM / ARM | H PM / ARM | Precise blocker and evidence |
|---|---|---|---|---|
| Claude baseline | BLOCKED / BLOCKED | BLOCKED / BLOCKED | BLOCKED / BLOCKED | No authorized existing Claude experiment PM/wake session accessible from this worker; Claude workers are disabled by the selected project's limit 0. Installed CLI alone proves no session/tool wake. No launch attempted. |
| Direct Codex CLI | BLOCKED / BLOCKED | BLOCKED / BLOCKED | BLOCKED / BLOCKED | One isolated C PM launch failed before role receipt: in-process app-server initialization denied by the current sandbox. ARM and candidates stopped at that dependency. |
| Direct Codex app | BLOCKED / BLOCKED | BLOCKED / BLOCKED | BLOCKED / BLOCKED | Existing independent app sender/target not reachable with the supplied tools; no `send_message_to_thread`. No resume fallback or replacement sender. |
| DOT cloud + local execution child | BLOCKED / BLOCKED | BLOCKED / BLOCKED | BLOCKED / BLOCKED | No accessible actual cloud parent/local child handoff and no existing ARM sender access. This worker's local CLI is not proof of that topology. |

CLI control command, in the newly created isolated empty-board scope only:

```sh
codex exec --json -s workspace-write -c sandbox_workspace_write.network_access=true \
  -C <isolated-project> --skip-git-repo-check 'taskq pm'
```

The isolated project contained the candidate `taskq.py`/contract, a stdlib board adapter returning no
issues and refusing every mutation, `update: false`, and the same Claude 0 / Codex 3 limits.
Its AGENTS context was exactly:

```text
Isolated #595 probe. The owner requests only this project. No production board, publication or deployment.
Use python3 ./taskq.py for taskq. Run taskq pm and follow its returned authorized instructions.
Do not create a sender, task or replacement bridge. Stop and report any unsupported action;
do not bypass sandbox restrictions. No model, effort or permission changes.
```

Inherited parent session ids were removed for the child, as the runtime adapter does; no synthetic
PM id was installed. This setup is only a capability/control probe, not an idle-wake qualification.
The 45-second bound was not reached: at `2026-10-09T18:46:03.923248Z`, exit 1 in 0.209 seconds,
zero JSONL events, with `Error: failed to initialize in-process app-server client: Operation not permitted (os error 1)`.
Raw local probe logs contain no production transcript inputs and remain private; only this safe summary is public.

Observed live successes: 0. Completed supported idle-wake trials: 0; success rate and wake latency unknown,
not 0%. Omitted authorized actions and unnecessary clarification counts are unknown because no model received
the test role. This is an access/capability blocker, not an instruction-understanding failure. No rejected
step was retried via a different permission, credential, bridge or transport.

### Reproduced implementation defect and minimal candidate

An isolated synthetic route check found a separate deterministic defect: a manager link was normalized
for `wait --pm`, but the original full link was used for rollout lookup and resume/send. A local CLI
thread passed by `codex://threads/<id>` or its taskq wrapper link therefore received the unknown/app
fallback; an archived link missed the archived blocker. This does not explain the owner's screenshot.

The candidate uses the already extracted PM id for lookup, quoted CLI resume and sender instructions.
`taskq.md` was clarified first. This restores the existing id/link contract; **no R3/R4/R6/R9/R12/R13
principle is amended**, no new wake transport or automatic monitor/adoption is introduced.
General prompt overlays above remain unimplemented because their effect is unproved.

Reproduction / focused regressions:

```sh
python3 -m unittest tests.test_single.Wait.test_arm_tick_links_use_the_same_pm_for_lookup_wait_and_send
python3 -m unittest tests.test_single.Wait tests.test_single.MultiPM tests.test_single.Contract
```

The new test covers id, direct link and wrapper link for local/archived/unknown synthetic rollouts.
Before the fix: six link subcases failed; after: all nine route subcases pass. Repeated rendering is
identical and leaves the fake board unchanged. The existing printed-shell-loop test now starts from a
link and executes its generated loop through real bash with fake wait/send downstream, proving event
forwarding count and first-error stop. Neither test proves a model follows prose or an idle manager wakes.
Focused checks: 21 tests pass in 2.267 seconds. Cost/model calls for these checks: no direct model calls;
development cost is unknown.

| Requirement / issue | Check and failure oracle | Red before fix | Blindspot | Disposition |
|---|---|---|---|---|
| #595 id/link routing | Wait link matrix selects local/archived/unknown route and the same PM for wait/send | Observed six synthetic failures | No real resume/app receipt | Retain unique link regression; live proof still required |
| #595 printed CLI sender boundary | Existing Wait shell test, now link input; two sends on success, one on send failure; stops on failed wait | Existing ID test was green; link matrix above failed | Fake CLI sends are not a manager wake | Reuse existing boundary test; no redundant test harness |
| #595 foreign PM/paused state and duplicate ARM | Existing MultiPM routing/ownership and ARM no-write/repeated-render assertions | Historical red unknown | Rendering does not inspect or deduplicate a live monitor; paused-project live case blocked | Keep current safety coverage; require live observation |
| #595 role/capability/context | Existing Contract and Wait role/transport tests; proposed context cells above | Wording omission unknown | No Claude/app/DOT model execution | Do not invent a mocked model PASS; qualify separately |

### Required completion evidence and recommendations

Before accepting the full research task or publishing the changed transport output to main, the existing
supervisor must obtain the required accessible isolated topology evidence. Publication, deployment and
any claim of successful arming remain held. Exact-head independent review/CI is that supervisor's job;
this worker does not substitute its own checks or start a replacement reviewer/sender.

For each unblocked cell record genuine PM/session identity, explicit projects, monitor state, observed
capabilities, runtime/version/model/effort (unknown if unavailable), exact prompt revision/commands,
timestamps and interventions. Prove this sequence with the existing supported route:

1. Role/command received; intended PM/execution-child selected without additional wording-only clarification.
2. A manager demonstrably idle before the event receives and handles that event in its own next turn,
   then invokes the intended tick. Sender submission/acceptance alone is insufficient.
3. The intended manager or sender subsequently enters the next targeted wait, evidenced by a running
   tool/wait invocation with the same PM/project. A printed instruction or timer assertion is insufficient.
4. Repeat ARM with the existing monitor; verify the same monitor/targeted wait, no duplicate wait/timer,
   paused state and foreign ownership unchanged, and no unsupported fallback. If the existing monitor
   cannot be inspected, mark that step BLOCKED rather than provisioning another.

Count omitted actions and wording-only clarification separately from required approval/access questions.
A direct CLI foreground continuation is not proof of wake between turns. A DOT child's result is not proof
of cloud-parent receipt. Reuse the current contract and supported capabilities; do not add a persistent
bridge, receipt store, benchmark, model/effort change or pending-approval bypass.

If later evidence justifies adopting H, propose a spec-first amendment to R3 (verified parent/child role
handoff) and R4 (next-step/receipt/continued-wait instruction), under R13, before implementation.
R6's exact report/ARM proof, R9's settings gate and R12's unknown/blocker semantics must be preserved;
name any actual change to them separately. No such principle change is made by this candidate.

Sources checked: [official Codex non-interactive documentation](https://learn.chatgpt.com/docs/non-interactive-mode)
distinguishes bounded CLI execution/resume; [official app-server documentation](https://learn.chatgpt.com/docs/app-server)
describes its own thread/turn protocol, which is not evidence that this worker has an app messaging tool.
[Langfuse experiment documentation](https://langfuse.com/docs/evaluation/experiments/datasets) supports a fixed
comparison dataset/context. Documentation does not qualify the local wake topology.
No #269 cadence trial, #538 onboarding trial or #577 execution-policy qualification was duplicated or treated as PASS.

Final local gate on the frozen code/test diff: `python3 -m unittest tests.test_single` ran 146 tests
in 24.206 seconds and **FAILED** (2 failures, 11 errors, 1 skipped). All failures/errors were in
`HermesNativeBoundary`: the current macOS runtime lacks Linux `os.pidfd_open`; spawn refuses with
`Hermes needs Linux pidfds, explicit isolated HERMES_HOME and TASKQ_HERMES_COMMAND`.
The Hermes runtime has no diff from origin/main, and the failing class's AST is identical to origin/main.
This establishes unchanged source/platform incompatibility, not a completed baseline execution.
No test was disabled and no Linux support shim was added. The final gate is not green; applicable
exact-head checks and live wake proof remain required before acceptance/publication.


### Diagnostic continuation, 2026-10-09 19:00-19:03 UTC

The existing branch/PR #596 and supervisor were preserved. This continuation performs no model launch,
production pass, monitor wait/send, adoption, rebinding, credential read or permission change. No role or
cross-agent behavior changed; no Memory or R-number amendment is needed for this diagnostic record.

The initial error above is the retained safe error evidence. The matching installed CLI still reports
`codex-cli 0.159.3`; even `codex --version` also reports
`WARNING: proceeding, even though we could not create PATH aliases: Operation not permitted (os error 1)`.
That warning is nonfatal and does not establish the app-server failure's syscall.

Read-only inspection of the [version-tagged exec source](https://github.com/openai/codex/blob/rust-v0.159.3/codex-rs/exec/src/lib.rs#L985)
locates the reported error at `InProcessAppServerClient::start`, before thread start/resume and user turn.
The [in-process source](https://github.com/openai/codex/blob/rust-v0.159.3/codex-rs/app-server/src/in_process.rs#L377)
propagates startup configuration/auth bootstrap and installation-ID errors before its initialize request.
An initialize RPC rejection would instead include `in-process initialize failed:`. The retained error has
no such prefix. No runtime trace exists to distinguish all startup substeps.

The [installation-ID source](https://github.com/openai/codex/blob/rust-v0.159.3/codex-rs/core/src/installation_id.rs#L19)
unconditionally opens the installation-ID file read/write/create and locks it, even when a valid ID exists.
A metadata-only check at `19:02:23.470467Z` found the existing installation-ID file present, with advisory
write access false for both file and parent. No contents, open-for-write or chmod was attempted. Combined
with this worker's restricted writable roots, this identifies a concrete incompatible required startup
operation. It is a strong filesystem/sandbox hypothesis, not proof that this particular syscall produced
the retained errno; configuration/auth bootstrap remains another possible source. No credentials were inspected.

| Hypothesis / bounded diagnostic | Observation | Classification / stop |
|---|---|---|
| CLI error is caused by PM/ARM wording | Tagged exec source fails before thread/user turn; original zero JSONL events | No instruction-understanding evidence; do not spend candidate model trials on this unavailable route |
| Local IPC socket creation/bind is generally prohibited | At `19:01:14.442315Z`, fresh AF_UNIX and loopback AF_INET sockets each created, bound and listened successfully in an auto-cleaned temporary directory | Broad IPC hypothesis rejected; no app-server or wake proof |
| Required installation metadata is writable | Existing file/parent report advisory write access false; tagged source requires read/write open | Filesystem/sandbox blocker candidate; exact failing syscall unknown; no denied open retry |
| Prior macOS failures indicate an ARM regression | `runtimes/hermes.py` byte-identical and `HermesNativeBoundary` AST-identical to origin/main; Darwin has no `os.pidfd_open`, while adapter explicitly requires Linux pidfds | Unchanged unsupported Linux-only boundary; no shim, skip annotation or weakened assertion introduced |

Safe reproduction of socket diagnostics (use the existing authorized shell; no Codex-home override):

```sh
export TASKQ_TASK=595 TASKQ_RUNTIME=codex && python3 - <<'PYCODE'
import socket, tempfile
with tempfile.TemporaryDirectory(prefix='taskq-595-boundary-') as root:
    for family in (socket.AF_UNIX, socket.AF_INET):
        with socket.socket(family, socket.SOCK_STREAM) as probe:
            probe.bind(root + '/probe.sock' if family == socket.AF_UNIX else ('127.0.0.1', 0))
            probe.listen(1)
            print(family, 'bind/listen PASS')
PYCODE
```

Capabilities were inspected from this session's actual callable inventory: shell execution/continuation
available; no independent app `send_message_to_thread` and no established DOT parent/local-child route.
Collaboration and UI tooling do not establish those missing routes. No replacement sender was created.
All control/I/H live matrix cells remain BLOCKED as previously classified, with no winning wording,
idle wake, receipt, next targeted wait, omission counts or wake latency. Claude 0 remains preserved.

Focused candidate regression command `python3 -m unittest tests.test_single.Wait tests.test_single.MultiPM
 tests.test_single.Contract`: 21 tests PASS in 2.807 s. A separate diagnostic selection of all classes except
`HermesNativeBoundary` assesses whether reported failures extend beyond the unsupported Linux boundary;
135 tests PASS in 20.901 s. It is explicitly not the contract's full final gate and does not turn that earlier failed gate green.
Existing PR exact-head CI reports both test checks SUCCESS at `3fcbb6ab8db019827347ec596c85e1c917899ea5`;
CI is not live macOS wake evidence or independent supervisor acceptance.

Smallest prerequisite: the existing PM/supervisor must make the already authorized isolated qualification
reachable through an existing supported app-session route and provide safe receipt/next-wait evidence, or
obtain an explicitly reviewed narrowly scoped CLI startup-access prerequisite. The worker cannot grant
itself access or relocate Codex home to evade the denial. Prefer the existing app route; do not change model,
effort, credentials, monitor ownership or security settings. Hold main publication/deployment and acceptance.
