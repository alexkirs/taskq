# Proposal: Wiki specification and drift process

The user has selected the published [TaskQ Wiki](https://github.com/alexkirs/taskq/wiki) as normative SoT and mandatory docs-first for new features/functions. This [#189](https://github.com/alexkirs/taskq/issues/189) candidate proposes structure, delegation and rollout only. Publishing this proposal does not activate governance or prove implementation.

## Structure and proposed delegation

Keep one Wiki: [Home](https://github.com/alexkirs/taskq/wiki/Home) links this process and short separate feature-specification pages; [Required settings](https://github.com/alexkirs/taskq/wiki/Required-settings) contains configuration rules, allowed values, defaults and precedence only; [Known issues](https://github.com/alexkirs/taskq/wiki/Known-issues) keeps symptom/action bullets linking unresolved drift. Read baseline: Wiki revision `6bcc8868cd2be0c635f17d180b1ff07dc554eaeb`; this extends Home's existing page links without changing live pages.

**Proposed delegation, not globally active:** PM approves technical specifications within agreed goals and constraints. New product decisions, spending and security changes require the applicable approval; PM specification approval cannot override those boundaries. The owner still decides this delegation and publication authority.

## One docs-first sequence

1. **Draft/proposed:** before code for a new feature/function, publish its short Wiki specification with intended behavior, exceptions, configuration where relevant, and executable acceptance criteria. Link its task/project. A repository review candidate is not the required Wiki specification.
2. **Accepted:** obtain required approval identifying the normative text and Wiki revision; record the approval permalink and accepted Wiki commit/page link in the issue/PR. Mark accepted but not yet implemented. Only then code against that pinned revision; changed requirements need recorded acceptance of a new revision before dependent implementation.
3. **Implemented:** submit the exact candidate SHA, pinned spec/approval links, focused conformance checks and project-required gate through the existing task/PR/review workflow. Reviewer compares that SHA with the accepted version; mark implemented only with reviewed code and passing evidence. An accepted spec may precede implementation; `taskq result` remains a review submission, not acceptance/publication.

Wiki defines normative intent; timestamped runtime readback establishes effective values, source, project/machine and code revision. Apply existing [runtime observation/freshness contracts](https://github.com/alexkirs/taskq/blob/main/taskq/contracts/taskq-manager.md#silent-worker), including observation time versus event time and [permission visibility limits](https://github.com/alexkirs/taskq/blob/main/taskq/contracts/taskq-manager.md#permission-observations-176). Missing/stale visibility stays unknown; do not invent a new universal expiry. Escalate a missing bound only if material to the rollout decision. A Wiki statement or preflight success alone does not prove effective machine/launch state.

## One drift procedure

Report Wiki/code/runtime disagreement in the affected issue with `taskq problem`, project identity, pinned Wiki/code versions, expected/actual behavior and redacted timestamped evidence. Hold dependent implementation/publication, preserve task ownership and the same session, and do not silently select a convenient version. PM records the technical correction within accepted scope; decisions outside delegated authority use the existing `taskq ask`/answer route. If intent changes, approve the revised spec first; then correct authorized code/settings, rerun conformance and relevant runtime checks, and review the exact new SHA. Link closure evidence or escalate unresolved drift; no automatic approvals or duplicate workers.

## Two illustrative examples

- **Configuration default:** draft a Required settings change from `[workspace] publish` default `direct` to `review`, retaining allowed values `direct`/`review` and shared-only scope from the [current contract](https://github.com/alexkirs/taskq/blob/main/taskq/contracts/taskq.md). Record applicable approval and accepted Wiki revision `W1`; implement candidate `C1`; check omitted/explicit/invalid values, local-override rejection and the project gate. Readback verifies authorized rollout. Drift: an older machine still uses the old default with no explicit setting; report its revision, correct rollout or approve a transition change before correction, then recheck. An explicit permitted `direct` override is not drift.
- **New read-only policy function:** draft a short separate Wiki page linked from Home specifying effective-mode/source output, error behavior, secret redaction and no writes/dispatch. Record applicable approval and accepted revision `W2`; implement `C2`; check default/shared modes, invalid configuration, forbidden local override and absence of writes plus the project gate. Drift: code accepts a forbidden local override; report the failing case, fix against `W2` or first approve revised requirements, then verify the corrected SHA. `W1/W2/C1/C2` are illustrative references, not actual approval or implementation evidence.

## Rollout and remaining choices

Incrementally replace duplicated normative README/contracts paragraphs with canonical Wiki links as those areas change. Preserve usable onboarding, CLI reference, concise safety instructions and generated briefs linked to the accepted specification; keep implementation details in code/contracts. No whole-document migration, database or scheduler is required.

Pending owner choices: accept the same-Wiki page structure; accept the proposed PM delegation and designate publication authority; choose migration/rollout order using existing freshness contracts, addressing only material gaps. Wiki SoT and mandatory docs-first are already selected and are not requested again.

Wiki stores no secrets/credentials and does not replace issue/label/claim/result state ([#21](https://github.com/alexkirs/taskq/issues/21)). Related pending [#185](https://github.com/alexkirs/taskq/issues/185), [#186](https://github.com/alexkirs/taskq/issues/186), [#187](https://github.com/alexkirs/taskq/issues/187) and the separate [#188 proposal](https://github.com/alexkirs/taskq/blob/main/docs/wiki-notes-proposal.md) are not completion gates or activated by this document. This candidate changes no live Wiki, code, configuration or timers.
