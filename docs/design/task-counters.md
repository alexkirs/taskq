# Task counters

Approved: aggregate above Projects; all queue states available; Closed visually quiet. Human = Ask: tasks waiting for a human answer (owner confirmed).

## Semantics

- Home: one compact aggregate strip immediately above Projects. Scope: all registered observed boards, not just pinned/recent or admitting projects. Click opens the corresponding task list.
- Every visible project row (pinned, recent and All projects) includes its own nonzero counters beside its name; aggregate counters never replace them. A known all-zero project shows its name only, without an empty strip or `No tasks` caption.
- Project: same order and symbols, only that board/project. Deduplicate by provider/server/repository/issue across registrations/hosts; conflicting or stale observations mark incomplete.
- Order: Doing, Review, Ask, Failed, Ready, Blocked, Later, Closed. No Work total alongside Doing/Review; that would count twice.
- Zero hides its capsule; unknown shows `?`, never zero. Empty accurate state says `No tasks`. Incomplete sum says `Partial` with affected sources.
- `Ask` means a human answer is needed. It is not human execution. `assignee-only` is an admission constraint, not a human-owned task state. Add a separate Human execution counter only after a state/data contract exists.
- Closed: current total of closed TaskQ-managed issues, independent of cadence period. Not all board issues, not sum of close events, not successful attempts. Reopening removes a task from Closed.
- Service obtains authoritative paginated closed totals or maintains a verified board projection. Until available show unknown; never infer all-time totals from local event history. Tray stays token-free and reads status only.
- Breakdowns by task type are filters, not extra additive lifecycle counts. AI failures and service incidents remain separate.
- Square tray summary stays Work = Doing + Review, Ask, Ready, cadence. These expanded counters are menu-only.

## Visual

- Compact wrapping capsules with stable order; symbol + number, accessible state name and tooltip. Do not stretch across full width.
- Doing mint, Ask peach, Failed muted red. Review/Ready/Blocked/Later neutral; shape carries identity.
- Closed last: pale gray background, quiet gray text, no colored fill. Retain readable contrast and a full accessible label; do not fade with opacity.
- At large type, wrap counters together beneath the project name. Never shrink numbers or squeeze project titles into a narrow column.
- Counter clicks are separate targets from project navigation. One project name row, then its compact strip if needed; no nested controls.

## Acceptance

- K-A1: same task registered twice counts once; Doing and Review stay disjoint.
- K-A2: Failed/Ask/Blocked/Later remain visible when nonzero; zero absent, unknown explicit.
- K-A3: Closed unaffected by cadence period; reopen and incomplete pagination handled honestly.
- K-A4: aggregate includes projects outside shortlist; scope and partial sources inspectable.
- K-A5: 320 px and 20 px type wrap without horizontal overflow; Closed readable in both themes.
- K-A6: selected capsule opens matching tasks; no accidental project navigation.

- K-A7: pinned/recent/All projects preserve per-project nonzero counters; known all-zero rows show only project identity; each counter opens that project and state, without triggering the project-name action.
