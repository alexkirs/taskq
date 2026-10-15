#![cfg(windows)]

use std::{fs, process::Command};

// Chain: § 8 Windows service install → release/launcher start. Risk: GNU's loader DLL stays in Cargo's build paths.
#[test]
fn installed_windows_release_starts_without_build_paths() {
    let dir = std::env::temp_dir().join(format!("taskq-windows-install-{}", std::process::id()));
    fs::create_dir_all(dir.join("project/.taskq")).unwrap();
    let root = dir.join("project");
    let appdata = dir.join("appdata");
    let install = appdata.join("taskq");
    fs::write(root.join("taskq.json"), r#"{"repo":"taskqorg/taskq"}"#).unwrap();
    // A fixture child defers unit reload: exercise real installation without registering or starting an OS service.
    fs::write(root.join(".taskq/status.json"), r#"{"kids":[{"task":993}]}"#).unwrap();
    for _ in 0..2 {
        let out = Command::new(env!("CARGO_BIN_EXE_taskq"))
            .args(["service", "install"]).current_dir(&root)
            .env("TASKQ_ROOT", &root)
            .env("LOCALAPPDATA", &appdata).env("TASKQ_INCIDENT_REPORTS", "off")
            .output().unwrap();
        assert!(out.status.success(), "install: {} {}", out.status, String::from_utf8_lossy(&out.stderr));
        assert!(root.join(".taskq/restart").exists(), "unit reload was not deferred");
    }
    let version = fs::read_to_string(install.join("current")).unwrap();
    let release = install.join("releases").join(version.trim());
    let system32 = std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join("System32");
    for exe in [release.join("taskq.exe"), install.join("taskq.exe")] {
        let out = Command::new(&exe).arg("selftest").current_dir(&dir)
            .env("PATH", &system32).env("LOCALAPPDATA", &appdata)
            .env("COREQ_TELEMETRY_STORE_TRACE", "1")
            .env("TASKQ_ROOT", &root)
            .env("TASKQ_SELFTEST_OFFLINE", "1").env("TASKQ_INCIDENT_REPORTS", "off")
            .output().unwrap();
        let detail = fs::read_to_string(root.join(".taskq/cli-errors.log")).unwrap_or_default();
        assert!(out.status.success(), "{}: {} {}\n{}", exe.display(), out.status, String::from_utf8_lossy(&out.stderr), detail.chars().take(4096).collect::<String>());
        assert!(String::from_utf8_lossy(&out.stdout).contains("golden traces PASS"));
    }
    #[cfg(target_env = "gnu")]
    for folder in [&install, &release] {
        assert!(folder.join("WebView2Loader.dll").is_file(), "missing loader in {}", folder.display());
    }
    fs::remove_dir_all(dir).unwrap();
}
