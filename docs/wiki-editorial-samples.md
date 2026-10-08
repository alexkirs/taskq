# Wiki editorial samples (#205)

Draft for root and user review. Not published. Sources, status check and destinations: [inventory](wiki-editorial-inventory.md).

## Proposed Wiki diff (frozen)

- Base: Wiki `9ccb572258c14c0252ba74e2db6d1f288a5f12fb`.
- Diff SHA-256: `4acd386c45851d9d14c305365a7bb6c48f7d60b9d5f87145760c4ff307c459f2`. Bytes hashed: every line between the opening `diff` fence and the closing four-backtick fence, each line ending with a newline, fence lines excluded. Reproduce:

  ```
  awk '/^````diff$/{f=1;next} /^````$/{f=0} f' docs/wiki-editorial-samples.md | shasum -a 256
  ```

- Blank context lines are empty, not a single space, so `git diff --check` stays clean. `git apply` accepts them; the diff applies cleanly to the base.
- Pages: Home (five links), new Specifications index, Required-settings, Known-issues. Cleanup-schedule and Atomic-reservation-before-worker-launch are unchanged.
- Draft only: the Specifications page does not exist. The Home link to it is part of this proposal, not a working link today.
- Publication order: Specifications, then Required-settings and Known-issues, then Home.
- Before publication: confirm the Wiki is still at the base revision. If it moved, rebuild the diff and review again.
- Removed text stays readable at the linked revisions and issues.

