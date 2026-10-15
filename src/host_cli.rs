//! Host lifecycle and local policy commands (taskq.md § 5, § 8.5).
use crate::{Res, Args, CapEdit, host, platform, resources};
use serde_json::{Value, json};
use std::{fs, path::{Path, PathBuf}, time::Duration};

pub fn lock(install: &Path, name: &str) -> Res<fs::File> {
    fs::create_dir_all(install).map_err(|e| e.to_string())?;
    let f = fs::OpenOptions::new().create(true).truncate(false).write(true).open(install.join(name)).map_err(|e| e.to_string())?;
    f.lock().map_err(|e| e.to_string())?;
    Ok(f)
}

pub fn register(install: &Path, root: &Path, add: bool) -> Res<Vec<PathBuf>> {
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    if root.to_str().is_none_or(|s| s.chars().any(char::is_control)) { return Err("project root must be UTF-8 without control characters".into()); }
    let root = root.as_path();
    let _lock = lock(install, "projects.lock")?;
    let mut roots = if !install.join("projects").exists() && add { vec![] } else { host::projects(install)? };
    if add {
        if !roots.iter().any(|p| p == root) { roots.push(root.to_path_buf()); }
    } else { roots.retain(|r| r != root); }
    let text = roots.iter().map(|r| format!("{}\n", r.display())).collect::<String>();
    let write = || {
        platform::write_atomic(&install.join("projects"), &text)?;
        if fs::read_to_string(install.join("projects")).map_err(|e| e.to_string())? != text { return Err("project registration readback differs".into()); }
        platform::write_atomic(&install.join("poke"), "")
    };
    if add { write()?; } else { resources::unregister(install, root, write)?; }
    Ok(roots)
}

pub fn snapshot(install: &Path) -> Res<Value> {
    let p = install.join("service.json");
    if crate::file_age(&p).is_none_or(|a| a > 60) { return Err("host status missing or stale".into()); }
    let v = crate::data::json(&p)?.ok_or("host status missing")?;
    let pid = pid(install)?.ok_or("host process stopped")?;
    if v["pid"] != pid { return Err("host status belongs to another process".into()); }
    if !v["projects"].is_array() || !v["used"].is_object() || !v["idle"].is_boolean() || !v["drain"].is_boolean() {
        return Err("host status malformed".into());
    }
    Ok(v)
}

// Unlike startup's freshness guard, shutdown must verify the recorded process even after a long pass.
pub fn pid(install: &Path) -> Res<Option<u32>> {
    let p = install.join("service");
    let text = match fs::read_to_string(p) {
        Ok(t) => t, Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None), Err(e) => return Err(e.to_string()),
    };
    let pid = text.trim().parse::<u32>().ok().filter(|p| *p > 0).ok_or("host pid unknown")?;
    Ok(crate::runtime::liveness(pid)?.then_some(pid))
}

pub fn status(install: &Path) -> Res<()> {
    match snapshot(install) {
        Ok(v) => {
            println!("Service: running (host, pid {})", v["pid"]);
            if let Some(reason) = v["reason"].as_str() { println!("Status: unknown ({reason})"); }
            for p in v["projects"].as_array().ok_or("host project status unknown")? {
                let root = Path::new(p["root"].as_str().ok_or("project root unknown")?);
                println!("{}", crate::service_fields(root));
                if p["draining"] == true { println!("Draining: {}", crate::project_name(root)); }
            }
        }
        Err(e) => {
            println!("Service: unknown ({e})");
            for root in host::projects(install)? { println!("{}", crate::service_fields(&root)); }
        }
    }
    Ok(())
}

/// A failed/unknown observation cannot authorize stopping a unit that may still own children.
fn drained(v: &Value, expected: u32) -> bool {
    v["pid"] == expected && v["drain"] == true && v["idle"] == true && v["reason"].is_null()
}

