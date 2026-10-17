//! taskq on coreq: install layout, self-update, launcher and OS units come from the shared crate (coreq.md); this file
//! keeps taskq's own parts: the error format, the install sweep and the load readings (taskq.md § 3, § 8).

use std::fs;
use std::path::{Path, PathBuf};

#[cfg(test)] pub use coreq::service::Unit;
pub use coreq::{Tool, quiet, write_atomic};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Built in, never from config: project files are written by workers, so config must not choose the code hosts run.
pub const RELEASE_REPO: &str = "taskqorg/taskq";

/// taskq as a coreq tool installed in `install`.
pub fn tool(install: &Path) -> Tool {
    Tool { name:"taskq",title:"TaskQ",repo:RELEASE_REPO,version:VERSION,install:install.into(),fault:Some(crate::incident::launcher) }
}

pub fn default_install() -> PathBuf {
    let aliases:&[&str]=if cfg!(windows) {&["taskq-tray.exe"]}else{&[]};
    coreq::install_dir("taskq","TaskQ",aliases).unwrap_or_else(|error| {
        use std::io::Write;
        let _=writeln!(std::io::stderr(),"taskq: {error}; set TASKQ_INSTALL explicitly to inspect or repair this installation");
        coreq::telemetry::capture::exit(1)
    })
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
    transition_ready(install)?;
    crate::profile::measure("update", || update_inner(install, check))
}

pub(crate) fn update_lock(install: &Path) -> Result<coreq::file_lock::Lock, String> {
    fs::create_dir_all(install).map_err(|e| e.to_string())?;
    coreq::file_lock::try_lock(&install.join("update.lock")).map_err(|e| format!("update busy or lock unavailable: {e}"))
}

