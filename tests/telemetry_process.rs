use std::{fs, process::Command};

// Real binary -> shared bootstrap -> generated methods -> return/exit -> offline durable readback.
#[test]
fn commands_collect_without_configuration_and_flush_on_exit() {
    for (_name, args, success) in [("help", vec!["help"], true), ("failure", vec!["service", "run"], false)] {
        let root = std::env::temp_dir().join(format!("qt-{}-{}", u8::from(success), std::process::id()));
        if root.exists() { fs::remove_dir_all(&root).unwrap(); }
        fs::create_dir_all(&root).unwrap();
        let install = root.join("install");
        let out = Command::new(env!("CARGO_BIN_EXE_taskq")).args(args).current_dir(&root)
            .env("TASKQ_INSTALL", &install).env("TASKQ_ROOT", &root)
            .env("TASKQ_INCIDENT_REPORTS", "off").output().unwrap();
        assert_eq!(out.status.success(), success, "{}", String::from_utf8_lossy(&out.stderr));
        assert!(!String::from_utf8_lossy(&out.stderr).contains("Telemetry: unknown"), "{}", String::from_utf8_lossy(&out.stderr));
        let store = coreq::telemetry::Store::open(&install.join("telemetry.log/universal.sqlite3")).unwrap();
        let export = store.export(0, 0, 1024).unwrap();
        let rows = export["events"].as_array().unwrap();
        assert!(rows.iter().any(|r| r["observation"]["operation"] == "taskq.application"
            && r["observation"]["kind"] == "method.start"));
        assert!(rows.iter().any(|r| r["observation"]["operation"] == "taskq.run"
            && r["observation"]["kind"] == "method"
            && r["observation"]["outcome"] == if success { "ok" } else { "failed" }));
        assert!(!export.to_string().contains(root.to_str().unwrap()), "host paths leaked");
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }
}

// An unavailable collector must fail qualification while leaving ordinary command semantics intact.
#[test]
fn selftest_requires_committed_local_capture() {
    let root=std::env::temp_dir().join(format!("qc-{}",std::process::id()));
    if root.exists() {fs::remove_dir_all(&root).unwrap();}
    fs::create_dir_all(&root).unwrap();
    let blocked=root.join("blocked");fs::write(&blocked,"fixture").unwrap();
    let invoke=|install: &std::path::Path,command: &str| Command::new(env!("CARGO_BIN_EXE_taskq"))
        .arg(command).current_dir(&root).env("TASKQ_INSTALL",install).env("TASKQ_ROOT",&root)
        .env("TASKQ_SELFTEST_OFFLINE","1").env("TASKQ_INCIDENT_REPORTS","off").output().unwrap();
    let help=invoke(&blocked,"help");assert!(help.status.success(),"ordinary help outcome changed");
    let failed=invoke(&blocked,"selftest");
    assert!(!failed.status.success(),"selftest accepted unavailable capture: {}",String::from_utf8_lossy(&failed.stdout));
    assert!(String::from_utf8_lossy(&failed.stderr).contains("telemetry: FAIL telemetry collector unavailable"));
    assert!(!String::from_utf8_lossy(&failed.stdout).contains("telemetry: local capture committed"));
    assert_eq!(fs::read_to_string(&blocked).unwrap(),"fixture");
    let good=root.join("good");let success=invoke(&good,"selftest");
    assert!(success.status.success(),"{}",String::from_utf8_lossy(&success.stderr));
    assert!(String::from_utf8_lossy(&success.stdout).contains("telemetry: local capture committed"));
    let store=coreq::telemetry::Store::open(&good.join("telemetry.log/universal.sqlite3")).unwrap();
    assert!(!store.export(0,0,1024).unwrap()["events"].as_array().unwrap().is_empty());
    drop(store);fs::remove_dir_all(root).unwrap();
}