pub fn wait_idle(install: &Path) -> Res<Option<u32>> {
    platform::write_atomic(&install.join("drain"), "host lifecycle\n")?;
    platform::write_atomic(&install.join("poke"), "")?;
    let Some(p) = pid(install)? else {
        let v = crate::data::json(&install.join("service.json"))?.ok_or("host drain unknown: no acknowledgement; unit kept")?;
        if v["drain"] != true || v["idle"] != true || !v["reason"].is_null() { return Err("host drain unknown; unit kept".into()); }
        return Ok(None);
    };
    for _ in 0..30 {
        if let Ok(v) = snapshot(install) { if drained(&v, p) { return Ok(Some(p)); } }
        std::thread::sleep(Duration::from_secs(1));
    }
    Err("host drain pending or unknown; unit kept, retry the same command".into())
}

/// Stop only an idle process; verify that exact pid exited before removing the unit (§ 8.5).
pub fn stop_idle(install: &Path, u: &platform::Unit, p: Option<u32>) -> Res<()> {
    if p.is_some() { platform::run_all(&u.stop, false)?; }
    #[cfg(windows)]
    if let Some(p) = p {
        platform::run_all(&[vec!["taskkill".into(), "/F".into(), "/T".into(), "/FI".into(), format!("PID eq {p}"), "/FI".into(), "IMAGENAME eq taskq.exe".into()]], false)?;
    }
    if let Some(p) = p {
        for _ in 0..30 {
            if !crate::runtime::liveness(p)? { return Ok(()); }
            std::thread::sleep(Duration::from_secs(1));
        }
        return Err("host process exit unverified; unit kept".into());
    }
    // No daemon does not establish absence of a legacy owner of a registered root.
    for root in host::projects(install)? {
        if host::project_service(&root)?.is_some() { return Err("legacy process still running; unit kept".into()); }
    }
    Ok(())
}

pub fn lifecycle(install: &Path, sub: &str, root: Option<&Path>, all: bool) -> Res<()> {
    let _op = lock(install, "service.lock")?;
    let u = platform::unit(&platform::tool(install), install, "");
    match sub {
        "restart" => {
            platform::write_atomic(&install.join("restart"), "")?;
            platform::write_atomic(&install.join("poke"), "")?;
            println!("Restart requested for host; every registered project");
        }
        "install" => {
            let root = root.ok_or("install needs a project root")?;
            crate::load(root)?;
            let off = crate::install_focus(root)?;
            platform::install_self(&platform::tool(install))?;
            register(install, root, true)?;
            // Explicit install ends an earlier completed or interrupted lifecycle drain.
            if install.join("drain").exists() { fs::remove_file(install.join("drain")).map_err(|e| e.to_string())?; }
            let changed = match fs::read(&u.path) {
                Ok(bytes) => bytes != unit_bytes(&u.text),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => true,
                Err(e) => return Err(format!("host unit unknown: {e}")),
            };
            if changed {
                platform::write_unit(&u)?;
                if crate::load_unit(&u, install)? {
                    println!("Registered; host unit reload deferred until children end");
                    return Ok(());
                }
            } else if unit_reload_pending(install)? {
                platform::write_atomic(&install.join("restart"), "unit\n")?;
                platform::write_atomic(&install.join("poke"), "")?;
                println!("Registered; host unit reload deferred until children end");
                return Ok(());
            } else if pid(install)?.is_none() { platform::run_all(&u.load, false)?; }
            platform::write_atomic(&install.join("poke"), "")?;
            for _ in 0..20 {
                if let Some(pid) = pid(install)? {
                    let p = crate::dot(root).join("status.json");
                    if crate::file_age(&p).is_some_and(|a| a <= 60) && crate::data::json(&p)?.is_some_and(|s| s["pid"] == pid) {
                        pin_topology(install, platform::VERSION)?;
                        println!("Installed {} on host", crate::project_name(root));
                        if off { println!("Focus: off; nothing admitted until `taskq service focus all|<label..>`"); }
                        return Ok(());
                    }
                }
                std::thread::sleep(Duration::from_secs(1));
            }
            println!("Registered {}; waiting for host service readback", crate::project_name(root));
        }
        "uninstall" | "stop" => {
            if sub == "uninstall" && !all {
                let roots = register(install, root.ok_or("uninstall needs a project root or --host")?, false)?;
                if !roots.is_empty() { println!("Project unregistered; existing attempts drain on host"); return Ok(()); }
            }
            let p = wait_idle(install)?;
            stop_idle(install, &u, p)?;
            if sub == "uninstall" {
                remove_unit(install, &u)?;
            }
            fs::remove_file(install.join("drain")).map_err(|e| e.to_string())?;
            println!("Host service {}; every project{}", if sub == "stop" { "stopped" } else { "uninstalled" }, if sub == "stop" { "; starts at next logon" } else { "" });
        }
        _ => return Err("unknown host lifecycle command".into()),
    }
    Ok(())
}

