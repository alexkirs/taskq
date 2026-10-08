# PM intake: default algorithm for a stream of owner requests

Research for [#256](https://github.com/alexkirs/taskq/issues/256). Owner decision 2026-10-08: the PM files one task per request, and tasks pile up, overlap and conflict.
This document proposes the default PM intake logic, the confirm card, worked examples from #240–#256 and the contract text to adopt.
Rules cited are [principles.md](../taskq/contracts/principles.md) R1–R12. No code change.

## Problem seen in this session

The owner said 'do X', 'also Y', 'idea Z' in one stream. The PM filed #242–#256 one at a time. Results:

- Work done and then deleted: #244 fixed how `refs/taskq/lock/N` is deleted; #249 then deletes the lock entirely. #245 made dead workers release their #208 reservation; #249 then deletes reservations.
- Tasks that contradict their own source: #251 asks cleanup to close Claude and Codex sessions, yet cites "Audit F4 acceptance 1-4", and F4 acceptance 3 says "sessions are F3's retire step".
- Missing links: #251 uses "the runtime adapters' list/close functions" that #252 creates, but #251 has no deps.
- Near-conflicts between tasks: #248 deletes the prompt-version and outdated-prompt machinery; #253 adds "prints one line telling the session to re-read the contract" when contracts change, a new form of the same thing.

Each item was cheap to catch at intake and costly after a worker ran.

## Known practices, and what an agent PM takes from each

| Practice | What it does | Keep for taskq | Drop |
|---|---|---|---|
| Intake triage (Kanban, ITIL request intake) | New requests land in an inbox; triage decides accept, duplicate, reject, needs-info before work starts | One triage pass per batch, four verdicts plus amend | A separate inbox column or tool (R1: board is the only state) |
| Backlog grooming / refinement (Scrum) | Periodically merge, split, re-scope and re-order backlog items | Search open tasks on every intake, not on a cadence; rebuild tasks in one batch | Meetings, story points, sprints |
| Duplicate linking (Jira "duplicates", GitHub "closed as duplicate") | Keep one canonical item, close others with a link | Canonical task keeps all source links and requirements (#241 acceptance 2) | Auto-close on similarity: owner decides (R2) |
| ADR / decision log (Nygard) | Each decision is a short record: context, decision, consequences; superseded, not edited | The confirm card answer is the decision; it goes into the task goal as "Owner decision YYYY-MM-DD" and as a board note | Separate ADR files: the issue history is the log (R1); rule-level decisions amend principles.md (Change rule) |
| WIP limits (Kanban) | Limit work in progress | Already the tick slots (R3) | — |

Agent constraints that shape the default:

- **Short context.** The PM cannot hold the whole board. It reads a bounded view: open task titles, scopes and deps (`taskq list`), then full bodies only of candidate overlaps.
- **Board is the SoT (R1).** Pending cards live only in the chat; nothing is written until the owner answers. If the session dies before the answer, the board is unchanged and the owner repeats the stream. No draft store.
- **Unverified means unknown (R12).** "No overlap" means "none found in the bounded read"; the card says what was searched.
- **Style (R7).** Cards are short tables, not prose.

## Default algorithm

1. **Collect.** Split the owner's message (or stream, until the owner says "go" / «всё» / asks a question) into atomic requests, one line each, in the owner's words. Answer pure questions directly; they are not requests.
2. **Read the board once.** `taskq list` for all open tasks (title, state, scope, deps, claim). For each request pick candidates by shared paths, shared mechanism or R-number, and read only those bodies.
3. **Classify each request.** One proposal per row:
   - **amend #N**: the request changes an open, unclaimed task. Edit goal/acceptance/scope.
   - **merge #A #B → #A**: two open tasks (or a request and a task) do one job. Canonical keeps source links, requirements, decisions, acceptance, deps (#241 acceptance 2). Others close as duplicate with a link.
   - **new**: no overlap found. Give title, type, scope, deps, priority.
   - **dep #A → #B**: separate tasks, but one needs the other's output or deletes its code. Add deps.
   - **reject / defer**: conflicts with a rule the owner does not want to amend, already done, or not worth it now (`taskq later`).
   - **ask**: the PM cannot classify without a product choice; the row states the options.
   A claimed task (`doing`, `review`) is never amended or merged automatically (R2): the row proposes a follow-up task or an `ask` to its supervisor.
4. **Check conflicts.** For each row list:
   - **R-conflicts**: which R1–R12 the request breaks or amends. A rule change follows the Change rule (names the R-number, edits principles.md and the Wiki in one deliverable).
   - **Task conflicts**: open or recently closed tasks whose code it deletes, re-adds or contradicts; scope overlap with tasks that may run in parallel; missing deps.
5. **Advise.** One line per row: the PM's recommendation and why, e.g. "skip #244: #249 deletes the lock".
6. **Confirm.** Show the card (format below). The owner answers per row: `yes`, `no`, or an edit ("3: yes, but priority 2"). No answer means no change.
7. **Apply in one batch.** Only rows answered yes, in this order: closes and merges, amends, new tasks, deps. Each change carries "Owner decision YYYY-MM-DD (intake)" in the goal or the note. One command per change (`taskq add`, `taskq edit`, `taskq close`/duplicate note, `taskq edit N --deps`).
8. **Verify and report.** Re-read the changed tasks (R12) and print the R6 table of what changed. Rows answered no are dropped; they are not filed "for later".

Cost bound: one `taskq list` plus at most ~5 body reads per request. If the board is larger than the PM can read, say so on the card ("searched titles only").

## Confirm card

One table per batch. Columns are fixed; cells are short.

| # | Request | Proposal | Conflicts | Advice | Yes/no |
|---|---|---|---|---|---|
| 1 | owner's words | amend #N / merge #A #B→#A / new "title" (type, scope, deps) / dep #A→#B / reject / ask | R-numbers; task numbers with one word (deletes, re-adds, overlaps, needs) | one line | |

Below the table, one line: "Searched: N open tasks, bodies of #a #b #c." The owner answers in one message, e.g. `1 yes, 2 no, 3 yes priority 2`.

## Worked examples from #240–#256

### Example 1: request duplicates an open task (merge) — #241 into #242

Requests: #241 "PM triage: merge duplicate requests into one canonical task" was open; the owner then asked for #242 "one canonical contract", whose content list says "R2 ... dedup tasks at intake (ex-#241)".

| # | Request | Proposal | Conflicts | Advice | Yes/no |
|---|---|---|---|---|---|
| 1 | one canonical contract R1–R12 | new #242 (docs) | none | file | yes |
| 2 | (open #241) PM dedup at intake | merge #241 → #242 as rule R2; close #241 as duplicate with link | #241 acceptance 1–4 must survive in R2 | merge: one rule, one place (R8) | yes |

Outcome then: #241 closed after #242 absorbed it, which is what this card produces in one batch.

### Example 2: request deletes code another task just fixed (dep / reject) — #244, #245 vs #249

Requests in one stream: "release must delete the lock ref" (#244), "dead worker releases its reservation" (#245), and later, from the audit, "launch and take without reservations or tracker lock" (#249).

| # | Request | Proposal | Conflicts | Advice | Yes/no |
|---|---|---|---|---|---|
| 1 | release deletes `refs/taskq/lock/N` | reject, or hold behind the audit | #249 deletes the lock (R1) | skip: the lock goes away | |
| 2 | dead worker releases reservation | amend: liveness only for claims; drop reservation part | #249 deletes reservations; #252 owns `liveness()` | keep liveness, drop reservations | |
| 3 | take without reservations or lock | new #249 (code) | R1, R2 | file first, then 1–2 shrink | |

Had the three arrived together, two tasks of work would have been avoided or halved.

### Example 3: task contradicts its own source and misses a dep (amend) — #251, #252

Request: F4 cleanup (#251) with "cleanup also closes Claude and Codex sessions of closed tasks" (owner note Q5).

| # | Request | Proposal | Conflicts | Advice | Yes/no |
|---|---|---|---|---|---|
| 1 | one `taskq cleanup` also removes orphan sessions | amend #251: drop "Audit F4 acceptance 3" (sessions are F3's retire step) or state that Q5 overrides it; add dep #252 | R11 (who retires sessions); #250 (F3 retire step) overlaps; #252 provides `list`/`close` | amend, add dep #252 | |

### Example 4: replace an older task (amend by replacement) — #246 replaces #186

Request: "manage projects A, B by phrase" while #186 (old multiproject task) and audit F1 existed.

| # | Request | Proposal | Conflicts | Advice | Yes/no |
|---|---|---|---|---|---|
| 1 | PM manages a listed set of projects by phrase | new #246; close #186 as superseded with link; #246 covers audit F1 (`taskq/multiproject.py`), so F1 is not filed separately | R10 (explicit list only), R6, R1 | file one task, not #246 plus F1 | yes |

Outcome then: #246 says "replaces #186", and no separate F1 task was filed: the right result, reached by hand.

### Example 5: two tasks re-add what a third deletes (task conflict) — #248 vs #253

Requests: "tick is a message; delete prompt-version and re-arm" (#248) and "running sessions pick up new contracts on next tick" (#253).

| # | Request | Proposal | Conflicts | Advice | Yes/no |
|---|---|---|---|---|---|
| 1 | delete prompt-version, re-arm, outdated-prompt | new #248 (code) | amends R4 | file | yes |
| 2 | sessions re-read changed contracts | new #253 with dep #248; acceptance: one digest line, no version field | #248 deletes the version check; #253 must not re-add it | file after #248, keep it one line | |

### Example 6: request that is this task's own dependency — #254 vs #256

Requests: "remember: ..." prefs (#254) and "the PM intake default must be editable by the owner" (#256).

| # | Request | Proposal | Conflicts | Advice | Yes/no |
|---|---|---|---|---|---|
| 1 | owner can edit the PM intake default | dep #254 → #256 adoption: personal override is a pref line | R9 (no silent changes); R1 (prefs are local config, not task state) | adopt the contract text now; personal override works once #254 lands | |

## How the owner overrides the default

Precedence, highest first:

1. **R1–R12** (principles.md). An override cannot break a rule; changing a rule is a rule change (Change rule: a task naming the R-number).
2. **Owner's words in the current session.** "File it now, no card" or "skip the card for typo fixes" applies to that message or session.
3. **Personal prefs** in `taskq.local.toml` `[prefs] notes` (from #254): the owner says «запомни: …» / "remember: …", e.g. "remember: PM files single bug reports without a card" or "remember: PM intake card without the Advice column". Every PM brief prints the prefs verbatim (#254), so the PM applies them. Personal, uncommitted, no code change.
4. **Project default**: the "Intake" section of `taskq/contracts/taskq-manager.md` (text below). The owner edits it like any contract text: a docs task, or a direct edit; it ships to every machine with `taskq update`.

Until #254 lands, level 3 is not available; the owner uses level 2 or 4.

## Contract text to adopt

### principles.md — amend R2 and R3 (Change rule)

Replace the last sentence of **R2** with:

> The PM finds duplicates and conflicts at intake and proposes amend, merge, new, dependency or reject per request; the owner answers yes or no per item before any task changes; active claims are never re-bound automatically. Mechanics: [taskq-manager.md § Intake](https://github.com/alexkirs/taskq/blob/main/taskq/contracts/taskq-manager.md#intake).

Replace the *Root PM* bullet of **R3** with:

> - *Root PM*: talks to the owner; triages requests against open tasks and R1–R12, shows one confirm card per batch, and changes the board only for confirmed items, in one batch. Never merges, never closes after review.

The task that adopts this lists "amends R2, R3" in its result and mirrors the Wiki Principles page (R8).

### taskq-manager.md — new section "Intake" (replaces the duplicates bullet in § 4)

> ## Intake
>
> Default for owner requests ('do X', 'also Y', 'idea Z'). Owner prefs and the owner's words in the session override it; R1–R12 do not yield.
>
> 1. Collect the requests of the message or stream, one line each, in the owner's words.
> 2. `taskq list`; read bodies only of tasks sharing paths, mechanism or R-number with a request.
> 3. Propose per request: amend #N, merge #A #B → #A, new, dep #A → #B, reject/defer, or ask. Never amend or merge a claimed task; propose a follow-up instead.
> 4. Name conflicts: R-numbers broken or amended; tasks whose code it deletes, re-adds or overlaps; missing deps.
> 5. Show one card: `# | Request | Proposal | Conflicts | Advice | Yes/no`, then "Searched: …".
> 6. Change nothing until the owner answers per row. No answer is no.
> 7. Apply confirmed rows in one batch (close/merge, amend, new, deps), each with "Owner decision YYYY-MM-DD (intake)"; re-read the changed tasks; print the R6 table.
>
> Merge keeps the source links, requirements, decisions, acceptance and deps in the canonical task and closes the others as duplicates with a link.

## Follow-up for the PM to file

One docs task: "Adopt PM intake (amends R2, R3)": edit `taskq/contracts/principles.md` R2/R3 and the Wiki Principles page, add the Intake section to `taskq/contracts/taskq-manager.md`, drop the duplicates bullet in § 4. Dep: none; the prefs override needs #254. Coordinate with #255 (F7 also edits taskq-manager.md).
