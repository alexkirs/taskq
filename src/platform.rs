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

static SERVICE_LOG: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// From here on this process is the service: each log line is stamped and folded (§ 4).
pub fn service_log() {
    SERVICE_LOG.store(true, std::sync::atomic::Ordering::Relaxed);
}

pub fn is_service_log() -> bool {
    SERVICE_LOG.load(std::sync::atomic::Ordering::Relaxed)
}

/// `<UTC time> <text>` on one line: the lines of `text`, trimmed, joined by a space (§ 4).
pub fn log_line(at: i64, text: &str) -> String {
    format!("{} {}", crate::board::stamp(at), text.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" "))
}

thread_local!(static PROJECT_LOG: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) });

pub fn project_log(path: PathBuf) { PROJECT_LOG.set(Some(path)); }

pub fn log(text: &str) {
    if is_service_log() {
        let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        let line = log_line(at, text);
        PROJECT_LOG.with_borrow(|p| {
            use std::io::Write;
            if let Some(p) = p { let _ = fs::OpenOptions::new().create(true).append(true).open(p).and_then(|mut f| writeln!(f, "{line}")); }
        });
        use std::io::Write;
        let _ = writeln!(std::io::stderr(), "{line}");
    } else {
        eprintln!("{text}");
    }
}

pub fn die(msg: &str) -> ! {
    log(&error_text(msg, crate::find_root().ok().as_deref()));
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

/// Complete bounded dependency output, including nonzero exits whose receipt the caller must inspect.
pub fn output(name: &'static str, command: &mut std::process::Command, budget: std::time::Duration) -> Result<std::process::Output,String> {
    match coreq::process::output(name,command,budget) {
        Ok(output) | Err(coreq::process::Failure::Exit(output)) => Ok(output),
        Err(error) => Err(format!("{name}: {error}; outcome unknown")),
    }
}

/// § 8 update steps (coreq), then the sweep: Ok(Some(version)) when a new release became current; with `check`, when
/// one is available.
pub fn update(install: &Path, check: bool) -> Result<Option<String>, String> {
    let _lock = update_lock(install)?;
    crate::profile::measure("update", || update_inner(install, check))
}

fn update_lock(install: &Path) -> Result<coreq::file_lock::Lock, String> {
    fs::create_dir_all(install).map_err(|e| e.to_string())?;
    coreq::file_lock::try_lock(&install.join("update.lock")).map_err(|e| format!("update busy or lock unavailable: {e}"))
}

fn update_inner(install: &Path, check: bool) -> Result<Option<String>, String> {
    let v = update_result(coreq::update::update(&tool(install), check))?;
    if v.is_some() && !check {
        let shim = std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/bin/taskq")).unwrap_or_default();
        sweep(install, &shim);
    }
    #[cfg(target_os = "macos")]
    if let Some(v) = v.as_deref().filter(|_| !check) {
        sign_copies(&tool(install), v);
    }
    Ok(v)
}

/// § 8 step 5: coreq `install_self` copies over an existing release binary in place, and macOS then kills every later
/// exec of it (`OS_REASON_CODESIGNING`), so a second `service install` of one build rolled it back (#973). Unlinking
/// first makes the copy a new file; running processes keep the old one.
pub fn install_self(t: &Tool) -> Result<(), String> {
    let (me, dst) = (std::env::current_exe().map_err(|e| e.to_string())?, t.release_exe(t.version));
    #[cfg(all(windows, target_env = "gnu"))]
    install_webview2_loader(t)?;
    if fs::canonicalize(&me).ok() != fs::canonicalize(&dst).ok() {
        // ponytail: a start in the unlink-to-copy gap fails once; coreq writing tmp + rename would close it
        let _ = fs::remove_file(&dst);
    }
    coreq::update::install_self(t)?;
    #[cfg(target_os = "macos")]
    sign_copies(t, t.version);
    Ok(())
}

/// § 8 step 5: the local code-signing identity in the login keychain; absent, copies stay ad-hoc.
#[cfg(target_os = "macos")]
pub const SIGNING_IDENTITY: &str = "TaskQ Local Signing";

/// Signs `path` in place as `id` with `identity` and verifies it against `identifier <id> and certificate leaf`.
/// Ok(false): the identity is absent, `path` is untouched; Err: present but signing or verification failed.
#[cfg(target_os = "macos")]
pub fn codesign(identity: &str, path: &Path, id: &str) -> Result<bool, String> {
    use std::process::Command;
    let ids = coreq::process::output("signing.identity", Command::new("security").args(["find-identity", "-p", "codesigning"]), std::time::Duration::from_secs(30)).map_err(|e| format!("security: {e}"))?;
    let Some(sha1) = identity_hash(identity, &ids)? else { return Ok(false) };
    let run = |c: &mut Command| match coreq::process::output("signing.command", c, std::time::Duration::from_secs(30)) {
        Ok(_) => Ok(()),
        Err(coreq::process::Failure::Exit(o)) => Err(format!("{}: {}", path.display(), String::from_utf8_lossy(&o.stderr).trim())),
        Err(e) => Err(format!("{}: {e}", path.display())),
    };
    run(Command::new("codesign").args(["-f", "-s", &sha1, "-i", id]).arg(path))?;
    run(Command::new("codesign").arg(format!("-R=identifier \"{id}\" and certificate leaf = H\"{sha1}\"")).args(["-v", "--strict"]).arg(path))?;
    Ok(true)
}

/// The SHA-1 of `identity` in `security find-identity` output; Err when `security` failed, so an unreadable
/// keychain is not reported as an absent identity.
#[cfg(target_os = "macos")]
fn identity_hash(identity: &str, ids: &std::process::Output) -> Result<Option<String>, String> {
    if !ids.status.success() {
        return Err(format!("security find-identity: {}: {}", ids.status, String::from_utf8_lossy(&ids.stderr).trim()));
    }
    let quoted = format!("\"{identity}\"");
    Ok(String::from_utf8_lossy(&ids.stdout).lines().find(|l| l.contains(&quoted))
        .and_then(|l| l.split_whitespace().nth(1).map(str::to_owned)))
}

/// § 8 step 5: TCC keys an ad-hoc grant to the cdhash, so each release would need a new grant. Signs a copy of the
/// release `v` and of the launcher as `org.taskq.taskq`, then renames it into place (step 5); logs an absent identity
/// and each copy that failed, which keeps its ad-hoc signature.
#[cfg(target_os = "macos")]
fn sign_copies(t: &Tool, v: &str) {
    for exe in [t.release_exe(v), t.install.join(t.exe_name())] {
        let tmp = exe.with_extension("signing");
        let r = fs::copy(&exe, &tmp).map_err(|e| format!("{}: {e}", exe.display()))
            .and_then(|_| codesign(SIGNING_IDENTITY, &tmp, "org.taskq.taskq"))
            .and_then(|s| if s { fs::rename(&tmp, &exe).map(|_| true).map_err(|e| format!("{}: {e}", exe.display())) } else { Ok(false) });
        let _ = fs::remove_file(&tmp);
        match r {
            Ok(true) => {}
            Ok(false) => return log(&format!("signing: no `{SIGNING_IDENTITY}` identity; {v} stays ad-hoc, macOS grants will not survive the next release")),
            Err(e) => log(&format!("signing failed, copy stays ad-hoc: {e}")),
        }
    }
}

/// GNU links the DLL; MSVC release artifacts link the loader statically.
#[cfg(all(windows, target_env = "gnu"))]
fn install_webview2_loader(t: &Tool) -> Result<(), String> {
    use std::os::windows::ffi::OsStringExt;
    use windows::{core::w, Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleW}};
    let mut path = vec![0u16; 32768];
    // SAFETY: the imported loader remains loaded for this process; the API writes within the supplied buffer.
    let len = unsafe {
        let module = GetModuleHandleW(w!("WebView2Loader.dll")).map_err(|e| e.to_string())?;
        GetModuleFileNameW(Some(module), &mut path) as usize
    };
    if len == 0 || len >= path.len() {
        return Err("cannot locate loaded WebView2Loader.dll".into());
    }
    let bytes = fs::read(PathBuf::from(std::ffi::OsString::from_wide(&path[..len]))).map_err(|e| e.to_string())?;
    for folder in [t.release_exe(t.version).parent().unwrap(), t.install.as_path()] {
        fs::create_dir_all(folder).map_err(|e| e.to_string())?;
        let dst = folder.join("WebView2Loader.dll");
        // Repeated installs must not rewrite a loader already mapped by a running launcher.
        match fs::read(&dst) {
            Ok(old) if old == bytes => continue,
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(format!("{}: {e}", dst.display())),
            _ => {}
        }
        let tmp = dst.with_extension("dll.tmp");
        fs::write(&tmp, &bytes).and_then(|_| fs::rename(&tmp, &dst)).map_err(|e| format!("{}: {e}", dst.display()))?;
    }
    Ok(())
}

