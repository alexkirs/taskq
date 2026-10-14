//! Worker access (taskq.md R9, § 8 Access): operator-owned grants in `<install>/worker-policy.json`. Project config,
//! task text and board answers never grant; legacy `taskq.json` flags are a request, never a grant
//! ([permission authority](https://github.com/taskqorg/specq/blob/main/engineering/permission-authority.md)).

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub const FILE: &str = "worker-policy.json";
pub const MODES: &[&str] = &["restricted", "workspace", "full"];
/// The oldest version that enforces this file: an older one (4.1.11) runs legacy flags and its update sweep deletes it.
pub const FLOOR: &str = "4.2.0-dev";

#[derive(Default, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    pub revision: u64,
    #[serde(default)]
    pub grants: Vec<Grant>,
}

/// One preset for one runtime: the host default, or one registered project root (scope).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Grant {
    pub runtime: String,
    pub mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// Provenance: the local OS account that wrote it and when (unix seconds).
    pub by: String,
    pub at: u64,
}

/// Missing: no grants. Unreadable or invalid: an error, which refuses every launch (unknown is never a grant).
pub fn load(install: &Path) -> Result<Policy, String> {
    let path = install.join(FILE);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Policy::default()),
        Err(e) => return Err(format!("{FILE}: {e}")),
    };
    let p: Policy = serde_json::from_str(&text).map_err(|e| format!("{FILE}: {e}"))?;
    for g in &p.grants {
        if !MODES.contains(&g.mode.as_str()) || !crate::service::RUNTIMES.contains(&g.runtime.as_str()) {
            return Err(format!("{FILE}: unknown grant {} {}", g.runtime, g.mode));
        }
    }
    Ok(p)
}

/// Where a launch runs: the inputs `resolve` reads besides the policy.
pub struct Host<'a> {
    pub os: &'a str,
    pub install: &'a Path,
    pub root: &'a Path,
    pub home: &'a Path,
}

impl Host<'_> {
    pub fn here<'a>(install: &'a Path, root: &'a Path, home: &'a Path) -> Host<'a> {
        Host { os: std::env::consts::OS, install, root, home }
    }
}

/// Requested vs effective access of one runtime. `refused`: no launch, with the reason.
#[derive(Debug, Clone, PartialEq)]
pub struct Effective {
    pub runtime: String,
    pub requested: String,
    pub grant: String,
    pub mode: String,
    pub enforcement: String,
    /// The Seatbelt profile taskq wraps the child in (macOS).
    pub profile: Option<String>,
    pub linux: bool,
    /// Claude permission mode replacing the project's (Full).
    pub claude: Option<String>,
    /// Codex arguments replacing the project's: a sandbox cannot nest inside taskq's; Full has none.
    pub codex: Option<Vec<String>>,
    pub refused: Option<String>,
}

impl Effective {
    pub fn json(&self) -> Value {
        json!({"runtime": self.runtime, "requested": self.requested, "grant": self.grant, "effective": self.mode, "enforcement": self.enforcement, "refused": self.refused})
    }

    pub fn line(&self) -> String {
        let eff = match &self.refused {
            Some(r) => format!("refused: {r}"),
            None => format!("{} · {}", self.mode, self.enforcement),
        };
        format!("{}: requested {} · grant {} · effective {eff}", self.runtime, self.requested, self.grant)
    }
}

fn canon(p: &Path) -> PathBuf {
    p.canonicalize().unwrap_or_else(|_| p.to_path_buf())
}

/// Codex chooses its own sandbox with these: it cannot run inside taskq's (Seatbelt does not nest).
fn codex_sandbox(args: &[String]) -> Option<&str> {
    args.iter().enumerate().find_map(|(i, a)| match a.as_str() {
        "-s" | "--sandbox" => args.get(i + 1).map(String::as_str),
        "--full-auto" => Some("workspace-write"),
        _ => a.strip_prefix("--sandbox="),
    })
}