/// An interrupted registry/install transition retains admission and active release selection together.
pub(crate) fn transition_ready(install:&Path)->Result<(),String> {
    let prepare=format!("{}.pending",crate::host::INSTALL_PENDING);
    for name in ["projects.pending",crate::host::INSTALL_PENDING,prepare.as_str()] {
        match fs::symlink_metadata(install.join(name)) {
            Err(e) if e.kind()==std::io::ErrorKind::NotFound=>{},
            Err(e)=>return Err(format!("installation transition unknown: {e}; update held")),
            Ok(_)=>return Err(format!("{name} retained; installation reconciliation required before update")),
        }
    }
    Ok(())
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

/// Freeze the source before local signing; an immutable release retains the original source binding.
pub fn prepare_install(t:&Tool)->Result<String,String> {
    let source=fs::read(std::env::current_exe().map_err(|e|e.to_string())?).map_err(|e|e.to_string())?;
    let hash=|bytes:&[u8]|->String {ring::digest::digest(&ring::digest::SHA256,bytes).as_ref().iter().map(|b|format!("{b:02x}")).collect()};
    let source_hash=hash(&source);
    let binding=t.release_exe(t.version).with_file_name("install-source");
    match fs::symlink_metadata(&binding) {
        Ok(meta) if meta.is_file()&&meta.len()<=256=>{},Ok(_)=>return Err("prepared source binding unsupported; kept".into()),
        Err(e) if e.kind()==std::io::ErrorKind::NotFound=>{},Err(e)=>return Err(e.to_string()),
    }
    match fs::read_to_string(&binding) {
        Ok(old)=>{
            let bound:Vec<_>=old.lines().collect();
            if bound.len()!=2||bound.iter().any(|v|v.len()!=64||!v.bytes().all(|b|b.is_ascii_hexdigit()&&!b.is_ascii_uppercase())) {return Err("prepared release source binding invalid; kept".into());}
            match fs::read(t.release_exe(t.version)) {
                Ok(bytes)=>{
                    let candidate_hash=hash(&bytes);
                    if bound[1]!=candidate_hash||(bound[0]!=source_hash&&candidate_hash!=source_hash) {return Err("prepared release source differs; use a new version, existing file kept".into());}
                    coreq::update::prepare_install(t,&bytes)?;
                    #[cfg(all(windows,target_env="gnu"))] prepare_webview2_loader(t)?;
                    return Ok(candidate_hash);
                },
                Err(e) if e.kind()==std::io::ErrorKind::NotFound&&bound[0]==source_hash=>{},Err(e)=>return Err(e.to_string()),
            }
        },
        Err(e) if e.kind()==std::io::ErrorKind::NotFound=>{},Err(e)=>return Err(e.to_string()),
    }
    if fs::read(t.release_exe(t.version)).ok().as_deref()==Some(source.as_slice()) {
        coreq::update::prepare_install(t,&source)?;
        coreq::write_durable(&binding,format!("{source_hash}\n{source_hash}\n"))?;
        #[cfg(all(windows,target_env="gnu"))] prepare_webview2_loader(t)?;
        return Ok(source_hash);
    }
    #[cfg(target_os="macos")]
    let source={
        let scratch=std::env::temp_dir().join(format!("taskq-install-sign-{}",crate::runtime::uuid()));
        fs::write(&scratch,&source).map_err(|e|e.to_string())?;
        let result=codesign(SIGNING_IDENTITY,&scratch,"org.taskq.taskq").and_then(|signed| {
            if !signed {log("signing: local identity absent; candidate keeps its original signature");}
            fs::read(&scratch).map_err(|e|e.to_string())
        });
        let cleanup=fs::remove_file(&scratch).map_err(|e|format!("signing scratch cleanup: {e}"));
        let bytes=result?;cleanup?;bytes
    };
    let candidate=hash(&source);
    match fs::symlink_metadata(t.release_exe(t.version)) {
        Ok(meta) if meta.is_file()&&fs::read(t.release_exe(t.version)).map_err(|e|e.to_string())?==source=>{},
        Ok(_)=>return Err("prepared release differs; use a new version, existing file kept".into()),
        Err(e) if e.kind()==std::io::ErrorKind::NotFound=>{},Err(e)=>return Err(e.to_string()),
    }
    // The source binding precedes immutable publication, so a crash cannot leave an unbound signed candidate.
    fs::create_dir_all(binding.parent().ok_or("prepared release directory missing")?).map_err(|e|e.to_string())?;
    coreq::write_durable(&binding,format!("{source_hash}\n{candidate}\n"))?;
    coreq::update::prepare_install(t,&source)?;
    #[cfg(all(windows,target_env="gnu"))] prepare_webview2_loader(t)?;
    Ok(candidate)
}

/// Activate the immutable executable and its prepared GNU loader before any native start.
pub fn activate_install(t:&Tool)->Result<(),String> {
    coreq::update::activate_install(t)?;
    #[cfg(all(windows,target_env="gnu"))] {
        let bytes=fs::read(t.release_exe(t.version).with_file_name("WebView2Loader.dll")).map_err(|e|e.to_string())?;
        let target=t.install.join("WebView2Loader.dll");
        if fs::read(&target).ok().as_deref()!=Some(bytes.as_slice()) {coreq::write_durable(&target,bytes)?;}
    }
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
fn loaded_webview2_loader() -> Result<Vec<u8>, String> {
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
    fs::read(PathBuf::from(std::ffi::OsString::from_wide(&path[..len]))).map_err(|e|e.to_string())
}

#[cfg(all(windows,target_env="gnu"))]
fn prepare_webview2_loader(t:&Tool)->Result<(),String> {
    let bytes=loaded_webview2_loader()?;
    let target=t.release_exe(t.version).with_file_name("WebView2Loader.dll");
    match fs::symlink_metadata(&target) {
        Ok(meta) if meta.is_file()&&fs::read(&target).map_err(|e|e.to_string())?==bytes=>Ok(()),
        Ok(_)=>Err("prepared WebView2 loader differs; existing release kept, use a new version".into()),
        Err(e) if e.kind()==std::io::ErrorKind::NotFound=>coreq::write_durable(&target,bytes),
        Err(e)=>Err(e.to_string()),
    }
}

/// Service step 7 also catches a release made current by another service on this install.
pub fn service_update(install: &Path) -> Result<Option<String>, String> {
    let _lock=update_lock(install)?;
    transition_ready(install)?;
    crate::profile::measure("update",||service_update_locked(install))
}

fn service_update_locked(install:&Path)->Result<Option<String>,String> {
    if let Some(v) = tool(install).current().filter(|v| coreq::update::newer(v, VERSION)) {
        log!("current is {v}; restarting");
        return Ok(Some(v));
    }
    if update_inner(install, true)?.is_some() {
        let v = update_inner(install, false)?;
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
            ||n==crate::host::INSTALL_PENDING||n.strip_suffix(".pending")==Some(crate::host::INSTALL_PENDING)
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
        fs::remove_dir_all(&install).unwrap();
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

    // H-A5/#973: local signing and repeated preparation cannot rewrite the immutable release or active files.
    #[test]
    fn repeated_install_preparation_preserves_immutable_release() {
        let install=std::env::temp_dir().join(format!("taskq-install-self-{}",crate::runtime::uuid()));
        let t=tool(&install);
        let first=prepare_install(&t).unwrap();
        assert!(!install.join("current").exists()&&!install.join(t.exe_name()).exists());
        let bytes=fs::read(t.release_exe(VERSION)).unwrap();
        #[cfg(unix)] let inode={use std::os::unix::fs::MetadataExt;fs::metadata(t.release_exe(VERSION)).unwrap().ino()};
        assert_eq!(prepare_install(&t).unwrap(),first);
        assert_eq!(fs::read(t.release_exe(VERSION)).unwrap(),bytes);
        #[cfg(unix)] {use std::os::unix::fs::MetadataExt;assert_eq!(fs::metadata(t.release_exe(VERSION)).unwrap().ino(),inode);}
        activate_install(&t).unwrap();
        assert_eq!(fs::read(install.join(t.exe_name())).unwrap(),bytes);
        assert_eq!(fs::read_to_string(install.join("current")).unwrap(),VERSION);
        fs::write(t.release_exe(VERSION),"changed-same-version").unwrap();
        assert!(prepare_install(&t).unwrap_err().contains("differs"));
        assert_eq!(fs::read(t.release_exe(VERSION)).unwrap(),b"changed-same-version");
        fs::remove_dir_all(install).unwrap();
    }

    // Windows install layout: relocated executable/real launcher must find GNU's loader without Cargo's PATH.
    #[cfg(windows)]
    #[test]
    fn installed_windows_release_starts_without_build_paths() {
        let base=std::env::temp_dir().join(format!("taskq-windows-install-{}",crate::runtime::uuid()));
        let appdata=base.join("appdata");let install=appdata.join("taskq");let t=tool(&install);
        prepare_install(&t).unwrap();activate_install(&t).unwrap();
        let system32=PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join("System32");
        for exe in [t.release_exe(VERSION),install.join(t.exe_name())] {
            let out=coreq::process::output("test.installed-windows-binary",std::process::Command::new(&exe)
                .args(["--exact","platform::tests::installed_windows_binary_fixture","--ignored","--nocapture"])
                .current_dir(&base).env("PATH",&system32).env("LOCALAPPDATA",&appdata).env_remove("TASKQ_ROOT")
                .env("TASKQ_INSTALL_FIXTURE",&install).env("TASKQ_SELFTEST_OFFLINE","1").env("TASKQ_INCIDENT_REPORTS","off"),std::time::Duration::from_secs(30));
            assert!(out.is_ok(),"{}: {out:?}",exe.display());
            assert!(String::from_utf8_lossy(&out.unwrap().stdout).contains("golden traces PASS"));
        }
        #[cfg(target_env="gnu")] for folder in [&install,t.release_exe(VERSION).parent().unwrap()] {assert!(folder.join("WebView2Loader.dll").is_file());}
        fs::remove_dir_all(base).unwrap();
    }

    #[cfg(windows)]
    #[test]
    #[ignore="owned relocated-binary fixture; requires TASKQ_INSTALL_FIXTURE"]
    fn installed_windows_binary_fixture() {
        let install=PathBuf::from(std::env::var_os("TASKQ_INSTALL_FIXTURE").expect("owned fixture installation"));
        if std::env::current_exe().unwrap().parent()==Some(install.as_path()) {
            coreq::launcher::launch(&tool(&install),&std::env::args().skip(1).collect::<Vec<_>>());
        }
        let _telemetry=coreq::telemetry::capture::Runtime::install_bounded(
            &install.join("telemetry.log/universal.sqlite3"),"taskq",VERSION,crate::telemetry::budget(&install,"telemetry.sqlite3").unwrap(),
        ).unwrap();
        crate::run(&["selftest".into()]).unwrap();
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

    // H-A5: all update entrypoints respect installation ownership, including the current-version fast path.
    #[test]
    fn installation_transition_fences_updates_before_release_selection_or_effects() {
        let install=std::env::temp_dir().join(format!("taskq-install-update-{}",crate::runtime::uuid()));
        fs::create_dir_all(&install).unwrap();
        fs::write(install.join("current"),"999.0.0").unwrap();
        let lock=update_lock(&install).unwrap();
        assert!(service_update(&install).unwrap_err().contains("busy"));
        for check in [true,false] {assert!(update(&install,check).unwrap_err().contains("busy"));}
        drop(lock);
        // An installer crash releases the OS lock but leaves its recovery/admission marker.
        let prepare=format!("{}.pending",crate::host::INSTALL_PENDING);
        for name in ["projects.pending",crate::host::INSTALL_PENDING,prepare.as_str()] {
            fs::write(install.join(name),"retained-install-binding").unwrap();
            assert!(service_update(&install).unwrap_err().contains(name));
            for check in [true,false] {assert!(update(&install,check).unwrap_err().contains(name));}
            assert_eq!(fs::read_to_string(install.join("current")).unwrap(),"999.0.0");
            assert!(!install.join("releases").exists(),"a held transition must not download or prune");
            fs::remove_file(install.join(name)).unwrap();
        }
        assert_eq!(service_update(&install).unwrap(),Some("999.0.0".into()));
        fs::remove_dir_all(install).unwrap();
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
