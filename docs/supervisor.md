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
   (#532), never `.taskq/pm.json`; a task with none starts no supervisor and waits (the table says `no manager`)
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
