# Historical README phrase evidence

The #1007 page replaces the slogan table with illustrative conversations. The responses below remain historical evidence for the earlier phrases only; they do not qualify the new dialogue or a completed user journey. UC-14 now describes evidence review and publication boundaries, not the dropped automatic “Review #12.” promise.

Owner answer on #925: check both runtimes without writes; correct proposed commands count as phrase evidence.
These are fresh PM interpretation checks with the full contract supplied, not live board or installation qualification.
Each candidate phrase was presented once, as an independent scenario, in one new session per runtime.
Claude ran with tools disabled and strict MCP configuration; Codex ran read-only and used no tools.
No model selection or saved permission configuration was changed. No task/service actions were executed.

## Kept rows

| UC | Exact phrase | Accepted proposal in both sessions | Shipped coverage |
|---|---|---|---|
| UC-02 | `File a task: fix the login redirect.` | `taskq add` with agreed goal and acceptance; no admission change | §5 add; golden `new task is ready`; service `waiting_and_filters_hold_admission` |
| UC-04 | `File it as p0.` | New task via `taskq add ... --priority 0`; clarify an existing task | service `p0_passes_any_focus_but_not_pause_or_limits` |
| UC-07 | `Split this plan into tasks.` | Agreed tasks via `add --hold`, successor `--deps`; explicit `start` later | golden `open dependency waits`, held/start traces |
| UC-09 | `Answer #12 with 2.` | `taskq answer 12.2` after reading the current card and relaying the owner's pick | golden `hand-typed CLI form answers`; service `answer_resumes_the_same_session_and_requeue_stops_the_child` |
| UC-13 | `Pause the service.` | `taskq service pause`, then readback; leave running attempts alone | §5 pause; service `p0_passes_any_focus_but_not_pause_or_limits` |
| UC-02 | `Use Codex for the visuals.` | New agreed visual task via `add ... --runtime codex`; no global policy change | §5 add/runtime; service `waiting_and_filters_hold_admission` |

## Dropped rows

- UC-14 `Review #12.`: both sessions identify R3's PM/reviewer boundary; no review command. Inspecting or waiting for the service is not the plan's promised accept/reject action.
- UC-12 `Install taskq on this host too.`: both require exact setup approval and host qualification; proposed service commands alone do not prove a second host works the board.

## Review limits

- Both setup variants were also checked: inspect and preserve existing state, confirm concrete external effects, verify queue read/write, then PM handoff. Neither proposes an unqualified shell installer.
- Accepted evidence is the proposed CLI mapping. Responses are not accepted wholesale: Codex incorrectly says p0 passes focus off; source and `p0_passes_any_focus_but_not_pause_or_limits` show pause blocks p0 too.
- Claude's setup diagnosis overstates that a failed selftest necessarily requires cutover and that successful selftest leaves only service installation. Those claims are absent from the README.
- An answer may choose publication or return to ready; the page promises only posting the choice. Pause stops new worker admission, not every service action. Task starts need an actual accepted spawn.
- Commands, prerequisites and limits were checked against §5, §6, §7, `src/service.rs` and golden traces. Runtime responses below are retained verbatim for review.

## Claude

Session: `1467abe5-1c8c-435c-a169-1bbe3eb49e09`. One successful turn.

# README phrase check: TaskQ PM interpretation

No tools used. Angle-bracket values are placeholders. "Observable" means what readback shows, not what PM assumes.

## 1. UC-02 "File a task: fix the login redirect."

- **Intent:** file one ready task. Do not change focus or admission.
- **Prerequisites:** run in project root with `taskq.json`. Board token available.
- **Command:**
  ```
  taskq add "Fix login redirect" --goal "Return signed-in users to their requested page." --acceptance "<agreed redirect check>" --type code
  ```
  Priority absent means 2. Optional `--op-id <id>` makes a retry safe.
- **Observable:**
  - CLI prints `#<N> ready`.
  - `taskq status` shows the row.
  - Start depends on current focus and capacity. Focus `off` admits nothing.
  - PM reports "filed", never "started", until `spawn worker` appears.

## 2. UC-04 "File it as p0."

- **Intent:** file the same agreed task with `--priority 0`.
- **Clarify if the task is already filed.** §5 has no reprioritize command. Ask the owner: re-file the task, or have the owner change the label on the board. Do not invent a command.
- **Command:**
  ```
  taskq add "Fix login redirect" --goal "Return signed-in users to their requested page." --acceptance "<agreed redirect check>" --type code --priority 0
  ```
- **Tell the owner one effect:** `p0` passes any focus except `off`. The task can start even when focus names other labels.
- **Observable:**
  - CLI prints `#<N> ready`. Task sorts first.
  - Next service pass (poke or `poll`) can claim the task. Load gate and host admission can delay the claim.
  - Report `doing` only after status shows a `spawn worker` and a `T<N> …` session link.

