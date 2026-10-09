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
| (c2) supervisor orders, pass spawns | Works; wake by its own background `taskq wait --task N`, never resumed: no fork. Unverified in a `claude --bg` job (§ 5). | Works within `workspace-write` (board writes need only `gh` and network); wake by `exec resume` from the pass, same id. Orders wait for an unsandboxed pass (§ 5). | As Codex; the cloud manager only receives the one line. |

## 2. Old bugs, root cause, workaround

| Bug | What happened | Root cause | Workaround in (c2) |
|---|---|---|---|
| #284 | Waking a stopped Claude supervisor (`claude --bg --resume <id>`) started a copy under a new id; taskq refused it ("supervised by claude:fa18ac97: only that session decides"); supervisors renamed themselves and started helpers. | Claude resumes a stopped session as a new session (copied transcript, new id); authority was bound to the old id. | A Claude supervisor is never stopped or `send`-ed to. It wakes on its own background `taskq wait --task N`, so its id never changes and the id gate of § 4 holds. Names come from the spawn (`--name`, 3f2fbc8); the brief forbids starting sessions. |
| #291 | A worker resumed after a reject became a copy that pushed but could not `take`. | Same fork, on the worker. | Rework is a new worker that continues branch `taskq-<N>` (R11, step 4); a worker is never resumed for rework. |
| #270 | Every Codex supervisor failed to spawn its worker ("no Codex app server on this machine"); the worker resume after a reject ran sandboxed too; tests archived live threads. | `workspace-write` blocks the app-server socket and process starts outside the sandbox; only `danger-full-access` worked. Tests reached the live app server (25cb09f). | The supervisor only writes its order (`taskq run N`, rework `requeue`) to the board; the pass outside the sandbox spawns. The app-server client is gone (#290); tests never reach live runtimes. |
| #207 | DOT-created Codex sessions stayed in the active UI; archive routes unqualified. | Cloud threads and app-held threads belong to another process; `thread/archive` fails "active writer" (#158). | `S<N>` and `T<N>` are local headless `codex exec` threads with `.taskq/<name>.pid`, retired by recorded id. DOT only runs the manager. |
| #290 | The supervisor was deleted in the single-file cutover. | Cost: typed supervisor, handoff races (#240 P1), fork adoption (#284, #291), sandboxed access (#270), wake logic, an authority model; about a third of the old code. | No fork adoption (no resume of Claude), no sandboxed spawn, one id gate on four commands, one block field, one brief. |
| #502 | A tick inside a sandboxed Codex session called live workers gone and requeued them. | The sandbox cannot see other processes; `alive` returned False. | Unchanged: no pass runs in the sandbox. Liveness of `S<N>` and `T<N>` is checked only by an unsandboxed pass. |
| #510 | A Codex manager is not woken when a background `taskq wait` ends. | Codex has no wake on a finished background command. | A Codex supervisor ends its turn and is woken by `exec resume` from the pass (a local rollout exists: `codex exec` made it). The manager gets the one line through its sender (R4, #522). |

## 3. Per runtime (c2)

