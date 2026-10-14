//! Linux §8: fixed launcher, entered only by the root-owned helper copy.
use std::{path::{Path, PathBuf}, process::Command};
use std::os::unix::{fs::MetadataExt, process::CommandExt};

pub const HELPER: &str = "/usr/local/libexec/taskq-sandbox";
const BWRAP: &str = "/usr/local/libexec/taskq-bwrap";

fn trusted(path: &Path) -> Result<(), String> {
    for p in path.ancestors() {
        let m = std::fs::symlink_metadata(p).map_err(|e| format!("linux sandbox {}: {e}", p.display()))?;
        if m.file_type().is_symlink() || m.uid() != 0 || m.mode() & 0o6022 != 0 {
            return Err(format!("linux sandbox requires root-owned, non-writable, non-setid {}", p.display()));
        }
    }
    Ok(())
}

pub fn ready(root: &Path) -> Result<(), String> {
    for p in [HELPER, BWRAP] { trusted(Path::new(p))?; }
    let home = codex_home()?;
    for p in [home.join("config.toml"), root.join(".codex")] {
        if std::fs::symlink_metadata(&p).map_err(|e| format!("linux sandbox prerequisite {}: {e}", p.display()))?.file_type().is_symlink() {
            return Err(format!("linux sandbox refuses symlink {}", p.display()));
        }
    }
    Ok(())
}

fn codex_home() -> Result<PathBuf, String> {
    std::env::var_os("CODEX_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".codex")))
        .ok_or("linux sandbox: Codex home unknown")?.canonicalize().map_err(|e| e.to_string())
}

pub fn run(args: &[String]) -> Result<(), String> {
    if args.len() < 6 { return Err("linux sandbox: expected root, install, mode, Codex executable and exec arguments".into()); }
    let root = Path::new(&args[0]).canonicalize().map_err(|e| e.to_string())?;
    let install = Path::new(&args[1]).canonicalize().map_err(|e| e.to_string())?;
    let mode = &args[2];
    if !["workspace", "restricted"].contains(&mode.as_str()) { return Err("linux sandbox: restricted/workspace only".into()); }
    ready(&root)?;
    let p = crate::policy::load(&install)?;
    let scope = root.to_str().ok_or("linux sandbox: root must be UTF-8")?;
    let g = p.grants.iter().find(|g| g.runtime == "codex" && g.project.as_deref() == Some(scope))
        .ok_or("linux sandbox: explicit project grant required")?;
    if g.mode != *mode && !(g.mode == "workspace" && mode == "restricted") { return Err("linux sandbox: grant changed or does not permit this mode".into()); }
    let bin = Path::new(&args[3]).canonicalize().map_err(|e| e.to_string())?;
    if bin != crate::runtime::which("codex").canonicalize().map_err(|e| e.to_string())? || args[4] != "exec" || args[5..].iter().any(|a| ["danger-full-access", "--yolo", "--dangerously-bypass-approvals-and-sandbox"].contains(&a.as_str())) {
        return Err("linux sandbox: only bounded Codex exec allowed".into());
    }
    let home = PathBuf::from(std::env::var_os("HOME").ok_or("linux sandbox: HOME missing")?);
    let ch = codex_home()?;
    let mut c = Command::new(BWRAP);
    c.args(["--die-with-parent", "--new-session", "--unshare-user", "--unshare-pid", "--unshare-ipc", "--cap-drop", "ALL", "--ro-bind", "/", "/", "--proc", "/proc", "--dev", "/dev", "--tmpfs", "/tmp", "--tmpfs", "/run", "--tmpfs", "/var/tmp"]);
    let dns = Path::new("/etc/resolv.conf").canonicalize().map_err(|e| format!("linux sandbox DNS: {e}"))?;
    c.arg("--ro-bind").arg(&dns).arg(&dns);
    let workspace = if mode == "workspace" { root.clone() } else { root.join(".taskq") };
    for p in std::iter::once(workspace).chain(std::iter::once(ch.clone())).chain([".cargo", ".rustup", ".npm", ".cache"].map(|s| home.join(s))) {
        if p.exists() { let p = p.canonicalize().map_err(|e| e.to_string())?; c.arg("--bind").arg(&p).arg(&p); }
    }
    // Read-only mounts follow every write mount; no caller supplies mount arguments.
    let mut protected = vec![install, root.join(".codex")];
    protected.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).filter(|p| p.is_absolute() && p.is_dir()));
    if let Some(d) = bin.parent() { protected.push(if d.ends_with("bin") { d.parent().unwrap_or(d) } else { d }.to_path_buf()); }
    for e in std::fs::read_dir(&ch).map_err(|e| e.to_string())? {
        let p = e.map_err(|e| e.to_string())?.path();
        if p.extension().is_some_and(|e| e == "toml") || ["auth.json", "hooks.json", "AGENTS.md", "plugins", "skills", "packages", "app-server-control", "app-server-daemon"].iter().any(|n| p.ends_with(n)) { protected.push(p); }
    }
    for p in protected {
        let p = p.canonicalize().map_err(|e| e.to_string())?;
        c.arg("--ro-bind").arg(&p).arg(&p);
    }
    // No sudo/setid escape, even when the host account has broader privileges.
    if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } != 0 { return Err(std::io::Error::last_os_error().to_string()); }
    c.args(["--setenv", "TMPDIR", "/tmp", "--chdir"]).arg(&root).arg("--").arg(bin).args(&args[4..]);
    Err(format!("linux sandbox: {}", c.exec()))
}