// Match coreq's on-disk encoding without interpreting a UTF-16 XML file as UTF-8.
fn unit_bytes(text: &str) -> Vec<u8> {
    if cfg!(windows) {
        std::iter::once(0xFEFFu16).chain(text.encode_utf16()).flat_map(u16::to_le_bytes).collect()
    } else { text.as_bytes().to_vec() }
}

pub fn unit_reload_pending(install: &Path) -> Res<bool> {
    match fs::read_to_string(install.join("restart")) {
        Ok(s) => match s.trim() {
            "" => Ok(false), "unit" => Ok(true),
            _ => Err("host restart request unknown; request retained".into()),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(format!("host restart request unknown: {e}")),
    }
}

/// Native managers own the helper, so stopping the host cannot stop its reloader.
pub fn dispatch_unit_reload(install: &Path) -> Res<()> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?.display().to_string();
    #[cfg(windows)]
    let cmd = {
        let line = format!("\"{exe}\" service reload-unit").replace('\'', "''");
        vec!["powershell.exe".into(), "-NoProfile".into(), "-NonInteractive".into(), "-Command".into(),
            format!("$r = Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{{CommandLine='{line}';CurrentDirectory='{}'}}; if ($r.ReturnValue -ne 0) {{ exit 1 }}", install.display().to_string().replace('\'', "''"))]
    };
    #[cfg(target_os = "macos")]
    let cmd = {
        // A completed submit job keeps its label; an explicit retry replaces that job.
        platform::run_all(&[vec!["launchctl".into(), "remove".into(), "com.taskq.unit-reload".into()]], true)?;
        vec!["launchctl".into(), "submit".into(), "-l".into(), "com.taskq.unit-reload".into(), "--".into(), exe, "service".into(), "reload-unit".into()]
    };
    #[cfg(not(any(windows, target_os = "macos")))]
    let cmd = vec!["systemd-run".into(), "--user".into(), "--collect".into(), "--unit=taskq-unit-reload".into(), exe, "service".into(), "reload-unit".into()];
    let _ = install;
    platform::run_all(&[cmd], false)
}

/// Internal helper: failure leaves the request and drain intact for a repeated install.
pub fn reload_unit(install: &Path) -> Res<()> {
    let u = platform::unit(&platform::tool(install), install, "");
    reload_unit_with(install, &u, unit_loaded)
}

