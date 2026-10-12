//! Install layout, self-update and OS units (taskq.md § 8).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn exe_name() -> &'static str {
    if cfg!(windows) { "taskq.exe" } else { "taskq" }
}

pub fn default_install() -> PathBuf {
    if let Some(p) = std::env::var_os("TASKQ_INSTALL") {
        return p.into();
    }
    if cfg!(windows) {
        PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap_or_default()).join("taskq")
    } else {
        PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share/taskq")
    }
}

pub fn target() -> String {
    let os = match std::env::consts::OS {
        "macos" => "macos",
        "windows" => "windows",
        o => o,
    };
    format!("{os}-{}", std::env::consts::ARCH)
}

/// `4.1.0` > `4.0.12` > `4.0.0-dev`: numeric parts, a pre-release suffix sorts first.
pub fn newer(a: &str, b: &str) -> bool {
    let key = |v: &str| {
        let (core, pre) = v.split_once('-').map_or((v, None), |(c, p)| (c, Some(p.to_string())));
        let nums: Vec<u64> = core.split('.').map(|x| x.parse().unwrap_or(0)).collect();
        (nums, pre.is_none(), pre)
    };
    key(a) > key(b)
}

fn read(p: &Path) -> Option<String> {
    fs::read_to_string(p).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// Atomic small-file write: temp file, then rename.
pub fn write_atomic(p: &Path, text: &str) -> Result<(), String> {
    let tmp = p.with_extension("tmp");
    fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
    fs::rename(&tmp, p).map_err(|e| format!("{}: {e}", p.display()))
}

pub fn current(install: &Path) -> Option<String> {
    read(&install.join("current"))
}

pub fn release_exe(install: &Path, v: &str) -> PathBuf {
    install.join("releases").join(v).join(exe_name())
}

/// The launcher `<install>/taskq`: run the current release. Only `service run` counts starts of a version that is not
/// yet healthy; the third one restores `previous` (§ 8).
/// Windows: no console window for a child; a console-less parent would otherwise get one per child.
pub fn quiet(c: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    c
}

pub fn launch(install: &Path, args: &[String]) -> ! {
    let service = args.first().map(String::as_str) == Some("service") && args.get(1).map(String::as_str) == Some("run");
    let pick = || {
        let mut v = current(install).unwrap_or_else(|| die("no current release: run `taskq update`"));
        if service {
            let dir = install.join("releases").join(&v);
            if !dir.join("healthy").exists() {
                let n = read(&dir.join("starts")).and_then(|s| s.parse::<u32>().ok()).unwrap_or(0) + 1;
                let _ = fs::write(dir.join("starts"), n.to_string());
                if n >= 3 {
                    if let Some(prev) = read(&install.join("previous")).filter(|p| *p != v) {
                        eprintln!("taskq: {v} failed to become healthy {n} times; back to {prev}");
                        let _ = write_atomic(&install.join("current"), &prev);
                        v = prev;
                    }
                }
            }
        }
        v
    };
    #[cfg(unix)]
    {
        let v = pick();
        let mut c = Command::new(release_exe(install, &v));
        c.args(args);
        use std::os::unix::process::CommandExt;
        let e = c.exec();
        die(&format!("exec {v}: {e}"));
    }
    // ponytail: a Scheduled Task restarts only a failed launch, so on Windows the launcher supervises `service run`:
    // exit 75 (updated) runs the new current at once, any other exit waits a minute; stderr goes to <install>/taskq.log.
    // The task runs in the interactive session, so the launcher drops its console window (closing it was STATUS_CONTROL_C_EXIT
    // for the service) and the child gets none. The std handles are reset too: stale console handles made spawn fail and
    // the error print panic (exit 101).
    #[cfg(windows)]
    if service {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn FreeConsole() -> i32;
            fn SetStdHandle(id: u32, h: *mut std::ffi::c_void) -> i32;
        }
        unsafe {
            FreeConsole();
            for id in [-10i32, -11, -12] {
                SetStdHandle(id as u32, std::ptr::null_mut());
            }
        }
    }
    #[cfg(not(unix))]
    {
        let log = || fs::OpenOptions::new().create(true).append(true).open(install.join("taskq.log")).ok();
        let note = |msg: &str| {
            use std::io::Write;
            match log().filter(|_| service) {
                Some(mut f) => {
                    let _ = writeln!(f, "taskq: {msg}");
                }
                None => eprintln!("taskq: {msg}"),
            }
        };
        loop {
            let v = pick();
            let mut c = Command::new(release_exe(install, &v));
            c.args(args);
            if service && let Some(out) = log() {
                c.stdin(std::process::Stdio::null());
                if let Ok(err) = out.try_clone() {
                    c.stderr(err);
                }
                c.stdout(out);
                quiet(&mut c);
            }
            let code = match c.status() {
                Ok(s) => s.code().unwrap_or(1),
                Err(e) if service => {
                    note(&format!("start {v}: {e}"));
                    1
                }
                Err(e) => die(&format!("start {v}: {e}")),
            };
            if !service {
                std::process::exit(code);
            }
            if code != 75 {
                note(&format!("{v} exited with {code}; restart in 60 s"));
                std::thread::sleep(std::time::Duration::from_secs(60));
            }
        }
    }
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

fn download(url: &str, to: &Path) -> Result<(), String> {
    let out = Command::new("curl").args(["-fsSL", "--max-time", "300", "-o"]).arg(to).arg(url).output().map_err(|e| format!("curl: {e}"))?;
    if out.status.success() { Ok(()) } else { Err(format!("download {url}: {}", String::from_utf8_lossy(&out.stderr).trim())) }
}

/// `SHA256SUMS` lists `name` with the sha256 of `bytes`.
pub fn verify(sums: &[u8], name: &str, bytes: &[u8]) -> Result<(), String> {
    let digest: String = ring::digest::digest(&ring::digest::SHA256, bytes).as_ref().iter().map(|b| format!("{b:02x}")).collect();
    let listed = String::from_utf8_lossy(sums).lines().any(|l| l.split_whitespace().collect::<Vec<_>>() == [digest.as_str(), name]);
    if listed { Ok(()) } else { Err(format!("{name}: sha256 {digest} not in SHA256SUMS")) }
}

/// § 8 update steps: Ok(Some(version)) when a new release became current; with `check`, when one is available.
/// Built in, never from config: project files are written by workers, so config must not choose the code hosts run.
pub const RELEASE_REPO: &str = "alexkirs/taskq";

pub fn update(install: &Path, check: bool) -> Result<Option<String>, String> {
    let repo = RELEASE_REPO;
    let rel = install.join("releases");
    fs::create_dir_all(&rel).map_err(|e| e.to_string())?;
    for e in fs::read_dir(&rel).map_err(|e| e.to_string())?.flatten() {
        if e.file_name().to_string_lossy().ends_with(".tmp") {
            let _ = fs::remove_dir_all(e.path());
        }
    }
    let base = format!("https://github.com/{repo}/releases/latest/download");
    let probe = rel.join("latest.json.tmp");
    download(&format!("{base}/latest.json"), &probe)?;
    let latest: serde_json::Value = serde_json::from_slice(&fs::read(&probe).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let _ = fs::remove_file(&probe);
    let ver = latest["version"].as_str().ok_or("latest.json: no version")?.to_string();
    let running = current(install).unwrap_or_else(|| VERSION.to_string());
    if !newer(&ver, &running) || release_exe(install, &ver).exists() {
        return Ok(None);
    }
    let name = latest["files"][target()].as_str().ok_or(format!("latest.json: no file for {}", target()))?.to_string();
    // both become paths under `releases/`: only the exact expected names pass
    if !ver.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'.' || c == b'-') || ver.contains("..") || name != format!("taskq-{ver}-{}{}", target(), std::env::consts::EXE_SUFFIX) {
        return Err(format!("latest.json: unexpected version {ver:?} or file {name:?}"));
    }
    if check {
        return Ok(Some(ver));
    }
    let tmp = rel.join(format!("{ver}.tmp"));
    fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    let result = (|| {
        for f in ["SHA256SUMS", name.as_str()] {
            download(&format!("{base}/{f}"), &tmp.join(f))?;
        }
        let bytes = fs::read(tmp.join(&name)).map_err(|e| e.to_string())?;
        verify(&fs::read(tmp.join("SHA256SUMS")).map_err(|e| e.to_string())?, &name, &bytes)?;
        fs::rename(tmp.join(&name), tmp.join(exe_name())).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(tmp.join(exe_name()), fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
        }
        let smoke = Command::new(tmp.join(exe_name())).arg("selftest").env("TASKQ_SELFTEST_OFFLINE", "1").output().map_err(|e| e.to_string())?;
        if !smoke.status.success() {
            return Err(format!("{ver} selftest failed: {}", String::from_utf8_lossy(&smoke.stderr).trim()));
        }
        fs::rename(&tmp, rel.join(&ver)).map_err(|e| e.to_string())
    })();
    if let Err(e) = result {
        let _ = fs::remove_dir_all(&tmp);
        return Err(e);
    }
    if let Some(cur) = current(install) {
        write_atomic(&install.join("previous"), &cur)?;
    }
    write_atomic(&install.join("current"), &ver)?;
    Ok(Some(ver))
}

/// First install of this binary: it becomes the current release and the launcher.
pub fn install_self(install: &Path) -> Result<(), String> {
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    let dst = release_exe(install, VERSION);
    fs::create_dir_all(dst.parent().unwrap()).map_err(|e| e.to_string())?;
    if me != dst {
        fs::copy(&me, &dst).map_err(|e| e.to_string())?;
    }
    let launcher = install.join(exe_name());
    if me != launcher {
        // ponytail: the launcher stays at the version that installed it; it only reads `current` and `previous`
        fs::copy(&me, &launcher).map_err(|e| format!("{}: {e}", launcher.display()))?;
    }
    // a newer binary installs over an older one as an update does (§ 8 step 4); never a downgrade
    match current(install) {
        Some(c) if !newer(VERSION, &c) => {}
        old => {
            if let Some(c) = old {
                write_atomic(&install.join("previous"), &c)?;
            }
            write_atomic(&install.join("current"), VERSION)?;
        }
    }
    Ok(())
}

fn xml(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

pub struct Unit {
    pub path: PathBuf,
    pub text: String,
    pub load: Vec<Vec<String>>,
    pub unload: Vec<Vec<String>>,
    /// Stops the service until the next logon; the unit stays.
    pub stop: Vec<Vec<String>>,
}

/// The service's output file (§ 8); Linux logs to the journal.
pub fn log_path(install: &Path, project: &str) -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        Some(install.join(format!("{project}.log")))
    } else if cfg!(windows) {
        Some(install.join("taskq.log"))
    } else {
        None
    }
}