/// The project's Codex arguments without sandbox choices (`-c sandbox_*` included), under `-s danger-full-access`;
/// model, effort and other options stay (R9).
fn codex_unsandboxed(args: &[String]) -> Vec<String> {
    let mut out = vec!["-s".to_string(), "danger-full-access".into()];
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-s" | "--sandbox" => { it.next(); }
            "-c" | "--config" => match it.next() {
                Some(v) if v.starts_with("sandbox") => {}
                Some(v) => out.extend([a.to_string(), v.clone()]),
                None => {}
            },
            "--full-auto" | "--dangerously-bypass-approvals-and-sandbox" => {}
            a if a.starts_with("--sandbox=") || a.starts_with("--config=sandbox") => {}
            _ => out.push(a.clone()),
        }
    }
    out
}

fn sb(p: &Path) -> String {
    format!("\"{}\"", p.display().to_string().replace('\\', "\\\\").replace('"', "\\\""))
}

/// Seatbelt profile: everything allowed except writes to `<install>` (policy store, token, releases), to `PATH`
/// directories (the service runs `claude`/`codex` from there unwrapped), to Codex config (`.codex/*.toml` can widen
/// Codex's own sandbox) and, when `writable` is given, outside it. Paths are canonical: Seatbelt matches real paths
/// (`/tmp` is `/private/tmp`).
pub fn profile(install: &Path, writable: Option<&[PathBuf]>, home: &Path) -> String {
    let mut s = String::from("(version 1)\n(allow default)\n");
    if let Some(w) = writable {
        let mut any: Vec<String> = w.iter().map(|p| format!("(subpath {})", sb(&canon(p)))).collect();
        // Claude rewrites `~/.claude.json` through sibling temp files
        let esc: String = canon(home).join(".claude.json").display().to_string().chars().flat_map(|c| if c.is_alphanumeric() || c == '/' { vec![c] } else { vec!['\\', c] }).collect();
        any.push(format!("(regex #\"^{esc}\")"));
        s += &format!("(deny file-write* (require-not (require-any {})))\n", any.join(" "));
    }
    s += &format!("(deny file-write* (subpath {}))\n", sb(&canon(install)));
    // ponytail: every absolute PATH entry; a PATH holding temp or the root itself would close it too
    for d in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).filter(|d| d.is_absolute() && d.is_dir()) {
        s += &format!("(deny file-write* (subpath {}))\n", sb(&canon(&d)));
    }
    // the service probes the runtimes unwrapped: close their real package dirs too (an npm `bin/` shim's package)
    for b in ["claude", "codex"].map(crate::runtime::which).iter().filter(|b| b.is_absolute()).map(|b| canon(b)) {
        if let Some(d) = b.parent() {
            s += &format!("(deny file-write* (subpath {}))\n", sb(if d.ends_with("bin") { d.parent().unwrap_or(d) } else { d }));
        }
    }
    s += "(deny file-write* (regex #\"/\\.codex/[^/]*\\.toml$\"))\n";
    s
}

/// Writable paths of a preset: runtime homes, OS temp, devices and user build caches; Workspace adds the root,
/// Restricted only `<root>/.taskq` (the CLI's poke and error log).
fn writable(mode: &str, h: &Host) -> Vec<PathBuf> {
    // ponytail: fixed cache list; named per-policy cache grants (worker permissions P4) when a project needs another
    let mut w: Vec<PathBuf> = [".claude", ".codex", ".cargo", ".rustup", ".npm", ".cache", "Library/Caches"].iter().map(|d| h.home.join(d)).collect();
    w.extend(["/private/tmp", "/private/var/folders", "/dev"].map(PathBuf::from));
    w.push(if mode == "workspace" { h.root.to_path_buf() } else { h.root.join(".taskq") });
    w
}

