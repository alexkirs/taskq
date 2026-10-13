//! taskq on coreq: install layout, self-update, launcher and OS units come from the shared crate (coreq.md); this file
//! keeps taskq's own parts: the error format, the install sweep and the load readings (taskq.md § 3, § 8).

use std::fs;
use std::path::{Path, PathBuf};

pub use coreq::service::{Unit, run_all, unit, write_unit};
pub use coreq::{Tool, quiet, write_atomic};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Built in, never from config: project files are written by workers, so config must not choose the code hosts run.
pub const RELEASE_REPO: &str = "taskqorg/taskq";

/// taskq as a coreq tool installed in `install`.
pub fn tool(install: &Path) -> Tool {
    Tool { install: install.into(), fault: Some(crate::incident::launcher), ..Tool::new("taskq", "TaskQ", RELEASE_REPO, VERSION) }
}

pub fn default_install() -> PathBuf {
    Tool::new("taskq", "TaskQ", RELEASE_REPO, VERSION).install
}

pub fn die(msg: &str) -> ! {
    eprintln!("{}", error_text(msg, crate::find_root().ok().as_deref()));
    std::process::exit(1);
}

pub fn error_text(msg: &str, root: Option<&Path>) -> String {
    let next = if msg.chars().count() > 153 || msg.contains(['\r', '\n']) {
        use std::io::Write;
        let saved = root.and_then(|root| {
            let d = crate::dot(root);
            fs::create_dir_all(&d).ok()?;
            let p = d.join("cli-errors.log");
            writeln!(fs::OpenOptions::new().create(true).append(true).open(&p).ok()?, "{msg}").ok()?;
            Some(p)
        });
        match saved {
            Some(p) => format!("Read `{}`; use `taskq help` for syntax and retry.", p.display()),
            None => return format!("Error: {msg}\nNext: Diagnostic could not be saved; check `taskq help` and retry."),
        }
    } else { "Check the input and board/service status, then retry; use `taskq help` for syntax.".into() };
    format!("Error: {}\nNext: {next}", crate::compact(msg, 153))
}

/// § 8 update steps (coreq), then the sweep: Ok(Some(version)) when a new release became current; with `check`, when
/// one is available.
pub fn update(install: &Path, check: bool) -> Result<Option<String>, String> {
    let v = update_result(coreq::update::update(&tool(install), check))?;
    if v.is_some() && !check {
        let shim = std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/bin/taskq")).unwrap_or_default();
        sweep(install, &shim);
    }
    Ok(v)
}

fn update_result(result: Result<Option<String>, String>) -> Result<Option<String>, String> {
    if let Err(e) = &result {
        if let Some((release, _)) = e.split_once(" selftest failed:") {
            let field = format!("release:{release}");
            crate::incident::detect("update.smoke", concat!("src/platform.rs:", line!()), "none", &[&field], &["smoke:failed"]);
        }
    }
    result
}

/// § 8 step 2 sweep: everything in `<install>` the current version does not own, and the v3 shim. Best effort: a file
/// in use (a running Windows exe) stays until a later update.
pub fn sweep(install: &Path, shim: &Path) {
    let read = |f: &str| fs::read_to_string(install.join(f)).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let mut keep: Vec<String> = [read("current"), read("previous"), Some(VERSION.into())].into_iter().flatten().collect();
    for root in fs::read_to_string(install.join("projects")).unwrap_or_default().lines().filter(|l| !l.is_empty()) {
        let st: serde_json::Value = fs::read(Path::new(root).join(".taskq/status.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        keep.extend(st["version"].as_str().map(String::from));
    }
    let owned = |n: &str| {
        ["releases", "current", "previous", "projects", "token", "taskq", "taskq.exe", "taskq-tray.exe", "capacity"].contains(&n)
            || n.ends_with(".log") || n.ends_with(".tmp") || n.starts_with("tray") || (n.starts_with("TaskQ-") && n.ends_with(".xml"))
    };
    let gone = |p: &Path| {
        let _ = if p.is_dir() && !p.is_symlink() { fs::remove_dir_all(p) } else { fs::remove_file(p) };
    };
    for e in fs::read_dir(install).into_iter().flatten().flatten() {
        if !owned(&e.file_name().to_string_lossy()) {
            gone(&e.path());
        }
    }
    for e in fs::read_dir(install.join("releases")).into_iter().flatten().flatten() {
        let n = e.file_name().to_string_lossy().to_string();
        if !n.ends_with(".tmp") && !keep.contains(&n) {
            gone(&e.path());
        }
    }
    if fs::read_to_string(shim).is_ok_and(|t| t.contains("taskq.py launch") && t.contains(&install.display().to_string())) {
        gone(shim);
    }
}

// ---------- host load for the gate (§ 3): native APIs, no executables ----------

/// `/proc/stat` aggregate `cpu` line: (busy, total) ticks; idle includes iowait, guest ticks are already in user/nice.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn proc_stat(text: &str) -> Option<(u64, u64)> {
    let f: Vec<u64> = text.lines().next()?.strip_prefix("cpu ")?.split_whitespace().take(8).map(|x| x.parse().ok()).collect::<Option<_>>()?;
    let total: u64 = f.iter().sum();
    Some((total - f.get(3)? - f.get(4).unwrap_or(&0), total))
}

/// `/proc/meminfo`: `MemAvailable / MemTotal` in %; no `MemAvailable` is unknown.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn meminfo(text: &str) -> Option<f64> {
    let kb = |k: &str| text.lines().find_map(|l| l.strip_prefix(k)?.trim().strip_suffix("kB")?.trim().parse::<f64>().ok());
    let (avail, total) = (kb("MemAvailable:")?, kb("MemTotal:")?);
    (total > 0.0 && avail <= total).then(|| avail * 100.0 / total)
}

