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
pub fn launch(install: &Path, args: &[String]) -> ! {
    let mut v = current(install).unwrap_or_else(|| die("no current release: run `taskq update`"));
    if args.first().map(String::as_str) == Some("service") && args.get(1).map(String::as_str) == Some("run") {
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
    let mut c = Command::new(release_exe(install, &v));
    c.args(args);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let e = c.exec();
        die(&format!("exec {v}: {e}"));
    }
    #[cfg(not(unix))]
    {
        let code = c.status().map(|s| s.code().unwrap_or(1)).unwrap_or_else(|e| die(&format!("start {v}: {e}")));
        std::process::exit(code);
    }
}

pub fn die(msg: &str) -> ! {
    eprintln!("taskq: {msg}");
    std::process::exit(1);
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

/// § 8 update steps: Ok(Some(version)) when a new release became current.
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
        println!("update available: {running} -> {ver} ({name})");
        return Ok(None);
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
        let log = install.join(format!("{project}.log")).display().to_string();
        let text = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict>\n<key>Label</key><string>{}</string>\n<key>ProgramArguments</key><array><string>{}</string><string>service</string><string>run</string></array>\n<key>WorkingDirectory</key><string>{}</string>\n<key>RunAtLoad</key><true/>\n<key>KeepAlive</key><true/>\n<key>StandardOutPath</key><string>{}</string>\n<key>StandardErrorPath</key><string>{}</string>\n<key>EnvironmentVariables</key><dict><key>PATH</key><string>{}</string></dict>\n</dict></plist>\n",
            xml(&label), xml(&l), xml(&r), xml(&log), xml(&log), xml(&path_env)
        );
        let uid = Command::new("id").arg("-u").output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
        let p = path.display().to_string();
        return Unit {
            load: vec![sv(&["launchctl", "bootstrap", &format!("gui/{uid}"), &p])],
            unload: vec![sv(&["launchctl", "bootout", &format!("gui/{uid}/{label}")])],
            path,
            text,
        };
    }
    if cfg!(windows) {
        let name = format!("TaskQ-{project}");
        let path = install.join(format!("{name}.xml"));
        let text = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-16\"?>\n<Task version=\"1.2\" xmlns=\"http://schemas.microsoft.com/windows/2004/02/mit/task\">\n<Triggers><LogonTrigger><Enabled>true</Enabled></LogonTrigger></Triggers>\n<Settings><MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy><DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries><StopIfGoingOnBatteries>false</StopIfGoingOnBatteries><ExecutionTimeLimit>PT0S</ExecutionTimeLimit><RestartOnFailure><Interval>PT1M</Interval><Count>999</Count></RestartOnFailure><Hidden>true</Hidden></Settings>\n<Actions><Exec><Command>{}</Command><Arguments>service run</Arguments><WorkingDirectory>{}</WorkingDirectory></Exec></Actions>\n</Task>\n",
            xml(&l), xml(&r)
        );
        let p = path.display().to_string();
        return Unit {
            load: vec![sv(&["schtasks", "/Create", "/TN", &name, "/XML", &p, "/F"]), sv(&["schtasks", "/Run", "/TN", &name])],
            unload: vec![sv(&["schtasks", "/End", "/TN", &name]), sv(&["schtasks", "/Delete", "/TN", &name, "/F"])],
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
        let s = Command::new(&c[0]).args(&c[1..]).status().map_err(|e| format!("{}: {e}", c[0]))?;
        if !s.success() && !ignore {
            return Err(format!("`{}` failed: {s}", c.join(" ")));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
