# Agent delivery behavior

Product selection: D04 option 1 is accepted in the [decision register](decision-register.md). Remaining candidate language below refers to implementation/calibration, not a request to reapprove that direction. No running policy changes.

Proposal, not runtime policy, except the [§ 7 delivery presets](../../taskq.md#7-worker-reviewer-publication) (`auto-merge`, `direct`, `review-pr`, policy revision, exact-SHA approval, publication receipt), which §7 governs. Shared UI rules: [specq operator UI](https://github.com/taskqorg/specq/blob/main/design/operator-ui.md). Access grants remain in [agent settings](agent-settings.md).

## B1 Scope and owner

- Settings → Agents → Delivery. Access stays a separate tab. Applies to workers/reviewers across runtimes; operator publication needs explicit owner authorization under R3/R9 and the shared approval boundaries; tool availability alone grants none.
- One owner-managed default plus explicit project overrides. Project identity includes forge, repository and target branch; display source and effective policy. Avoid per-agent/per-host publication defaults fighting over one project.
- Task requests can narrow policy. Broader effects require an owner-reviewed change. Agent-written files, issue text and Full access cannot authorize publication.
- Execution still requires host credentials, runtime capability and repository protections. Show blocked capability separately from workflow intent; never weaken protections.
- Policy storage/authority across hosts needs a versioned owner-approved contract before implementation. Fixture UI changes no repository, config or service.

## B2 Presets

Representative workflows, not measured popularity. Proposed new-project default: Review PR. Existing projects retain their behavior through explicit migration review.

| Preset | Commit | Push | PR/MR | Merge | Handoff |
|---|---|---|---|---|---|
| Local | After checks | No | None | Never | Inspect local branch; task remains open |
| Push branch | After checks | Task branch | None | Never | Share branch; task remains open |
| Review PR | After checks | Task branch | Ready | Ask owner | Owner reviews exact candidate |
| Auto-merge | After checks | Task branch | Ready | After gates | Publish, read back, close |
| Direct | After checks | Task branch | No review PR | After gates | Advance target without force; read back, close |

- Direct is a publication method, not permission to bypass CI/review. GitLab direct currently uses a fast-forward MR; never promise no MR there.
- Custom controls: commit Leave changes / After checks; push None / Task branch; PR None / Draft / Ready; merge Never / Ask owner / After gates; PR merge method Squash / Merge commit / Rebase, gated by forge/repo capability.
- Local/no-commit is a handoff proposal; current `result` requires a committed SHA. Do not pretend it can finish today's queue task.
- Draft PR never auto-merges. Push requires a commit; PR requires push; merging requires publication. Disable incompatible controls, explain dependency; never silently enable broader effects.
- Preset selection drafts its complete policy. Individual edits show Custom. Review includes derived changes and exact target.
- Commit granularity follows task acceptance; no per-file knobs. Branch naming follows project contract. Automatic force-push, release/deploy and destructive cleanup are outside presets; no implied authorization.

## B3 Gates and completion

- Project checks before handoff; CI and task reviewer on the exact publication candidate; required human approvals and branch protections before merge. Missing/failed/unknown gates block. Latest SHA invalidates prior approval.
- Conflict: preserve branch, return to work/review; no force update. New SHA reruns gates. Human-answer flow remains `ask`.
- Never equate PR creation with delivery or close. Local/branch/manual-merge workflows need explicit pending-handoff states and dependency semantics before shipping.
- After publication, verify remote target and merge receipt before close. Uncertain writes reconcile by attempt/SHA/PR identity before retry; no duplicate PR.
- Current §7: commit/rebase/checks; direct or PR push; reviewer close publishes; PR squash only; clean worktree cleanup. New modes require §2/§7/§9 and executable model changes. Existing `publish: pr` maps to automatic PR publication, not proposed Review PR.

## B4 UI

- Compact target picker: Default / project override. One preset selector, one plain-language outcome, details only on request. Show Codex + Claude once, no duplicate runtime controls.
- Project override starts from effective inherited policy. Remove means inherit; review any broadened behavior. Keep separate drafts when switching targets/tabs.
- Details expose workflow stages, target branch and merge strategy. Repository choices come from registered projects and verified capabilities; free text never chooses an unverified remote.
- Review shows old → new, exact target, affected inheritors and active-attempt treatment. Confirm binds revision; Saved is not Applied. Stale revision blocks; fresh service readback establishes effective policy.
- New policy affects new attempts. Tightening publication policy must fence pending publish operations at their final gate; widenings never silently upgrade existing attempts. This mechanism requires lifecycle implementation.
- Attention shows Waiting for merge / CI failed / Conflict / Policy changed with one next action and issue/PR link.

## B5 Acceptance

- B-A1: all presets have a truthful stage sequence; manual commit disables remote actions; Draft cannot auto-merge.
- B-A2: project overrides survive default changes; removal reviews inherited effects; drafts survive navigation.
- B-A3: Full runtime access does not select Auto-merge; behavior stays consistent across Codex/Claude.
- B-A4: confirm cannot repeat; stale revision refuses; saved/failed/unknown never claim applied.
- B-A5: exact-SHA gates, manual handoff, remote readback, unknown-write reconciliation and existing-attempt fencing have model traces before release.
- B-A6: compact 16/20px, light/dark, narrow views; readable outcome and all actions reachable.

[Interactive component](agent-behavior.html), composed into [workspace](workspace-prototype.html). Merge capabilities and revision/readback use labelled fixtures. No runtime changes.

## Contract detail

Moved verbatim from `taskq.md` (#1179) to keep the contract under its R8 size bound. `taskq.md` keeps the decision and links here; this text keeps the same authority (R8, R13).
**Delivery presets** ([agent behavior](agent-behavior.md) B1–B5; commit, push, PR and merge, apart from § 8 Access) are § 9 `delivery` `{preset, revision}`, the owner's (R9; the revision +1 per owner edit): `auto-merge` (the `pr` route), `direct`, `review-pr` (the `pr` route after the owner gate below). `local` and `push-branch` are refused until B3 pending-handoff states exist. Without `delivery`, `publish` names the preset at revision 0 (`pr` is `auto-merge`); nothing converts it to `review-pr`, a migration is an owner-reviewed edit. `close` reads the policy then. Its publication receipt is the last verdict line `delivery <preset> r<revision>: <main fast-forwarded to SHA|merged #P as SHA|SHA already on main>`. `review-pr` needs every host at `4.2.0-dev` or newer: 4.1.11 ignores `approve` and publishes it as `auto-merge`.