#[cfg(test)]
mod tests {
    use super::*;
    // Paid integration chain: actual TaskQ worker/reviewer start+resume and OS-denied writes, not per-commit CI.
    #[test]
    #[ignore = "requires installed root-owned Linux helper and an explicit disposable project grant"]
    fn live_linux_boundary_matrix() {
        use crate::runtime::{Env, Launch};
        let root = PathBuf::from(std::env::var_os("TASKQ_QUALIFY_ROOT").expect("disposable project"));
        let install = PathBuf::from(std::env::var_os("TASKQ_INSTALL").expect("disposable installation"));
        let exe = PathBuf::from(std::env::var_os("TASKQ_QUALIFY_EXE").expect("candidate CLI"));
        let sandbox = std::env::var("TASKQ_QUALIFY_SANDBOX").unwrap_or("workspace-write".into());
        assert!(["workspace-write", "read-only"].contains(&sandbox.as_str()));
        let codex = ["-s", sandbox.as_str(), "-c", "sandbox_workspace_write.network_access=true"].map(String::from);
        let env = Env { root: &root, install: &install, exe: &exe, codex: &codex, permission_mode: "dontAsk", publish: "pr", board_env: &[] };
        std::fs::create_dir_all(root.join(".codex")).unwrap();
        std::fs::create_dir_all(root.join(".taskq")).unwrap();
        let canaries = [root.parent().unwrap().join("outside.txt"), install.join("canary.txt"), root.join(".codex/canary.toml")];
        for p in &canaries { std::fs::write(p, "UNCHANGED\n").unwrap(); }
        std::fs::remove_file(root.join("outside-link")).ok();
        std::os::unix::fs::symlink(&canaries[0], root.join("outside-link")).unwrap();
        std::fs::write(root.join("fixture-install-path"), install.to_str().unwrap()).unwrap();
        std::fs::write(root.join("check.py"), include_str!("../tests/fixtures/linux-boundary.py")).unwrap();
        std::fs::write(root.join("taskq.md"), "# Owner-authorized isolated runtime qualification\nFor both worker and reviewer, run exactly `python3 check.py`, inspect its JSON stdout (proof.json is also written when workspace writes are allowed) and compare baseline, expected and candidate active/repeated/empty cases. Identify regressions even when the command exits 0. Report denied canaries and evidence limits. This fixture has no board task: do not run inbox, create worktrees, commit, publish or request elevation. These owner instructions override generic board workflow. Every continuation repeats the check after stage changes.\n").unwrap();
        std::fs::write(root.join("AGENTS.md"), "Read taskq.md; disposable qualification only.\n").unwrap();
        let attempt = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() * 2;
        for (i, role) in ["worker", "reviewer"].iter().enumerate() {
            let mut l = Launch { task: 1, title: "Owner-authorized isolated boundary qualification. Follow fixture taskq.md; no board or publication.".into(), attempt: attempt + i as u64, role: role.to_string(), rt: "codex".into(), session: crate::runtime::uuid(), url: "https://github.com/taskqorg/taskq".into(), resume: None };
            for turn in 0..2 {
                std::fs::write(root.join("stage"), turn.to_string()).unwrap();
                std::fs::remove_file(root.join("proof.json")).ok();
                if turn == 1 { l.resume = Some("Owner: repeat the exact fixture check against the changed stage; retain this session. No other actions.".into()); }
                let mut child = crate::runtime::spawn(&env, &l).unwrap_or_else(|e| panic!("{e}"));
                let began = std::time::Instant::now();
                let code = loop {
                    if let Some(code) = child.exited().unwrap() { break code; }
                    if began.elapsed().as_secs() > 180 { child.kill(); panic!("owned qualification deadline"); }
                    std::thread::sleep(std::time::Duration::from_millis(200));
                };
                let log = std::fs::read_to_string(crate::runtime::log_path(&root, &l)).unwrap();
                assert_eq!(code, 0, "{log}");
                let ids: Vec<String> = log.lines().filter_map(|s| serde_json::from_str::<serde_json::Value>(s).ok()).filter(|v| v["type"] == "thread.started").filter_map(|v| v["thread_id"].as_str().map(String::from)).collect();
                if turn == 0 { l.session = ids.first().expect("native session").clone(); }
                assert!(ids.iter().all(|id| *id == l.session), "same session");
                let proof: serde_json::Value = log.lines().filter_map(|s| serde_json::from_str::<serde_json::Value>(s).ok())
                    .filter(|v| v["type"] == "item.completed" && v["item"]["type"] == "command_execution")
                    .filter_map(|v| serde_json::from_str::<serde_json::Value>(v["item"]["aggregated_output"].as_str()?).ok())
                    .filter(|v| v["stage"] == turn.to_string()).last().expect("real command proof");
                assert_eq!(proof["stage"], turn.to_string());
                assert_eq!(proof["allowed"], sandbox == "workspace-write");
                for p in &canaries { assert_eq!(std::fs::read_to_string(p).unwrap(), "UNCHANGED\n", "{}", p.display()); }
                for key in ["outside", "policy", "config", "symlink", "child"] { assert_eq!(proof[key], "denied", "{key}: {proof}"); }
                println!("QUALIFIED Linux {sandbox} {role} turn={turn} session={} proof={proof}", l.session);
            }
            let store = install.join("worker-policy.json");
            let saved = std::fs::read(&store).unwrap();
            crate::policy::grant(&install, "codex", "none", Some(&root), "qualification revocation", 2).unwrap();
            let refused = crate::runtime::spawn(&env, &l).err();
            std::fs::write(&store, saved).unwrap();
            assert!(refused.is_some(), "revoked continuation must not launch");
            println!("QUALIFIED {role} revoked continuation refused before spawn");
        }
    }
}