/// The effective access of `rt` in this project (§ 8 Access). A project grant replaces the host default for its root.
pub fn resolve(policy: &Result<Policy, String>, h: &Host, rt: &str, permission_mode: &str, codex: &[String]) -> Effective {
    let requested = if rt == "claude" { format!("--permission-mode {permission_mode}") } else if codex.is_empty() { "codex defaults".into() } else { codex.join(" ") };
    let mut e = Effective { runtime: rt.into(), requested, grant: "none".into(), mode: "legacy".into(), enforcement: String::new(), profile: None, linux: false, claude: None, codex: None, refused: None };
    let p = match policy {
        Ok(p) => p,
        Err(err) => {
            e.grant = "unknown".into();
            e.refused = Some(format!("policy unreadable ({err})"));
            return e;
        }
    };
    let root = canon(h.root).display().to_string();
    let g = p.grants.iter().find(|g| g.runtime == rt && g.project.as_deref() == Some(root.as_str())).or_else(|| p.grants.iter().find(|g| g.runtime == rt && g.project.is_none()));
    let mac = h.os == "macos" && rt != "codex-app"; // an app turn is no child of the service: nothing to wrap
    match g {
        None => {
            // legacy flags run unchanged, never as a grant; only the policy store is closed to the worker
            e.grant = "none (taskq.json is a request)".into();
            let own = (rt != "claude").then(|| codex_sandbox(codex)).flatten().filter(|s| *s != "danger-full-access");
            if !mac {
                e.refused = Some(format!("no operator grant and no policy store protection for {rt} on {}; grant a preset", h.os));
            } else if let Some(s) = own {
                // codex's own Seatbelt writes cwd (the root), temp and added dirs: only one sandbox choice, a model,
                // network access and added dirs that do not hold `<install>` may pass
                let picks = codex.iter().filter(|a| ["-s", "--sandbox", "--full-auto"].contains(&a.as_str()) || a.starts_with("--sandbox=")).count();
                let mut ok = picks == 1 && !canon(h.install).starts_with(canon(h.root));
                let mut it = codex.iter();
                while let Some(a) = it.next() {
                    ok &= match a.as_str() {
                        "-s" | "--sandbox" | "-m" | "--model" => it.next().is_some_and(|v| !v.starts_with('-')),
                        "--add-dir" => it.next().is_some_and(|v| Path::new(v).is_absolute() && !canon(h.install).starts_with(canon(Path::new(v)))),
                        "-c" | "--config" => it.next().is_some_and(|v| v.starts_with("sandbox_workspace_write.network_access=")),
                        "--full-auto" => true,
                        a => a.starts_with("--sandbox=") || a.starts_with("--model="),
                    };
                }
                if !ok {
                    e.refused = Some("codex arguments or root may widen its sandbox to the policy store".into());
                }
                e.enforcement = format!("codex sandbox {s}, policy store not writable");
            } else {
                e.enforcement = "taskq seatbelt, policy store not writable".into();
                e.profile = Some(profile(h.install, None, h.home));
            }
        }
        Some(g) => {
            e.grant = format!("{} ({}) rev {} by {}", g.mode, if g.project.is_some() { "project" } else { "host" }, p.revision, g.by);
            e.mode = g.mode.clone();
            if g.mode == "full" {
                e.enforcement = "runtime sandbox off, uncontained; the same user can edit the policy".into();
                if rt == "claude" {
                    e.claude = Some("bypassPermissions".into());
                } else {
                    e.codex = Some(codex_unsandboxed(codex));
                }
            } else if h.os == "linux" && rt == "codex" && g.project.is_some() {
                match linux_codex(&g.mode, h.root, codex) {
                    Ok((mode, args)) => { e.mode = mode; e.codex = Some(args); e.linux = true; e.enforcement = "taskq Linux mount/PID sandbox plus native Codex sandbox".into(); }
                    Err(r) => e.refused = Some(r),
                }
            } else if !mac {
                e.refused = Some(format!("{} enforcement unknown for {rt} on {}", g.mode, h.os));
            } else {
                e.enforcement = format!("taskq seatbelt, writes only to the {} set", g.mode);
                e.profile = Some(profile(h.install, Some(&writable(&g.mode, h)), h.home));
                if rt != "claude" {
                    e.codex = Some(codex_unsandboxed(codex));
                }
            }
        }
    }
    e
}