/// Service step 7 also catches a release made current by another service on this install.
pub fn service_update(install: &Path) -> Result<Option<String>, String> {
    if let Some(v) = tool(install).current().filter(|v| coreq::update::newer(v, VERSION)) {
        log!("current is {v}; restarting");
        return Ok(Some(v));
    }
    if update(install, true)?.is_some() {
        let v = update(install, false)?;
        if let Some(v) = &v {
            log!("updated to {v}; restarting");
        }
        return Ok(v);
    }
    Ok(None)
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
    // a corrupt status.json leaves its project's version unknown: every release stays (§ 4 Data)
    let registry=coreq::telemetry::capture::operation("update.projects",||crate::host::project_paths(install),|_|"registry-unreadable");
    let mut unknown=registry.is_err();
    for root in registry.unwrap_or_default() {
        let version=coreq::telemetry::capture::operation("update.project-version",|| {
            crate::data::json(&root.join(".taskq/status.json"))?.as_ref()
                .and_then(|st|st["version"].as_str()).filter(|v|!v.is_empty()).map(String::from)
                .ok_or_else(||"project version unknown".to_string())
        },|_|"version-unknown");
        match version {Ok(version)=>keep.push(version),Err(_)=>unknown=true}
    }
    let owned = |n: &str| {
        ["releases", "current", "previous", "projects", "token", "installation-id", "connection", "taskq", "taskq.exe", "WebView2Loader.dll", "taskq-tray.exe", "TaskQ.app", "capacity", "resources", "windows-native", crate::policy::FILE, "taskq-profile.lock", "hold", "admission.json", "admission.lock", "update.lock", "host.json", "host.json.pending", "host-policy-migration", "native", "service", "service.lock", "poke", "restart", "update", "projects.lock", "projects.pending", "rollback-floor"].contains(&n)
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
        if !unknown && !n.ends_with(".tmp") && !keep.contains(&n) {
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

/// Available GiB and the memory pressure level: `MemAvailable`; Linux has no pressure level (0 never holds).
#[cfg(target_os = "linux")]
pub fn host_memory() -> Option<(f64, u32)> {
    let t = fs::read_to_string("/proc/meminfo").ok()?;
    let kb = t.lines().find_map(|l| l.strip_prefix("MemAvailable:")?.trim().strip_suffix("kB")?.trim().parse::<f64>().ok())?;
    Some((kb / GIB * 1024.0, 0))
}

#[cfg_attr(not(any(target_os = "linux", target_os = "macos", windows)), allow(dead_code))]
const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

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
    vm().map(|(avail, total)| avail as f64 * 100.0 / total as f64)
}

/// Available GiB (as [`memory_available`]) and `kern.memorystatus_vm_pressure_level` (1 normal, 2 warning, 4 critical).
#[cfg(target_os = "macos")]
pub fn host_memory() -> Option<(f64, u32)> {
    let (mut level, mut len) = (0u32, std::mem::size_of::<u32>());
    // SAFETY: the level is a plain int output of the given size
    let ok = unsafe { libc::sysctlbyname(c"kern.memorystatus_vm_pressure_level".as_ptr(), (&mut level as *mut u32).cast(), &mut len, std::ptr::null_mut(), 0) } == 0;
    (ok && len == 4).then_some(())?;
    Some((vm()?.0 as f64 / GIB, level))
}

/// `(available, total)` bytes.
#[cfg(target_os = "macos")]
fn vm() -> Option<(u64, u64)> {
    let (mut v, mut n, mut page) = ([0i32; 38], 38u32, 0usize);
    let (mut total, mut len) = (0u64, std::mem::size_of::<u64>());
    // SAFETY: flavor 4 fills HOST_VM_INFO64_COUNT (38) integer_t; the size and memsize outputs are plain integers
    let ok = unsafe {
        mach::host_statistics64(mach::host(), 4, v.as_mut_ptr(), &mut n) == 0
            && mach::host_page_size(mach::host(), &mut page) == 0
            && libc::sysctlbyname(c"hw.memsize".as_ptr(), (&mut total as *mut u64).cast(), &mut len, std::ptr::null_mut(), 0) == 0
    };
    let avail = (v[0] as u32 as u64 + v[2] as u32 as u64) * page as u64;
    (ok && total > 0 && avail <= total).then_some((avail, total))
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

/// Available GiB; Windows has no pressure level here (0 never holds).
#[cfg(windows)]
pub fn host_memory() -> Option<(f64, u32)> {
    let mut m = win::MemoryStatusEx { length: std::mem::size_of::<win::MemoryStatusEx>() as u32, load: 0, total_phys: 0, avail_phys: 0, rest: [0; 5] };
    // SAFETY: dwLength set as the API requires
    (unsafe { win::GlobalMemoryStatusEx(&mut m) } != 0).then(|| (m.avail_phys as f64 / GIB, 0))
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub fn cpu_ticks() -> Option<(u64, u64)> {
    None
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub fn memory_available() -> Option<f64> {
    None
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub fn host_memory() -> Option<(f64, u32)> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // Chain: § 4 service log lines. Risk: a line without its UTC time, or one message split over lines.
    // Chain: short adapter -> silent/nonzero dependency -> bounded response and private failure evidence.
    #[test]
    fn short_dependency_outputs_are_bounded_and_preserve_nonzero_receipts() {
        crate::with_test_capture(|store,_| {
            let fixture=|mode:&str| {
                let mut command=std::process::Command::new(std::env::current_exe().unwrap());
                command.args(["--exact","platform::tests::short_dependency_child","--ignored","--nocapture"]).env("TASKQ_SHORT_DEPENDENCY",mode);command
            };
            let began=std::time::Instant::now();
            let error=output("fixture.silent",&mut fixture("silent"),std::time::Duration::from_millis(100)).unwrap_err();
            assert!(error.contains("timeout")&&error.contains("unknown"));
            assert!(began.elapsed()<std::time::Duration::from_secs(2));
            let receipt=output("fixture.nonzero",&mut fixture("nonzero"),std::time::Duration::from_secs(5)).unwrap();
            assert_eq!(receipt.status.code(),Some(7));
            assert!(String::from_utf8_lossy(&receipt.stderr).contains("private dependency receipt"));
            tracing::dispatcher::get_default(|d|d.downcast_ref::<coreq::telemetry::capture::Capture>().unwrap().flush().unwrap());
            let export=store.export(0,0,1024).unwrap();let rows=export["events"].as_array().unwrap();
            for class in ["timeout","exit"] {assert!(rows.iter().any(|r|r["observation"]["error"]==class));}
            assert!(!export.to_string().contains("private dependency receipt"));
        });
    }

    #[test]
    #[ignore="owned silent/nonzero subprocess for the short dependency adapter"]
    fn short_dependency_child() {
        if std::env::var("TASKQ_SHORT_DEPENDENCY").unwrap()=="nonzero" {eprintln!("private dependency receipt");std::process::exit(7);}
        loop {std::thread::park();}
    }

    #[test]
    fn service_log_lines_start_with_utc_time_and_fold() {
        let at = crate::board::iso("2026-10-14T04:27:05Z").unwrap();
        assert_eq!(log_line(at, "pass: PATCH x: HTTP 502 <html>\n  <p>bad</p>\r\n\n</html>"),"2026-10-14T04:27:05Z pass: PATCH x: HTTP 502 <html> <p>bad</p> </html>");
    }

    // Chain: § 3 Auto memory floor. Risk: the floor or pressure reading fails on a real host and Auto never admits.
    #[cfg(any(target_os = "linux", target_os = "macos", windows))]
    #[test]
    fn host_memory_reads_on_this_host() {
        let memory = host_memory();
        #[cfg(target_os = "macos")]
        if memory.is_none() {
            use std::io::Write;
            writeln!(std::io::stderr(), "sandbox-blocked: host_memory_reads_on_this_host: OS memory probe unavailable").unwrap();
            return;
        }
        let (gib, level) = memory.unwrap();
        assert!(gib > 0.0 && level <= 4, "{gib} GiB, level {level}");
    }

    // Chain: § 8 shared update across services. Risk: a second service downloads or misses the update.
    #[test]
    fn two_services_share_current_without_download_or_smoke() {
        struct FakeService { version: String, starts: usize }
        let install = std::env::temp_dir().join(format!("taskq-shared-update-{}", std::process::id()));
        fs::create_dir_all(&install).unwrap();
        write_atomic(&install.join("current"), VERSION).unwrap();
        let mut first = FakeService { version: VERSION.into(), starts: 1 };
        let mut second = FakeService { version: VERSION.into(), starts: 1 };
        // First service's successful update publishes current; the second still runs the old image.
        let next = "999.0.0";
        write_atomic(&install.join("current"), next).unwrap();
        first.version = next.into();
        first.starts += 1;
        assert!(coreq::update::newer(&tool(&install).current().unwrap(), &second.version));
        if let Some(v) = service_update(&install).unwrap() {
            second.version = v;
            second.starts += 1;
        }
        assert_eq!(second.starts, 2);
        assert_eq!(second.version, first.version);
        // No release executable exists to smoke, and even the download directory was never created.
        assert!(!install.join("releases").exists());
        assert!(!coreq::update::newer(next, &tool(&install).current().unwrap()));
        assert_eq!(fs::read_to_string(install.join("current")).unwrap().trim(), next);
        fs::remove_file(install.join("current")).unwrap();
        fs::remove_dir(&install).unwrap();
    }

    // Chain: incident update.smoke. Risk: download failures reported as smoke failures.
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

    // Chain: § 3 load gate readings. Risk: load read wrong or missing on this OS.
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

    // Chain: § 8 step 5 repeated `service install`. Risk: the second install rewrites the release binary in place and
    // macOS kills its next starts until rollback (#973).
    #[cfg(unix)]
    #[test]
    fn second_install_writes_a_new_release_file() {
        use std::os::unix::fs::MetadataExt;
        let install = std::env::temp_dir().join(format!("taskq-install-self-{}", std::process::id()));
        let _ = fs::remove_dir_all(&install);
        let t = tool(&install);
        let ino = || fs::metadata(t.release_exe(VERSION)).unwrap().ino();
        install_self(&t).unwrap();
        let first = ino();
        // A hard link keeps the first inode alive, as a running service does; ext4 would reuse a freed one.
        fs::hard_link(t.release_exe(VERSION), install.join("first")).unwrap();
        install_self(&t).unwrap();
        assert_ne!(ino(), first, "the release binary was rewritten in place");
        let _ = fs::remove_dir_all(&install);
    }

    // Chain: § 8 step 5 macOS signing (#1089). Risk: an absent identity or a failed signature passes silently, or two
    // releases end up with requirements TCC does not match as one app.
    #[cfg(target_os = "macos")]
    #[test]
    fn local_identity_signs_releases_under_one_requirement() {
        let dir = std::env::temp_dir().join(format!("taskq-sign-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let (a, b) = (dir.join("a"), dir.join("b"));
        fs::copy("/usr/bin/true", &a).unwrap();
        fs::copy("/usr/bin/false", &b).unwrap();
        let before = fs::read(&a).unwrap();
        assert_eq!(codesign("TaskQ No Such Identity", &a, "org.taskq.taskq"), Ok(false));
        assert_eq!(fs::read(&a).unwrap(), before, "an absent identity touched the file");
        let failed = std::process::Command::new("/bin/sh").args(["-c", "echo boom >&2; exit 1"]).output().unwrap();
        assert!(identity_hash(SIGNING_IDENTITY, &failed).unwrap_err().contains("boom"));
        // ponytail: CI has no identity; the rest runs on a host where `service install` signs
        let signed = codesign(SIGNING_IDENTITY, &a, "org.taskq.taskq");
        assert!(signed.is_ok(), "{signed:?}");
        if signed == Ok(true) {
            assert_eq!(codesign(SIGNING_IDENTITY, &b, "org.taskq.taskq"), Ok(true));
            let req = |p: &Path| String::from_utf8(std::process::Command::new("codesign").args(["-d", "-r-"]).arg(p).output().unwrap().stdout).unwrap();
            assert_eq!(req(&a), req(&b));
            assert!(req(&a).contains("identifier \"org.taskq.taskq\" and certificate leaf"), "{}", req(&a));
            assert!(codesign(SIGNING_IDENTITY, &dir.join("missing"), "org.taskq.taskq").is_err());
        }
        let _ = fs::remove_dir_all(&dir);
    }

    // Chain: CLI/service update ownership -> cleanup -> the same metadata and activation lock stays exclusive.
    #[test]
    fn updates_share_one_lock_through_cleanup_and_release_it() {
        let install = std::env::temp_dir().join(format!("taskq-update-lock-{}", crate::runtime::uuid()));
        let lock = update_lock(&install).unwrap();
        assert!(update_lock(&install).is_err(), "a competing updater must return without blocking service heartbeats");
        sweep(&install, &install.join("absent-shim"));
        let other = fs::OpenOptions::new().write(true).open(install.join("update.lock")).unwrap();
        assert!(other.try_lock().is_err(), "cleanup must not unlink the live update lock");
        #[cfg(unix)]
        let inherited = unsafe { libc::fork() };
        #[cfg(unix)]
        {
            assert!(inherited >= 0);
            if inherited == 0 { unsafe { libc::poll(std::ptr::null_mut(), 0, 250); libc::_exit(0); } }
        }
        drop(lock);
        let released = other.try_lock();
        #[cfg(unix)]
        unsafe { libc::waitpid(inherited, std::ptr::null_mut(), 0); }
        released.unwrap();
        drop(other);
        fs::remove_dir_all(install).unwrap();
    }

    // Chain: § 8 install sweep. Risk: sweep deletes a live release or owned file.
    #[test]
    fn sweep_keeps_the_allowlist_and_live_releases() {
        let t = std::env::temp_dir().join(format!("taskq-sweep-{}", std::process::id()));
        let (install, root) = (t.join("install"), t.join("proj"));
        let _ = fs::remove_dir_all(&t);
        // the running version's release stays, whatever the build sets it to (release CI: the tag)
        let mine = format!("releases/{VERSION}");
        let dirs = ["releases/4.2.0", "releases/4.1.0", "releases/4.0.5", &mine, "releases/cc33ae6", "releases/4.3.0.tmp", "obsolete-service/x", "mac-pre-update-backup", "capacity", "resources", "windows-native/state", "failed-release-evidence"];
        for d in dirs {
            fs::create_dir_all(install.join(d)).unwrap();
        }
        fs::create_dir_all(root.join(".taskq")).unwrap();
        // a project service still on 4.0.5 with a live child: its release stays (A3)
        fs::write(root.join(".taskq/status.json"), r#"{"version": "4.0.5", "kids": [{"task": 1}]}"#).unwrap();
        let files = ["current", "previous", "token", "taskq", "taskq.log", "taskq.log.tmp", "proj.log", "tray", "tray.json", "TaskQ-proj.xml", "worker-policy.json", "hold", "admission.json", "admission.lock", "update.lock", ".freshness.json", "current.json", "mac-scheduled-pass.py"];
        for f in files {
            fs::write(install.join(f), "").unwrap();
        }
        fs::write(install.join("hold"), "invented incident hold").unwrap();
        fs::write(install.join("current"), "4.2.0").unwrap();
        fs::write(install.join("previous"), "4.1.0").unwrap();
        fs::write(install.join("windows-native/state/cap_sid"), "native control state").unwrap();
        fs::write(install.join("projects"), format!("{}\n", root.display())).unwrap();
        // H-A5: update keeps policy, interrupted writes, exact native recovery and the qualified rollback floor.
        let protected=[("host.json","{\"limits\":{\"codex\":2},\"owner_field\":\"keep\"}"),
            ("host.json.pending","interrupted policy; preserve"),("service","123"),("service.lock",""),("poke","wake"),
            ("restart","restart requested"),("update","update requested"),("projects.lock","")];
        for (name,text) in protected {fs::write(install.join(name),text).unwrap();}
        fs::create_dir_all(install.join("host-policy-migration")).unwrap();
        fs::write(install.join("host-policy-migration/original.json"),"original configuration").unwrap();
        let native=crate::kids_path(&install,&root).unwrap();fs::create_dir_all(native.parent().unwrap()).unwrap();
        let owner=coreq::process::identity(std::process::id()).unwrap().unwrap();
        let recovery=format!("pid {} {} executor-v1\n7 41 codex 123 child 43 40\n",owner.pid,owner.started);
        fs::write(&native,&recovery).unwrap();
        coreq::launcher::set_rollback_floor(&tool(&install),"4.2.0").unwrap();
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
        want.dedup();
        assert_eq!(ls(&install.join("releases")), want);
        for (name,text) in protected {assert_eq!(fs::read_to_string(install.join(name)).ok().as_deref(),Some(text),"update lost {name}");}
        assert_eq!(fs::read_to_string(install.join("host-policy-migration/original.json")).unwrap(),"original configuration");
        assert_eq!(fs::read_to_string(&native).unwrap(),recovery);
        assert_eq!(crate::kids_file(&install,&root).unwrap()[0].execution,43,"native execution identity survives update");
        assert_eq!(fs::read_to_string(install.join("rollback-floor")).unwrap(),"4.2.0");
        let mut owned=vec!["TaskQ-proj.xml", "admission.json", "admission.lock", "capacity", "current", "hold", "previous", "proj.log", "projects", "releases", "resources", "taskq", "taskq.log", "taskq.log.tmp", "token", "tray", "tray.json", "update.lock", "windows-native", "worker-policy.json", "native", "host-policy-migration", "rollback-floor"];
        owned.extend(protected.iter().map(|(name,_)|*name));owned.sort();
        assert_eq!(ls(&install),owned);
        assert_eq!(crate::admission::hold(&install).as_deref(), Some("invented incident hold"), "update cleanup must never release an emergency hold");
        assert_eq!(fs::read_to_string(install.join("windows-native/state/cap_sid")).unwrap(), "native control state");
        assert!(!shim.exists() && other.exists(), "only the v3 shim of this install goes");
        // § 4 Data: a corrupt status.json leaves the project's version unknown, so every release stays and so does the file
        fs::write(root.join(".taskq/status.json"), "{\"version\": \"4.0").unwrap();
        fs::create_dir_all(install.join("releases/4.0.1")).unwrap();
        sweep(&install, &shim);
        assert!(install.join("releases/4.0.1").exists() && install.join("releases/4.0.5").exists());
        assert_eq!(fs::read_to_string(root.join(".taskq/status.json")).unwrap(), "{\"version\": \"4.0");
        crate::with_test_capture(|store,_| {
            fs::write(root.join(".taskq/status.json"),r#"{"version":"4.0.5"}"#).unwrap();
            fs::write(install.join("projects"),b"\xff").unwrap();
            fs::create_dir_all(install.join("releases/4.0.2")).unwrap();sweep(&install,&shim);
            assert!(install.join("releases/4.0.2").exists(),"unreadable registry cannot authorize release deletion");
            fs::write(install.join("projects"),format!("{}\n",root.display())).unwrap();
            fs::write(install.join("projects.pending"),"retained registry prepare").unwrap();
            sweep(&install,&shim);
            assert!(install.join("releases/4.0.2").exists(),"interrupted registry cannot authorize release deletion");
            assert_eq!(fs::read_to_string(install.join("projects.pending")).unwrap(),"retained registry prepare");
            fs::remove_file(install.join("projects.pending")).unwrap();
            fs::remove_file(root.join(".taskq/status.json")).unwrap();
            sweep(&install,&shim);
            assert!(install.join("releases/4.0.2").exists(),"missing project status cannot authorize release deletion");
            tracing::dispatcher::get_default(|d|d.downcast_ref::<coreq::telemetry::capture::Capture>().unwrap().flush().unwrap());
            let export=store.export(0,0,1024).unwrap();
            for class in ["registry-unreadable","version-unknown"] {
                assert!(export["events"].as_array().unwrap().iter().any(|row|row["observation"]["error"]==class),"missing cleanup failure {class}");
            }
            let private=serde_json::to_string(root.to_str().unwrap()).unwrap();
            assert!(!export.to_string().contains(private.trim_matches('"')),"private local paths entered telemetry");
        });
        let _ = fs::remove_dir_all(&t);
    }
}
