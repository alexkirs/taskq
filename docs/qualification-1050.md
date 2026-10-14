# Linux service qualification (#1050)

G8 Linux row of the [implementation plan](design/implementation-plan.md), run on
2026-10-14 for [#1050](https://github.com/taskqorg/taskq/issues/1050) under the
owner's answer: host `wsl`, a Full grant for Claude on a WSL test root only, one
held test task labelled `host-wsl`, update from 4.1.11 to a main dev build,
restart with a live kid, then revoke the grant and close the test task.

## Candidate

- Source: `main` at `4086feed6ff9b8dfda4ba955a411e0bfda2fd79c`, built in WSL
  with the development-loop version `4.2.0-dev.1791998010` (no `v4.2.0` release
  exists yet; `Cargo.toml` was restored after the build).
- Binary: sha256
  `f8b3e742eb3baa28d6cd9d1d9f86709c5c15ca32b7e863ddb2ef3b3fa68b6482`; it embeds
  the source SHA, and the installed release under `releases/` has the same
  digest. The build's own `selftest` passed (69 golden traces).
- This branch is based on a later `main`. The difference from `4086fee` outside
  `docs/` is empty, and `taskq.md`, the only document compiled into the binary,
  is unchanged, so the qualified binary is the binary of this branch.

## Setup

- Host: WSL 2 Ubuntu 24.04 on the Windows qualification machine, kernel
  `6.18.33.2-microsoft-standard-WSL2`, user systemd. Hostname `DESKTOP` maps to
  `win` in the repository's `taskq.json`, so the test root (a fresh clone of
  this repository in the OS temp dir, detached at the candidate) used its own
  `taskq.json` with `"hosts": {"DESKTOP": "wsl"}`, `limits` claude 1 / codex 0
  and `incident_reports: false`.
- Installed release before the test: `current` and `previous` 4.1.11. The five
  other registered WSL projects had inactive, disabled units and were not
  touched; their `projects` lines read back unchanged after the test.
- Runtime: native Claude CLI 2.1.299, logged in through claude.ai. Codex in WSL
  is a Windows shim and was not used.

## Results

All times are UTC on 2026-10-14.

| Step | Observed | Result |
|---|---|---|
| Install 4.1.11 | `releases/4.1.11/taskq service install` at 17:14:09Z: unit `taskq-taskq-wsl.service` active; `status.json` version 4.1.11; journal `taskq 4.1.11 service on wsl for taskqorg/taskq`; heartbeat [#1065](https://github.com/taskqorg/taskq/issues/1065) `version 4.1.11` at 17:14:30Z | Pass |
| Update to the candidate | candidate `service install` at 17:14:43Z: `current` 4.2.0-dev.1791998010, `previous` 4.1.11; journal `taskq 4.2.0-dev.1791998010 service on wsl`; `status.json` version matches; heartbeat at 17:15:05Z `version 4.2.0-dev.1791998010`, focus `off` kept | Pass |
| Access | `service permissions --runtime claude --mode full --scope project`: revision 5, `effective full · runtime sandbox off, uncontained`; codex stayed refused. The grant moved `previous` to the candidate (rollback floor: 4.1.11 ignores worker grants) | Pass |
| Run a task | Held test task [#1097](https://github.com/taskqorg/taskq/issues/1097) started at 17:15:39Z with focus `host-wsl`. Worker attempt 6174070383 launched at 17:16:43Z, ran `uname -sr` (`Linux 6.18.33.2-microsoft-standard-WSL2`), slept 240 s and handed in `result` at 17:20:57Z; Claude reviewer attempt 6174155803 on `wsl` closed it at 17:21:39Z | Pass |
| Restart with a live kid | `service restart` at 17:17:02Z while the worker slept: journal `restart to 4.2.0-dev.1791998010 waits for #1097`, `status.json` `restart_waits` `["#1097"]`, kid kept `running`; after the close, `restart requested` at 17:21:55Z and the service re-exec'd in place (same main pid, unit active) | Pass |
| Selftest | Installed launcher from the test root at 17:22:09Z: 69 golden traces PASS, formats PASS, live board read as alexkirs, host `wsl`, exit 0; `kids` and `restart_waits` empty | Pass |

## Earlier runs

- The first attempt qualified `main` at 2fc800c with test task
  [#1068](https://github.com/taskqorg/taskq/issues/1068). Review sent it back
  because later service changes landed before hand-in.
- A second run on `main` at 68f5821 with test task
  [#1088](https://github.com/taskqorg/taskq/issues/1088) passed every step, but
  `main` moved to `4086fee` during it, so it is superseded by the table above.
- A slip in this session's environment created a held issue in the unrelated
  `taskqorg/dashq` repository (#115); it was closed as not planned at once.

## Findings

- **PATH in the systemd unit.** The unit writes `Environment=PATH=<path>`
  unquoted. WSL's inherited PATH contains spaces (`/mnt/c/Program Files/...`),
  so systemd logs `Invalid environment assignment, ignoring: ...` for every
  piece and keeps the PATH only up to the first space. Claude was still found
  because `~/.local/bin` comes early in PATH. The unit text comes from coreq's
  systemd writer; the value needs systemd quoting. Seen again on the candidate.
- **Native Claude prerequisites are only seen at launch.** `service permissions`
  read `effective full` while the Full mode still needed the one-time native
  bypass disclaimer (`skipDangerousModePermissionPrompt`) and workspace trust.
  On 2fc800c, #1068 hit `--bg with bypassPermissions requires accepting the
  disclaimer first` and then `Workspace not trusted`; each failure posted
  `gone` with the native message and the task moved to `ask`, with no launch
  loop. The owner-approved runs accepted both temporarily for this root, the
  same way as the earlier Windows Full run, and reverted them afterwards.
- **Login loss.** In the first attempt WSL Claude's stored tokens were emptied
  two seconds after a launch that failed on workspace trust; the cause is
  unknown. TaskQ reported it correctly (`Login expired`). It did not recur.
- **`p0` passes focus on a test host.** With focus `host-wsl`, the first
  attempt's test service claimed a real `p0` task, lost the claim to `win` and
  launched nothing. This is the specified focus rule, but a test host on the
  shared board needs focus `off` or no `p0` work in the queue between its own
  steps. The candidate run checked that no `p0` task was open first.
- **Admission while a restart waits.** The service admitted the reviewer while
  the restart still waited for the worker, so the re-exec waited for both roles
  (17:17:05Z to 17:21:55Z on the candidate). A continuous queue on a Unix host
  with Claude or Codex app kids could therefore keep extending a pending
  restart.

## Cleanup

Read back after the candidate run: worker policy revision 6 with no grants;
test unit removed, no unit loaded and no test service process; `projects` equal
to the backup; `current` and `previous` 4.1.11 (the dev releases stay under
`releases/`); Claude user settings equal to their backup, no trust entry for the
test root, login kept. #1097 is closed by its reviewer, and #1068 and #1088 were
closed the same way. Heartbeat #1065 is no longer updated.

## Scope

This qualifies the installed Linux service lifecycle (install, update from
4.1.11, deferred restart with a live Claude kid, selftest) and one Claude
research worker and reviewer chain under an explicit Full grant on WSL. It does
not qualify Linux Claude with a restricted or workspace preset, a code task
with a commit and PR, a non-WSL Linux host, or the final `v4.2.0` release
update path through GitHub.
