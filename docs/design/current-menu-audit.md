# Current menu inventory

Historical 4.1.8 baseline. Latest review: [4.1.9 audit](release-4.1.9-ui-audit.md); preserve its additional capabilities.

Review date: 2026-10-13. Installed release pointer: 4.1.8.
Source inspected: `src/tray.html`, `src/tray.rs`, `src/operator.rs`, checkout `cfae06708776d252d52f31510afed6b47918cee6`.
Installed binary rendered the repository's isolated tray fixtures; this was not a live production-board audit. Binary and checkout differ: 4.1.8 shows service incidents absent from the inspected HTML. Keep that feature.

## Preserve and relocate

| Existing capability | Final destination | Required change |
|---|---|---|
| macOS Icon/Summary | Fixed top strip | Accepted square; Windows qualification remains open |
| Font 14/16/18/20 | Settings / Appearance | Apply throughout; bounded window with scrolling |
| Ask and failed groups | Attention | Direct task/question access; keep complete list |
| Project counters | Project rows | Symbols, nonzero only; unknown distinct |
| All registered projects | Pinned/recent + searchable All | Navigation does not alter observation |
| Task issue links | Flowing task rows | Remove fixed metadata columns; preserve full title |
| Board link below tasks | Project header | Direct icon link |
| Scope and original request | Work mode | Current / understood / original; one shared editor |
| Per-project pause/resume | Header controls and project | Preserve explicit targets and saved focus |
| Global pause/resume review | Fixed Controls | Preserve mixed and observation-only behavior |
| Labels when saved focus is missing | Work mode recovery | Never silently resume all |
| Per-project/runtime limits | Work mode / Limits | Preserve Codex, Claude and codex-app, not just two hardcoded fields |
| Load gate, CPU/memory thresholds | Work mode / Load guard | Preserve off/open/held/unknown, samples and saved-vs-applied readback |
| Cadence: four series, periods and custom range | Settings / Activity history | Preserve equal scale, timezone, incomplete history and current-failed independence |
| Agent versions, accounts, login/install status | Settings / Agents | Host-level read-only with probe age |
| AI input and newest request | Menu composer + AI workspace | Replace newest-only display with conversation IDs |
| Last 20 distinct command texts and Clear | Composer History & commands | Keep fill-only behavior; separate conversation history |
| AI clarification chain | Conversation | Continue any selected conversation, not only newest clarify |
| Proposal/revision confirmation/readback | Conversation operation card | Preserve concurrency checks; add partial receipts and recovery |
| AI retry by editing original text | Failed/interrupted request | Reconcile uncertain writes before retry |
| Add folder, classify, setup with AI, install review | Projects + / onboarding conversation | Preserve all four folder cases and exact effects |
| Update available/progress/readback | Settings / About; attention on failure | Preserve version verification and rollback status |
| Log/folder/restart/stop/uninstall | Settings / Services / target | Keep nested uninstall confirmation and per-project ownership |
| Installed service incidents | Service details; attention if actionable | Preserve; source/render mismatch requires adapter audit |
| Version, host and mixed versions | About and scoped headers | Do not hide unknown or version differences |

## Confirmed source gaps

- AI `newest()` exposes one request. Text history is not a conversation/session browser.
- The bridge maps task rows to number/title/state; execution host, runtime, attempt and session permalink are not passed through there.
- The URL dispatcher permits HTTP(S), not Codex deep links. A verified allowlist and recorded-session routing are needed.
- Home calls `ai()` before attention; answer/readback content competes with project access.
- Task rows use fixed metadata columns and ellipsize at 14/16 px. Board link follows all task groups.
- `counter()` renders zeros; count markup lacks persistent symbols.
- No attachment pipeline, detach/reattach window ownership or durable conversation browser was found in these source paths.
- Operator input contains status summaries, not arbitrary project files/tools. Allowed operations exclude general research, dependency-plan creation, p0 assignment and capacity edits through AI.
- Sending targets the first fresh registered service. No registered/running service means setup through AI is unavailable; empty-install onboarding needs a supported bootstrap design.

## Evidence limits

Inspected installed dark home and light project snapshots at 16 px. The snapshot path expands some history/agent sections, so snapshot height is not ordinary opening height.
No production mutation, model change, deployment or agent launch was performed. Windows, live session links, AI execution and service action effects were not qualified.
