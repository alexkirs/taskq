# Worker permissions

Design proposal. No active permissions changed. Approve the R9/§8 amendment and migration before implementation.
Shared candidate: [permission authority](https://github.com/taskqorg/specq/blob/main/engineering/permission-authority.md) (local, unpublished).
Applies to workers/reviewers; the tool-less AI operator keeps R3/R9 restrictions.

## Evidence and #865

- Inspected checkout: `cfae06708776d252d52f31510afed6b47918cee6`. `src/runtime.rs::command` passes project `permission_mode` to Claude and project `codex` arguments to spawn/resume. `src/main.rs` defaults to `dontAsk` / `danger-full-access`. No independent host ceiling.
- [#865](https://github.com/taskqorg/taskq/issues/865), closed through [#866](https://github.com/taskqorg/taskq/pull/866): worker `find /` for missing taskq.md caused folder prompts attributed to taskq. Report cites tccd timing and worker transcripts. This review read the report, not those original logs.
- Result `1ed9efc442a1b5f6108ebefa27ed11929ac84a87`, merge `c933c2c6925718c376afae86df4b20be5ad317b3`: briefs name an existing release contract and forbid broad searches. Runtime sandbox was explicitly not enabled because R9 preserved permissions. Owner confirmation after restart remains open.
- Upstream §8 Folders names root/install/runtime homes/temp and an owner-picker exception. The inspected checkout predates that change. Preserve the fix when merging; do not overwrite newer upstream text.
- A prompt instruction is not filesystem enforcement. Closing #865 does not prove containment or absence of future TCC prompts.
- [coreq trust rule](https://github.com/taskqorg/coreq/blob/main/coreq.md): agent-writable project configuration must not choose executable code. Apply the same trust distinction to permission escalation.

## P1 Recommendation

Small preset vocabulary; explicit effective boundary beneath it. No runtime flag editor in the primary menu.

| UI / stored mode | Intended work | Boundary |
|---|---|---|
| Read only / `restricted` | Research, inspection, review | No project writes; private runtime/temp writes still needed |
| Workspace / `workspace` | Edit, build, test | Writes to approved workspace set; network separately controlled |
| Full / `full` | Operator-trusted unrestricted automation | Runtime sandbox disabled; OS/admin policy still applies |

Workspace is the proposed new-install default, only where qualified. Unsupported enforcement blocks admission; never fall back to Full.
The owner's isolated runner can use Full after one host-level approval. No per-task nag for unchanged grants.
Show `Full · VM isolation verified`, `Full · runtime sandbox off`, or `Isolation unknown`. A checkbox claiming a safe environment is not verification.

| Alternative | Benefit | Cost / decision |
|---|---|---|
| Raw flags per project | Exact runtime controls | Easy escalation, poor comparability; reject as authority |
| Three presets only | Small UI | Hides network/path differences; insufficient alone |
| Presets + effective details | Simple common path, inspectable exceptions | Recommended; adapter qualification required |
| Mandatory external isolation for every job | Strongest uniform boundary | More setup and OS work; required when strict containment cannot be proven natively |

## P2 Owner, precedence and trust

`effective = intersection(admin ceiling, host grant, project restriction, task restriction, runtime capability)`.
Modes are shorthand; compare permissions as capabilities, not just ordered strings. Paths/tools/network form separate sets.

| Layer | Who controls it | Can raise? |
|---|---|---|
| OS / managed runtime policy | System administrator | Outside TaskQ; never bypass |
| Host default per runtime | Local operator | Explicit reviewed grant, within admin ceiling |
| Host-owned project override | Local operator; keyed by canonical project identity | Yes within admin ceiling; never inferred from repository contents |
| Project config | Project authors/agents | No; reduce capabilities or request more |
| Task constraint/request | Task author | No; reduce, declare required access, or ask |
| Runtime adapter | TaskQ with versioned capability probe | No; unsupported combinations refuse |

Absent restriction inherits; it does not grant. A request above the ceiling waits for an operator decision, not silent clipping followed by failure.
No automatic escalation via urgency, scope, labels, task answers, agent role, fallback runtime or old session.
Raising host defaults previews affected projects/runtimes. Local one-attempt grants bind project, task, host, runtime and next attempt; consumed once, cannot transfer on retry/migration.

Proposed store: host-owned `worker-policy.json` outside projects and worker-writable install/runtime paths. Authority channel authenticates the human operator independently of worker OS identity.
0600 is hygiene, not protection from a worker running under the same user. `taskq service permissions` alone is not authentication. Workers must not edit policy, impersonate tray confirmation, replace the service or invoke the grant broker.
Workspace/restricted profiles deny those paths and broker access. Full on the same host/user cannot offer this invariant against arbitrary worker code: show `operator-trusted`, or run workers under an isolated identity/VM and keep the broker outside. This is an explicit architecture gate.

## P3 Runtime catalogue

Documentation checked 2026-10-13; local help: Codex 0.159.3, Claude Code 2.1.296. Pin adapter evidence to version/OS. Help is not execution proof.

### Codex

- `-s read-only`, `workspace-write`, `danger-full-access` select sandbox policy; approvals are separate.
- TaskQ unattended candidate: explicit sandbox plus `approval_policy=never`; denied operations become TaskQ decisions, not invisible terminal prompts. Full uses explicit host authorization.
- `--full-auto` is a deprecated compatibility flag in current documentation; do not use it as a stable preset or synonym for Full.
- `sandbox_permissions` is version/surface-specific capability configuration or tool escalation input, not a portable fourth preset. Local help accepts config syntax, but vocabulary and enforcement need adapter tests. Never forward arbitrary project values.
- Enumerate effective additional roots, network and protected paths. `workspace-write` alone does not prove reads are confined to root.
- codex-app needs separate proof: handshake policy and resumed app turn must retain the ceiling. Hide unsupported modes rather than assume CLI equivalence.

Sources: [sandbox/approvals](https://learn.chatgpt.com/docs/sandboxing), [CLI reference](https://learn.chatgpt.com/docs/developer-commands?surface=cli).

### Claude

| Native option | Meaning for TaskQ |
|---|---|
| `default` | Interactive approval; unsupported headless without an approval transport |
| `acceptEdits` | Auto-accept edits; other tools may still prompt; not equivalent to Workspace |
| `plan` | Planning behavior, not a complete OS read-only boundary |
| `dontAsk` | Deny would-be prompts; allowed operations still run; not Full |
| `bypassPermissions` | Skip permission prompts; not an OS/TCC bypass |
| `--allowedTools` / `--disallowedTools` | Tool permission rules; neither alone contains arbitrary shell/MCP side effects |
| `--tools` | Available tool surface; keep operator tools empty |
| `--add-dir` | Additional working paths; host-approved only, not independent containment |

Restricted/Workspace candidate: `dontAsk` plus host-generated tool grants and enforced filesystem policy; qualified shell/network behavior. Full candidate: `bypassPermissions` plus the explicitly selected isolation profile.
Sandbox is separate from permission mode. Enforce startup failure when unavailable and no unsandboxed retry; reject excluded-command escapes and project-supplied weakening. Native Windows needs a supported isolated runner for the same guarantee.
Sources: [permissions](https://code.claude.com/docs/en/permissions), [sandbox](https://code.claude.com/docs/en/sandboxing), [CLI](https://code.claude.com/docs/en/cli-reference).

## P4 Filesystem and environment

“Never leaves root” means access, not current directory. `current_dir(root)` is not a guard.
A working process also needs system libraries, executables, certificates, temporary storage and runtime state. Define read/write/execute grants separately.

| Resource | Intended grant |
|---|---|
| Root, owned worktree, Git common directory | Read; write only in Workspace/Full as explicitly resolved |
| UC-06 repository set | Explicit canonical members; external worktree path is a request, not authority |
| Runtime state and per-attempt temp | Minimal private writable paths; no blanket shared home grant |
| Release binary and bundled contract | Read/execute; never worker-write authority files, updater or policy |
| Build caches / package sources | Named grants, read/write scope, owner-visible exceptions |
| System runtime dependencies | Necessary read/execute; not arbitrary user data |
| Documents/Desktop/Downloads/iCloud, volumes, other app containers | No incidental discovery; explicit exception requires operator review and qualified enforcement |

Deny sensitive home data by default. Narrow injected credentials to required capabilities; do not pass the host policy broker credential. Runtime homes can contain credentials/hooks/config, so allowing all writes to them undermines the ceiling.
Canonicalize roots and constrain symlinks, junctions, mounts, child processes, hooks, MCP/plugins and resumed sessions. Revalidate before spawn; no raw project security flags or additive user config may weaken policy. If the adapter cannot prove the effective merge, refuse.
Network: `Off`, `Allowlist`, `Unrestricted`, bounded by the host grant. Full can restrict network only through an independently enforced external boundary; without it, report Unrestricted. Provider API transport and worker-tool egress are distinct. Show package download/publish requirements explicitly.

| Platform | Enforcement candidate | Qualification gap |
|---|---|---|
| macOS | Native runtime sandbox; Seatbelt-backed Codex/Claude shell boundaries | TCC is separate user-data consent, not a root allowlist; no automatic Full Disk Access grant |
| Linux / WSL2 | Supported runtime sandbox with available dependencies | Reads, children, sockets, mounts and network need negative tests |
| Native Windows | Qualified Codex native sandbox; separate Claude supported runner if required | ACL/reparse-point and child-process tests; no claim that Claude native sandbox exists |
| Isolated VM/container | Boundary around the whole worker, broker outside | Mounts, credentials, host sockets and network must match the declared profile |

Full without external isolation has no strict root boundary. Do not label it contained or promise no macOS prompts. Preserve #865's precise-contract lookup in every mode.

## P5 Denial and escalation

- Detect structured denial with tool/path/capability evidence. Do not label every nonzero exit an escape.
- End the blocked attempt through a fenced `ask`; preserve patch/logs. Queue state is Ask, not Failed. Retry only after a concrete disposition.
- Incident `worker.escape`: confirmed outside-boundary access attempt, outcome `blocked` or `breach`; distinguish from missing permission, TCC denial and unavailable sandbox. Store sensitive paths locally; public reports follow incident consent/redaction.
- Missing sandbox before spawn: admission blocked + service incident; no phantom attempt or failure count.
- Ask options: keep restriction/revise task; grant named capability once; change host/project grant; move to an eligible isolated host. Migration starts a new attempt under R2.
- A board answer requests a grant; it cannot authenticate one. Local review names the exact resource, host, runtime, duration and blast radius.
- Confirmed breach: hold new admission on the affected target, preserve evidence; terminate/quarantine only under an approved R11/emergency-stop amendment. Do not call this containment complete while a process can still run.
- Tightening policy affects new attempts. Mark existing sessions with their old revision; never silently resume an old broader session. Incompatible resume requires a new attempt or explicit refusal.

## P6 Operator UI and commands

`Settings → Agents → <runtime>`: identity/version/account, effective mode, isolation, source, freshness. Disabled runtime remains visible.
Primary controls: `Read only | Workspace | Full`. Details: Files, Network, Tools, Project exceptions. Model/effort stay distinct; do not reset them while changing access.
Host shown once. Project exceptions inherit by default; list only exceptions plus Add. One review shows old→new, affected targets, active attempts unchanged. Cancel writes nothing.
Full review states `Runtime sandbox off` and actual external isolation evidence. No repeated confirmation for unchanged settings.
Permission blockage appears in Attention and the task, with `Review access` deep-linking to the exact request. Host policy never lives in the AI transcript.

Proposed CLI shape, not implemented:
```
taskq service permissions --host mac
taskq service permissions --host mac --runtime codex --mode workspace --review
taskq service permissions --host mac --runtime claude --project <registered-id> --mode full --review
```
Apply requires the authenticated local review channel and revision check; no public `--yes` route available to a worker. Local external editing imports a candidate through the same validation/readback, not a project-file grant.
`service status`: requested/effective mode, policy revision, source, sandbox/isolation proof, active-old-revision count, pending/unsupported reason. No credentials.
Spawn comment: mode + policy digest/version, additive metadata compatible with existing parsers; no local paths. Full audit also records actor, old/new, target, time and approval locally outside worker write access.
Saved is not Applied. Verify service readback before reporting completion. Unsupported or stale remains explicit.

## P7 Migration

1. Inventory raw `permission_mode`, `codex` args, runtime config sources, existing sessions and host identity. Never auto-import a dangerous grant from project files.
2. Show current resolved behavior beside proposed presets, exceptions and unsupported arguments. Preserve model/effort and unrelated config.
3. Operator approves a host policy; choose existing Full deliberately or a narrower profile. Approval is per host, not cloned with a repository.
4. New secure-policy release holds new admission until migration review; current attempts follow R11. This visible hold is a proposed rollout decision, not a change made here.
5. After compatible rollout, legacy project security fields become requests/restrictions only. Conflicts block with a migration explanation. Preserve originals for review; do not silently delete.
6. Old binaries ignore the new ceiling: drain/stop incompatible consumers through approved lifecycle, enforce a minimum policy-capable version, and prevent automatic rollback from launching an incompatible service. Roll back UI/code only to a compatible version; otherwise hold admission for operator recovery.
7. Verify spawn and resume for each runtime/OS, then resume admission. Share no host grants or private paths on the board.

## P8 Proposed taskq.md amendment

Apply with implementation and executable lifecycle traces, after owner approval. Current R9/§8/§9 remain runtime authority.

- R9 replacement: operator-owned host policy defines permission grants per runtime and optional registered-project override. Project/task input may narrow or request access, never raise it. Full requires explicit local approval, audited outside worker writes. Operator remains tool-less.
- §8 Runtime addition: resolve and validate effective policy before spawn/resume; record revision/mode; refuse unsupported containment. Prompt-denied work enters fenced Ask. No in-place escalation or silent broader resume.
- §8 Folders replacement after reconciling #865: exact contract path, no broad discovery; access follows approved read/write/execute roots and runtime dependencies. Full is uncontained unless an independent boundary is verified. TCC consent is separate.
- §4/§5: status readback and local permissions-review command; policy decisions cannot be applied by the AI allowlist or a board answer alone.
- §9: remove project security options as grant authority after reviewed per-host migration; add compatibility floor for rollback. Preserve non-security runtime options.
- R1: host policy/audit is local operator configuration, not board task state. Board remains authoritative for Ask/attempt transitions.

## P9 Implementation acceptance

| ID | Required proof |
|---|---|
| P-A1 | Project/task text, config, CLI args, hooks and runtime merge cannot raise the host ceiling |
| P-A2 | Only authenticated local operator grants Full/exception; stale/replayed approvals fail; durable audit |
| P-A3 | Preset mapping proven for spawn/resume, workers/reviewers and codex-app separately |
| P-A4 | Negative read/write/child/network tests across roots, symlinks, Git common dir, homes and policy store on each supported OS |
| P-A5 | Sandbox unavailable fails closed; Full reports absent isolation honestly; no implicit TCC consent |
| P-A6 | Denial→Ask fenced once; missing capability vs escape vs breach distinguished; repeated delivery creates no extra grant/attempt |
| P-A7 | Status, tray and spawn agree on revision/mode; pending and active-old-policy are explicit |
| P-A8 | UC-01/03/04/06/07: setup, research, urgency, five repos and parallel workers respect independent grants |
| P-A9 | Migration preserves unrelated settings; no legacy auto-escalation; incompatible rollback cannot admit |
| P-A10 | Native launchd regression against #865: exact contract lookup, protected-folder negative tests, owner no-prompt confirmation |

Suggested slices: policy authority/migration; runtime adapters/boundaries; denial lifecycle; tray/CLI/readback. Each slice gets its own executable acceptance and supported-platform evidence.
Open: authenticate local grants; external-isolation baseline; host default/migration hold; emergency breach response. Do not implement a security guarantee using only prompt text or same-user file permissions.