#[cfg(target_os = "linux")]
pub fn cpu_ticks() -> Option<(u64, u64)> {
    proc_stat(&fs::read_to_string("/proc/stat").ok()?)
}

#[cfg(target_os = "linux")]
pub fn memory_available() -> Option<f64> {
    meminfo(&fs::read_to_string("/proc/meminfo").ok()?)
}

#[cfg(target_os = "macos")]
mod mach {
    unsafe extern "C" {
        pub fn mach_host_self() -> u32;
        pub fn host_statistics(host: u32, flavor: i32, info: *mut i32, count: *mut u32) -> i32;
        pub fn host_statistics64(host: u32, flavor: i32, info: *mut i32, count: *mut u32) -> i32;
        pub fn host_page_size(host: u32, size: *mut usize) -> i32;
    }

    /// One send right for the life of the process.
    pub fn host() -> u32 {
        static H: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
        // SAFETY: no arguments; returns this task's host port
        *H.get_or_init(|| unsafe { mach_host_self() })
    }
}

/// `HOST_CPU_LOAD_INFO`: user, system, idle, nice ticks over all logical CPUs.
#[cfg(target_os = "macos")]
pub fn cpu_ticks() -> Option<(u64, u64)> {
    let (mut t, mut n) = ([0u32; 4], 4u32);
    // SAFETY: flavor 3 fills CPU_STATE_MAX natural_t values into a buffer of that size
    if unsafe { mach::host_statistics(mach::host(), 3, t.as_mut_ptr().cast(), &mut n) } != 0 || n != 4 {
        return None;
    }
    let total: u64 = t.iter().map(|x| *x as u64).sum();
    Some((total - t[2] as u64, total))
}

/// `HOST_VM_INFO64` (free + inactive pages) × page size / `hw.memsize`. `ponytail:` free + inactive approximates
/// reclaimable memory, not Apple's pressure metric; compressed and purgeable pages are not counted again.
#[cfg(target_os = "macos")]
pub fn memory_available() -> Option<f64> {
    let (mut v, mut n, mut page) = ([0i32; 38], 38u32, 0usize);
    let (mut total, mut len) = (0u64, std::mem::size_of::<u64>());
    // SAFETY: flavor 4 fills HOST_VM_INFO64_COUNT (38) integer_t; the size and memsize outputs are plain integers
    let ok = unsafe {
        mach::host_statistics64(mach::host(), 4, v.as_mut_ptr(), &mut n) == 0
            && mach::host_page_size(mach::host(), &mut page) == 0
            && libc::sysctlbyname(c"hw.memsize".as_ptr(), (&mut total as *mut u64).cast(), &mut len, std::ptr::null_mut(), 0) == 0
    };
    let avail = (v[0] as u32 as u64 + v[2] as u32 as u64) * page as u64;
    (ok && total > 0 && avail <= total).then(|| avail as f64 * 100.0 / total as f64)
}

#[cfg(windows)]
mod win {
    #[repr(C)]
    pub struct MemoryStatusEx {
        pub length: u32,
        pub load: u32,
        pub total_phys: u64,
        pub avail_phys: u64,
        pub rest: [u64; 5],
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        pub fn GetSystemTimes(idle: *mut u64, kernel: *mut u64, user: *mut u64) -> i32;
        pub fn GlobalMemoryStatusEx(m: *mut MemoryStatusEx) -> i32;
    }
}

/// `GetSystemTimes`: kernel time already includes idle.
#[cfg(windows)]
pub fn cpu_ticks() -> Option<(u64, u64)> {
    let (mut idle, mut kernel, mut user) = (0u64, 0u64, 0u64);
    // SAFETY: three FILETIME outputs, each the layout of a little-endian u64
    if unsafe { win::GetSystemTimes(&mut idle, &mut kernel, &mut user) } == 0 {
        return None;
    }
    let total = kernel.checked_add(user)?;
    Some((total.checked_sub(idle)?, total))
}

