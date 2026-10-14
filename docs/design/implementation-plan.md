# Implementation plan

Executable plan for the [refactoring backlog](refactoring-backlog.md) WS1–WS11, written against checkout 4a99d79 (2026-10-14). The backlog stays the single work breakdown; this file orders it into board batches, fixes the development loop and records the owner decisions that the [brief](implementation-brief.md) left open. Runtime truth stays [taskq.md](../../taskq.md); process rules stay [specq](https://github.com/taskqorg/specq).

## Reconciliation (2026-10-14, 06:50Z)

- Done: batches 1-4 closed on the board: WS1 #886, WS2 #887, WS10 #888, WS3 #892, D03 #893, D-data #894, T-tests #895, WS5 #896, WS6a #897, F-retro #902, WS11b #908, WS6b #970, WS4 #971, WS7 #972, WS11 #976. WS8/WS9: enrollment #912 closed, #913 open.
- Open before G8: #913, the tray Sending readback #1024, the dev-build rollback incidents (#991, #994, #1002) and the GitHub request budget #1011.
- Installed: mac and win on `4.2.0-dev.<epoch>` builds (decision 3 below); `Cargo.toml` still says `4.0.0-dev` and only the build sets the version.
- Evidence gap: codex workers on this Mac cannot reach GUI services or the host checks (`platform::tests::host_memory_reads_on_this_host`, `policy::tests::seatbelt_keeps_workers_out_of_the_policy_store`); the PM ran them for #960 and #1021.

## Owner decisions

1. Incremental 4.x releases under § 8 N/N-1; no 5.0 cutover. Every merged slice must be readable by a 4.1.11 host because 4.1.11 is the published release a host can roll back to (Windows runs dev builds since 2026-10-14, see decision 3).
2. Work runs as taskq tasks (dogfooding): the PM files one reviewable slice per task, the reviewer closes per § 7. Design-package planning never starts queue work by itself.
3. macOS first: no release tag or Linux deploy during development (see the loop below). Amended 2026-10-14: Windows runs the same `4.2.0-dev.<epoch>` builds as the Mac to take load off the Mac, so Windows evidence is collected from those builds; Linux stays `Not tested` (R12) until the qualification batch.
4. taskq.md stays at or under 300 lines: an amendment condenses prose and moves detail into the linked design spec rather than adding paragraphs.
5. Auto admission mechanics (D03, [dynamic resources](dynamic-resources.md) R-A1–R-A8) get one research task with measurements on this Mac before WS5 is filed.

## Development loop on this Mac

The service, the tray and the CLI run from `<install>/releases/<current>`; `current` moves only forward (coreq `newer`). A development build therefore carries a version that sorts above every published 4.1.x and below the final 4.2.0: `4.2.0-dev.<unix seconds>` (pre-release suffix, fixed width, later build sorts later). The update check (§ 8 step 7) sees no newer GitHub release and keeps it; the final `v4.2.0` tag replaces it. `previous` keeps 4.1.11 for rollback.

After each merge to `main`:

```bash
cd ~/Projects/taskq && git pull --ff-only
v=4.2.0-dev.$(date +%s)
sed -i '' "s/^version = .*/version = \"$v\"/" Cargo.toml
cargo build --release && target/release/taskq selftest
git checkout Cargo.toml Cargo.lock
idle=$(for p in $(cat ~/.local/share/taskq/projects); do python3 -c "import json,sys; sys.exit(1 if json.load(open('$p/.taskq/status.json')).get('kids') else 0)" && { echo "$p"; break; }; done)
(cd "$idle" && ~/Projects/taskq/target/release/taskq service install)
for p in $(cat ~/.local/share/taskq/projects); do touch "$p/.taskq/update"; done
```

`service install` copies the binary into `releases/<v>`, writes `current` and reloads the unit of the project it runs in; that reload restarts the service without the § 8 step 3 handover, so the install runs in a registered project whose `status.json` lists no `kids`; since D-data (#894) an install where children run hands them over through step 3 instead. Every other registered service re-execs through step 3 on its next pass and keeps its children (the `update` poke makes that immediate); the running tray exits because its version differs from `current` and the service starts the new one. Before #894, installing from a project with live children posted `gone` for them (seen on #886 on 2026-10-13). Menu evidence for a task: `target/release/taskq tray --snapshot <dir>` from the worktree, or the live tray after the install.

Rollback to the published release (4.1.11's sweep deletes `worker-policy.json`, so keep a copy outside the install):

```bash
cp ~/.local/share/taskq/worker-policy.json ~/taskq-worker-policy.json 2>/dev/null
echo 4.1.11 > ~/.local/share/taskq/current
for p in $(cat ~/.local/share/taskq/projects); do (cd "$p" && ~/.local/share/taskq/taskq service restart); done
```

Forward again: pause every project, install the development build as above, then restore the grants before new starts:

```bash
cp ~/taskq-worker-policy.json ~/.local/share/taskq/worker-policy.json
~/.local/share/taskq/taskq service permissions   # grants listed, then `service resume` in each project
```

Limits: the development build serves every registered project on this host; the launcher's three-start rollback and `previous` are the safety net. No `v*` tag until the qualification batch, so `release.yml` never runs; `tests.yml` still checks every PR.

## Batches

Order follows the backlog dependencies and the scenario review (shared read and operation contracts before live actions; admission and access before Auto or grants; recipes and delivery next; environments and adapters only on a proven gap; native qualification last). `main.rs` and `tray.rs` are split module by module inside the slices that touch them; no standalone "split the file" task.

| Batch | Workstream | Task | Contract amendment | Gate | Depends on |
|---|---|---|---|---|---|
| 1 | WS1 | Canonical read model: `taskq status --json` snapshot v1; CLI text, `status.json` and the tray render the same projection | R6, § 4 `status.json`, § 5 | G6 | — |
| 1 | WS2 | Operation dispatcher: op id, expected revision, receipts for every mutating command and the tray apply path | § 2 (entry fields, idempotency), § 4 M7 apply, § 5 | G2 | — |
| 1 | WS10 | Shared icon renderer: Summary preset and Custom on one versioned schema; render/validate report; snapshot proof | § 4 tray icon, [icon summary](icon-summary.md) I5–I6, [custom icon](custom-icon.md) CI1–CI5 | G8 (icon part) | — |
| 2 | WS3 | Safe held creation: hold before eligibility, idempotent bulk create with per-item receipts, explicit start | § 1 `add`, § 2 start state, § 5 | G2 | WS2 |
| 2 | D03 | Research: Auto admission calibration on this Mac (baseline window, sampling, launch spacing, memory floor) and the service's own ceiling for CPU, memory, descriptors and disk with its behavior at the ceiling ([performance](https://github.com/taskqorg/specq/blob/main/engineering/performance.md)) | none; feeds WS5 | G3 | — |
| 2 | WS5 | Host admission: one host authority, Auto default, optional caps, reservation lifecycle, min service version | R1, § 3, § 4, [dynamic resources](dynamic-resources.md) | G3, G1 | WS1, D03 |
| 2 | WS6a | Emergency hold and fenced recovery: interlock, liveness reconciliation, recovery proposal and receipt | R1, R11, § 2, § 4, [emergency recovery](emergency-recovery.md) E1–E5 | G1, G2 | WS2, WS5 |
| 2 | D-data | Data rules: a named bound and pruner for every growing file (`taskq.log`, attempt logs, `cli-errors.log`, `incidents/`, `requests/comments`), corruption tests for `status.json`, `tray.json`, `kids` and `focus` (unknown and kept, never reset), and `service install` hands live children over like § 8 step 3 | § 4, § 8, [data](https://github.com/taskqorg/specq/blob/main/engineering/data.md), [release contract](https://github.com/taskqorg/specq/blob/main/engineering/release-contract.md#update) | G1 | WS1 |
| 2 | T-tests | Test review per [test classes](https://github.com/taskqorg/specq/blob/main/engineering/verification.md#test-classes): inventory every cargo test, golden trace, node test and prototype test; keep a test only with a named chain and its risk; delete the rest; prototype tests default to the installed Chrome and write evidence only on request; `cargo test --locked` and `selftest` stay the whole CI gate | R12, [delivery speed](https://github.com/taskqorg/specq/blob/main/engineering/release-contract.md#delivery-speed) | G1 | — |
| 3 | F-retro | Request retrospective: every owner and operator request (tray requests, owner comments, `ask`/`answer`/`requeue` cards, PM proposals from chat) is kept in one bounded log; `taskq retro` runs the operator over that log and the use cases and prints numbered recommendations (simpler flows, missing options, quick commands); accepted ones become tasks through `add` | § 4 requests, § 5, R3, [use cases](../use-cases/README.md), [data](https://github.com/taskqorg/specq/blob/main/engineering/data.md) | G2 | WS2 |
| 3 | WS6b | Access presets and `worker-policy.json`, operator-owned grants, OS negative tests; the policy store is protected from the workers it governs, migration never turns legacy configuration into grants, unknown enforcement refuses a privileged launch ([permission authority](https://github.com/taskqorg/specq/blob/main/engineering/permission-authority.md)) | R9, § 8, [worker permissions](worker-permissions.md) P1–P4 | G1 | WS6a |
| 3 | WS4 | Recipes and owner gates over tasks; gate bound to exact revision; restart keeps the gate | § 2, § 7, [plans and decisions](plans-and-decisions.md) | G2, G4 | WS3 |
| 3 | WS7 | Delivery presets separated from permissions; exact-SHA approval; publication receipt | § 7, [agent behavior](agent-behavior.md) B1–B5 | G4 | WS2, WS6b |
| 4 | WS8, WS9 | Multi-repo environment and AI/client adapters: filed only after a slice proves the gap with a real fixture | § 6, § 8, [remote manager](remote-manager.md) | G5, G6, G7 | WS4, WS7 |
| 4 | WS11 | Native menu per route from `workspace-prototype.html` into `src/tray.html`: navigation, project counters, work mode, settings, AI workspace, recovery; delivered as an [F5 acceptance package](https://github.com/taskqorg/specq/blob/main/design/implementation-fidelity.md#f5-acceptance-package) and checked against [W1–W8](https://github.com/taskqorg/specq/blob/main/design/dashboards.md) | § 4 tray paragraph (condensed to a link), [menu](menu.md), [ui acceptance](ui-acceptance.md) | G8 | WS1, WS2, WS10; later routes WS3–WS7 |
| 4 | WS11b | Pinned window: pin control in the menu header turns the popover into a movable window above normal windows (no blur close, drag by header, edge snap on Windows, work-area clamp on macOS), state and position in `tray.json`, unpin returns to the popover | [menu](menu.md) M13, M-A21, § Product Operator UI | G8 | WS10; independent of WS11 (the pin moves with the menu when WS11 lands) |
| 4 | WS12 | Notifications: native OS notifications from the tray for new `ask`, `close`, `failed` and `gone` events; per-kind Off/Silent/Banner/Banner with sound, per-project override, once per event via a cursor in `tray.json`, burst summary, click opens the card, OS denial shown | [menu](menu.md) M14, M-A22, § 4 tray | G8 | WS1; independent of WS11 (the settings page moves with the menu when WS11 lands) |
| 5 | G8 | Qualification: Windows and Linux service, tray on Windows, update path from 4.1.11, `v4.2.0` release, native matrix, docs cleanup of specq paraphrases | [ui acceptance report](ui-acceptance-report.md) | all | everything above |

Batch 1 was authorized on 2026-10-14 and closed the same day (#886, #887, #888). Batch 2 was filed on 2026-10-14 after the owner accepted the test classes, delivery speed and Next rules in specq. The specq review of 2026-10-14 (agent workflow, data, performance, permission authority, fidelity, dashboards) added the D-data row and the specq links above; the approval boundary conflict was resolved in specq (delivery contract as grant) and § 7. Later batches are filed when the previous batch closes or when a slice becomes independent of it.

## Task template

Each task carries: the backlog row and design IDs; the affected R and § rules; current behavior with file references; the contract amendment made in the same PR (taskq.md first, spec size test green); the golden trace or adapter test that fails without the change; N/N-1 proof against 4.1.11; macOS evidence; and a report that separates Specified, Prototyped, Implemented and Qualified. Acceptance items name observable checks only (§ 1). Each task links the specq pages its slice touches (data, permission authority, implementation fidelity, dashboards) so the worker and reviewer check them; the task is the owner's grant for § 7 delivery and nothing beyond it. Tasks take runtime `any` and priority 1; `--host mac` only when the acceptance needs macOS evidence (tray, service, admission on this Mac), so the Windows host (4 claude, 4 codex, taskq 4.1.11) takes the rest (owner 2026-10-14); the promo-video tasks stay 2.

## Owner request log

Owner decision 2026-10-14: TaskQ keeps every owner and operator request, and a manual `taskq retro` turns them into recommendations. The PM appends each improvement proposal it makes in chat as one line to [use cases](../use-cases/README.md) so the retrospective sees it; the F-retro row carries the implementation.

## Batch 2 tasks

**WS3 held creation.** `add` holds a task before it is eligible: a held task is on the board but not `ready`; bulk create is idempotent per item with one receipt each; an explicit start moves it to the queue. Golden traces for hold, duplicate create and start.

**D03 admission research.** Measured on this Mac: baseline window, sampling cadence, launch spacing, memory floor, and the service's own ceiling for CPU, memory, descriptors and disk with its behavior at the ceiling. Output is a short report with numbers and the recommended defaults for WS5; no code change.

**WS5 host admission.** One host authority decides admission: Auto default from the D03 numbers, optional caps, a reservation with a lifecycle (requested, held, released) and a minimum service version. Unknown host data refuses admission with a reason.

**WS6a emergency hold.** An interlock that stops new spawns, liveness reconciliation of live children, a recovery proposal with the exact operations and a receipt; E1–E5 in the same PR, with lifecycle amendments to § 2 and § 4 where the behavior changes.

**D-data.** Every growing file has a named bound and a pruner; corruption of `status.json`, `tray.json`, `kids` or `focus` reads unknown and keeps the file; `service install` hands live children over like § 8 step 3.

**T-tests.** Inventory every test; keep one only with a named chain and its distinct failure risk; delete the rest; prototype tests default to the installed Chrome and write evidence only on request; the CI gate stays `cargo test --locked` and `selftest`, green on the three platforms.

## Batch 1 tasks

**WS1 read model.** One function builds the snapshot from one board read (or from `status.json` for the tray); `taskq status` text, `taskq status --json`, `service status` and the tray read it. Snapshot fields: `schema` 1, source (board or host) and observed time, `complete` with a reason when the board was unreadable, project and board identity, per-state totals, open tasks with labels and attempt, capabilities. Unknown is never zero. No new transport, no UI change.

**WS2 dispatcher.** One dispatch path for `answer`, `requeue`, `later`, `failed`, `steer`, `close`, `result`, `ask` and the tray proposal apply: validate the target through `fold`, compare the expected revision (the id of the current card or attempt), write one entry, read back, return a receipt (accepted with readback, rejected with reason, stale with the current revision, unknown with a reconcile hint). An op id repeated inside the retry window returns the first receipt without a second write. Golden traces cover two concurrent answers, a stale revision and a retried op id.

**WS10 icon renderer.** One Rust renderer draws the Summary preset and a Custom configuration from the versioned `icon-summary.config.json` schema; `tray --snapshot` and the live icon use it; a validate step reports errors and keeps the current icon on an invalid draft; the same configuration yields the same bytes (hash test); macOS DPI and both themes are proven with snapshots, Windows `Not tested`.