## 3. UC-07 "Split this plan into tasks."

- **Intent:** two held tasks. Test task depends on implement task. Same board.
- **Commands, in order** (you need the first number for `--deps`):
  ```
  taskq add "Implement login redirect" --goal "Return signed-in users to their requested page." --acceptance "<A1 redirect check>" --type code --hold
  taskq add "Test login redirect" --goal "<agreed test goal>" --acceptance "<A1 agreed test evidence>" --type code --deps <A> --hold
  ```
- **Observable:**
  - CLI prints `#<A> held` and `#<B> held`. Label is `q-held`, not `taskq`. No service admits them.
  - PM gives one reminder for this planning milestone.
- **On explicit start:** run `taskq start <A>` and `taskq start <B>`. Then A is `ready` and B is `waiting` (`blocked (#A open)`) until A closes.

## 4. UC-09 "Answer #12 with 2."

- **Intent:** relay the owner's pick. The PM does not choose.
- **Prerequisites:**
  - #12 is in `ask` in a fresh read.
  - Use the revision from that read.
  - If the PM manages several projects, name the project (R10).
- **Command:**
  ```
  taskq answer 12.2 --revision <R> --op-id <id>
  ```
- **Check option 2 text before sending.** If it starts with `close`, the answer runs §7 close, which publishes or merges. Tell the owner that before sending.
- **Observable:**
  - `#12 doing` (returns to state before `ask`), else `#12 ready`.
  - If option 2 is a close: closed, plus a publication link or SHA line.
  - If the CLI reports `stale`: re-read, show the owner the current card, and ask again. Never re-pick for the owner.

## 5. UC-14 "Review #12."

- **Authority limits:**
  - PM does no task work and gives no verdict of its own.
  - Reviewer `R12` is a service child, spawned on demand for `review`. No command spawns it.
  - Capacity, load gate and admission apply to the reviewer.
- **PM action:** read only. Run `taskq inbox 12` and `taskq status`. Show the result, any live `R12` session and CI state.
- **Clarify with options:**
  1. ★ Wait for reviewer verdict. `taskq wait --only <label>` until `closed #12`, `ask #12` or `failed #12`. Relay verdict verbatim, links intact.
  2. Owner accepts on their own review: `taskq close 12 --text "<owner's Changed/Checks/Open verdict>"`.
     - PM does not invent `A<k> ok` lines.
     - Close refuses if CI is pending or missing. Close requeues if CI failed.
  3. Owner rejects: `taskq requeue 12 --text "<owner's exact fixes>"`. This clears the result.
- **Observable:** closed with a publication link, `#12 ready`, or `ask` card relayed.

## 6. UC-13 "Pause the service."

- **Prerequisites:**
  - Run in the named project root.
  - Service installed: `taskq service list` shows the project.
- **Command:**
  ```
  taskq service pause
  ```
- **Effect:**
  - Focus is saved to `focus.paused`, then focus becomes `off`.
  - No new admission. Running children continue.
  - Heartbeats and updates continue.
  - Do not use `service stop`. That ends the service process until next logon.
- **Observable:** one-line acknowledgement. `taskq service status` shows `Focus: off` and `Paused`.
- **Resume:** `taskq service resume` restores the saved focus. Without a saved focus, resume fails and names labels. Never assume `all`.

## 7. UC-12 "Install taskq on this host too."

- **Intent:** add this host to the existing project. This is a §6 setup conversation.
- **Clarify first:**
  - Which project (explicit list, R10).
  - Exact project folder on this host. Is a clone needed?
  - Host name.
  - Token source.
  - Runtimes and limits. These are the owner's (R9). With Fixed, no limit admits none.
  - `has` capabilities.
  - Focus.
- **Inspect (read only):** folder, `taskq.json`, Git remotes, dirty work, `gh` login, `taskq agents`.
- **Gap to raise:** the claim needs a two-host test on this board before use (§3, R12). If the board is unqualified, ask before running two hosts.
- **Confirm-list items** (apply only after the owner confirms the exact list):
  1. Download and verify the release binary (see setup phrases below).
  2. Add this host's name to `hosts` in `taskq.json`. This is a repo file change: agree how it lands.
  3. `taskq service capacity --host <name> --claude <N> --codex <N>`, only on owner's values.
  4. Token placement.
  5. `<binary> service install`: OS unit, `<install>` binary and tray. On Windows, run outside MSIX apps.
  6. Incident-report disclosure if the key is on. An existing project without the key stays local.
- **Verify:** `taskq selftest`, then `taskq service status`.
- **Observable:**
  - Heartbeat issue `taskq host <name>`.
  - `Hosts:` line lists `<name> v<version>`.
  - `service list` shows the project.
  - Focus stays `off` until the owner sets it.