fn reload_unit_with(install: &Path, u: &platform::Unit, verify: impl Fn() -> Res<()>) -> Res<()> {
    let _op = lock(install, "service.lock")?;
    if !unit_reload_pending(install)? { return Ok(()); }
    let p = wait_idle(install)?;
    stop_idle(install, u, p)?;
    platform::run_all(&u.unload, true)?;
    // Admission stays disabled until native registration and the new process read back.
    platform::run_all(&u.load, false)?;
    verify()?;
    for _ in 0..30 {
        if let Ok(v) = snapshot(install) {
            if reloaded(&v, p) {
                fs::remove_file(install.join("restart")).map_err(|e| e.to_string())?;
                fs::remove_file(install.join("drain")).map_err(|e| e.to_string())?;
                platform::write_atomic(&install.join("poke"), "")?;
                return Ok(());
            }
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    Err("host unit reloaded; new host ownership unknown, drain retained".into())
}

fn reloaded(v: &Value, previous: Option<u32>) -> bool {
    v["pid"].as_u64().is_some_and(|pid| pid > 0 && Some(pid) != previous.map(u64::from))
        && v["drain"] == true && v["idle"] == true && v["reason"].is_null()
}

fn unit_loaded() -> Res<()> {
    #[cfg(windows)]
    let cmd = vec!["schtasks".into(), "/Query".into(), "/TN".into(), "TaskQ".into(), "/XML".into()];
    #[cfg(target_os = "macos")]
    let cmd = {
        let id = platform::quiet(std::process::Command::new("id").arg("-u")).output().map_err(|e| e.to_string())?;
        if !id.status.success() { return Err("host unit readback: user unknown".into()); }
        vec!["launchctl".into(), "print".into(), format!("gui/{}/com.taskq.service", String::from_utf8_lossy(&id.stdout).trim())]
    };
    #[cfg(not(any(windows, target_os = "macos")))]
    let cmd = vec!["systemctl".into(), "--user".into(), "is-active".into(), "taskq.service".into()];
    platform::run_all(&[cmd], false)
}

/// Registry and exact process readbacks precede every destructive host-unit action.
pub fn remove_unit(install: &Path, u: &platform::Unit) -> Res<()> {
    remove_unit_with(install, u, || unit_unloaded(install))
}

pub(crate) fn remove_unit_with(install: &Path, u: &platform::Unit, verify: impl Fn() -> Res<()>) -> Res<()> {
    let roots = host::projects(install)?;
    if pid(install)?.is_some() { return Err("host still running; unit kept".into()); }
    for root in roots {
        if host::project_service(&root)?.is_some() { return Err("legacy owner still running; unit kept".into()); }
    }
    // Stop may already have unloaded it. Native readback, not a second unload's exit code, decides absence.
    platform::run_all(&u.unload, true)?;
    verify()?;
    if let Err(e) = fs::remove_file(&u.path) { if e.kind() != std::io::ErrorKind::NotFound { return Err(e.to_string()); } }
    verify()?;
    if u.path.try_exists().map_err(|e| e.to_string())? || pid(install)?.is_some() { return Err("host unit removal readback differs".into()); }
    Ok(())
}

fn unit_unloaded(_install: &Path) -> Res<()> {
    #[cfg(not(any(target_os = "macos", windows)))]
    platform::run_all(&[vec!["systemctl".into(), "--user".into(), "daemon-reload".into()]], false)?;
    #[cfg(target_os = "macos")]
    let (cmd, name) = {
        let id = platform::quiet(std::process::Command::new("id").arg("-u")).output().map_err(|e| e.to_string())?;
        if !id.status.success() { return Err("host unit readback: user unknown".into()); }
        (vec!["launchctl".into(), "print".into(), format!("gui/{}", String::from_utf8_lossy(&id.stdout).trim())], "com.taskq.service")
    };
    #[cfg(windows)]
    let (cmd, name): (Vec<String>, &str) = (vec!["schtasks".into(), "/Query".into(), "/FO".into(), "CSV".into(), "/NH".into()], r#"\TaskQ""#);
    #[cfg(not(any(target_os = "macos", windows)))]
    let (cmd, name): (Vec<String>, &str) = (vec!["systemctl".into(), "--user".into(), "list-unit-files".into(), "--no-legend".into(), "--no-pager".into(), "taskq.service".into()], "taskq.service");
    let o = platform::quiet(std::process::Command::new(&cmd[0]).args(&cmd[1..])).output().map_err(|e| e.to_string())?;
    if !o.status.success() { return Err("host unit readback failed; removal unverified".into()); }
    let out = String::from_utf8(o.stdout).map_err(|e| e.to_string())?;
    #[cfg(not(any(target_os = "macos", windows)))]
    if out.lines().any(|l| l.split_whitespace().next() == Some(name) && !l.split_whitespace().any(|s| s == "disabled")) {
        return Err("host unit still enabled; removal unverified".into());
    }
    #[cfg(any(target_os = "macos", windows))]
    if out.contains(name) { return Err("host unit still registered; removal unverified".into()); }
    Ok(())
}

/// The first verified installation pins its release; later installs must not raise the topology floor.
fn pin_topology(install: &Path, version: &str) -> Res<()> {
    let floor = match fs::read_to_string(install.join("rollback-floor")) {
        Ok(s) => s, Err(e) if e.kind() == std::io::ErrorKind::NotFound => version.into(), Err(e) => return Err(e.to_string()),
    };
    coreq::launcher::set_rollback_floor(&platform::tool(install), floor.trim())
}

pub fn edit_policy(policy: &host::Policy, value: &Value, e: &CapEdit, project: bool) -> Res<Value> {
    if e.mode.is_some() { return Err("--mode removed; local host policy has no Fixed mode".into()); }
    if project && (e.max.is_some() || e.gate.is_some() || e.cpu_max.is_some() || e.memory_min.is_some()) {
        return Err("--project accepts only lowering runtime limits".into());
    }
    let mut v = value.clone();
    for (rt, n) in &e.limits { v["limits"][rt] = json!(n); }
    for rt in &e.unset { if let Some(l) = v["limits"].as_object_mut() { l.remove(rt); } }
    if let Some(max) = e.max {
        if !v["admission"].is_object() { v["admission"] = json!({}); }
        if let Some(n) = max { v["admission"]["max"] = json!(n); } else { v["admission"].as_object_mut().unwrap().remove("max"); }
    }
    if let Some(x) = e.gate { v["load_gate"]["enabled"] = json!(x); }
    if let Some(x) = e.cpu_max { v["load_gate"]["cpu_max"] = json!(x); }
    if let Some(x) = e.memory_min { v["load_gate"]["memory_min"] = json!(x); }
    if project { policy.project(&v)?; } else { host::Policy::parse(v.clone(), &crate::hostname())?; }
    Ok(v)
}

pub fn select(install: &Path, name: &str) -> Res<PathBuf> {
    let roots = host::projects(install)?;
    let path = Path::new(name).canonicalize().ok();
    let matches: Vec<_> = roots.into_iter().filter(|r| Some(r) == path.as_ref() || crate::project_name(r) == name).collect();
    if matches.len() != 1 { return Err("--project must select exactly one registered root or name".into()); }
    Ok(matches[0].clone())
}

pub fn capacity(install: &Path, a: &Args) -> Res<()> {
    if a.flags.contains_key("host") || a.flags.contains_key("mode") { return Err("--host and --mode removed; capacity edits local host.json".into()); }
    host::Policy::read(install)?;
    let project = a.one("project").map(|p| select(install, p)).transpose()?;
    let e = CapEdit::from_args(a)?;
    let path = project.as_ref().map_or_else(|| install.join("host.json"), |p| p.join("taskq.json"));
    let mut want: Value = serde_json::from_str(&fs::read_to_string(&path).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    if !e.is_empty() {
        let _lock = lock(install, "host-policy.lock")?;
        let before = fs::read_to_string(&path).map_err(|e| e.to_string())?;
        let original: Value = serde_json::from_str(&before).map_err(|e| e.to_string())?;
        let policy = host::Policy::read(install)?;
        want = edit_policy(&policy, &original, &e, project.is_some())?;
        if project.is_none() {
            let next = host::Policy::parse(want.clone(), &crate::hostname())?;
            for p in host::projects(install)? {
                let v = crate::data::json(&p.join("taskq.json"))?.ok_or("project configuration unavailable")?;
                next.project(&v)?;
            }
        }
        if fs::read_to_string(&path).map_err(|e| e.to_string())? != before { return Err("policy changed while saving; nothing written".into()); }
        platform::write_atomic(&path, &(serde_json::to_string_pretty(&want).map_err(|e| e.to_string())? + "\n"))?;
        if crate::data::json(&path)? != Some(want.clone()) { return Err("policy readback differs".into()); }
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|e| e.to_string())?.as_secs();
        for root in host::projects(install)? {
            if project.as_ref().is_none_or(|p| p == &root) { resources::revised(install, &root, now)?; crate::poke(&root); }
        }
        platform::write_atomic(&install.join("poke"), "")?;
        println!("Saved; waiting for service; running tasks continue");
    }
    println!("Host limits: {}", host::Policy::read(install)?.cap.json()["limits"]);
    if let Some(root) = &project { println!("Project {} caps: {}", crate::project_name(root), want["limits"]); }
    match snapshot(install) {
        Ok(s) => {
            println!("Host used: {}", s["used"]);
            for p in s["projects"].as_array().into_iter().flatten() {
                if let Some(root) = p["root"].as_str() {
                    if let Ok(Some(st)) = crate::data::json(&crate::dot(Path::new(root)).join("status.json")) {
                        if st["pid"] == s["pid"] && crate::file_age(&crate::dot(Path::new(root)).join("status.json")).is_some_and(|a| a <= 60) {
                            println!("Load gate: {}", crate::gate_text(&st["capacity"])); break;
                        }
                    }
                }
            }
            let applied = if let Some(root) = project {
                s["projects"].as_array().is_some_and(|ps| ps.iter().any(|p| p["root"] == root.display().to_string() && p["known"] == true && p["limits"] == want["limits"]))
            } else { s["policy"] == want };
            if applied && !e.is_empty() { println!("Applied; running tasks continue"); }
        }
        Err(e) => println!("Host use: unknown ({e})"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    // Chain: § 8.5 uninstall -> drain readback -> exact-pid exit -> unit removal. Unknown must retain the unit.
    #[test]
    fn lifecycle_requires_exact_drained_process() {
        let mut v = json!({"pid": 42, "drain": true, "idle": true, "reason": null});
        assert!(drained(&v, 42));
        assert!(!drained(&v, 43));
        v["idle"] = json!(false); assert!(!drained(&v, 42));
        v["idle"] = json!(true); v["reason"] = json!("liveness unknown"); assert!(!drained(&v, 42));
    }
    // Chain: repeat install -> encoded unit comparison -> deferred reload -> new drained owner acknowledgement.
    #[test]
    fn native_unit_encoding_and_reload_acknowledgement() {
        let install = std::env::temp_dir().join(format!("taskq-unit-reload-{}", std::process::id()));
        fs::create_dir_all(&install).unwrap();
        let u = platform::Unit { path: install.join("unit"), text: "unit: café".into(), load: vec![], unload: vec![], stop: vec![] };
        platform::write_unit(&u).unwrap();
        assert_eq!(fs::read(&u.path).unwrap(), unit_bytes(&u.text));
        assert!(!unit_reload_pending(&install).unwrap());
        fs::write(install.join("restart"), "unit\n").unwrap();
        assert!(unit_reload_pending(&install).unwrap());
        fs::write(install.join("restart"), "unknown\n").unwrap();
        assert!(unit_reload_pending(&install).is_err(), "unknown requests must be retained");
        fs::write(install.join("restart"), [0xff]).unwrap();
        assert!(unit_reload_pending(&install).is_err(), "unreadable request must not become a plain restart");
        let mut v = json!({"pid":42,"drain":true,"idle":true,"reason":null});
        assert!(!reloaded(&v, Some(42)), "the old process cannot acknowledge a native reload");
        v["pid"] = json!(43); assert!(reloaded(&v, Some(42)));
        v["idle"] = json!(false); assert!(!reloaded(&v, Some(42)));
        v["idle"] = json!(true); v["reason"] = json!("unknown"); assert!(!reloaded(&v, Some(42)));
        v["reason"] = json!(null); v["pid"] = json!(null); assert!(!reloaded(&v, Some(42)));
        fs::remove_dir_all(install).unwrap();
    }
    // Chain: deferred native reload -> unload/load commands -> registration and new ownership -> clear request.
    #[test]
    fn deferred_reload_keeps_requests_until_native_and_process_readback() {
        fn copy(from: &Path, to: &Path) -> Vec<String> {
            #[cfg(windows)]
            let mut cmd: Vec<String> = ["cmd.exe", "/d", "/c", "copy", "/Y"].map(Into::into).into();
            #[cfg(not(windows))]
            let mut cmd = vec!["cp".into()];
            cmd.extend([from.display().to_string(), to.display().to_string()]); cmd
        }
        for verified in [false, true] {
            let install = std::env::temp_dir().join(format!("taskq-reload-chain-{}-{verified}", std::process::id()));
            fs::create_dir_all(&install).unwrap();
            fs::write(install.join("projects"), "").unwrap();
            fs::write(install.join("restart"), "unit\n").unwrap();
            fs::write(install.join("service.json"), r#"{"drain":true,"idle":true,"reason":null}"#).unwrap();
            fs::write(install.join("new-pid"), std::process::id().to_string()).unwrap();
            fs::write(install.join("new-status"), json!({"pid":std::process::id(),"projects":[],"used":{},"drain":true,"idle":true,"reason":null}).to_string()).unwrap();
            fs::write(install.join("unloaded"), "unloaded").unwrap();
            let u = platform::Unit { path: install.join("unit"), text:String::new(), stop:vec![],
                unload:vec![copy(&install.join("unloaded"), &install.join("unit"))],
                load:vec![copy(&install.join("new-pid"), &install.join("service")), copy(&install.join("new-status"), &install.join("service.json"))] };
            let result = reload_unit_with(&install, &u, || {
                assert_eq!(fs::read_to_string(&u.path).unwrap(), "unloaded");
                assert!(install.join("drain").exists() && install.join("restart").exists());
                if verified { Ok(()) } else { Err("native registration unknown".into()) }
            });
            assert_eq!(result.is_ok(), verified);
            assert_eq!(install.join("restart").exists(), !verified);
            assert_eq!(install.join("drain").exists(), !verified);
            assert_eq!(fs::read_to_string(install.join("projects")).unwrap(), "");
            fs::remove_dir_all(install).unwrap();
        }
    }
    // Chain: § 5 host policy -> project cap. Reject raised ceilings and project gate edits without dropping unknown keys.
    #[test]
    fn local_policy_and_lowering_caps() {
        let p = host::Policy::parse(json!({"limits":{"codex":2},"load_gate":{"enabled":false},"custom":7}), "fake").unwrap();
        let mut e = CapEdit::default(); e.limits.insert("codex".into(), 1);
        let v = edit_policy(&p, &json!({"repo":"invented/project", "other":9}), &e, true).unwrap();
        assert_eq!(v["other"], 9); assert_eq!(v["limits"]["codex"], 1);
        e.limits.insert("codex".into(), 3); assert!(edit_policy(&p, &v, &e, true).is_err());
        e.limits.clear(); e.gate = Some(true); assert!(edit_policy(&p, &v, &e, true).is_err());
        let v = edit_policy(&p, &p.raw, &e, false).unwrap(); assert_eq!(v["custom"], 7);
    }
    // Chain: A2 two registrations -> either project removal order -> last-unit removal; --host retains roots.
    // Risk: dropping another root, lost concurrent registration, or deleting the unit before native readback.
    #[test]
    fn registration_and_host_removal_in_both_orders() {
        for order in [[0, 1], [1, 0]] {
            let install = std::env::temp_dir().join(format!("taskq-host-lifecycle-{}-{}", std::process::id(), order[0]));
            let _ = fs::remove_dir_all(&install);
            let roots = [install.join("a"), install.join("b")];
            for r in &roots { fs::create_dir_all(r).unwrap(); }
            std::thread::scope(|s| {
                for r in &roots { s.spawn(|| { register(&install, r, true).unwrap(); }); }
            });
            let u = platform::Unit { path: install.join("unit"), text: "invented unit".into(), load: vec![], unload: vec![], stop: vec![] };
            fs::write(&u.path, &u.text).unwrap();
            assert_eq!(host::projects(&install).unwrap().len(), 2);
            let original = fs::read(install.join("projects")).unwrap();
            assert!(remove_unit_with(&install, &u, || Err("unknown native state".into())).is_err());
            assert!(u.path.exists()); assert_eq!(fs::read(install.join("projects")).unwrap(), original);
            // Explicit host uninstall never drops project registration.
            remove_unit_with(&install, &u, || Ok(())).unwrap();
            assert!(!u.path.exists()); assert_eq!(fs::read(install.join("projects")).unwrap(), original);
            fs::write(&u.path, &u.text).unwrap();
            assert_eq!(register(&install, &roots[order[0]], false).unwrap().len(), 1);
            assert!(u.path.exists());
            assert!(register(&install, &roots[order[1]], false).unwrap().is_empty());
            let checks = std::cell::Cell::new(0);
            remove_unit_with(&install, &u, || {
                assert_eq!(u.path.exists(), checks.get() == 0, "verify unloaded before delete, then verify removal");
                checks.set(checks.get() + 1); Ok(())
            }).unwrap();
            assert_eq!(checks.get(), 2); assert!(!u.path.exists());
            fs::remove_dir_all(install).unwrap();
        }
    }

    // Chain: A3 rootless capacity -> host.json -> selected project cap -> refused raise, with files read back.
    #[test]
    fn rootless_capacity_edits_only_selected_policy() {
        let install = std::env::temp_dir().join(format!("taskq-host-capacity-{}", std::process::id()));
        let _ = fs::remove_dir_all(&install);
        let root = install.join("invented-project");
        fs::create_dir_all(&root).unwrap();
        let policy = json!({"limits":{"codex":2},"load_gate":{"enabled":false},"custom":"keep"});
        fs::write(install.join("host.json"), policy.to_string()).unwrap();
        fs::write(root.join("taskq.json"), r#"{"repo":"invented/project","keep":7}"#).unwrap();
        register(&install, &root, true).unwrap();
        let args = |flags: &[&str]| Args::parse(&[&["service", "capacity"][..], flags].concat().into_iter().map(String::from).collect::<Vec<_>>()).unwrap();
        capacity(&install, &args(&[])).unwrap();
        capacity(&install, &args(&["--codex", "3"])).unwrap();
        assert_eq!(crate::data::json(&install.join("host.json")).unwrap().unwrap()["limits"]["codex"], 3);
        capacity(&install, &args(&["--project", "invented-project", "--codex", "1"])).unwrap();
        let before = fs::read(root.join("taskq.json")).unwrap();
        assert_eq!(crate::data::json(&root.join("taskq.json")).unwrap().unwrap()["keep"], 7);
        for flags in [vec!["--project", "invented-project", "--codex", "4"], vec!["--project", "invented-project", "--max", "1"], vec!["--mode", "fixed"], vec!["--host", "other"]] {
            assert!(capacity(&install, &args(&flags)).is_err());
            assert_eq!(fs::read(root.join("taskq.json")).unwrap(), before);
        }
        assert_eq!(crate::data::json(&install.join("host.json")).unwrap().unwrap()["custom"], "keep");
        fs::remove_dir_all(install).unwrap();
    }

    // Chain: § 8.5 first host ownership -> rollback floor -> later host install. Risk: each update disables valid rollback.
    #[test]
    fn later_host_install_retains_the_first_floor() {
        let install = std::env::temp_dir().join(format!("taskq-host-floor-{}", std::process::id()));
        let _ = fs::remove_dir_all(&install); fs::create_dir_all(&install).unwrap();
        fs::write(install.join("current"), "10.0.0").unwrap();
        pin_topology(&install, "10.0.0").unwrap();
        fs::write(install.join("current"), "11.0.0").unwrap();
        pin_topology(&install, "11.0.0").unwrap();
        assert_eq!(fs::read_to_string(install.join("rollback-floor")).unwrap(), "10.0.0");
        fs::write(install.join("rollback-floor"), "invalid/floor").unwrap();
        assert!(pin_topology(&install, "11.0.0").is_err());
        assert_eq!(fs::read_to_string(install.join("rollback-floor")).unwrap(), "invalid/floor");
        fs::remove_dir_all(install).unwrap();
    }

}
