# Installation and project locations

Accepted product target. Extends UC-01/08/11/12/17 and taskq.md §6/§8; does not claim a new installer or filesystem sandbox is shipped. Reuse [worker permissions](worker-permissions.md), [remote manager](remote-manager.md) and shared bootstrap. No second project registry.

## S1. Two installation entrances, one setup flow

Default: ask an agent to install TaskQ from the official repository and connect/create the named project. Keep this route prominent.

Alternative: the release documentation provides one copyable command per supported shell/platform, including macOS/Linux and Windows PowerShell. The owner runs it without starting an AI agent. It installs the verified supported release, registers the local launcher and opens interactive setup (native application on desktop; terminal flow on a headless server). The command must work from an arbitrary launch directory, including home, without adopting that directory as a project.

Both routes use the same setup implementation and review/readback semantics. Do not create a separate agent-only installer. Keep exact working commands in one release-owned source, then reference them from README, app help and agent bootstrap. Do not publish a guessed curl/PowerShell command before the distribution endpoint exists and is qualified. The actual installation command is a required implementation deliverable.

Setup detects OS/architecture and available supported runtimes using executable lookup, known installation locations and bounded read-only version/auth probes. Reuse `taskq agents` capability inspection where possible. Never discover runtimes by recursively searching home or the disk. Report Ready / Installed, sign-in needed / Missing / Unknown without exposing credentials.

If no agent is available, setup still opens and explains the next action: install/sign in to a chosen supported runtime or continue limited setup. Basic settings and registration require no model turn. Installation of an additional runtime, authentication, repository creation, grants and queue admission retain their explicit scopes; detecting a runtime never starts a worker or changes its model/permissions. Finish with actual installed version, service readiness, registered project and remaining prerequisites. Cancellation/retry preserves existing installs, config and projects; no duplicate services.

## S2. Projects folder

A host-owned default parent for creating new projects and the initial folder-picker location. Suggested desktop default: the OS user's `Projects` directory (for example `~/Projects` or `%USERPROFILE%\Projects`); show it before creation and permit another location. Headless setup asks or accepts an explicit path through the same configuration boundary.

- Keep the Projects folder distinct from TaskQ installation, runtime state, project root and owned attempt worktree.
- This is a convenience default, not permission to scan, register or execute every child directory. R10's explicitly registered project list remains authoritative.
- Offer Create project, Open existing and the explicit registered list. Name-based resolution uses that list; a missing/ambiguous name prompts for a selection/path, never a filesystem search.
- Existing projects outside the default parent remain supported through an explicit selection and applicable host grants. Do not relocate existing checkouts during migration.
- Changing the parent affects future creation/picker defaults only. It moves no files and changes neither registration, active attempts nor permissions.
- Show the value in Settings → Projects → Projects folder. Permit changing it through the same reviewed host operation from the CLI or an agent.
- Reject filesystem root or the whole home directory as the default Projects folder. Protected/cloud/network/removable locations need a concrete availability/access explanation; do not silently choose them or request broad access to compensate.

Store the host preference outside project-authored configuration. It is not task lifecycle state. A project, board comment or worker cannot broaden this preference or filesystem grants.

## S3. Exact directories at every launch

| Process/context | Working directory and discovery scope |
|---|---|
| Installer/setup before selecting a project | Owned setup directory; inspect only explicit installation/runtime candidates |
| Service/tray without a selected project | Owned application state directory; read the explicit registry, not the home tree |
| Project PM conversation | Selected registered project root; pass its exact contract/config paths |
| Worker/reviewer | Owned attempt worktree when used; otherwise the exact authorized project/environment root |
| Multi-repository task | Declared environment entry directory and explicitly approved member paths; no implicit siblings |
| Remote PM | Resolve the selected project's path on the named execution host; never substitute the local client's path |

Pass an explicit canonical working directory and allowed operation targets to every spawn/resume/tool adapter. Searching for project files, contracts or configuration begins within that project; if absent, use the known release contract or ask. Do not walk parents, scan `/`, home, other projects or protected user folders to find context.

Runtime home, executables, certificates, required Git metadata, owned temporary storage and approved caches follow P4. Necessary runtime access is not permission for exploratory search. An external worktree's Git common directory is an explicit dependency, not permission to inspect neighboring repositories.

Missing, moved, inaccessible or invalid project root: show the exact error and offer relocation/selection. Hold affected new launches; never fall back to home, the Projects parent or the install directory. Revalidate symlinks/junctions and resumed sessions against actual host grants. A change of project in a conversation retargets only subsequent explicitly scoped operations; existing attempts retain their own roots.

## S4. Search discipline and enforced access

CWD and prompt instructions reduce accidental discovery but do not restrict an arbitrary subprocess. Full mode may still access other paths unless the environment supplies a real boundary. Do not describe the Projects folder as a sandbox or silently lower an owner's Full grant.

TaskQ-owned discovery and operations must enforce exact targets regardless of runtime mode. Strict worker read/write containment uses qualified runtime/OS boundaries under P4. If required enforcement is unavailable, report the capability gap rather than substituting a weaker mode.

For macOS warnings, diagnose the actual requesting process, launch/resume cwd, tool operation and canonical target before claiming a fix. Do not suppress TCC, request Full Disk Access or advise granting every prompt. A denied optional search should not become repeated scans. Explain a required explicit path access and provide a narrow selection/grant route. No promise that all TCC prompts disappear.

## S5. Development and acceptance

Extend backlog streams 1, 6, 9 and 11. First reconcile coreq's installer/service paths and current `taskq agents`, bootstrap, runtime launch/resume and tool-less operator paths; share existing primitives. Define the host-preference read/update schema, explicit root resolution and adapter capabilities before app wiring. Preserve current authority until corresponding runtime contract changes ship.

| ID | Required proof |
|---|---|
| S-A1 | Agent request and documented shell command reach the same setup and produce matching verified installation state |
| S-A2 | Fresh desktop/server without an AI runtime completes basic setup and displays actionable Missing/sign-in/Unknown states |
| S-A3 | Launch installer from home or unrelated cwd; no implicit registration, home scan or worker launch occurs |
| S-A4 | Choose/change Projects folder; new project uses it, existing roots/active work remain unchanged; siblings are not auto-discovered |
| S-A5 | Capture cwd and explicit roots for PM, worker, reviewer, resume, multi-repo and remote adapters; no fallback on missing root |
| S-A6 | Spaces, Unicode, symlinks/junctions, denied paths and unavailable volumes cannot retarget work or bypass grants |
| S-A7 | On macOS, trace a representative warning and verify corrected discovery without broad grants; separately qualify strict boundary denial and Full limitations |
| S-A8 | Retry/cancel/upgrade from an existing install preserves files, settings, registration and service ownership; verify release integrity and readback on each supported platform |

The existing Rust runtime already uses explicit `current_dir(env.root)` and a no-home-search brief in `src/runtime.rs`; these are baseline mechanisms, not proof of all adapter behavior or the reported macOS cause. This specification records the required audit and acceptance, not a reproduced incident fix.