## 8. UC-02 "Use Codex for the visuals."

- **Intent:** set the runtime for this one task only. Do not change `codex` options, limits, model or permissions (R9).
- **Clarify:**
  - Type: `asset`, or `code` if the visuals are UI code.
  - Does it need a screen? A screen task needs `needs-screen` (and possibly macOS `codex-app`). Ask; do not assume.
  - If the owner means a standing rule for all visual tasks, say no such config key exists and ask.
- **Prerequisite check (read only):**
  - `taskq agents` shows `codex` installed and logged in.
  - Some host has `limits.codex` above 0.
- **Command:**
  ```
  taskq add "<visual title>" --goal "<agreed goal>" --acceptance "<agreed visual check>" --type asset --runtime codex
  ```
- **Observable:**
  - `#<N> ready` with label `run-codex`.
  - After spawn, Session cell links `codex://threads/<id>`, or the pages wrapper.
  - If no Codex-enabled host has focus or capacity, the task stays `ready`. Report that.

## 9. "Install taskq from https://github.com/taskqorg/taskq and set it up for this project."

- **No shell installer assumed.** The copyable install command is an accepted target, not shipped.

**Gate 1: inspect (read only).**
- Resolve project name and exact folder.
- Read files, `taskq.json`, Git roots, remotes and uncommitted count.
- Read `gh`/`glab` logins.
- Run `gh repo view --json nameWithOwner,visibility`. Do not ask for visibility already read.

**Gate 2: binary.**
- Download `taskq-<ver>-<os>-<arch>` and `SHA256SUMS` from the release.
- Verify the checksum.
- Run `gh attestation verify <file> --repo taskqorg/taskq`.
- Without `gh`, provenance is unknown. Say so and let the owner decide.
- Run nothing unverified.