#[cfg(windows)]
pub fn memory_available() -> Option<f64> {
    let mut m = win::MemoryStatusEx { length: std::mem::size_of::<win::MemoryStatusEx>() as u32, load: 0, total_phys: 0, avail_phys: 0, rest: [0; 5] };
    // SAFETY: dwLength set as the API requires
    if unsafe { win::GlobalMemoryStatusEx(&mut m) } == 0 || m.total_phys == 0 || m.avail_phys > m.total_phys {
        return None;
    }
    Some(m.avail_phys as f64 * 100.0 / m.total_phys as f64)
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub fn cpu_ticks() -> Option<(u64, u64)> {
    None
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub fn memory_available() -> Option<f64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incident_smoke_detection_excludes_download_and_security_failures() {
        crate::incident::take_detected();
        let smoke = Err("4.2.0 selftest failed: private diagnostic".into());
        assert_eq!(update_result(smoke.clone()), smoke);
        assert_eq!(crate::incident::take_detected(), ["update.smoke"]);
        for e in ["download failed", "sha256 not in SHA256SUMS", "curl: timeout"] {
            let failure = Err(e.into());
            assert_eq!(update_result(failure.clone()), failure);
        }
        assert!(crate::incident::take_detected().is_empty());
    }

    #[test]
    fn load_readings_parse_and_read_on_this_host() {
        assert_eq!(proc_stat("cpu  10 2 8 70 5 1 1 3 4 0\ncpu0 1 1 1 1 1 1 1 1 1 1\n"), Some((25, 100)), "idle + iowait; guest not added");
        assert_eq!(proc_stat("intr 1"), None);
        assert_eq!(meminfo("MemTotal:  1000 kB\nMemFree: 100 kB\nMemAvailable:   250 kB\n"), Some(25.0));
        assert_eq!(meminfo("MemTotal:  1000 kB\nMemFree: 100 kB\n"), None, "no MemAvailable: unknown");
        // R12: a native reading on this platform, plausible bounds only
        let (busy, total) = cpu_ticks().expect("cpu ticks");
        assert!(busy <= total && total > 0);
        let m = memory_available().expect("available memory");
        assert!(m > 0.0 && m < 100.0, "{m}");
    }

    #[test]
    fn sweep_keeps_the_allowlist_and_live_releases() {
        let t = std::env::temp_dir().join(format!("taskq-sweep-{}", std::process::id()));
        let (install, root) = (t.join("install"), t.join("proj"));
        let _ = fs::remove_dir_all(&t);
        // the running version's release stays, whatever the build sets it to (release CI: the tag)
        let mine = format!("releases/{VERSION}");
        let dirs = ["releases/4.2.0", "releases/4.1.0", "releases/4.0.5", &mine, "releases/cc33ae6", "releases/4.3.0.tmp", "service/x", "mac-pre-update-backup", "capacity", "failed-release-evidence"];
        for d in dirs {
            fs::create_dir_all(install.join(d)).unwrap();
        }
        fs::create_dir_all(root.join(".taskq")).unwrap();
        // a project service still on 4.0.5 with a live child: its release stays (A3)
        fs::write(root.join(".taskq/status.json"), r#"{"version": "4.0.5", "kids": [{"task": 1}]}"#).unwrap();
        let files = ["current", "previous", "token", "taskq", "taskq.log", "taskq.log.tmp", "proj.log", "tray", "tray.json", "TaskQ-proj.xml", ".freshness.json", "current.json", "mac-scheduled-pass.py"];
        for f in files {
            fs::write(install.join(f), "").unwrap();
        }
        fs::write(install.join("current"), "4.2.0").unwrap();
        fs::write(install.join("previous"), "4.1.0").unwrap();
        fs::write(install.join("projects"), format!("{}\n", root.display())).unwrap();
        let (shim, other) = (t.join("bin-taskq"), t.join("bin-other"));
        fs::write(&shim, format!("exec python3 {}/releases/cc33ae6/taskq.py launch --install-dir {} -- \"$@\"", install.display(), install.display())).unwrap();
        fs::write(&other, "#!/bin/sh\nexec something").unwrap();
        sweep(&install, &shim);
        sweep(&install, &other);
        let ls = |d: &Path| {
            let mut v: Vec<String> = fs::read_dir(d).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into()).collect();
            v.sort();
            v
        };
        let mut want = vec![VERSION.to_string(), "4.0.5".into(), "4.1.0".into(), "4.2.0".into(), "4.3.0.tmp".into()];
        want.sort();
        assert_eq!(ls(&install.join("releases")), want);
        assert_eq!(ls(&install), ["TaskQ-proj.xml", "capacity", "current", "previous", "proj.log", "projects", "releases", "taskq", "taskq.log", "taskq.log.tmp", "token", "tray", "tray.json"]);
        assert!(!shim.exists() && other.exists(), "only the v3 shim of this install goes");
        let _ = fs::remove_dir_all(&t);
    }
}
