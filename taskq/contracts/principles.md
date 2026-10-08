---
type: Contract
status: Active
domain: agent-workflow
canonical: true
---

# TaskQ principles (R1–R12)

One canonical contract for every TaskQ rule, brief, contract and doc (owner decision 2026-10-08, [#242](https://github.com/alexkirs/taskq/issues/242)).
Other documents link here and add only mechanics; they never restate a principle.
Published on the Wiki as [Principles](https://github.com/alexkirs/taskq/wiki/Principles); this file is its packaged copy, kept identical.
Sources: closed [#185](https://github.com/alexkirs/taskq/issues/185), [#187](https://github.com/alexkirs/taskq/issues/187), [#191](https://github.com/alexkirs/taskq/issues/191), [#205](https://github.com/alexkirs/taskq/issues/205), [#206](https://github.com/alexkirs/taskq/issues/206), [#220](https://github.com/alexkirs/taskq/issues/220), [#224](https://github.com/alexkirs/taskq/issues/224), [#240](https://github.com/alexkirs/taskq/issues/240), [#241](https://github.com/alexkirs/taskq/issues/241).

## Change rule

A task that changes a rule names the R-number it amends in its title or goal, edits this file and the Wiki page in one deliverable, and its result lists the amended R-numbers.
Amend; never overwrite. A rule elsewhere that disagrees with this file is a defect: fix that rule or amend this file, never keep both.
A new rule gets the next R-number.

## Rules

**R1. Board is the only state and lock.** Issue state label, issue metadata block and trusted taskq notes hold all task state, claims and history. No extra database, queue, receipt store, mirror or protocol. Mechanics: [taskq.md § Where things are stored](https://github.com/alexkirs/taskq/blob/main/taskq/contracts/taskq.md#where-things-are-stored).

**R2. One task, one supervisor session, one worker session.** One canonical task has one supervisor and one worker session. Work bigger than one session is several tasks linked by deps, never sub-tasks or multi-task workers. The PM finds duplicates at intake and proposes merge or separate; the owner decides before any task changes; active claims are never re-bound automatically.

**R3. Roles.**
- *Root PM*: talks to the owner; files, triages and moves tasks. Never merges, never closes after review.
- *Queue tick* (coordinator): dispatches one supervisor session per ready task within free slots (one slot holds a task's supervisor and its worker), the same on Claude and Codex; wakes a supervisor its task needs; reconciles board and sessions.
- *Supervisor*: launches its one worker, reviews its result, publishes it per the project's `[workspace] publish` (`direct`: the worker pushed `main`, `close` verifies it; `review`: `close` fast-forwards `main` to the reviewed branch), writes the outcome to the board with `close` or `reject`, retires the worker (R11).
- *Worker*: does the task and writes its result to the board (R5).

Mechanics: [taskq.md § Four roles](https://github.com/alexkirs/taskq/blob/main/taskq/contracts/taskq.md#four-roles), [taskq-manager.md](https://github.com/alexkirs/taskq/blob/main/taskq/contracts/taskq-manager.md).

**R4. One tick sender per project.** Exactly one coordinating timer per project; moving it is an explicit handoff. A tick on another machine starts only its own `host-*` tasks and never coordinates. Mechanics: [taskq-manager.md § 2](https://github.com/alexkirs/taskq/blob/main/taskq/contracts/taskq-manager.md#2-arm-the-tick).

**R5. Worker writes completion to the task.** Result SHA, checks, question or blocker go to the task through `taskq result`, `ask` or `problem`. Completion never depends on session UI, chat or transcript.

**R6. Human report.** Per project: heading, Board link, one table Task | Status | Runtime | Session with clickable links, then the owner's open questions. Same table in Claude and Codex; links are built per runtime. No raw JSON to humans; JSON stays inside transport. Generated template: [pm-report-v1.md](https://github.com/alexkirs/taskq/blob/main/taskq/contracts/pm-report-v1.md).

**R7. Style.** Every role and message follows [gradus-public/caveman](https://gitlab.ufobe.com/gradus-public/caveman/-/tree/62579538f05fb6b69a12449c1ebad9567d1fdecc) pinned at `6257953`. Short. Unknowns stated honestly. TaskQ links the style; it does not redefine it.

**R8. Wiki is the SoT and matches code.** Briefs, onboarding and contracts link to the Wiki or this file; they never copy it. A code change that changes behavior updates the Wiki in the same deliverable.

**R9. No silent changes to model, effort or permissions.** Any change is named to the owner first; taskq never edits permission settings itself.

**R10. Multi-project only by explicit list.** A session manages several projects only from an owner-written list. Folders are never auto-discovered. Mechanics: the list is `[projects]` of the PM's `taskq.local.toml`; `taskq projects` runs one ordinary tick per listed checkout ([docs/multiproject-pm.md](https://github.com/alexkirs/taskq/blob/main/docs/multiproject-pm.md)).

**R11. Retire a task's sessions only after accepted review.** A worker or supervisor session of a task ends only after its result is accepted. Sessions without any task are cleanup's: [taskq-manager.md § Cleaning up finished work](https://github.com/alexkirs/taskq/blob/main/taskq/contracts/taskq-manager.md#cleaning-up-finished-work).

**R12. Unverified means unknown.** Report only what a fresh read proved. A delivery, exit code, checkout marker or chat turn is not proof of receipt, application or completion.

## Known gaps between rules and code

None open. Closed by [#243](https://github.com/alexkirs/taskq/issues/243): the supervisor closes and publishes its task (R3); the tick spawns a supervisor, not a worker, for every ready task (R3); a supervised task's worker continues in the same session after `answer`, `reject` or `release` (R2, R11). A task started before #243 without a `supervisor` keeps the legacy path until it closes.
