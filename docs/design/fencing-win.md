# Windows historical-writer fencing (#812)

Read-only observations on 2026-10-13, local time UTC+07:00. Evidence for #801.
Contract: [taskq.md](../../taskq.md). Owner decision recorded in
[the #812 answer](https://github.com/alexkirs/taskq/issues/812#issuecomment-6134963790).

## A1: Processes and sessions

| Process | PID | Parent PID | Start (2026-10-13, UTC+07:00) | Project | Task / session classification | Verdict |
|---|---:|---:|---|---|---|---|
| Codex | 18788 | 16668 | 05:01:13.7656020 | Not observed | Owner session | Fenced by owner decision |
| Codex | 21068 | 16668 | 05:01:29.5381160 | Not observed | Owner session | Fenced by owner decision |
| Claude | 25496 | 4544 | 05:01:50.3775080 | Not observed | Owner session | Fenced by owner decision |
| Claude | 27272 | 25496 | 05:01:50.6157990 | Not observed | Owner session child | Fenced by owner decision |
| Claude | 24856 | 25496 | 05:01:50.8406330 | Not observed | Owner session child | Fenced by owner decision |
| Claude | 4348 | 25496 | 05:01:50.8707030 | Not observed | Owner session child | Fenced by owner decision |
| Claude | 4292 | 25496 | 05:01:51.0615190 | Not observed | Owner session child | Fenced by owner decision |
| Claude | 26900 | 25496 | 05:01:51.2059120 | Not observed | Owner session child | Fenced by owner decision |
| Claude | 26592 | 25496 | 05:01:53.9698670 | Not observed | Owner session child | Fenced by owner decision |
| Claude | 22544 | 25496 | 05:01:53.9804270 | Not observed | Owner session child | Fenced by owner decision |
| Claude | 24384 | 25496 | 05:01:54.0082080 | Not observed | Owner session child | Fenced by owner decision |
| Claude | 21228 | 25496 | 05:01:56.0724880 | Not observed | Owner session child | Fenced by owner decision |
| Claude | 12052 | 25496 | 05:01:56.0729690 | Not observed | Owner session child | Fenced by owner decision |
| Claude | 28024 | 25496 | 06:29:05.6808200 | Not observed | Owner session child | Fenced by owner decision |
| Codex | 22116 | 18300 | 07:00:39.9301760 | taskq | Current attempt: #597, 6134944729 | Fenced by owner decision |
| Codex | 1908 | 2456 | 07:02:38.7126380 | taskq | Current attempt: #812, 6134859430 | Fenced by owner decision |

Source: `Get-CimInstance Win32_Process`, retaining only sanitized metadata.
Current attempts were mapped through ancestry to `taskq service status` children.
This worker's environment confirms `TASKQ_TASK=812`, `TASKQ_ATTEMPT=6134859430`.
Other processes' environments were not inspected; their classification is the owner's
attestation, not a claim that their TASKQ_ATTEMPT variable was read. Session identifiers
and other projects were not independently observed. All observed process starts are
after the 2026-10-12 cutoff; a process start alone does not prove session age.

Service snapshot: host win, version 4.1.0, service PID 8064, focus all, error null,
status timestamp 1791849760. Children: Codex #597 / attempt 6134944729 / PID 20464,
and Codex #812 / attempt 6134859430 / PID 22572. Child PIDs are launch ancestors,
not the native Codex PIDs above. Initial inspection also showed service launcher PID
9396, started 05:01:05.1060110, and service PID 8064, started 06:02:57.3605370.

## A2: Git and provider credentials

| Item | Presence / login observed | Account type | Scope / limitation | Verdict |
|---|---|---|---|---|
| GitHub (`gh auth status`) | Present; active login | Keyring-backed account | gist, read:org, repo, workflow; repository push not exercised during inspection | Fenced by owner decision |
| Git remote | Fetch and push configured | HTTPS | origin: https://github.com/alexkirs/taskq.git | Fenced by owner decision |
| Git credential helper | Present | Git Credential Manager (`manager`) | Stored credential presence and effective grants not inspected | Fenced by owner decision |
| SSH / deploy keys | SSH private-key files present; deploy-key authorization unknown | SSH key; account type unknown | No key contents or authorization inspected | Fenced by owner decision |
| Git environment credentials | TASKQ_TOKEN, GITHUB_TOKEN, GH_TOKEN absent | Not applicable in worker environment | Absence applies only to this worker's environment | Fenced by owner decision |
| OpenAI / ImageGen | OPENAI_API_KEY absent; provider login unknown | Unknown | Browser/app login and grants not inspected | Fenced by owner decision |
| Google | GOOGLE_API_KEY and GEMINI_API_KEY absent; provider login unknown | Unknown | Browser/app login and grants not inspected | Fenced by owner decision |
| Meshy | MESHY_API_KEY absent; provider login unknown | Unknown | Browser/app login and grants not inspected | Fenced by owner decision |

Sources: sanitized `gh auth status`, `git remote -v`,
`git config --get-all credential.helper`, SSH filename presence check and named
worker-environment presence checks. No credential values, file contents, account names,
cookies or private filesystem paths are included. Provider APIs were not called.
Absence of an environment API key does not establish a logged-out provider session.

## A3: Verdict

**Host win fenced by owner decision dated 2026-10-13.** The owner states every running
Codex and Claude process is an owner session or current taskq attempt; no historical
video writer (including WIN_CDX3 and tasks #584-#591) is alive or needs stopping.
This is policy acceptance of the recorded observations and unknown credential details,
not proof of revoked Git/provider rights or a technical credential isolation test.
No process termination, token revocation, login/logout, provider API call or configuration
edit was performed. No audit task or action request remains.
