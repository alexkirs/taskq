# Proposal: Wiki specification and drift process

Status: draft for owner review in [#189](https://github.com/alexkirs/taskq/issues/189). This file does not activate governance, approve a specification, or change runtime behavior. Publication of a proposal is not acceptance. The mandatory process below takes effect only after the owner accepts its governance choices.

## Canonical location and approval

Proposed canonical home: the [published TaskQ Wiki](https://github.com/alexkirs/taskq/wiki), with its existing three pages:

- **[Home](https://github.com/alexkirs/taskq/wiki/Home):** a short docs-first rule, links to specifications, and the approval/drift process below.
- **[Required settings](https://github.com/alexkirs/taskq/wiki/Required-settings):** normative configuration, allowed values, defaults, precedence, and intended feature/function behavior with acceptance criteria. Use concise named anchors inside this page; no additional knowledge base.
- **[Known issues](https://github.com/alexkirs/taskq/wiki/Known-issues):** short symptom/action bullets for unresolved drift, linking the affected specification and issue rather than duplicating either.

Read baseline: Wiki Git revision `6bcc8868cd2be0c635f17d180b1ff07dc554eaeb`; Home currently links the other two pages, Required settings lists worker prerequisites, and Known issues uses symptom/action bullets. Adding feature specifications to Required settings is a proposed placement, not an existing convention.

The project owner must designate who may approve normative Wiki changes and who may publish them. No approver or delegation is established by this proposal; PM review alone is not specification approval unless the owner explicitly grants that authority. Until designation and acceptance are recorded, normative changes remain proposed.

## Mandatory docs-first sequence

1. **Draft:** before coding a new feature/function, describe intended behavior, inputs/outputs, configuration and allowed values, exceptions, non-goals, and executable acceptance criteria in Wiki, visibly marked **proposed**. Link the task and affected project. A repository candidate such as this file is a review artifact, not a substitute for the required Wiki specification.
2. **Accepted:** obtain explicit approval from the designated approver. Record the approval permalink and exact Wiki Git commit/page anchor in the TaskQ issue or PR. Mark the specification **accepted; not yet implemented**, publish it, and pin the resulting accepted Wiki revision. Approval must identify the normative text/revision it covers; editorial status changes must not change that text.
3. **Implemented:** only then implement against the accepted revision. Changed requirements return to draft and receive recorded acceptance of a new revision before dependent implementation; never silently edit the target.
4. **Verified:** submit the exact implementation SHA, accepted Wiki revision, approval link, acceptance-command results, and remaining limitations through the existing issue/PR and `taskq result` workflow. Review checks conformance against that pinned revision. Mark **implemented** only with a linked reviewed implementation and conformance evidence. Machine rollout, when required, needs separate fresh runtime evidence.

These labels describe specification maturity, not queue states. An accepted specification may legitimately precede code. Publishing either a draft or an accepted specification does not prove implementation or machine application.

Minimal gates: before implementation, review the Wiki revision and approval link; before result/review, run the specification's focused acceptance checks and the project's required gate; before acceptance/publication, compare the candidate SHA with that same specification and resolve or explicitly escalate drift. Use existing issue comments, PR review and [review publication](https://github.com/alexkirs/taskq/blob/main/README.md#publication-before-or-after-review), not a new database or scheduler. `taskq result` submits a candidate; it does not itself grant acceptance or publish it.

## Normative configuration and actual state

Wiki defines configuration rules and allowed values. Readback proves what a particular machine actually applies: capture project identity, machine identifier, time, code revision, effective values, their source/precedence, and observation method. Redact secrets and credentials; never store them in Wiki or public evidence.

Use supported readbacks, not a configuration setter as a probe. Existing worker/tick profile output reports effective limits and their sources; [contracts](https://github.com/alexkirs/taskq/blob/main/taskq/contracts/taskq.md) describe precedence. [Local preflight and runtime observation](https://github.com/alexkirs/taskq/blob/main/taskq/contracts/taskq-manager.md) delimit what their evidence proves. Preflight success does not prove effective launch policy, and a Wiki statement does not prove any machine's state. Missing or stale evidence remains **unknown**; the owner must define a freshness requirement for the affected operation.

## Drift: report, decide, correct, verify

- **Report:** in the affected TaskQ issue, use `taskq problem --task <N> --text "<redacted discrepancy>"` and attach exact Wiki/code revisions, project identity, expected versus actual behavior, and timestamped runtime evidence where relevant. Name the task's responsible owner; add a concise Known issues symptom/action link if accepted for publication.
- **Hold:** do not silently choose Wiki, code, or local settings for convenience. Pause dependent implementation/publication; preserve existing claims and queue ownership. Unresolved conformance or unseen runtime state is **unknown**, not approved or verified.
- **Decide:** the designated approver records whether implementation/runtime must conform to the accepted spec or requirements must change. If this is a product decision or requires unavailable authority, use the existing `taskq ask`/answer path and retain the same task/session.
- **Correct in order:** accept a revised specification first if intent changes; otherwise retain the pinned accepted spec. Then correct code/configuration within authorized scope, rerun conformance checks, obtain fresh readback for affected machines, and submit the exact candidate for review. No automatic approvals or duplicate workers. Close the drift entry only after linked evidence proves the correction; otherwise escalate to the responsible owner.

Wiki does not replace issue bodies, `q-*` labels, dependencies, claims, questions, result SHAs or board reconciliation. [#21](https://github.com/alexkirs/taskq/issues/21) preserves queue labels as state SoT; Wiki owns accepted normative intent, not live task state.

## Worked example 1: configuration value

Illustrative future change, not approval or a settings edit: make `[workspace] publish = "review"` the default. The [current contract](https://github.com/alexkirs/taskq/blob/main/taskq/contracts/taskq.md) says the default is `direct`, values are only `direct`/`review`, and the setting is shared-only.

1. **Draft:** Required settings describes the proposed default, unchanged allowed values/shared-only rule, upgrade behavior for omitted settings, and acceptance checks for omitted, explicit `direct`, explicit `review`, invalid values and forbidden local override. No code yet.
2. **Approved spec/version:** the designated approver accepts that text; the issue records the approval permalink and accepted Wiki commit `W-config`/anchor (symbolic until real evidence exists).
3. **Implementation:** a worker changes the default and tests in an isolated task branch; result names candidate `C-config`, `W-config`, focused check results and `python3 -m unittest discover -s tests`. Review compares all specified cases before publication.
4. **Conformance:** after authorized rollout, timestamped redacted readback records each affected project's effective mode, source, machine and code revision. Explicit shared `direct` remains valid if the accepted spec retains that override; it is not drift merely because the default changed.
5. **Drift case:** a machine with an omitted setting still applies `direct` on an older code revision. Report that revision and evidence against `W-config`; do not declare rollout complete. The owner decides upgrade or a revised transition requirement. Correct the authorized deployment or first accept the revised spec, then read back again. An unseen machine stays unknown.

## Worked example 2: new function

Illustrative new CLI function, not an implementation request: a read-only effective-publication-policy command.

1. **Draft:** Required settings specifies its proposed name, JSON output (effective mode, default/shared source, observation time and code revision), no writes/no dispatch, secret redaction, and exit behavior for invalid configuration. Acceptance checks cover default/shared modes, rejected local override, invalid input and absence of writes.
2. **Approved spec/version:** record explicit approval and accepted Wiki commit `W-function`/anchor in its issue before code. These symbolic references are placeholders, not acceptance evidence.
3. **Implementation:** implement only the accepted function; submit candidate `C-function`, the pinned specification and runnable checks plus the project gate through the existing result/review flow.
4. **Conformance:** reviewer compares JSON/error behavior and no-write checks with `W-function`. A fresh local invocation can prove observed configuration on that project/machine; it cannot prove a worker launched with that policy or every machine updated.
5. **Drift case:** code returns `review` from a forbidden local override while the accepted spec requires rejection. Report Wiki/code versions and a redacted reproducible check; block acceptance. Fix code against `W-function`, or obtain acceptance of a changed precedence rule before implementing it. Recheck the exact corrected SHA; missing launch visibility remains unknown.

## Migration and owner decisions

After acceptance, replace duplicated normative README/contract paragraphs incrementally with canonical Wiki links when the affected area changes. Preserve working quickstart commands, CLI reference, generated-brief bootstrap and concise safety instructions; point briefs to the accepted Wiki revision and keep implementation-specific details in code/contracts. Review links and onboarding before removing duplicates. No immediate whole-document migration is required.

Owner decisions still required:

- Accept Wiki as normative SoT and the mandatory docs-first sequence; designate the normative approver, publisher and any delegation, with explicit approval evidence.
- Accept the proposed three-page placement, especially feature/function specifications within Required settings, and the maturity/revision notation.
- Choose rollout/freshness criteria and handling of urgent corrections to existing behavior: this proposal grants no exception to docs-first for new features/functions and no emergency authority.

Related pending work: [#188](https://github.com/alexkirs/taskq/issues/188) owns short observed/proposed notes; [#187](https://github.com/alexkirs/taskq/issues/187) owns unified briefs; [#185](https://github.com/alexkirs/taskq/issues/185) and [#186](https://github.com/alexkirs/taskq/issues/186) own proposed PM modes. They are neither accepted by this proposal nor prerequisites to reviewing it. Permission/bootstrap evidence remains bounded by [#176](https://github.com/alexkirs/taskq/issues/176) and [#177](https://github.com/alexkirs/taskq/issues/177). No live Wiki, code, timer or configuration change is part of this candidate.
