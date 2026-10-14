# Linux constrained Codex

§8 Access owner: `taskq.md`. Linux initially supports Codex CLI only. No automatic grant or Full fallback.

Require a working native Codex sandbox on the target host; this profile does not replace its prerequisites.

A root-owned copy of TaskQ at `/usr/local/libexec/taskq-sandbox` enters only the sandbox launcher. Its AppArmor profile permits user namespaces. A private, root-owned bubblewrap binary at `/usr/local/libexec/taskq-bwrap` avoids changing global PATH or another runtime's helper. Neither binary has setuid or file capabilities. All parent directories must be root-owned and not group/world writable.

The launcher re-reads the project grant; accepts only Restricted/Workspace and a Codex exec; constructs mount arguments itself. It never accepts bwrap flags. `/` is read-only; workspace mode adds the canonical project, restricted only `.taskq`. Existing Codex state and named build caches are writable. Private temp, PID namespace and proc/dev mounts isolate child processes and host Unix control sockets. Installation, PATH/runtime package directories, active Codex config and existing top-level TOML files, hooks, plugins and project `.codex` stay read-only. Require existing real Codex `config.toml` and project `.codex` directory; do not create or follow a replacement config through a worker. Native Codex keeps read-only/workspace-write and configured network restrictions. A read-only bind of the canonical resolver file preserves DNS when `/etc/resolv.conf` points into private `/run`. Provider transport is unchanged; network-off for the entire CLI is not implemented.

This is a process/mount boundary, not an AppArmor filesystem allowlist or a new identity. AppArmor's `unconfined` flag only makes the launcher eligible for user namespaces; it alone proves no containment. Launch failure stops the attempt. A nested launcher must not regain writes denied by its parent. The existing same-user persistence caveat still applies after a turn exits.

Install the two verified binaries as root with mode 0755; install `packaging/linux/taskq-sandbox.apparmor` at `/etc/apparmor.d/taskq-sandbox`, then load only that profile with `apparmor_parser -r`. Keep `kernel.apparmor_restrict_unprivileged_userns=1`. Grant only the explicit disposable qualification project first. Do not restart active services, change working project flags/grants, or enable their queues during qualification. Helper upgrades use atomic replacement after verification; never overwrite an executing binary in place.

Acceptance is the existing live runtime matrix: real TaskQ start and same-session continuation, all pinned rule bodies, native commands, positive project output and independent negative canary readback (outside, policy, runtime config, links and children). A missing dependency, kernel denial or unqualified row is blocked, never a pass.
