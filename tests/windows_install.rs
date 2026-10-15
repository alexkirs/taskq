#![cfg(windows)]

use std::{fs, process::Command};

// Chain: § 8 Windows service install → release/launcher start. Risk: GNU's loader DLL stays in Cargo's build paths.
#[test]
fn installed_windows_release_starts_without_build_paths() {
    assert_ne!(std::env::current_exe().unwrap().file_stem().unwrap(), "schtasks", "native manager calls are forbidden in this fixture");
    if std::env::var_os("TASKQ_INSTALL_COMMAND_PROBE").is_some() {
        let out = Command::new("schtasks").arg("--list").output().unwrap();
        assert!(out.status.success() && String::from_utf8_lossy(&out.stdout).contains("installed_windows_release_starts_without_build_paths"), "native command interception failed: {} {}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        return;
    }
    let dir = std::env::temp_dir().join(format!("taskq-windows-install-{}", std::process::id()));
    fs::create_dir_all(dir.join("project/.taskq")).unwrap();
    let root = dir.join("project");
    let appdata = dir.join("appdata");
    let install = appdata.join("taskq");
    // A failing install must not register a real task under the signed-in user's scheduler.
    let guard = root.join("native-guard");
    fs::create_dir_all(&guard).unwrap();
    fs::copy(std::env::current_exe().unwrap(), guard.join("schtasks.exe")).unwrap();
    fs::copy(std::env::current_exe().unwrap(), guard.join("probe.exe")).unwrap();
    fs::copy(env!("CARGO_BIN_EXE_taskq"), guard.join("taskq.exe")).unwrap();
    let path = std::env::join_paths(std::iter::once(guard.clone()).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap()))).unwrap();
    let probe = Command::new(guard.join("probe.exe")).args(["--exact", "installed_windows_release_starts_without_build_paths"])
        .current_dir(&guard).env("PATH", &path).env("TASKQ_INSTALL_COMMAND_PROBE", "1").output().unwrap();
    assert!(probe.status.success(), "native command interception: {} {}", String::from_utf8_lossy(&probe.stdout), String::from_utf8_lossy(&probe.stderr));
    fs::write(root.join("taskq.json"), r#"{"repo":"taskqorg/taskq"}"#).unwrap();
    // The host owns the child: a root-only snapshot no longer defers the host unit.
    fs::create_dir_all(&install).unwrap();
    fs::write(install.join("projects"), format!("{}\n", root.display())).unwrap();
    fs::write(install.join("service"), std::process::id().to_string()).unwrap();
    fs::write(install.join("service.json"), format!(r#"{{"pid":{},"projects":[],"used":{{}},"idle":false,"drain":false,"reason":null}}"#, std::process::id())).unwrap();
    fs::write(root.join(".taskq/status.json"), r#"{"kids":[{"task":993}]}"#).unwrap();
    for _ in 0..2 {
        let out = Command::new(guard.join("taskq.exe"))
            .args(["service", "install"]).current_dir(&guard).env("PATH", &path)
            .env("TASKQ_ROOT", &root)
            .env("LOCALAPPDATA", &appdata).env("TASKQ_INCIDENT_REPORTS", "off")
            .output().unwrap();
        assert!(out.status.success(), "install: {} {}", out.status, String::from_utf8_lossy(&out.stderr));
        assert_eq!(fs::read_to_string(install.join("restart")).unwrap(), "unit\n", "native unit reload was not deferred");
    }
    let version = fs::read_to_string(install.join("current")).unwrap();
    let release = install.join("releases").join(version.trim());
    let system32 = std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join("System32");
    for exe in [release.join("taskq.exe"), install.join("taskq.exe")] {
        let out = Command::new(&exe).arg("selftest").current_dir(&dir)
            .env("PATH", &system32).env("LOCALAPPDATA", &appdata)
            .env("TASKQ_ROOT", &root)
            .env("TASKQ_SELFTEST_OFFLINE", "1").env("TASKQ_INCIDENT_REPORTS", "off")
            .output().unwrap();
        assert!(out.status.success(), "{}: {} {}", exe.display(), out.status, String::from_utf8_lossy(&out.stderr));
        assert!(String::from_utf8_lossy(&out.stdout).contains("golden traces PASS"));
    }
    #[cfg(target_env = "gnu")]
    for folder in [&install, &release] {
        assert!(folder.join("WebView2Loader.dll").is_file(), "missing loader in {}", folder.display());
    }
    fs::remove_dir_all(dir).unwrap();
}