````diff
diff --git a/Home.md b/Home.md
index 7031739..4ea75cf 100644
--- a/Home.md
+++ b/Home.md
@@ -1,48 +1,7 @@
-taskq: a task queue in GitHub or GitLab Issues, worked by Claude Code and Codex agents. Start with the [README](https://github.com/alexkirs/taskq#readme).
+taskq: a task queue in GitHub or GitLab Issues, worked by Claude Code and Codex agents.

-- [Required settings](https://github.com/alexkirs/taskq/wiki/Required-settings): what Claude and Codex workers need on each machine.
-- [Cleanup schedule](https://github.com/alexkirs/taskq/wiki/Cleanup-schedule): accepted #197 specification; integration pending.
+- [README](https://github.com/alexkirs/taskq#readme): install and first steps.
+- [Required settings](https://github.com/alexkirs/taskq/wiki/Required-settings): what workers need on each machine.
 - [Known issues](https://github.com/alexkirs/taskq/wiki/Known-issues): what `taskq doctor` cannot fix, and what to do.
-
-- [Atomic reservation before worker launch](https://github.com/alexkirs/taskq/wiki/Atomic-reservation-before-worker-launch): #186 stage 1 specification; accepted, implementation pending.
-
-## CLI update gate and conditional Pages publication (#195)
-
-Accepted specification; implementation pending. PM reviewed normative design at Wiki revision `18038edc2ae72235d7f87a139919f30df09a441a`; acceptance recorded through TaskQ answer in #195. No measured savings yet. Follow-up: [#195](https://github.com/alexkirs/taskq/issues/195), accepted timing research: [#190](https://github.com/alexkirs/taskq/issues/190#issuecomment-6043677575). Wiki SoT/docs-first is already selected; implementation proceeds against the reviewed revision through normal candidate review.
-
-1. **CLI gate:** require successful tests for the exact target SHA from the expected trusted tests workflow, retaining other mandatory checks and existing review, signing and startup rollback. Missing/unreadable/pending/failed/skipped/neutral mandatory tests refuse; handle reruns and unrelated same-name checks. Exclude only positively identified Pages checks from CLI update eligibility. Site failure remains separately visible, never reported healthy.
-2. **Pages publication:** in a separate reviewable step, replace legacy branch publication with a compatible conditional workflow using actual site inputs: docs/** and its workflow plus real generator/config/dependency inputs. Preserve current Jekyll output, URLs, HTTPS, least required Pages permissions and open.html scheme/UUID/hash/fallback behavior. Initial/manual publication builds fully; deletion/rename/multi-commit changes are covered. CLI/tests-only outside site inputs and the separate Wiki do not require rebuild. No timer or profile changes.
-
-Acceptance for each step: focused positive and negative gate controls; actual comparable before/after CLI-only and site-input runs with immutable SHAs, required-check identities and timestamps; installed exact-SHA fresh-process readback; Pages built/skipped reason and deployed content/freshness plus bridge validation. Keep PR/main tests, signature and rollback tests. A failed site run is a site blocker, distinct from CLI eligibility. Estimates of 26–99 seconds from #190 are not measured savings; its sandbox unittest failure (188 tests, 49 errors) remains a historical failure. Rollback restores the previous gate and functioning publication. Submit candidates through TaskQ/PR review; no unreviewed merge or deployment.
-
-## Versioned PM tick report contract (#191)
-
-Accepted specification; implementation pending. PM reviewed exact Wiki revision `05cf6aab1c6ed5fc9589b9e4673365cec34c58e6` for the explicitly requested report-contract scope. Scope: [#191](https://github.com/alexkirs/taskq/issues/191); user-requested report-contract work and selected Wiki SoT/docs-first. Accepted process proposal #189 does not activate global governance delegation.
-
-- **Canonical payload v1:** reuse existing tick JSON/report and deterministic Workers/Board renderer. Add report-contract version, SHA-256 of canonical specification/template and immutable source revision/provenance; every pass carries these plus project/repo/board/profile identity, observed time, outcome/actions/refusals and worker rows. Rows retain task URL/title/state, runtime/machine, session URL when available and last activity/event time; explicit unknown/unavailable replaces fabricated values. Preserve existing fields and distinguish prompt version from report-schema version.
-- **Presentation and validation:** print Board link and Workers table once per pass; task/session/commit links remain clickable. Existing formatter generates data/table, PM adds judgment. Validate required version/provenance/identity/fields/links/timestamps against existing freshness contracts; unsupported table channels use an equivalent representation retaining all fields and links. Empty workers is explicit; unresolved data gaps remain visible and block only decisions needing them.
-- **Bootstrap and safe update:** include the current payload/template automatically at bootstrap and each tick. Reuse the former prompt version, contract_news/digest and existing updater; after update, a live PM checks/applies the version at the next safe pass without interrupting work, changing claims or dispatching twice. Keep backward-compatible existing fields; unsupported version produces an actionable report blocker, never silent obsolete output or loss of current execution.
-- **Receipts:** distinguish sent, received and applied version/hash. Verify apply using an actual supported-channel acknowledgement/report containing the applied version and validated required output. A checkout digest, delivered message or mocked assertion does not prove session application. If transport lacks such evidence, expose unknown/unqualified with session/source link; do not create a new receipt DB, scheduler or transport framework.
-- **Acceptance:** focused existing tests for valid/omitted field or link, stale/unavailable source, version mismatch and duplicate/interrupted update. Separately demonstrate fresh and already-running Claude, Codex and DOT PMs receiving/applying the format and hot update through supported transport, with exact versions, timestamped receipts and rendered output. Keep current ownership/admission and queue behavior. Record runtime/transport blockers honestly; Hermes extension stays separately qualified. #185/#186 transport/multiproject and #176/#177 qualification are not implemented by this scope. Submit exact candidate SHA, pinned accepted Wiki version and conformance evidence for normal PM review before publication.
-
-## Minimal critical path for multiproject TICK (#186)
-
-Accepted specification; implementation/qualification pending. PM reviewed exact Wiki revision `6b9027b29bf43d18a24d2cc96ef98d314f396794`. P1 on #186 and its actual prerequisites #176/#177/#185. Standalone macOS TaskQ only. No project rollout selected, csgo/Hermes excluded; existing claims continue. User-approved Wiki SoT/docs-first is already selected.
-
-- **#176 permission-safe observation qualification:** qualify existing supported adapters with owned-session readbacks and focused tests for permission wait, active/terminal/unknown, stale/unreachable, request identity/dedup and same-session preservation. Empty flags never prove no approval; unknown visibility forbids blind retry/requeue. No private DB, hidden reasoning or policy widening. Real request -> notification -> owner UI decision -> same-session continuation remains live qualification, not satisfied by fixtures. Do not hold fixture implementation for unrelated all-runtime feature work; retain unknown/unqualified routes and block live actions needing unavailable evidence.
-- **#177 command bootstrap qualification:** require real harmless local-command ACK (resolved cwd/host, stdout/stderr/exit/time) before acting admission. Test negative/missing ACK and network-denial versus authorized-auth read. Reuse preflight/current adapters; failed bootstrap admits nothing. No new command transport/framework. Current macOS ACK is evidence for this executor only; no universal DOT/Linux readiness claim.
-- **#185 bounded TICK/interactive PM separation:** reuse supported transport and existing timer ownership/handoff; explicit project identity, delivery versus completed-pass receipts, duplicate coalescing and one pass per project. Test slow/failed pass plus PM interaction, restart/interrupted delivery, manual plus timer duplicate via controlled fixtures without arming another scheduler. Unsupported receive/apply or approval route stays unknown/unqualified; actual delivery and responsiveness receipts are separate live gates.
-- **#186 multiproject pass:** explicit approved project set with stable identity and resolved cwd/repo/board/profile, never folder auto-discovery. Reuse existing bounded per-project TaskQ invocation and locks; timeout/error/approval isolation lets remaining eligible projects progress and preserves PM response. Reconcile claims before restart admission; removal stops new admission but preserves existing ownership. Aggregate truthful per-project stage/status/link/event/observed-time evidence. First qualify two isolated repository/settings/board fixtures: duplicate TICK, one failure, add/remove and restart. Fixture PASS is not live rollout; actual user project selection and supported transport qualification are required before live activation.
-
-Critical order: #176 and #177 can qualify independently; #185 consumes their safety/ACK evidence; #186 consumes bounded transport/ownership. Keep verifiable safety gates and distinguish fixture eligibility from live eligibility instead of deleting prerequisites. Each task submits focused candidate/receipts under the existing PM review flow; no unreviewed publication or automatic approvals. New pure coding workers use Claude; qualification work uses an eligible supported runtime. No fabricated tasks to fill slots.
-
-## Documentation style and two-level help (#187)
-
-Normative documentation requirement: Wiki is the canonical content source; docs first, then reviewed code. Packaged contracts, generated briefs and CLI help reuse the same versioned content rather than divergent copies. Preserve #187/#191 content-version and received/applied distinctions.
-
-Write action → command → result. Short, dry, meaningful words; only material conditions, blockers and errors. No introductions, filler or cryptic abbreviations. Keep permissions, ownership and destructive-action conditions explicit.
-
-1. **Short start:** installed version; configured project/profile or exact gap; what can run now; one next command; where full help lives. Never claim readiness from installation alone.
-2. **Full help:** today `taskq --help` lists commands, `taskq <command> --help` lists arguments, and `taskq contract` prints detailed contract paths. `taskq help` does not exist on tested `2609a3c` (exit 2). A complete scenario-oriented help command is **PROPOSED**, with its spelling reviewed before implementation.
-
-[Short start and scenario map](https://github.com/alexkirs/taskq/wiki/Required-settings#short-start-and-help-187). New multiproject/multimachine/delegation behavior remains proposed in #186; this documentation update does not implement or activate it.
+- [Specifications](https://github.com/alexkirs/taskq/wiki/Specifications): accepted and proposed specifications, with status.
+- [Gradus style](https://gitlab.ufobe.com/gradus-public/caveman/-/tree/62579538f05fb6b69a12449c1ebad9567d1fdecc): how TaskQ docs, briefs and reports are written.
diff --git a/Known-issues.md b/Known-issues.md
index 979e6d3..287494b 100755
--- a/Known-issues.md
+++ b/Known-issues.md
@@ -9,7 +9,7 @@ Facts `taskq doctor` cannot fix. Each line: symptom, then what to do.
 - **A process started with `nohup` or `disown` in a worker's shell dies when the tool call ends** (#130). Use the tool's background mode (Claude: `run_in_background`).
 - **Blender (or anything on Metal) exits 139 in a Codex worker on macOS** (#157). The Codex workspace-write sandbox denies the GPU (`AGXDeviceUserClient`), Metal finds no device, Blender 5.2 crashes at start even with `--background`. Codex has no setting that allows only the GPU. Run such a task in a Claude worker (`run-claude`), or label it `codex-full-access`: its Codex turns then run with `danger-full-access`. Risk: that worker has no sandbox at all, it can write and delete anything your user can, outside the checkout too; set the label only on tasks that need the GPU.
 - **Headless Codex workers (no desktop app) do not show in a Codex app sidebar** (#160). taskq runs them on the CLI's `codex app-server daemon`; the owner reads and writes to them with `taskq codex-read` and `taskq codex-send`, not in an app window.
-- **Windows: the Codex desktop app (Microsoft Store) cannot run taskq's Codex workers** (#159). taskq runs in WSL and drives Codex through `~/.codex/app-server-control/app-server-control.sock`. The Windows app keeps its app server private (stdio); the named pipe `\\.\pipe\codex-ipc` is only the app's IPC router. The CLI bundled with the app refuses `codex app-server daemon start` (`this CLI has no complete local package`), and WSL2 cannot connect to a Windows Unix socket or named pipe directly. A bridge does connect (`codex.exe app-server --listen unix://` on Windows, `codex.exe app-server proxy` over WSL interop), but the Windows sandbox cannot run commands in a checkout inside WSL: with `workspace-write` every command fails (`CreateProcessWithLogonW failed: 267` for a `\\wsl.localhost\…` working directory, `setup refresh had errors` for writable roots there). Only `danger-full-access` runs, and taskq does not run workers without a sandbox by default (#149). Keep `limit.codex = 0` on such a machine, or install the Linux Codex CLI inside WSL and run it headless as on any Linux machine (#160): in WSL `codex login --device-auth && codex app-server daemon start`, then set `limit.codex` above 0 and run `taskq doctor --fix --codex`. Those workers do not show in the Windows app; read and write to them with `taskq codex-read` and `taskq codex-send`.
+- **Windows: the Codex desktop app (Microsoft Store) cannot run taskq's Codex workers** (#159). Keep `limit.codex = 0` on such a machine, or install the Linux Codex CLI inside WSL and run it headless (#160): in WSL `codex login --device-auth && codex app-server daemon start`, then set `limit.codex` above 0 and run `taskq doctor --fix --codex`. Those workers do not show in the Windows app; read and write to them with `taskq codex-read` and `taskq codex-send`. Why the app cannot work: [#159](https://github.com/alexkirs/taskq/issues/159), [Known issues 1d9ce23](https://github.com/alexkirs/taskq/wiki/Known-issues/1d9ce23ddd9a71e2c6a1b7866c5af78b6a7c08ae).
 - **Linux: a Codex worker cannot commit or fetch in its task tree: `.git/worktrees/<task>/index.lock: Read-only file system`** (#163). Codex on Linux mounts the gitdir of a writable root that is a linked worktree read-only after the writable roots (openai/codex#14338). Since db26949 taskq lists that exact gitdir as a root too when it lies in the main checkout's `.git`, which is already writable: no access is added. Run `taskq update`; a turn started before the update keeps its old sandbox, so the worker needs a new turn (`taskq codex-send`). Never fix it with `codex-full-access`.
 - **Claude PM feels less responsive while handling TICKs (user observation).** Prefer a dedicated interactive PM session and a separate timer/TICK sender every 5 minutes; the user observed better responsiveness, but causality is not a proven benchmark. Transport and responsiveness qualification remain pending in [#185](https://github.com/alexkirs/taskq/issues/185).
 - **An active worker or empty flags look like “no permission request”.** They do not prove absence of a request ([observed in #176](https://github.com/alexkirs/taskq/issues/176)). PM reports positive permission evidence with a verified session link; after user approval in the runtime UI, resume the same session. Missing/stale visibility stays unknown; no automatic approval or duplicate worker. See the [recovery checklist](https://github.com/alexkirs/taskq/blob/main/docs/recovery-checklist.md#permission-wait).
diff --git a/Required-settings.md b/Required-settings.md
index f3da7ec..fe56d2e 100644
--- a/Required-settings.md
+++ b/Required-settings.md
@@ -1,36 +1,5 @@
 What a machine needs before its workers start. `taskq doctor` checks each item it can and prints the fix.

-## Short start and help (#187)
-
-Use the [canonical writing rule](https://github.com/alexkirs/taskq/wiki/Home#documentation-style-and-two-level-help-187). Report only verified status. Example after checks:
-
-```text
-Installed: taskq 2609a3c.
-Configured: alexkirs/taskq; this checkout; Claude 8 / Codex 4.
-Can do: inspect queue. Launch requires authority and eligibility.
-Next: taskq list
-Help: taskq --help; taskq <command> --help; taskq contract
-```
-
-Unconfigured: `taskq doctor` → exact gaps. Setup: use the existing onboarding/config flow; installation alone does not prove readiness. Preserve current settings and approvals.
-
-| Action | Existing command or route | Result / material limit |
-|---|---|---|
-| One project | `taskq doctor`; `taskq profile init --help` | Inspect readiness; profile saving requires setup authority. |
-| View tasks | `taskq list`; `taskq view <id>` | Queue/task status; visibility is not execution permission. |
-| Own/shared selection | `taskq worker --mine`; `taskq worker --no-mine` | Own assignments; or own plus unassigned pool. Selection/brief only; needs actual worker identity. No delegation ACL. |
-| Multiple projects/machines | Existing per-checkout commands | Saved project set and cross-machine pre-spawn arbitration: **PROPOSED #186**. |
-| Other users' agents | Existing backend access rules | Owner-approved delegation preserving assignee: **PROPOSED #186**. Filters do not grant ACL rights. |
-| Permission block | `taskq runtime-status --runtime <runtime> <session> --json` | Supported status or unknown; owner decides in runtime UI. No blind retry, replacement or autoapproval. |
-| Deliver review | `taskq result --help` | Submit exact candidate/checks from actual worker; submission is not acceptance or merge. |
-| Cleanup | `taskq cleanup --json`; `taskq cleanup --help` | Read plan first; apply only authorized safe Remove; Ask stays a decision. |
-| Pause/resume a pending task | `taskq later <id> --text "reason"`; `taskq answer <id> --text "continue"` | Later applies to ready/waiting/ask; answer applies to later/ask and normal guards still apply. Neither pauses running work or a timer. |
-| Pause/resume active work or timer | Existing owner-controlled runtime/scheduler handoff | No generic pause/resume command. Preserve claim and approval boundary; no automatic configure the sender again. |
-
-**PROPOSED full-help outline:** setup/profile; project bindings and task filters; status/selection; claims and execution; permission blockers; candidate/review/publication; cleanup; pause/recovery; external TICK; proposed multiproject/multimachine/delegation. Cover every actual CLI command with concise action/command/result examples; label unsupported flows planned. Keep syntax sourced from the parser and meaning sourced from versioned canonical documentation.
-
-Acceptance: a new user gets a useful short start and finds detailed help; examples use real commands; changed content versions propagate through #187/#191; minimal parser/example and generated-content drift checks. No setup, dispatch or timer is implied by reading help.
-
 ## Claude Code workers

 - **Login:** run `claude auth login` once, with the same `claude` the tick starts. [Authentication](https://code.claude.com/docs/en/authentication)
@@ -43,9 +12,22 @@ Acceptance: a new user gets a useful short start and finds detailed help; exampl
 - **A signed-in Codex app server:** workers run on `~/.codex/app-server-control/app-server-control.sock`. The Codex desktop app starts it; without the app (headless Linux, a server) run `codex login --device-auth && codex app-server daemon start` (`codex app-server daemon bootstrap` keeps it across reboots) (#160). [Codex app](https://developers.openai.com/codex/app), [App server](https://developers.openai.com/codex/app-server)
 - **Sandbox and approval:** nothing to set. taskq starts every Codex turn with approval `never` and sandbox `workspace-write` with network on; writable: the checkout's `.git` and `.worktrees` and the taskq state dir (#149), plus any project paths in `taskq.toml` `[codex] writable = ["../media"]` (relative to the main checkout or `~/`; a missing path is skipped and `taskq doctor` warns, #154). [Sandboxing](https://developers.openai.com/codex/concepts/sandboxing), [Approvals and security](https://developers.openai.com/codex/agent-approvals-security)

-- **DOT PM with a separate local TICK sender (recommended, proposed).** Prefer DOT for interactive PM and a separate local sender every 5 minutes, with one timer owner per project and explicit handoff. Supported send/receive still needs qualification in [#185](https://github.com/alexkirs/taskq/issues/185); proposed transport is not already-working transport. A delivery receipt proves neither local execution nor a completed pass; follow the [external PM guide](https://github.com/alexkirs/taskq/blob/main/docs/external-pm-tick.md).
-- **Actual local-execution ACK before worker launch (confirmed narrow evidence).** Have the selected executor run `taskq preflight --json` from the main checkout and return stdout, stderr, exit code and observation time. [#181 evidence](https://github.com/alexkirs/taskq/blob/main/docs/external-pm-tick.md#read-only-evidence-for-181) confirms local command execution only; runtime capability and effective launch policy remain unknown until separately qualified. Chat activity is not an ACK ([#177](https://github.com/alexkirs/taskq/issues/177)); a preflight ACK is not launch authority or a completed TICK pass.
-- **One DOT PM for several projects (proposed, unqualified).** Choose only an explicit project set, with separate directories, queues, settings, claims and bounded passes; never automatically manage every folder. Use one aggregate report preserving project identity, stages, blockers and session links, so a failed project does not obscure another. [#186](https://github.com/alexkirs/taskq/issues/186) owns qualification; [#187](https://github.com/alexkirs/taskq/issues/187) owns later canonical hints/onboarding.

 A machine that never runs Codex sets `limit.codex = 0` in `taskq.local.toml`; doctor then skips the Codex checks.
 A Codex-only machine sets `limit.claude = 0`; doctor then skips the Claude login, folder trust and permissions.
+
+## PM: local-command ACK before worker launch
+
+Before each worker launch, the PM's executor runs this command in the main checkout:
+
+```
+taskq preflight --json
+```
+
+Read the `local_command_ack` action. Record `cwd`, `host`, `stdout`, `stderr`, `exit_code`, `observed_at` and `status`.
+
+- `status: ready` with the main checkout as `cwd` proves local command execution only. It does not prove runtime readiness, launch authority, a completed TICK pass or admission on another host.
+- A missing or failed ACK blocks launch admission. Report its `exact_blocker`.
+- `runtime_capability` and `effective_launch_policy` stay `unknown` until qualified separately ([#176](https://github.com/alexkirs/taskq/issues/176), [#177](https://github.com/alexkirs/taskq/issues/177)).
+
+Guide: [External PM tick](https://github.com/alexkirs/taskq/blob/main/docs/external-pm-tick.md#preflight-and-observation). A separate TICK sender ([#185](https://github.com/alexkirs/taskq/issues/185)) and one PM for several projects ([#186](https://github.com/alexkirs/taskq/issues/186)) are proposed and not qualified. Previous notes: [Required settings d3cc01c](https://github.com/alexkirs/taskq/wiki/Required-settings/d3cc01c131d9d85b565d7c88f81757c03caa6bbb#codex-workers).
diff --git a/Specifications.md b/Specifications.md
new file mode 100644
index 0000000..af9a3ea
--- /dev/null
+++ b/Specifications.md
@@ -0,0 +1,21 @@
+Specifications and their status. Status checked 2026-10-08 against the issues and `main`. The issue is the current status; the linked text is the specification.
+
+## Accepted
+
+- **Cleanup on existing ticks** ([#197](https://github.com/alexkirs/taskq/issues/197)). Accepted. Integration in review. Text: [Cleanup schedule](https://github.com/alexkirs/taskq/wiki/Cleanup-schedule).
+- **Atomic reservation before worker launch** ([#186](https://github.com/alexkirs/taskq/issues/186), stage 1). Accepted. Implementation pending. Text: [Atomic reservation before worker launch](https://github.com/alexkirs/taskq/wiki/Atomic-reservation-before-worker-launch).
+- **Multiproject TICK critical path** ([#186](https://github.com/alexkirs/taskq/issues/186)). Accepted. Qualification pending. Prerequisites [#176](https://github.com/alexkirs/taskq/issues/176), [#177](https://github.com/alexkirs/taskq/issues/177) and [#185](https://github.com/alexkirs/taskq/issues/185) are in review. Text: [Wiki 49cbfcc](https://github.com/alexkirs/taskq/wiki/Home/49cbfcc8022988674a6a67aae701709fadd81966#minimal-critical-path-for-multiproject-tick-186).
+- **Versioned PM tick report contract** ([#191](https://github.com/alexkirs/taskq/issues/191)). Accepted. Code merged to `main` in [PR #199](https://github.com/alexkirs/taskq/pull/199). The issue stays open for review. Text: [Wiki 05cf6aa](https://github.com/alexkirs/taskq/wiki/Home/05cf6aab1c6ed5fc9589b9e4673365cec34c58e6#versioned-pm-tick-report-contract-191). The code pins this revision.
+
+## Implemented
+
+- **CLI update gate and conditional Pages publication** ([#195](https://github.com/alexkirs/taskq/issues/195)). Implemented. Closed 2026-10-07. Text: [Wiki 18038ed](https://github.com/alexkirs/taskq/wiki/Home/18038edc2ae72235d7f87a139919f30df09a441a#cli-update-gate-and-conditional-pages-publication-195). Evidence: [qualification record](https://github.com/alexkirs/taskq/blob/main/docs/pages-gate-qualification.md).
+
+## Proposed
+
+- **Short start and two-level help** ([#187](https://github.com/alexkirs/taskq/issues/187)). Proposed, later. `taskq help` does not exist. Help today: `taskq --help`, `taskq <command> --help`, `taskq contract`. Text: [Home d3cc01c](https://github.com/alexkirs/taskq/wiki/Home/d3cc01c131d9d85b565d7c88f81757c03caa6bbb#documentation-style-and-two-level-help-187), [Required settings d3cc01c](https://github.com/alexkirs/taskq/wiki/Required-settings/d3cc01c131d9d85b565d7c88f81757c03caa6bbb#short-start-and-help-187).
+- **Gradus style in TaskQ** ([#206](https://github.com/alexkirs/taskq/issues/206)). Proposed, later.
+
+## Process
+
+- **Docs-first specifications and drift** ([#189](https://github.com/alexkirs/taskq/issues/189), closed): [process proposal](https://github.com/alexkirs/taskq/blob/main/docs/wiki-sot-process-proposal.md).
````

## Message samples

Illustrative only. Values in `<angle brackets>` are placeholders, not evidence. Commands are shown as examples; this document does not claim they ran.

### Before and after

Before (real #195 close comment; numbers and words run together):

```text
manual full37674768854SUCCESS. CLI-only e4e6fec PR202/main tests37675786566SUCCESS, Pages37675786553 inputs4s/build+deploySKIPPED
```

After:

```text
Manual full Pages run 37674768854: success.
CLI-only push e4e6fec (PR 202): tests run 37675786566 success. Pages run 37675786553: inputs unchanged, build and deploy skipped.
```

### PM tick, English

The generator prints the report block; the PM copies it unchanged and adds judgment lines below. Seven Workers columns, as in `render_report` (`taskq/tick.py`, `main` `ec0cd34`).

```text
PM report v1 sha256:<sha256>
Source: <Wiki revision link>
Repository: <repository URL>
Profile: {"host": "<host>"}
Observed: <observed_at UTC>; outcome: ok
Board: <board URL>
## Workers
| Task | State | Runtime | Session | Last activity | Event time | Commit |
|---|---|---|---|---|---|---|
| [#205](<issue URL>) <title> | review | claude @<machine> | [session](<session URL>) | <last activity> | <event_at UTC or unknown> | [<sha7>](<commit URL>) |
Actions: []
Refusals: []
Source status: available; received/applied: unknown (verify supported-channel output).
Validation: []

Needs you: review #205.
Unknown: #176 permission state; runtime-status unavailable. Held; no retry, no new worker.
```

### PM tick, Russian

Same generated block, unchanged. Only the PM lines are in Russian.

```text
PM report v1 sha256:<sha256>
Source: <Wiki revision link>
Repository: <repository URL>
Profile: {"host": "<host>"}
Observed: <observed_at UTC>; outcome: ok
Board: <board URL>
## Workers
| Task | State | Runtime | Session | Last activity | Event time | Commit |
|---|---|---|---|---|---|---|
| [#205](<issue URL>) <title> | review | claude @<machine> | [session](<session URL>) | <last activity> | <event_at UTC or unknown> | [<sha7>](<commit URL>) |
Actions: []
Refusals: []
Source status: available; received/applied: unknown (verify supported-channel output).
Validation: []

Нужно от вас: ревью #205.
Неизвестно: состояние разрешений #176; runtime-status недоступен. Ждём; без повтора и нового воркера.
```

### Blocker, English

```text
#205 blocked: publishing the Wiki needs your approval.
Asked: taskq ask 205 --text "Publish the reviewed Wiki diff <diff SHA-256> on base <Wiki revision>?"
Worker [session](<session URL>) keeps the claim; nothing published.
Unknown: whether the Wiki changed after <Wiki revision>. Recheck before publishing.
```

### Blocker, Russian

```text
#205 заблокирована: публикация Wiki требует вашего решения.
Вопрос: taskq ask 205 --text "Publish the reviewed Wiki diff <diff SHA-256> on base <Wiki revision>?"
[Сессия](<session URL>) воркера держит задачу; ничего не опубликовано.
Неизвестно: менялась ли Wiki после <Wiki revision>. Перед публикацией перепроверить.
```

### Result, English

```text
#205 result: commit [<sha7>](<commit URL>) on branch taskq-205. Draft only.
Checks: python3 -m unittest discover -s tests -> <N> tests OK.
Not done: Wiki publication. It waits for your review.
Risk: the Wiki may change before review; the diff then needs a rebuild.
```

### Result, Russian

```text
#205 результат: коммит [<sha7>](<commit URL>) в ветке taskq-205. Только черновик.
Проверки: python3 -m unittest discover -s tests -> <N> тестов OK.
Не сделано: публикация Wiki. Ждёт вашего ревью.
Риск: Wiki может измениться до ревью; тогда diff нужно пересобрать.
```
