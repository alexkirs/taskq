# Implementation plan

Executable plan for the [refactoring backlog](refactoring-backlog.md) WS1–WS11, written against checkout 4a99d79 (2026-10-14). The backlog stays the single work breakdown; this file orders it into board batches, fixes the development loop and records the owner decisions that the [brief](implementation-brief.md) left open. Runtime truth stays [taskq.md](../../taskq.md); process rules stay [specq](https://github.com/taskqorg/specq).

## Reconciliation (2026-10-14)

- Installed: mac and win on 4.1.11; `Cargo.toml` says `4.0.0-dev`. No refactoring task exists on the board; open tasks are the promo-video set (#802, #803, #805).
- Code: `src/main.rs` (CLI, service loop, capacity, focus), `src/tray.rs` (menu, IPC, icon raster), `src/operator.rs` (file requests), `src/tray.html`. No canonical read model: `status`, `status.json` and the tray each compute states and counts. No operation id, revision check or receipt on any mutation. `add --later` creates an eligible issue before posting `later` (scenario review, UC-07). `wait` uses one cursor `<root>/.taskq/pm-cursor`.
- Design package tests pass (`docs/design/*.test.cjs`); `cargo test --locked` 91 passed; `selftest` 40 traces ([verification](verification.md)).

## Owner decisions

1. Incremental 4.x releases under § 8 N/N-1; no 5.0 cutover. Every merged slice must be readable by a 4.1.11 host because the Windows host stays on 4.1.11 until final qualification.
2. Work runs as taskq tasks (dogfooding): the PM files one reviewable slice per task, the reviewer closes per § 7. Design-package planning never starts queue work by itself.
3. macOS first: no release tag, update or Windows/Linux deploy during development (see the loop below). Results report macOS evidence; Windows and Linux are `Not tested` (R12) until the qualification batch.
4. taskq.md stays at or under 300 lines: an amendment condenses prose and moves detail into the linked design spec rather than adding paragraphs.
5. Auto admission mechanics (D03, [dynamic resources](dynamic-resources.md) R-A1–R-A8) get one research task with measurements on this Mac before WS5 is filed.

## Development loop on this Mac

The service, the tray and the CLI run from `<install>/releases/<current>`; `current` moves only forward (coreq `newer`). A development build therefore carries a version that sorts above every published 4.1.x and below the final 4.2.0: `4.2.0-dev.<unix seconds>` (pre-release suffix, fixed width, later build sorts later). The update check (§ 8 step 7) sees no newer GitHub release and keeps it; the final `v4.2.0` tag replaces it. `previous` keeps 4.1.11 for rollback.

After each merge to `main`:

```bash
cd ~/Projects/taskq && git pull --ff-only
v=4.2.0-dev.$(date +%s)
sed -i '' "s/^version = .*/version = \"$v\"/" Cargo.toml
cargo build --release && target/release/taskq selftest && target/release/taskq service install
git checkout Cargo.toml Cargo.lock
for p in $(cat ~/.local/share/taskq/projects); do touch "$p/.taskq/update"; done
```

`service install` copies the binary into `releases/<v>`, writes `current` and reloads the taskq unit; the other registered services re-exec through § 8 step 3 on their next pass (the `update` poke makes that immediate); the running tray exits because its version differs from `current` and the service starts the new one. Menu evidence for a task: `target/release/taskq tray --snapshot <dir>` from the worktree, or the live tray after the install.

Rollback to the published release:

```bash
echo 4.1.11 > ~/.local/share/taskq/current
for p in $(cat ~/.local/share/taskq/projects); do (cd "$p" && ~/.local/share/taskq/taskq service restart); done
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
| 2 | D03 | Research: Auto admission calibration on this Mac (baseline window, sampling, launch spacing, memory floor) | none; feeds WS5 | G3 | — |
| 2 | WS5 | Host admission: one host authority, Auto default, optional caps, reservation lifecycle, min service version | R1, § 3, § 4, [dynamic resources](dynamic-resources.md) | G3, G1 | WS1, D03 |
| 2 | WS6a | Emergency hold and fenced recovery: interlock, liveness reconciliation, recovery proposal and receipt | R1, R11, § 2, § 4, [emergency recovery](emergency-recovery.md) E1–E5 | G1, G2 | WS2, WS5 |
| 3 | WS6b | Access presets and `worker-policy.json`, operator-owned grants, OS negative tests | R9, § 8, [worker permissions](worker-permissions.md) P1–P4 | G1 | WS6a |
| 3 | WS4 | Recipes and owner gates over tasks; gate bound to exact revision; restart keeps the gate | § 2, § 7, [plans and decisions](plans-and-decisions.md) | G2, G4 | WS3 |
| 3 | WS7 | Delivery presets separated from permissions; exact-SHA approval; publication receipt | § 7, [agent behavior](agent-behavior.md) B1–B5 | G4 | WS2, WS6b |
| 4 | WS8, WS9 | Multi-repo environment and AI/client adapters: filed only after a slice proves the gap with a real fixture | § 6, § 8, [remote manager](remote-manager.md) | G5, G6, G7 | WS4, WS7 |
| 4 | WS11 | Native menu per route from `workspace-prototype.html` into `src/tray.html`: navigation, project counters, work mode, settings, AI workspace, recovery | § 4 tray paragraph (condensed to a link), [menu](menu.md), [ui acceptance](ui-acceptance.md) | G8 | WS1, WS2, WS10; later routes WS3–WS7 |
| 5 | G8 | Qualification: Windows and Linux service, tray on Windows, update path from 4.1.11, `v4.2.0` release, native matrix, docs cleanup of specq paraphrases | [ui acceptance report](ui-acceptance-report.md) | all | everything above |

Batch 1 was authorized on 2026-10-14; later batches are filed when the previous batch closes or when a slice becomes independent of it.

## Task template

Each task carries: the backlog row and design IDs; the affected R and § rules; current behavior with file references; the contract amendment made in the same PR (taskq.md first, spec size test green); the golden trace or adapter test that fails without the change; N/N-1 proof against 4.1.11; macOS evidence; and a report that separates Specified, Prototyped, Implemented and Qualified. Acceptance items name observable checks only (§ 1). Tasks take runtime `any` and priority 1; the promo-video tasks stay 2.

## Batch 1 tasks

**WS1 read model.** One function builds the snapshot from one board read (or from `status.json` for the tray); `taskq status` text, `taskq status --json`, `service status` and the tray read it. Snapshot fields: `schema` 1, source (board or host) and observed time, `complete` with a reason when the board was unreadable, project and board identity, per-state totals, open tasks with labels and attempt, capabilities. Unknown is never zero. No new transport, no UI change.

**WS2 dispatcher.** One dispatch path for `answer`, `requeue`, `later`, `failed`, `steer`, `close`, `result`, `ask` and the tray proposal apply: validate the target through `fold`, compare the expected revision (the id of the current card or attempt), write one entry, read back, return a receipt (accepted with readback, rejected with reason, stale with the current revision, unknown with a reconcile hint). An op id repeated inside the retry window returns the first receipt without a second write. Golden traces cover two concurrent answers, a stale revision and a retried op id.

**WS10 icon renderer.** One Rust renderer draws the Summary preset and a Custom configuration from the versioned `icon-summary.config.json` schema; `tray --snapshot` and the live icon use it; a validate step reports errors and keeps the current icon on an invalid draft; the same configuration yields the same bytes (hash test); macOS DPI and both themes are proven with snapshots, Windows `Not tested`.