/// Only known request fields cross the Linux boundary; no profile, command or arbitrary config overrides.
fn linux_codex(mode: &str, root: &Path, args: &[String]) -> Result<(String, Vec<String>), String> {
    #[cfg(target_os = "linux")]
    crate::linux_sandbox::ready(root)?;
    #[cfg(not(target_os = "linux"))]
    { let _ = (mode, root, args); return Err("linux sandbox must be resolved on Linux".into()); }
    #[allow(unreachable_code)]
    {
        let mut mode = mode.to_string();
        let mut kept = Vec::new();
        let mut it = args.iter();
        while let Some(a) = it.next() {
            match a.as_str() {
                "-s" | "--sandbox" => match it.next().map(String::as_str) {
                    Some("read-only") => mode = "restricted".into(),
                    Some("workspace-write" | "danger-full-access") => {},
                    _ => return Err("linux sandbox: unknown sandbox request".into()),
                },
                "-m" | "--model" => kept.extend([a.clone(), it.next().filter(|v| !v.starts_with('-')).ok_or("linux sandbox: missing model")?.clone()]),
                "-c" | "--config" => {
                    let v = it.next().ok_or("linux sandbox: missing config")?;
                    if !(v.starts_with("model_reasoning_effort=") || ["sandbox_workspace_write.network_access=true", "sandbox_workspace_write.network_access=false"].contains(&v.as_str())) { return Err(format!("linux sandbox: unsupported config request {v}")); }
                    kept.extend([a.clone(), v.clone()]);
                }
                _ => return Err(format!("linux sandbox: unsupported argument {a}")),
            }
        }
        let mut out = vec!["-s".into(), if mode == "restricted" { "read-only" } else { "workspace-write" }.into(), "-c".into(), "approval_policy=never".into(), "-c".into(), "sandbox_workspace_write.writable_roots=[]".into()];
        out.extend(kept);
        Ok((mode, out))
    }
}

/// Every runtime taskq starts here, for `service permissions` and `status.json` `policy`.
pub fn report(install: &Path, root: &Path, permission_mode: &str, codex: &[String]) -> Vec<Effective> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from).unwrap_or_default();
    let p = load(install);
    crate::runtime::agent_runtimes().iter().map(|rt| resolve(&p, &Host::here(install, root, &home), rt, permission_mode, codex)).collect()
}

/// § 8 Access rollback floor: with grants, or an unreadable store, the launcher must not roll back below [`FLOOR`]. A
/// `previous` older than it becomes `version`, so a failing release stays down (no admission) for the operator.
pub fn pin_rollback(install: &Path, version: &str) -> Option<String> {
    if matches!(load(install), Ok(p) if p.grants.is_empty()) || coreq::update::newer(FLOOR, version) {
        return None;
    }
    let prev = std::fs::read_to_string(install.join("previous")).ok()?.trim().to_string();
    if prev.is_empty() || !coreq::update::newer(FLOOR, &prev) {
        return None;
    }
    crate::platform::write_atomic(&install.join("previous"), version).ok()?;
    Some(format!("rollback floor: previous {prev} ignores worker grants; previous is now {version}"))
}