**Branch on the inspection result:**
- `taskq.json` exists and `selftest` passes: only `service install` remains.
- `taskq.json` exists and `selftest` fails: old project. Use the §9 cutover, not setup.
- No `taskq.json`: propose the config:
  - `board`, `repo`, `hosts`
  - `limits` (owner's values)
  - `admission` auto (setup default)
  - `publish` `direct|pr`
  - `incident_reports`

**Gate 3: owner confirms the exact effect list.**
- Account and repository.
- `taskq.json` write.
- `taskq labels` creates the label catalog on the board.
- Token source.
- Incident disclosure before any send: destination, public, posting account, payload keys, local fallback, opt-out.
- `<binary> service install`, which installs the unit and tray.
- The agreed queue write check.

Preserve dirty work, unknown keys and ownership throughout.

**Verify, then hand off:**
- `taskq selftest`.
- `taskq status` (read).
- Agreed write check, then readback.
- Hand off to PM. Focus stays `off` until the owner sets it.

## 10. "Install taskq from https://github.com/taskqorg/taskq and create project <name> from <idea or folder>."

Same gates as phrase 9, plus:

- **Folder:** agree an exact path. Never discover one. For an idea, ask for a new path. For an existing folder, inspect it and preserve files. Not a repo means `git init` is an effect.
- **Extra confirm-list effects, in order:**
  1. `git init`.
  2. Repository creation as the named account. Default visibility is PRIVATE.
  3. `origin` remote.
  4. Publish or initial push, if agreed.
  5. `taskq.json`.
  6. Labels.
  7. `service install`.
  The exact commands are shown in the list. Re-inspect if effects changed before apply.
- **From an idea:**
  - Setup does not file work.
  - After handoff, the PM proposes a plan as held tasks (`--hold`, `--deps`).
  - Nothing starts until explicit `taskq start`.
- **Observable:** `selftest` passes in the folder. `service list` lists the project. Board has labels. Repo visibility reads back as confirmed.

## Codex

Session: `01a13751-e845-7612-bdf6-bd6040987a54`. One completed turn.

1. **UC-02 — “File a task: fix the login redirect.”**  
   Propose:
   ```text
   taskq add "Fix the login redirect" --goal "Return signed-in users to their requested page." --acceptance "A1. A redirect check confirms signed-in users return to their requested page."
   ```
   Prerequisite: the current project and board are established. This authorizes filing an eligible task, which an existing service may admit under its current policy. It authorizes no focus, capacity or admission change. Observable result: an issue with the agreed goal and acceptance, with readback reporting `#N ready`; execution is a separate observation.

2. **UC-04 — “File it as p0.”**  
   For the agreed task **not yet filed**, propose the command in result 1 with `--priority 0`. The owner explicitly authorized that priority. Observable result: the issue carries `p0`, and readback reports its state. An eligible service can admit it within capacity; `p0` passes focus even when focus is `off`. No admission settings change is needed.  
   If “it” means an already-filed issue, establish its number first. The supplied CLI has no priority-update command; do not create a duplicate or invent one.

3. **UC-07 — “Split this plan into tasks.”**  
   Propose two held tasks, using agreed acceptance placeholders:
   ```text
   taskq add "Implement the login redirect" --goal "Return signed-in users to their requested page." --acceptance "A1. <agreed implementation acceptance>" --hold
   taskq add "Test the login redirect" --goal "Verify signed-in users return to their requested page." --acceptance "A1. <agreed test result and evidence>" --deps <implementation-task-number> --hold
   ```
   Resolve those acceptance items before filing. Observable result: both tasks are `held`, with the test depending on the implementation task on the same board. After an **explicit start instruction**, propose `taskq start <implementation-task-number>` and `taskq start <test-task-number>`. The test remains `waiting` until its dependency closes.

4. **UC-09 — “Answer #12 with 2.”**  
   Propose:
   ```text
   taskq answer 12.2
   ```
   The fresh inbox and owner’s explicit pick supply the decision; the PM only relays it. Observable result: accepted answer and folded-state readback. Normally this returns to the pre-ask state, or `ready`; if option 2 starts with `close`, it invokes publication checks instead. Do not promise a particular transition without the option text.

5. **UC-14 — “Review #12.”**  
   The PM cannot act as reviewer, approve the candidate or issue a reviewer verdict. Proposed inspection commands are:
   ```text
   taskq inbox 12
   taskq status
   ```
   The service spawns a reviewer for a result in `review` without a live reviewer, subject to eligibility and launch gates. There is no `taskq review` command in this contract. Observable result: a service-recorded reviewer attempt, followed by its exact-candidate verdict; the PM relays that verdict verbatim. A missing reviewer requires diagnosing service prerequisites, not assuming reviewer authority.

6. **UC-13 — “Pause the service.”**  
   In the named current project, propose:
   ```text
   taskq service pause
   taskq service status
   ```
   Observable result: saved previous focus, effective focus `off`, and paused readback. Running attempts continue; heartbeats and updates continue. This pauses focus-based worker admission. The contract does not say it stops reviewer launches or resumes. Do not substitute `service stop`.

7. **UC-12 — “Install taskq on this host too.”**  
   This requests adding a host to the existing board, not creating another board. First establish the exact project folder, host/platform, board identity, authentication, runtime availability, compatible release and intended host policy. Preserve existing settings and ownership.  
   Present the exact installation, registration, access, labels and incident-report effects for confirmation before applying them. Once approved and the verified binary/configuration are ready, applicable commands include:
   ```text
   taskq selftest
   taskq service install
   taskq service status
   ```
   Observable completion requires approved queue read/write verification, project registration and fresh service/host readback. These commands alone do not establish all of that. No shell installer is supplied or assumed.

8. **UC-02 — “Use Codex for the visuals.”**  
   For the new agreed visual task, propose:
   ```text
   taskq add "<agreed visual-task title>" --goal "<agreed visual outcome>" --acceptance "A1. <agreed observable visual check>" --runtime codex
   ```
   Resolve the task details before filing. Observable result: the issue has `run-codex` and the agreed acceptance. This selects Codex for this task; it changes no other settings. Installed and enabled does not itself prove login, free capacity or launch eligibility. “Visuals” does not automatically imply `codex-app` or screen access.

9. **Existing project — “Install taskq from https://github.com/taskqorg/taskq and set it up for this project.”**  
   This authorizes preparing setup from the named source. Resolve the exact folder and inspect existing files, TaskQ configuration, Git roots/remotes, account and repository visibility. Preserve dirty work, unknown keys and ownership.  
   Before applying setup, obtain confirmation of the concrete effects: installation, any Git/repository/remote changes, configuration, labels, access, service installation and the required incident-report disclosure. Existing repository visibility is read, not guessed or unnecessarily asked again. No `taskq setup` or shell installer is specified. After approval, applicable commands include `taskq labels`, `taskq selftest`, `taskq service install` and `taskq service status`. Completion requires verified queue read/write and PM handoff. Starting task admission needs an explicit scope instruction.

10. **New project — “Install taskq from https://github.com/taskqorg/taskq and create project <name> from <idea or folder>.”**  
    Resolve the name, exact destination folder, source idea/folder, initial goal and acceptance. An idea alone does not authorize implementing unspecified product work. Inspect any existing content and ownership before proposing changes.  
    Confirm the exact account, repository name, visibility—new repositories default to **private**—and every concrete effect, including Git initialization, repository creation/publication, remote, configuration, labels, access, service and incident reporting. Apply only that approved list. No project-creation CLI or shell installer is supplied. Observable completion is the approved project and repository, verified queue read/write, service readback where approved, and PM handoff. Plans remain held until explicit start.

No tools used, commands run, files edited or board accessed. Commands above are contract-based proposals; shipped behavior and live prerequisites remain unverified.
