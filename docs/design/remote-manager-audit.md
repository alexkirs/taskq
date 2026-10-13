# Remote manager audit

Read-only inspection, 2026-10-14. Local TaskQ design checkout and FRA (`hermes-fra1`). No installs, queue operations, messages, timers, policy changes or restarts. This evidence is a snapshot, not current health monitoring.

| Area | Observed evidence | Assessment |
|---|---|---|
| Existing integration | `/home/hermes/.hermes/skills/taskq-headless-onboarding/SKILL.md`, 31,393 bytes; external-coordinator reference | Substantial PM guidance exists; includes legacy package and single-file procedures. Thin version-aware bootstrap needs consolidation |
| TaskQ entry | `/home/hermes/.local/bin/taskq` points to pipx Python entry; package metadata Version 0 | Not evidence of installed Rust v4; do not run current-generation commands against it |
| Known clone | `/home/hermes/projects/alexkirs/taskq`, HEAD `349ead1`, root `taskq.py`, `runtimes/hermes.py` and `hermes_pilot.py` | Older integration code exists; runtime bridge is distinct from using Hermes as an external PM |
| Rust install/service | Standard `/home/hermes/.local/share/taskq/taskq` absent; Hermes user systemd query returned no loaded `*taskq*` units | Current v4 service not established by this inspection; other locations/processes were not inventoried |
| Codex | Explicit Hermes-user executable reports `codex-cli 0.162.0` | Binary available; authentication, worker launch and delivery not exercised |
| Hermes gateway | User service active/running, NRestarts 0 at read time | Process health only; Telegram reachability and real chat delivery unverified |
| Hermes observation | Installed `tools/cronjob_tools.py` handles origin routing, queued delivery and delivery failures | Reusable scheduling/transport primitives; no TaskQ subscription or original-session continuation proved |
| Hermes media | `tools/send_message_tool.py` resolves chat/thread, authorizes targets, filters media paths and passes attachments to platform delivery | Existing transport foundation; receiving/uploading and end-to-end media path unqualified |
| Plugins | Listed codex-quota-footer, tight-langfuse, web | No separately named TaskQ plugin in this directory; not proof no integration exists elsewhere |
| Current TaskQ wait | Local `src/main.rs`, `wait`: one `.taskq/pm-cursor`, state-string comparison, bounded loop | Not independent subscriptions; a revised question in unchanged state is not detected by that comparison |
| Current service events | Local `src/service.rs`, `events`: filters by state transition | Event identity alone does not deliver card revisions while state stays unchanged |
| Auto and held plans | Existing design gaps recorded in resource/plan specs and refactoring backlog | Remain implementation gates; skill instructions cannot provide these guarantees |

## Recommendation

Keep Hermes as the conversation client. Reuse its skill loader, scheduler and media transport where qualified; keep execution in TaskQ. No new Hermes worker runtime or Hermes core patch is needed for this scenario. Existing integration is useful groundwork, but its presence does not meet [RM-A1–A9](remote-manager.md#rm7-acceptance-and-implementation-order).

Next qualification requires exact project registrations, actual Hermes tool invocation PATH, authorized chat/topic and existing watcher inventory. Inspect only those selected objects; do not export credentials or private chat history. Existing skill instructions are historical source material, not authority to run their setup/selftest recipes.