fn sv(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

/// The one OS unit that keeps `<install>/taskq service run` running for the project at `root`.
pub fn unit(install: &Path, root: &Path, project: &str) -> Unit {
    let launcher = install.join(exe_name());
    let (l, r) = (launcher.display().to_string(), root.display().to_string());
    let path_env = std::env::var("PATH").unwrap_or_default();
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    if cfg!(target_os = "macos") {
        let label = format!("com.taskq.{project}");
        let path = home.join("Library/LaunchAgents").join(format!("{label}.plist"));
        let log = log_path(install, project).unwrap_or_default().display().to_string();
        let text = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict>\n<key>Label</key><string>{}</string>\n<key>ProgramArguments</key><array><string>{}</string><string>service</string><string>run</string></array>\n<key>WorkingDirectory</key><string>{}</string>\n<key>RunAtLoad</key><true/>\n<key>KeepAlive</key><true/>\n<key>StandardOutPath</key><string>{}</string>\n<key>StandardErrorPath</key><string>{}</string>\n<key>EnvironmentVariables</key><dict><key>PATH</key><string>{}</string></dict>\n</dict></plist>\n",
            xml(&label), xml(&l), xml(&r), xml(&log), xml(&log), xml(&path_env)
        );
        let uid = Command::new("id").arg("-u").output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
        let p = path.display().to_string();
        return Unit {
            load: vec![sv(&["launchctl", "bootstrap", &format!("gui/{uid}"), &p])],
            unload: vec![sv(&["launchctl", "bootout", &format!("gui/{uid}/{label}")])],
            stop: vec![sv(&["launchctl", "bootout", &format!("gui/{uid}/{label}")])],
            path,
            text,
        };
    }
    if cfg!(windows) {
        let name = format!("TaskQ-{project}");
        let path = install.join(format!("{name}.xml"));
        // this user's logon only: an any-user logon trigger needs admin to register
        // `whoami`, not USERDOMAIN: an ssh session reports WORKGROUP and schtasks rejects `WORKGROUP\<user>`
        let user = Command::new("whoami").output().ok().filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).filter(|u| !u.is_empty())
            .unwrap_or_else(|| format!("{}\\{}", std::env::var("USERDOMAIN").unwrap_or_default(), std::env::var("USERNAME").unwrap_or_default()));
        let text = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-16\"?>\n<Task version=\"1.2\" xmlns=\"http://schemas.microsoft.com/windows/2004/02/mit/task\">\n<Triggers><LogonTrigger><Enabled>true</Enabled><UserId>{u}</UserId></LogonTrigger></Triggers>\n<Principals><Principal id=\"Author\"><UserId>{u}</UserId><LogonType>InteractiveToken</LogonType><RunLevel>LeastPrivilege</RunLevel></Principal></Principals>\n<Settings><MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy><DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries><StopIfGoingOnBatteries>false</StopIfGoingOnBatteries><ExecutionTimeLimit>PT0S</ExecutionTimeLimit><RestartOnFailure><Interval>PT1M</Interval><Count>999</Count></RestartOnFailure><Hidden>true</Hidden></Settings>\n<Actions Context=\"Author\"><Exec><Command>{l}</Command><Arguments>service run</Arguments><WorkingDirectory>{r}</WorkingDirectory></Exec></Actions>\n</Task>\n",
            u = xml(&user), l = xml(&l), r = xml(&r)
        );
        let p = path.display().to_string();
        return Unit {
            load: vec![sv(&["schtasks", "/Create", "/TN", &name, "/XML", &p, "/F"]), sv(&["schtasks", "/Run", "/TN", &name])],
            unload: vec![sv(&["schtasks", "/End", "/TN", &name]), sv(&["schtasks", "/Delete", "/TN", &name, "/F"])],
            stop: vec![sv(&["schtasks", "/End", "/TN", &name])],
            path,
            text,
        };
    }
    let name = format!("taskq-{project}.service");
    let path = home.join(".config/systemd/user").join(&name);
    let text = format!(
        "[Unit]\nDescription=taskq {project}\n\n[Service]\nExecStart=\"{l}\" service run\nWorkingDirectory={r}\nRestart=always\nRestartSec=10\nEnvironment=PATH={path_env}\n\n[Install]\nWantedBy=default.target\n"
    );
    Unit {
        load: vec![sv(&["systemctl", "--user", "daemon-reload"]), sv(&["systemctl", "--user", "enable", "--now", &name])],
        unload: vec![sv(&["systemctl", "--user", "disable", "--now", &name])],
        stop: vec![sv(&["systemctl", "--user", "stop", &name])],
        path,
        text,
    }
}