| | Claude supervisor (Claude manager) | Codex supervisor (Codex or DOT manager) |
|---|---|---|
| Spawned | Pass step 3: `claude --bg --name "S<N> <ORCH> <title> (<machine>)"`, worker tools and `permission_mode` | Pass step 3: `codex exec --json` with the `codex` options, detached; `.taskq/S<N>.log`, `.taskq/S<N>.pid` |
| Launches worker | `taskq run N`; the event pass spawns `T<N>` at once | `taskq run N`; no pass in the sandbox: the next unsandboxed pass spawns `T<N>` |
| Woken | Its background `taskq wait --task N` (Bash `run_in_background`) ends: same session, same id | Pass step 4: `exec resume <thread> '<event> #N: read your issue'` when no turn runs |
| Reaches the board | `taskq` and `gh`/`glab` through Bash | Same, network on in the sandbox (as workers' `result`) |
| Worker dies | Pass comments `gone`, wakes it; it requeues (rework) or asks; second death: `ask` | Same |
| Supervisor dies | Pass respawns `S<N>`, replaces `supervisor`; the new one adopts the live worker from the board; second death: `ask` with `tail` | Same |
| CI red or merge refused | `close` sends the task back (§ 6); supervisor requeues with the fixes; third rework: `ask` | Same |
| Retire | Ends its turn after `close`; the next pass retires `S<N>` and `T<N>` by recorded id once stopped | `close` in the sandbox cannot kill or archive; the next unsandboxed pass retires both |
| Tokens (*estimate*) | One context per task: system + tools + `taskq.md` (~10k) + issue, then per review the diff and CI; about 50k-150k input per task, cached within a turn, re-read cold after 5 min idle on each wake | Similar; larger Codex system prompt; each `exec resume` re-reads the thread |

Manager saving (measured in #503): before compaction a manager tick cost 4.47M input tokens at 650k context, after
0.44M at 100k. Diffs and CI logs read during reviews feed that growth; with (c2) the manager reads one line per task.
Total cost rises by the supervisor sessions; no saving is claimed until #526 measures both (owner: count total cost,
not only manager context).

## 4. One controller

- The board's `supervisor` field names the controller. `run`, `close` and a rework `requeue` of a supervised task
  come from that session; another agent session is refused, except the machine's manager (recorded by `taskq pm`)
  on the owner's word, and the owner's shell. Their `requeue`, `later` or `close` drops the supervisor.
- The pass never judges: it spawns, wakes, checks liveness and retires, and only on the board's word (the order,
  the event, the recorded id). The dispatch lock (#357) keeps passes from racing on one machine.
- The gate is safe now because a supervisor's id never changes: Claude is never resumed, Codex `exec resume` keeps
  the thread id. #284 failed because the id did change.

## 5. Unsupported topologies (blockers, not bypassed)

1. **Codex supervisor, no unsandboxed pass on the machine.** A sandboxed supervisor's `run`, `requeue` and `close`
   start no pass (#502), and a sandboxed manager's tick only prints. With only sandboxed Codex sessions on a machine,
   nothing spawns, wakes or retires. Needs a pass outside the sandbox: a Claude manager or sender, a shell, or a
   scheduler (`taskq tick` from cron, § 7). taskq does not change the sandbox (R9).
2. **Claude supervisor wake inside a `claude --bg` job is unverified (R12).** The manager role proved a background
   `taskq wait` wakes an interactive session; a `--bg` job that ended its turn is not yet proved to be woken by it.
   If #526 shows it is not, a Claude supervisor cannot be woken without the #284 fork: a blocker for Claude
   supervisors, reported to the owner, not replaced by a reviewer.
3. **Manager not reachable on its runtime.** The supervisor never messages the manager; its one line goes to the
   board and the manager's sender carries it (R4). A Codex app or DOT cloud thread with no local rollout has no
   `exec resume` route; it gets the line only through an independent Codex app sender or when next talked to (#522).
   A Claude sender reaches only Claude sessions.
4. **No manager on the machine.** Supervisor runtime follows the machine's manager (`.taskq/pm.json`); a machine
   with none starts no supervisor and its tasks wait (the table says `no manager`).
5. **Cross-machine.** A Codex supervisor is woken only by a pass on its own machine; a pass elsewhere shows its bare
   id (R6) and cannot wake or retire it.

## 6. Follow-ups

- #525 implements § 3 and § 4 (`--deps 524`): `supervisor` block field, `run`, the id gate, pass step 4,
  `wait --task N`, `pm.json` runtime and id, retire of `S<N>`; tests on fakes only.
- #526 live check (`--deps 525`): Claude manager and Codex manager, each a full cycle with a controlled CI failure,
  rework, exact-SHA merge, close and retirement of both sessions; proves or refutes § 5.2; measures total cost and
  manager context.
