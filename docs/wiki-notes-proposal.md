# Proposed edits to existing TaskQ Wiki pages

Review candidate only; do not publish before independent PM review. Based on
Wiki revision `6bcc8868cd2be0c635f17d180b1ff07dc554eaeb` (read 2026-10-08, Asia/Bangkok, UTC+07:00).
Keep the existing symptom/action bullets and page structure; no new Wiki section.
These observed/proposed notes can be accepted now: related pending
[#185](https://github.com/alexkirs/taskq/issues/185),
[#186](https://github.com/alexkirs/taskq/issues/186) and
[#187](https://github.com/alexkirs/taskq/issues/187) are qualification/documentation
follow-ups, not completion gates for these notes.

## Known issues

Append these bullets to [Known issues](https://github.com/alexkirs/taskq/wiki/Known-issues):

- **Claude PM feels less responsive while handling TICKs (user observation).** Prefer a dedicated interactive PM session and a separate timer/TICK sender every 5 minutes; the user observed better responsiveness, but causality is not a proven benchmark. Transport and responsiveness qualification remain pending in [#185](https://github.com/alexkirs/taskq/issues/185).
- **An active worker or empty flags look like “no permission request”.** They do not prove absence of a request ([observed in #176](https://github.com/alexkirs/taskq/issues/176)). PM reports positive permission evidence with a verified session link; after user approval in the runtime UI, resume the same session. Missing/stale visibility stays unknown; no automatic approval or duplicate worker. See the [recovery checklist](https://github.com/alexkirs/taskq/blob/main/docs/recovery-checklist.md#permission-wait).
- **Hermes/Linux evidence looks like full readiness.** Keep qualification per scenario: [#169](https://github.com/alexkirs/taskq/issues/169) records real host evidence; [#179](https://github.com/alexkirs/taskq/issues/179) records passed single-project paths but unqualified three-worker/restart scenarios. Neither proves multi-project readiness; use the existing [external-scheduler contract](https://github.com/alexkirs/taskq/blob/main/taskq/contracts/taskq-manager.md#external-scheduler-hermes-cron-systemd).

## Required settings

Append these bullets under the existing [Codex workers](https://github.com/alexkirs/taskq/wiki/Required-settings#codex-workers) heading:

- **DOT PM with a separate local TICK sender (recommended, proposed).** Prefer DOT for interactive PM and a separate local sender every 5 minutes, with one timer owner per project and explicit handoff. Supported send/receive still needs qualification in [#185](https://github.com/alexkirs/taskq/issues/185); proposed transport is not already-working transport. A delivery receipt proves neither local execution nor a completed pass; follow the [external PM guide](https://github.com/alexkirs/taskq/blob/main/docs/external-pm-tick.md).
- **Actual local-execution ACK before worker launch (confirmed narrow evidence).** Have the selected executor run `taskq preflight --json` from the main checkout and return stdout, stderr, exit code and observation time. [#181 evidence](https://github.com/alexkirs/taskq/blob/main/docs/external-pm-tick.md#read-only-evidence-for-181) confirms local command execution only; runtime capability and effective launch policy remain unknown until separately qualified. Chat activity is not an ACK ([#177](https://github.com/alexkirs/taskq/issues/177)); a preflight ACK is not launch authority or a completed TICK pass.
- **One DOT PM for several projects (proposed, unqualified).** Choose only an explicit project set, with separate directories, queues, settings, claims and bounded passes; never automatically manage every folder. Use one aggregate report preserving project identity, stages, blockers and session links, so a failed project does not obscure another. [#186](https://github.com/alexkirs/taskq/issues/186) owns qualification; [#187](https://github.com/alexkirs/taskq/issues/187) owns later canonical hints/onboarding.

Retain current worker setup bullets. These additions describe observations and
proposals; they do not change permissions, configuration, timers or contracts.
