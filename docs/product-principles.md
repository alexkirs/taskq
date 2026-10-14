# TaskQ product principles

TaskQ coordinates agents over the owner's existing task board and computers.
Describe work, review the interpretation, start it, answer clear questions, inspect the result.

Accepted product direction under [taskq.md](../taskq.md#product). Runtime rules remain authoritative until matching implementation and verification ship.
Inherit [specq Core](https://github.com/taskqorg/specq/blob/main/caveman/rules/core.md), [documentation](https://github.com/taskqorg/specq/blob/main/caveman/rules/documentation.md), [change communication](https://github.com/taskqorg/specq/blob/main/caveman/rules/changes.md) and [design workflow](https://github.com/taskqorg/specq/blob/main/design/workflow.md). Do not duplicate their general writing rules here.

## Principles

- **P1 Board-owned work.** Tasks, questions, decisions and results remain readable on the connected board. GitHub and GitLab first; additional boards through adapters. Changing the manager or closing TaskQ's UI must not require copying tasks into another proprietary queue. Board migration is separate; no automatic cross-provider migration promise. R1 owns task state.
- **P2 Replaceable agents and clients.** Any capable agent may act as PM through the documented interface. Execution requires a qualified runtime adapter. Keep board identity and evidence independent of a provider's chat history. Session continuation depends on that runtime's capabilities; replacing a runtime never migrates a live attempt.
- **P3 Project-governed work.** Project specifications select practices, workflow, acceptance and publication constraints. Shared rules supply defaults; project files link them and record only local decisions. Conflicts become explained owner choices. Project text cannot grant host permissions.
- **P4 Useful defaults.** Ship coherent task, question, result and change-writing defaults. Normal work needs no template editor. Show effective behavior and its source; allow deliberate overrides within the contract. Existing settings never change silently. Full access and automatic merge are distinct explicit choices, not implications of convenient defaults.
- **P5 Intent-led control.** Typed or dictated requests become inspectable parameters, task sets or plans. Review the short meaning, exclusions and effects; then apply and read back. Edit supported fields/chips directly or revise through AI. Both routes use the same operation contract. Unknown meaning requires clarification, never a broader guessed scope. Speech capture availability is explicit.
- **P6 AI for reasoning, service for execution.** Use available agents to explain, classify, plan, diagnose and propose settings. The service validates authority, identity, dependencies, capacity and effects. Avoid model calls for deterministic polling or known operations. AI failure leaves confirmed work and current settings intact.
- **P7 Agent-friendly operations.** Agents call documented commands instead of recreating board, process or retry logic. Human output stays concise; machine consumers get stable structured results, identities and errors. Communication carries task/attempt context and reaches the right recipient once; chat never becomes another queue or an owner-answer bypass. Shared cross-client receipts are an implementation gate.
- **P8 Explicit host resources and access.** Inspect available runtimes, occupied slots, limits, load and permissions per named computer. Explain why work waits. Configure differences without editing unrelated hosts. Keep Access, Delivery and capacity separate. [Auto admission](design/dynamic-resources.md) is the new-host target, with optional strict caps and visible waiting reasons. Shared host admission remains a required implementation gap, not a property of current per-project limits.
- **P9 Reliable sessions and results.** Record the exact task, attempt, runtime, host and session link. Launch, resume, cancel and recover through the service; preserve work and show uncertain outcomes. Reopen the same supported session. A turn ending, a process exiting or a link opening is not proof of task completion. R2/R5/R11 apply.
- **P10 Complete operator path.** Menu and workspace support planning, attachments, questions, configuration, results and resource inspection without an external agent desktop. The service still needs an available runtime. UI close never means cancel; app-dependent runtimes remain visibly app-dependent. Headless CLI runtimes are the route for desktop-independent work.
- **P11 Small composable parts.** Reuse board/runtime adapters, commands, existing dependency scheduling and proven resource tools. Add modules for demonstrated use-case gaps, not an unrestricted plugin engine. Keep advanced detail available without making ordinary work configure every stage.

## Default artifact contract

Two equal entry paths: [menu/workspace](design/menu-product.md) and [an existing agent session](design/agent-entry.md). Both use the same board, identities, reports and operation rules; neither owns a separate queue.

| Artifact | TaskQ requirement | Authority |
|---|---|---|
| Task | Outcome title; Goal, Acceptance, optional Scope; explicit dependencies; no duplicate work | §1; project spec |
| Question | Checked context, decision/reason, source-numbered choices, recommendation, relevant evidence | §2 Ask card; [Q1](design/plans-and-decisions.md#q1-answer-where-the-question-appears) |
| Answer | Same option number/text in board, CLI and UI; custom text stays distinct; exact question identity | §2, R6/R10; proposed Q2 receipts |
| Progress/comment | New result, blocker or decision; no transcript dumps or repetitive activity noise | R7 and §5 |
| Visual result | Embed relevant image/video evidence in the ticket/result when the board supports it; otherwise link a durable accessible artifact with a preview when possible | R5; project acceptance; never only a local path or private chat |
| Commit/PR/merge | Clear change and effect, actual checks/limits, task traceability; project convention plus shared change profile | §7; [Delivery proposal](design/agent-behavior.md) |
| Completion | Verified candidate, acceptance evidence and publication outcome; preserve incomplete/unknown facts | §7, R12 |

Evidence uploads use an authorized destination and its visibility. Screenshots prove appearance; motion requires video or temporal checks. Artifacts remain available to reviewers outside TaskQ's UI.

## Feature selection

Defaults, alternatives and overrides follow [shared AI-assisted controls](https://github.com/taskqorg/specq/blob/main/design/workflow.md#ai-assisted-controls). Defaults are visible, versioned and scoped. [Decision register](design/decision-register.md) separates proposed defaults from approved settings.

Before adopting a feature, identify the owner use case, board-visible outcome, existing primitive, authority, failure behavior and shortest UI path.
Prefer a reviewed AI proposal plus an existing command. Add a new engine only when that cannot satisfy the case.
Evaluate features through [use cases](use-cases/README.md) and the [comparison/recommendations](design/comparative-workflow-review.md#fit-to-taskq-principles).

## Minimum composition

- Board adapter: native task identity, readable log, authorization, attachments/links and verified writes. Expose unsupported capabilities.
- Runtime adapter: readiness, launch/resume, session identity/link, supported questions, cancellation and result delivery. Expose app/host dependencies.
- Service/commands: one validation and execution path for UI and agents. Extend existing `status`, `inbox`, `steer`, `answer`, `result` and service operations before adding another API.
- Agent-facing output target: versioned result/error shape, exact targets, freshness, operation identity and per-item outcomes. A timeout is unknown, not permission to repeat a write. Current human-readable CLI is not automatically this contract.
- Agent communication: dependency results and board/inbox messages first; explicit sender, recipient, task/attempt and delivery outcome. Add direct messaging only for a case those primitives cannot serve. PM still cannot answer owner-only questions.
- UI/AI: clients of those operations. AI supplies proposals; controls expose the same fields; runtime authority does not live in a prompt or window.

## Acceptance scenarios

- Continue the same board task through another capable PM; no task re-entry or private-state export.
- Close external agent desktops while using a headless runtime; menu planning and numbered answers remain available. App-dependent runtimes show their limitation.
- Request a scope/limit change by dictation or text; inspect exact parameters, edit one field, confirm once, see applied or partial state.
- Give two computers different limits/access; verify the actual execution host and waiting reason. Cross-project totals need a shared-budget implementation.
- Answer the same question through a menu button, CLI or board comment; preserve the source number and decision. Stale/concurrent answers need Q2 proof.
- Review visual work from the ticket; inspect evidence without opening the original chat. Verify configured publication separately.

This document defines product intent. Missing adapters, machine-output contracts, host budgets, menu planning capabilities and evidence-upload paths remain implementation work; no new runtime, permission or publication default is enabled here.