pub fn write_unit(u: &Unit) -> Result<(), String> {
    fs::create_dir_all(u.path.parent().unwrap()).map_err(|e| e.to_string())?;
    let bytes: Vec<u8> = if cfg!(windows) {
        // schtasks reads task XML as UTF-16 with a BOM
        std::iter::once(0xFEFFu16).chain(u.text.encode_utf16()).flat_map(|c| c.to_le_bytes()).collect()
    } else {
        u.text.clone().into_bytes()
    };
    fs::write(&u.path, bytes).map_err(|e| format!("{}: {e}", u.path.display()))
}

pub fn run_all(cmds: &[Vec<String>], ignore: bool) -> Result<(), String> {
    for c in cmds {
        // captured, not inherited: a console-less caller (the tray) would give the child its own window
        let o = quiet(Command::new(&c[0]).args(&c[1..])).output().map_err(|e| format!("{}: {e}", c[0]))?;
        print!("{}", String::from_utf8_lossy(&o.stdout));
        eprint!("{}", String::from_utf8_lossy(&o.stderr));
        if !o.status.success() && !ignore {
            return Err(format!("`{}` failed: {}", c.join(" "), o.status));
        }
    }
    Ok(())
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
    fn versions_and_checksums() {
        assert!(newer("4.1.0", "4.0.12") && newer("4.0.0", "4.0.0-dev") && !newer("4.0.0", "4.0.0"));
        let bin = b"binary";
        let digest: String = ring::digest::digest(&ring::digest::SHA256, bin).as_ref().iter().map(|b| format!("{b:02x}")).collect();
        let sums = format!("{digest}  taskq-4.0.0-macos-aarch64\n");
        assert_eq!(verify(sums.as_bytes(), "taskq-4.0.0-macos-aarch64", bin), Ok(()));
        assert!(verify(sums.as_bytes(), "taskq-4.0.0-macos-aarch64", b"evil").is_err());
        assert!(verify(sums.as_bytes(), "taskq-4.0.0-linux-x86_64", bin).is_err());
    }
}
