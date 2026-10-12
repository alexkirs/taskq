# Fencing check: host mac

Evidence for [#811](https://github.com/alexkirs/taskq/issues/811), cited by
[#801](https://github.com/alexkirs/taskq/issues/801). Taken 2026-10-13 06:55 host time (UTC+7),
read-only: `ps`, process working directories and `TASKQ_*` environment names, session file
timestamps, `gh auth status`, `git config`, keychain item names. No process was stopped, no
token revoked, no login changed, no provider API called. No secret values or file contents
are recorded. Historical means started before 2026-10-12.

## A1. Codex and Claude processes and sessions

| Process / session | Started | Project | Task | Verdict |
|---|---|---|---|---|
| taskq service 4.1.0 | 10-13 02:37 | taskq | — | fenced: current release; children are tray and the two workers below |
| Claude worker, `TASKQ_ATTEMPT` 6134799277 | 10-13 06:47 | taskq | #809 | fenced: current attempt |
| Claude worker (this check), `TASKQ_ATTEMPT` 6134849193 | 10-13 06:52 | taskq | #811 | fenced: current attempt |
| Claude desktop, two Claude Code sessions | 10-12 15:42, 16:42 | taskq | none (`TASKQ_*` unset) | live: owner sessions, not historical |
| Claude desktop, scratch session | 10-13 06:41 | scratch workspace | none | live: owner session, not historical |
| ChatGPT/Codex desktop app-server and its tools | 10-12 23:01; 10-13 01:43 | taskq and none | none | live: owner sessions, not historical |
| Codex app-server daemon (codex-cli 0.162.1) and its managed server | **10-03 11:54**, **10-10 03:23** | home directory | none | live: historical, idle (no child process) |
| Codex thread started **10-11 13:42**, resumed in the desktop app 10-13 03:05 | 10-11 | agent-context (reads taskq docs) | none | live: historical session, last write 10-13 06:52; runs `gh pr merge` on the agent-context repo |
| Claude in Chrome native host | 10-07 22:13 | — | — | live: bridge only, no session |

Not found on mac: any process with `TASKQ_TASK` other than #809 and #811; any Claude session file
created before 10-12 and written after it; any process or session tied to #584–#591; `WIN_CDX3`
(a win identity, see [#812](https://github.com/alexkirs/taskq/issues/812)).

## A2. Git and provider credentials

| Credential | State | Account type | Scope | Verdict |
|---|---|---|---|---|
| GitHub CLI login (keyring) | present | user `alexkirs`, owner | scopes `repo`, `workflow`, `delete_repo`, `gist`, `project`, `read:org`; admin and push on taskq | live |
| Git remote `origin` | HTTPS `github.com/alexkirs/taskq` | — | push via credential helper | live |
| Git credential helpers | `osxkeychain` (system), `store` (user); the store file is present and empty | — | GitHub HTTPS | live (keychain) |
| SSH agent identities | none loaded; 6 private key files present | — | unknown per key | unknown |
| taskq deploy keys | absent (0) | — | — | fenced |
| OpenAI / ImageGen (Codex login) | present | ChatGPT account login, no API key | Codex, ImageGen through ChatGPT | live |
| Claude Code login | present | claude.ai subscription | Claude Code | live |
| Meshy | keychain item `csgo.meshy.api` present | API key | loaded by a bounded MCP server of an unrelated project in the running Codex app; tool `advance` spends credits after approval | live |
| Google (NotebookLM) | Google Drive desktop items present; no `gcloud`; browser login not inspected | consumer Google account | Drive; NotebookLM unknown | unknown |
| Provider keys in shell environment (OpenAI, Meshy, Google, Gemini, Anthropic) | absent | — | — | fenced |

## A3. Verdict

Host mac is **not fenced**. No historical taskq writer (PM, supervisor or worker with a taskq
attempt) is alive: the only taskq workers run current attempts. Still live or unknown:

1. Historical Codex daemon (since 10-03 and 10-10), idle.
2. Historical Codex thread from 10-11 in agent-context, resumed and writing today.
3. Owner GitHub token with `repo`, `workflow`, `delete_repo`; admin on taskq.
4. OpenAI/ImageGen ChatGPT login and the Meshy key reachable from the running Codex app.
5. Unknown: SSH key files' GitHub registration; Google NotebookLM browser login.

These are owner sessions and credentials, not taskq roles. Any stop, revoke or re-login is an
owner decision asked in #811.