/// Add, replace or remove (`mode` none) one grant (the caller refuses workers); the revision moves by one; written atomically and read back.
pub fn grant(install: &Path, rt: &str, mode: &str, project: Option<&Path>, by: &str, at: u64) -> Result<Policy, String> {
    if !crate::service::RUNTIMES.contains(&rt) || !(MODES.contains(&mode) || mode == "none") {
        return Err(format!("unknown runtime or mode: {rt} {mode}; modes: {}, none", MODES.join(", ")));
    }
    let mut p = load(install)?; // a corrupt store stays for the operator, never overwritten
    let project = project.map(|r| canon(r).display().to_string());
    p.grants.retain(|g| !(g.runtime == rt && g.project == project));
    if mode != "none" {
        p.grants.push(Grant { runtime: rt.into(), mode: mode.into(), project, by: by.into(), at });
    }
    p.revision += 1;
    let text = serde_json::to_string_pretty(&p).map_err(|e| e.to_string())? + "\n";
    crate::platform::write_atomic(&install.join(FILE), &text)?;
    if load(install)? != p {
        return Err(format!("{FILE}: readback differs"));
    }
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host<'a>(os: &'a str, install: &'a Path, root: &'a Path) -> Host<'a> {
        Host { os, install, root, home: Path::new("/Users/u") }
    }

    fn g(rt: &str, mode: &str, project: Option<&str>) -> Grant {
        Grant { runtime: rt.into(), mode: mode.into(), project: project.map(String::from), by: "op".into(), at: 1 }
    }

    // Chain: R9 / § 8 Access resolution. Risk: legacy config or a missing grant becomes a grant, or unknown
    // enforcement launches.
    #[test]
    fn grants_resolve_and_unknown_enforcement_refuses() {
        let (i, r) = (Path::new("/i"), Path::new("/p"));
        let ws = ["-s".to_string(), "workspace-write".into(), "--add-dir".into(), "/p/.git".into()];
        let none = Ok(Policy::default());
        // legacy: flags unchanged, never a grant; macOS closes the store, other OS refuse
        let e = resolve(&none, &host("macos", i, r), "claude", "bypassPermissions", &[]);
        assert_eq!((e.mode.as_str(), e.refused.as_ref(), e.requested.as_str()), ("legacy", None, "--permission-mode bypassPermissions"));
        assert!(e.profile.unwrap().starts_with("(version 1)\n(allow default)\n(deny file-write* (subpath \"/i\"))\n"));
        assert!(resolve(&none, &host("macos", i, r), "codex", "", &["--full-auto".into(), "-m".into(), "gpt-5".into()]).refused.is_none());
        let e = resolve(&none, &host("macos", i, r), "codex", "", &ws[..2]);
        assert_eq!((e.profile.as_ref(), e.codex.as_ref(), e.enforcement.as_str()), (None, None, "codex sandbox workspace-write, policy store not writable"));
        if cfg!(unix) {
            assert!(resolve(&none, &host("macos", i, r), "codex", "", &ws).refused.is_none(), "an absolute added dir");
        }
        let up = ["-s".to_string(), "workspace-write".into(), "--add-dir".into(), "/".into()];
        for w in [&up[..], &["-s".into(), "workspace-write".into(), "--add-dir".into(), "..".into()], &["-s".into(), "workspace-write".into(), "-c".into(), "x=1".into()], &["--full-auto".into(), "--yolo".into()], &["-s".into(), "workspace-write".into(), "-C".into(), "/".into()], &["-s".into(), "workspace-write".into(), "--sandbox=danger-full-access".into()]] {
            assert!(resolve(&none, &host("macos", i, r), "codex", "", w).refused.is_some(), "{w:?}");
        }
        assert!(resolve(&none, &host("macos", Path::new("/p/i"), r), "codex", "", &ws[..2]).refused.is_some());
        assert!(resolve(&none, &host("windows", i, r), "codex", "", &ws).refused.unwrap().contains("no operator grant"));
        assert!(resolve(&none, &host("macos", i, r), "codex-app", "", &[]).refused.is_some());
        assert!(resolve(&Err("x".into()), &host("macos", i, r), "claude", "dontAsk", &[]).refused.unwrap().contains("unreadable"));
        // grants: a project grant replaces the host default; full is uncontained everywhere; workspace only where enforced
        let p = Ok(Policy { revision: 4, grants: vec![g("claude", "full", None), g("claude", "workspace", Some("/p")), g("codex", "restricted", None)] });
        let e = resolve(&p, &host("macos", i, r), "claude", "dontAsk", &[]);
        assert_eq!((e.mode.as_str(), e.grant.as_str()), ("workspace", "workspace (project) rev 4 by op"));
        assert!(e.profile.unwrap().contains("(subpath \"/p\")"));
        let e = resolve(&p, &host("macos", i, Path::new("/q")), "claude", "dontAsk", &[]);
        assert_eq!((e.mode.as_str(), e.profile.as_ref(), e.refused.as_ref(), e.claude.as_deref()), ("full", None, None, Some("bypassPermissions")));
        assert_eq!(resolve(&p, &host("windows", i, Path::new("/q")), "claude", "dontAsk", &[]).refused, None);
        // Full turns the runtime sandbox off on start and resume, model and effort kept (A3)
        let full = Ok(Policy { revision: 2, grants: vec![g("codex", "full", None)] });
        let own: Vec<String> = ["-s", "workspace-write", "-m", "gpt-5", "-c", "model_reasoning_effort=high", "-c", "sandbox_mode=read-only", "--full-auto"].map(String::from).into();
        for os in ["macos", "windows", "linux"] {
            let e = resolve(&full, &host(os, i, r), "codex", "", &own);
            assert_eq!((e.codex.unwrap(), e.profile, e.refused), (["-s", "danger-full-access", "-m", "gpt-5", "-c", "model_reasoning_effort=high"].map(String::from).to_vec(), None, None));
        }
        let e = resolve(&p, &host("macos", i, r), "codex", "", &ws);
        assert_eq!(e.codex.unwrap(), ["-s", "danger-full-access", "--add-dir", "/p/.git"]);
        assert!(e.profile.unwrap().contains(&format!("(subpath {})", sb(&r.join(".taskq")))));
        assert!(resolve(&p, &host("linux", i, r), "codex", "", &ws).refused.unwrap().contains("enforcement unknown"));
    }

    // Chain: § 8 Access store. Risk: a grant loses its revision, or a corrupt store is replaced by a grant.
    #[test]
    fn grants_write_with_revision_and_refuse_workers() {
        let install = std::env::temp_dir().join(format!("taskq-policy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&install);
        std::fs::create_dir_all(&install).unwrap();
        let p = grant(&install, "claude", "workspace", None, "op", 5).unwrap();
        assert_eq!((p.revision, p.grants.len()), (1, 1));
        let p = grant(&install, "claude", "none", None, "op", 6).unwrap();
        assert_eq!((p.revision, p.grants.len()), (2, 0));
        assert!(grant(&install, "claude", "admin", None, "op", 7).is_err());
        std::fs::write(install.join(FILE), "{broken").unwrap();
        assert!(grant(&install, "claude", "full", None, "op", 8).is_err());
        assert_eq!(std::fs::read_to_string(install.join(FILE)).unwrap(), "{broken");
        let _ = std::fs::remove_dir_all(&install);
    }

    // Chain: § 8 Access N/N-1 rollback against 4.1.11 (same coreq v0.5.0 launcher). Risk: an unhealthy release rolls
    // back to 4.1.11, which ignores grants and runs legacy flags, or the pin moves `previous` without a grant.
    #[test]
    fn grants_keep_the_launcher_above_the_policy_floor() {
        let install = std::env::temp_dir().join(format!("taskq-floor-{}", std::process::id()));
        // the launcher without the incident hook: a test writes no incident
        let t = coreq::Tool { install: install.clone(), fault: None, ..coreq::Tool::new("taskq", "TaskQ", crate::platform::RELEASE_REPO, crate::platform::VERSION) };
        let new = "4.2.0-dev.1760000000";
        for granted in [false, true] {
            let _ = std::fs::remove_dir_all(&install);
            std::fs::create_dir_all(install.join("releases").join(new)).unwrap();
            std::fs::write(install.join("current"), new).unwrap();
            std::fs::write(install.join("previous"), "4.1.11").unwrap();
            if granted {
                grant(&install, "claude", "workspace", None, "op", 1).unwrap();
            }
            assert_eq!(pin_rollback(&install, new).is_some(), granted);
            let picks: Vec<String> = (0..3).map(|_| coreq::launcher::pick(&t, true).unwrap()).collect();
            assert_eq!(picks[2], if granted { new } else { "4.1.11" }, "granted {granted}");
            assert_eq!(std::fs::read_to_string(install.join("previous")).unwrap().trim(), if granted { new } else { "4.1.11" });
        }
        // a store that reads unknown pins too; a newer previous stays
        std::fs::write(install.join(FILE), "{broken").unwrap();
        std::fs::write(install.join("previous"), "4.1.11").unwrap();
        assert!(pin_rollback(&install, new).is_some());
        std::fs::write(install.join("previous"), "4.2.0-dev.1").unwrap();
        assert!(pin_rollback(&install, new).is_none());
        let _ = std::fs::remove_dir_all(&install);
    }

    // Chain: § 8 Access OS boundary on macOS (A2). Risk: a worker under any non-Full mode writes, replaces or
    // links over worker-policy.json; Workspace cannot write outside its set; Restricted cannot write the root.
    #[cfg(target_os = "macos")]
    #[test]
    fn seatbelt_keeps_workers_out_of_the_policy_store() {
        use std::process::Command;
        // outside the OS temp dir, which every preset may write
        let t = canon(Path::new(env!("CARGO_MANIFEST_DIR"))).join("target").join(format!("seatbelt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&t);
        let (install, root, home, other) = (t.join("install"), t.join("root"), t.join("home"), t.join("other"));
        for d in [&install, &root, &home, &other] {
            std::fs::create_dir_all(d).unwrap();
        }
        let path = install.join(FILE);
        std::fs::write(&path, "{\"revision\": 1}\n").unwrap();
        let run = |profile: &str, script: &str| Command::new("sandbox-exec").args(["-p", profile, "/bin/sh", "-c", script]).env("P", &path).env("I", &install).env("R", &root).env("O", &other).env("T", &t).status().unwrap().success();
        let h = |os| Host { os, install: &install, root: &root, home: &home };
        let none = Ok(Policy::default());
        let legacy = resolve(&none, &h("macos"), "claude", "dontAsk", &[]).profile.unwrap();
        let ws = resolve(&Ok(Policy { revision: 1, grants: vec![g("claude", "workspace", None)] }), &h("macos"), "claude", "dontAsk", &[]).profile.unwrap();
        let ro = resolve(&Ok(Policy { revision: 1, grants: vec![g("claude", "restricted", None)] }), &h("macos"), "claude", "dontAsk", &[]).profile.unwrap();
        for p in [&legacy, &ws, &ro] {
            assert!(!run(p, "echo '{\"revision\":9,\"grants\":[]}' > \"$P\""), "write");
            assert!(!run(p, "x=$(mktemp) && echo x > \"$x\" && mv \"$x\" \"$P\""), "rename over");
            assert!(!run(p, "rm \"$P\""), "delete");
            assert!(!run(p, "echo x > \"$I/new\""), "create in install");
            assert!(!run(p, "ln -s \"$I\" \"$R/l\" 2>/dev/null; echo x > \"$R/l/new\""), "symlink");
            assert!(!run(p, "ln \"$P\" \"$T/h\""), "hard link");
            assert!(run(p, "cat \"$P\" >/dev/null && mktemp >/dev/null"), "read and temp");
        }
        assert!(run(&legacy, "echo x > \"$O/a\"") && !run(&legacy, "mkdir -p \"$O/.codex\" && echo x > \"$O/.codex/config.toml\""));
        assert!(!run(&ws, "mkdir -p \"$R/.codex\"; echo x > \"$R/.codex/config.toml\"") && run(&ws, "echo x > \"$R/.codex/state\""));
        let first = std::env::split_paths(&std::env::var_os("PATH").unwrap()).find(|d| d.is_absolute() && d.is_dir()).unwrap();
        assert!(legacy.contains(&sb(&canon(&first))), "PATH dirs closed");
        assert!(run(&ws, "echo x > \"$R/a\"") && !run(&ws, "echo x > \"$O/a\""));
        assert!(!run(&ro, "echo x > \"$R/b\"") && !run(&ro, "echo x > \"$O/b\""));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\"revision\": 1}\n");
        let _ = std::fs::remove_dir_all(&t);
    }
}
