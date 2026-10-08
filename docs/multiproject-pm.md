# Multiproject observation and acting pass (#186, stages 2 and 3)

Specifications: observation, [accepted Wiki 9ffd02dd](https://github.com/alexkirs/taskq/wiki/Multiproject-read-only-preparation/9ffd02dd07dbff81da333089613642c883bbcf6f);
acting, [accepted Wiki 5e238093](https://github.com/alexkirs/taskq/wiki/Multiproject-acting-pass/5e238093a26fc49ac0166079f0ded55ab9fc67b4)
(the dcc97303 specification with its accepted v1 correction: explicit catalog enrollment and fail-closed admission).
The default observes and never acts. Acting needs `--act` and an execution policy the owner enrolled explicitly ([§ Acting](#acting-stage-3)).

```
python -m taskq.multiproject --manifest FILE --json
```

Prints one aggregate: per project its status, blockers and the project's existing #191 v1 report (`taskq/contracts/pm-report-v1.md`). Exit status 1 when any project is not `ok`. Without `--json` it prints each report the way `tick` renders it.

## Manifest

A TOML file the user writes on each machine. It lists only the projects chosen for observation. Folders are never auto-discovered.

```toml
version = 1

[[project]]
provider = "github"            # "github" or "gitlab"
host = "github.com"            # optional; github.com / gitlab.com by default
repository_id = 123456789      # the tracker's numeric id: GitHub `gh api repos/OWNER/REPO --jq .id`, GitLab project id
repository = "owner/repo"      # canonical path, as in [github] repo / [gitlab] project of the project's taskq.toml
board = "repo"                 # the board name the project's taskq.toml resolves to
checkout = "/abs/path/to/main/checkout"  # this machine's binding: the main checkout, not a worktree
timeout = 60                   # optional; seconds for this project's reader, 1..600
view = { filter = "labels=area-x", mine = false, limits = { codex = 2 } }  # optional; the same meaning as tick's flags
```

`view` only narrows what the report shows. It confers no ACL and no execution authority.

## Checks before any queue read

The whole manifest is refused if `version` is not 1 or it has no entries (50 at most). An entry is refused, and its reader never starts, if:

- a field has the wrong type;
- another entry has the same stable identity (provider, host, `repository_id`), the same repository, or the same checkout.

Inside the entry's reader, before the queue is read, the project is refused if any of these fails:

- the checkout has a `taskq.toml` and is a main checkout;
- `origin` points to `host`/`repository`;
- the project's `taskq.toml` names the same provider, host, repository and board;
- the tracker's repository id equals `repository_id`.

A refused project is reported with its blockers. It never counts as empty work.

## Reading

Each entry gets exactly one attempt per invocation, in manifest order. The attempt runs `python -P -m taskq.multiproject --observe` as its own subprocess. Its cwd is the verified checkout and it configures that project's `taskq.toml`. The subprocess has its own process group. When the timeout passes, the whole group is killed. That is safe only because the reader admits no mutation and launches no worker. The reader output is capped at 1 MiB.

The reader uses only existing read paths: `profile` (queue and effective profile), the board lookup, `claude agents`, `liveness`, `report_row` and `validate_report`. It never calls `tick` (even plain `tick` releases and unlocks), `update`, `take`, `spawn`, `release`, `cleanup`, board moves or timers. It writes no tracker state, configuration, profile or permission.

## Results

| status | meaning |
|---|---|
| `ok` | valid fresh v1 report, outcome `ok` |
| `blocked` | report present, but v1 validation, repository match or outcome failed: e.g. stale `observed_at`, source unavailable, missing session link |
| `refused` | manifest or identity check failed |
| `timeout` | reader killed at its timeout |
| `failed` | reader did not start or exited non-zero |
| `malformed` | reader output is not a v1 report |

The parent revalidates every report with `validate_report`, so stale or invalid data stays visible. Each report keeps its repository, board, profile, outcome, refusals, task/session/commit links, `event_at`, `observed_at` and unknown/unavailable values as the reader produced them. One project's failure never stops the next project.

`received_applied` is always `unknown`. Delivery, receipt and application stay with `taskq report-verify` and a supported channel's readback.

## Acting (stage 3)

```
python -m taskq.multiproject --accept-execution-policy POLICY --json   # the owner's explicit enrollment or replacement
python -m taskq.multiproject --manifest FILE --act --execution-policy POLICY --json
```

The manifest stays the view: which projects to visit in this invocation, and their `filter`, `mine` and `limits`. A view confers nothing. Acting authority comes only from the execution policy, a second TOML file the owner writes, has reviewed and enrolls. The adapter never writes a profile, the policy file, a configuration or a permission. Its only writes are the enrollment's small anchor ([§ Enrollment](#enrollment-and-the-catalog-anchor)) and one output record per actor ([§ Actor output records](#actor-output-records)).

### Execution policy

```toml
version = 1
machine = "macbook-m2"   # this host's taskq machine name (TASKQ_HOST, [hosts] or the hostname)
os_user = 501            # the numeric OS user of the guard's domain (`id -u`)
caps = { claude = 2, codex = 6 }  # optional; an omitted runtime keeps its default, claude 8 and codex 4

[[project]]              # the complete approved catalog, one binding per project
provider = "github"
host = "github.com"
repository_id = 123456789
repository = "owner/repo"
board = "repo"
checkout = "/abs/path/to/main/checkout"
principal = 1640869      # the tracker user id this project's pass runs as (`gh api user --jq .id`)
act = true               # false: catalog-only, never admitted; its ownership still counts
limits = { claude = 1, codex = 0 }  # the project limit, 0 when omitted; a view's limits can only lower it
effects = ["queue", "cleanup", "idle_stop"]
timeout = 600            # optional; seconds for this project's actor, 1..600
```

Caps and limits are explicit whole counts 0..64 of the runtimes `claude` and `codex`; a boolean, a non-integer, a negative count, another runtime or a count above 64 refuses the policy (caps) or the binding (limits). 64 is only a validation bound, never a capacity grant: the places an acting pass may use are the enrolled caps, minus fresh occupancy (§ Budget). Without `caps`, an anchor keeps the 8/4 default it always had; no environment variable or anchor migration changes it. The owner's target caps (for example 2/6) take effect only through an explicit enrollment.

Identity fields are checked as in the manifest. A view entry is admitted only when a valid binding has the same provider, host, repository id, repository, board and checkout, and `act = true`. Any other key is refused: readiness and effects evidence never come from this file.

`effects` only narrows. The actor reads which native effects the project's own settings turn on: `queue` always (releases, unlocks, board and ready/waiting moves, reservation reconcile, Codex archive, nudges, retire, launches); `cleanup` when its `[cleanup]` is enabled; `idle_stop` when `[idle] stop` is not 0 (on the idle stop `tick --act` removes its launchd timer and runs idle cleanup). An effect the settings turn on and the binding does not list refuses the project before the native pass.

### Enrollment and the catalog anchor

The policy file alone is not an approved catalog. `--accept-execution-policy POLICY` is the owner's explicit enrollment; the first `--act` never enrolls. It holds the host guard for its whole run and starts no native tick or action:

1. Load the policy; every binding must be valid.
2. Check every binding in its own read-only subprocess, with the checkout as cwd: identity as in observation, `machine` and `principal`; for `act = true` bindings also the effects and `taskq doctor`.
3. For a replacement, read back every binding of the previous anchor (step 3 of § Per project). Each must show no known grant and no uncertainty: no same-host claim, reservation or ownerless lock. The Claude and Codex inventories of every runtime the old catalog allowed must be readable and show no live session in an old checkout. A missing old checkout, unknown, unavailable or foreign ownership, or a failed read keeps the old anchor unchanged. Nothing is released, stolen or discovered; the candidate file or an empty inventory never settles an omitted grant.
   A caps-only replacement changes the aggregate caps and nothing else: version, machine, OS user and every binding's identity, checkout, principal, `act`, limits, effects and timeout are exactly the anchored ones. Its bindings stay, so their grants are not settled: the fresh readback of every anchored binding must be complete and known (no failure, off-shape read or uncertainty), and the inventory of every runtime whose cap changes must be readable. Known retained claims (review, ask, doing) and reservations stay as they are and keep counting in occupancy; nothing is released. Unknown ownership or an unreadable inventory keeps the old anchor. Any other change, together with a caps change or not, needs the full settlement above.
4. Write `multiproject-execution.json` beside the guard: version, generation (1, then +1 per replacement), the SHA-256 of the canonical catalog, the canonical catalog itself and who accepted it. The write is a draft in the same folder, fsync, rename, fsync of the folder. An identical catalog leaves it `unchanged`.

The canonical catalog is every effective field: machine, OS user, caps and each binding's identity (host lower-cased, checkout resolved, timeout defaulted), principal, `act`, limits (0 filled in) and effects. Only the order of tables and of effects is ignored.

`--act` refuses the whole invocation, before any actor starts, unless the anchor is valid and its catalog equals the policy file's. A changed binding, principal, selection, limit or effect therefore needs a replacement enrollment.

A valid anchor has exactly its known keys and types. Its hash matches its catalog. Its catalog is exactly what enrollment derives from a valid policy: every shape, type and range, canonical spelling included. It belongs to this OS user and to the policy's `machine`, which every binding check verifies against the project's own machine name. Anything else refuses `--act`, enrollment and replacement alike: unreadable, a symlink, another owner or mode, another version, off-schema, another OS user or machine. The anchor is never reset, and a refusal leaves its bytes as they were. A lost anchor stops all acting until the owner enrolls again.

The anchor records the owner's execution selection and its version, nothing else. It is not evidence of ownership, readiness, UI approval, PM transport or a received/applied report. Doctor proves a local preflight only. Tracker ACL and assignee rules still apply. Parent #186's dependencies #185, #176 and #177, the actual project selection and live authorization remain external release gates.

### Per project

Each view entry gets at most one attempt, serially, in manifest order. The wrapper first checks the binding, then checks that no actor holds the host guard. Then it starts the actor: `python -P -m taskq.multiproject --actor`, a subprocess in its own session with the checkout as cwd. Everything below happens in that one process:

1. Lock the host guard and hold it until the process ends.
2. Check that the anchor still equals the policy. Check identity as in observation. Check the policy's `machine` against this host and `principal` against the tracker's authenticated user; there is no delegation. Run `taskq doctor` read-only; a gap refuses. Refuse when `[update] auto` is due: its exec would end the guarded pass, so run `taskq update` there first. Check the effects.
3. Read back the complete anchored catalog, act = false bindings and bindings removed from the view included, one read-only subprocess per binding: identity and principal, then its same-host ownership. Each retained claim or reservation is first placed: local (its node is this machine's, a pre-#39 host is this hostname, or, without either, native local app evidence of its session), remote (a well-formed node or host of another machine: that machine settles it, it is not counted), or unknown (a malformed node or host, or neither with no local evidence). Unknown is uncertain. A released claim (runtime and session empty) owns nothing. A local claim in any state (doing, review, ask, ready, later) with a session of a known runtime (claude, codex) is a known grant; an empty runtime inventory never settles it. A local reservation of a known runtime by the binding's principal is a known grant too, with the session of its launch note or with its launching pid (native reconcile settles that one). Everything else is listed as uncertain: a claim or reservation of unknown place; a local claim without a session or of another runtime; a reservation of another principal or runtime, or with neither session nor pid; an own tracker lock that no claim or reservation owns. The actor checks every readback's exact shape. An off-shape readback counts as unknown. Read the host's runtimes: every row of `claude agents --json --all` of any kind without a terminal state, and the Codex app server's running threads and `T<N>` worker threads.
4. Refuse, before the native pass, when any catalog binding's readback failed, was off-shape or lists uncertainty, or when the inventory of a runtime this project may start (project limit > 0) or already holds (L > 0) is unknown, or when its known occupancy is above its cap (§ Budget). Zero limits are no protection: the native pass would still release, move and reconcile.
5. Compute the budget below and run the existing `tick --act --json` in this process, under its own checkout lock, with `--limit` set to it for every runtime.

The actor never takes, spawns or releases by itself. Only the native pass does, through #208 `reserve`, which rechecks the whole queue and room under the tracker lock.

### Budget

For each runtime `r`:

- occupancy is the number of distinct identities across host and catalog. Two grants count once only when they name the same exact session. A reservation without a launch session, an ownerless lock and a pid-only row each count separately. A retained Codex claim in `review`, `ask` or `later` is excluded only when a fresh supported `thread/read` proves the exact session is archived and its executor status is `idle`, `notLoaded` or `systemError`; actual unarchived inventory for that session wins, and a reservation still counts. Claude has no equivalent supported archive proof, so its retained claims stay counted. Missing, stale, malformed or mismatched evidence stays counted.
- F = max(0, cap[r] - occupancy).
- L is exactly what native `core.room` subtracts for this project now: its same-host doing claims and reservations.
- limit[r] = min(project limit[r], L + F). A runtime without a cap (a `[runtimes]` app) gets 0.

Occupancy above a cap, after a caps-only lowering for example, refuses the pass of every project that may start or holds that runtime before the native pass (step 4): the native pass could release, reconcile or clean up the occupancy it is measured by. Workers and claims stay; nothing is stopped, restarted or released to fit the cap. At the cap exactly, F = 0 and the pass runs with its L. A raised cap grants only cap - fresh occupancy, never more.

An unreadable inventory, or an unidentified live row (no session id and no pid), makes F = 0 for its runtime; for a runtime the project may start or holds, step 4 refuses before that matters. The budget is recomputed for every project, after the previous project's actor ended.

### Host guard

The guard is one file: `multiproject-acting.lock` in the OS user's taskq state folder (`$XDG_STATE_HOME/taskq`, default `~/.local/state/taskq`, beside `machine-id`). It is keyed by the host, never by the backend principal: different principals of one OS user contend on the same inode. The file is never deleted or replaced.

- **Domain.** Its domain is one OS user: the policy's `os_user`, checked against the process. Its folder and the file must belong to that user and not be writable by group or others. Otherwise acting is refused; no permission is changed. Two OS users on one host do not share a guard. So the caps hold for one OS user's acting passes, not host-wide; the adapter claims no host-global capacity. Windows and other platforms without flock are refused.
- **Continuity.** The actor locks the guard before final admission and holds it through the native pass in the same process. Its descriptor is close-on-exec, so a spawned worker or other descendant inherits nothing. The actor runs in its own session: a killed wrapper does not end it or its guard. A crash ends the process, and the OS frees the lock; the inode stays for the next readback.
- **Not a boundary.** Single-project `tick`, manual commands and other coordinators keep their own supervision contract. They do not take this guard; the readback counts what they own.

### Timeout, crash and restart

The actor is never killed. At its `timeout` the wrapper reports it `unknown`, leaves it running with its guard, and admits nothing more in this invocation. An actor that ends without a result (crash, lost output) is also `unknown` and stops admission. A refusal or a known native failure lets the next project run.

A restarted wrapper first meets the same guard: while an actor runs, nothing is admitted. After release, the next actor reads everything back fresh (step 3) and releases nothing in that readback. Ownership the tracker and runtimes show is the only durable record of ownership. Tracker claims, reservations and locks survive a wrapper restart and a view change. An orphan reservation of a crashed launch holds its place until native reconcile settles it. There is no receipt file. Guard release or a successful readback never means a report was received or applied.

Every acting result names its run id and the exact recovery command (`run_id`, `recovery`). An actor that outlives its wrapper still writes its actual output to its record (§ Actor output records).

### Actor output records

Each actor gets one output record: `multiproject-output/<run-id>.json` beside the guard. The record holds that run's actual output, and nothing else. It is not a job, a queue, a receipt or a transport.

1. **Allocation.** Before the actor starts, the wrapper creates the empty record: a fresh 32-hex run id, the file created exclusively with mode 0600. The folder must be a real folder (no symlink) of the OS user alone (0700). Otherwise nothing starts. The actor refuses before its guard when its record is not that allocated empty file.
2. **Completion.** The actor writes its result once, while it still holds the guard. The write is a draft in the same folder, fsync, rename, fsync of the folder. The record holds the run id, the SHA-256 of the canonical catalog and of its binding, the OS user, the machine, the repository, the actor pid, its time, the result exactly as the actor printed it (guard, budget, catalog readback, native outcome and the v1 report) and its newest diagnostics. The record is stored as UTF-8 JSON, and bounds count stored bytes: the result at most 1 MiB, the diagnostics at most 64 KiB, so escaping never makes a bounded record unreadable. A result above 1 MiB, or any failed write, leaves the record empty, never partial.
3. **Retention.** At most 32 files in the folder, of any kind, counted after allocation; two racing wrappers both refuse rather than pass the limit. At the limit, the next actor is refused before it starts, so no native mutation happens. Nothing is evicted, cleaned up or migrated, and recovery deletes nothing. The operator recovers each record, then removes resolved ones by hand. The folder's file names are the run ids, so a run stays recoverable even when its wrapper died before printing.

```
python -m taskq.multiproject --recover-actor-output RUN_ID --execution-policy POLICY --json
```

Recovery is read only. It never calls the native pass, `tick`, `take`, `spawn`, `release` or enrollment, and never replays an action. It creates, changes and removes no folder or file, the guard included. It locks the existing guard file read only: no symlink, a regular file and folder of the policy's OS user alone. It holds that lock through the record check and the fresh readback, so no actor starts while it qualifies. A missing or invalid guard, folder or domain gives `unknown`; the guard is never recreated. Its statuses:

| status | meaning |
|---|---|
| `pending` | the host guard is held: an actor (or another pass) still runs, its record is not final |
| `unknown` | no exact completed record. The record is missing, empty (still running, crashed or cut off), a symlink, of another owner or mode, oversized, truncated or off-shape. Or one field fails the check below |
| `refused` | the record matches, but the fresh read-only readback of every anchored binding is not known, or the inventory of a runtime the binding may start or holds is unreadable |
| `recovered` | `output` is the actor's actual result, exactly as recorded |

A record qualifies only when every field passes. Any missing, extra, mistyped or out-of-range field makes the output `unknown`:

- exactly its keys; `version` the integer 1; the run id; `completed_at` a valid UTC time, not in the future;
- the policy and binding SHA-256 of the anchored catalog, an `act = true` binding with its repository; the OS user (an integer, this process's and the policy's); the machine;
- `actor` exactly `{pid}`, a positive integer; diagnostics a string of at most 64 KiB stored;
- `result` exactly its keys, a known status, string errors, at most 1 MiB stored;
- the guard: its path, device, inode, pid (the actor's) and OS-user domain equal the stable identity of the guard file recovery holds locked. No guard is valid only for a refusal before admission, with no budget, readback or native output;
- `refused`: nothing native; `blocked` and `failed`: a native outcome; `ok` and `judgement_needed`: a native outcome and a complete v1 report that `validate_report` passes as of `completed_at`. A success without its report is never recovered.

The policy must equal the accepted anchor. Guard release or a fresh readback alone never makes output `recovered`: only the exact completed record does. `received_applied` is always `unknown`. Output that was lost before this change, such as a wrapper timeout with no record, stays unknown. Nothing is rerun to rebuild it.

### Results

Statuses are `ok`, `judgement_needed` (native exit 1 with a valid report), `blocked` (report validation failed), `failed` (known native failure), `refused` (before any mutation: no anchor or a different one, binding, workflow, unknown ownership or inventory), `unknown` (timeout, crash, unreadable output) and `not_admitted`. Enrollment statuses are `enrolled`, `replaced`, `unchanged` and `refused`. Each actor result keeps its guard (path, inode, domain), the budget per runtime, the catalog readback, the native outcome/actions/refusals and the native v1 report, revalidated by the wrapper. `received_applied` stays `unknown`.

### Removing bindings

Removing a view entry only stops admission; the anchored binding and its ownership are still read and counted. Deleting a binding from the policy file, or changing its principal or any other field, makes the file differ from the anchor: `--act` refuses until a replacement enrollment succeeds, and that needs every previously anchored binding settled (§ Enrollment, step 3). A hidden reservation or lock of a deleted binding therefore keeps the old anchor and blocks acting; it is never dropped from the count. Uncertain ownership is never settled here: a stale own lock or an unproven reservation blocks every acting pass of the catalog until the project's existing workflow settles it (its own single-project `taskq tick` or `release`). The adapter never releases or steals it.

## Not here

The manifest and execution policy are the only inputs. The enrollment anchor and the bounded actor output records are the only state: there is no database, job, queue, scheduler, replay, polling or receipt store. Removing a manifest entry stops future reads and admission and leaves the tracker's claims and ownership as they are. Single-project `tick` is unchanged.

There is no timer, extra PM or live project activation. Parent #186 still depends on #185, #176 and #177, and live project selection is still open. Fixture tests prove behavior against fakes only: on-disk in-memory trackers, a file `claude agents` list and logged fake launches. They do not qualify live transport, approval, runtime launch or multiproject readiness. Present authorization covers this implementation and its isolated fixtures only: no real enrollment and no live acting pass.
