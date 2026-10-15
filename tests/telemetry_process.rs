use std::{fs, process::Command};

// Chain: Resume refusal/success -> focus readback -> durable linked trace after process exit.
// Risk: a successful command is mistaken for admission, or a failed command disappears from history.
#[test]
fn resume_records_its_result_without_releasing_the_host_hold() {
    let root=std::env::temp_dir().join(format!("qr-{}",std::process::id()));
    fs::create_dir_all(root.join(".taskq")).unwrap();
    let install=root.join("i");fs::create_dir_all(&install).unwrap();
    fs::write(install.join("hold"),"owner-held fixture").unwrap();
    fs::write(root.join(".taskq/focus"),"off").unwrap();
    let invoke=||Command::new(env!("CARGO_BIN_EXE_taskq")).args(["service","resume"])
        .current_dir(&root).env("TASKQ_ROOT",&root).env("TASKQ_INSTALL",&install)
        .env("TASKQ_INCIDENT_REPORTS","off").output().unwrap();
    let refused=invoke();
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("no saved focus"));
    assert_eq!(fs::read_to_string(root.join(".taskq/focus")).unwrap(),"off");
    fs::write(root.join(".taskq/focus.paused"),"telemetry-fixture").unwrap();
    let resumed=invoke();
    assert!(resumed.status.success(),"{}",String::from_utf8_lossy(&resumed.stderr));
    assert_eq!(fs::read_to_string(root.join(".taskq/focus")).unwrap(),"telemetry-fixture");
    assert_eq!(fs::read_to_string(install.join("hold")).unwrap(),"owner-held fixture");
    let store=coreq::telemetry::Store::open(&install.join("telemetry.log/universal.sqlite3")).unwrap();
    let export=store.export(0,0,1024).unwrap();
    let observations:Vec<_>=export["events"].as_array().unwrap().iter().map(|r|&r["observation"]).collect();
    let terminals:Vec<_>=observations.iter().filter(|r|r["operation"]=="taskq.resume"&&r["kind"]=="method").collect();
    assert_eq!(terminals.len(),2);
    assert_eq!(terminals[0]["outcome"],"failed");assert_eq!(terminals[1]["outcome"],"ok");
    assert_ne!(terminals[0]["trace"],terminals[1]["trace"]);
    for terminal in terminals {
        assert!(terminal["duration_us"].as_u64().is_some());
        assert!(observations.iter().any(|r|r["kind"]=="method.start"&&r["span"]==terminal["span"]&&r["trace"]==terminal["trace"]));
        assert!(observations.iter().any(|r|r["span"]==terminal["parent"]&&r["trace"]==terminal["trace"]));
    }
    assert!(!export.to_string().contains("owner-held fixture"));
    assert!(!export.to_string().contains(root.to_str().unwrap()));
    drop(store);fs::remove_dir_all(root).unwrap();
}

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
        .env("TASKQ_SELFTEST_OFFLINE","1").env("TASKQ_INCIDENT_REPORTS","off").env("COREQ_TELEMETRY_STORE_TRACE","1").output().unwrap();
    let help=invoke(&blocked,"help");assert!(help.status.success(),"ordinary help outcome changed");
    let failed=invoke(&blocked,"selftest");
    assert!(!failed.status.success(),"selftest accepted unavailable capture: {}",String::from_utf8_lossy(&failed.stdout));
    assert!(String::from_utf8_lossy(&failed.stderr).contains("telemetry: FAIL telemetry collector unavailable"));
    assert!(!String::from_utf8_lossy(&failed.stdout).contains("telemetry: local capture committed"));
    assert_eq!(fs::read_to_string(&blocked).unwrap(),"fixture");
    let good=root.join("good");let success=invoke(&good,"selftest");
    let detail=fs::read_to_string(root.join(".taskq/cli-errors.log")).unwrap_or_default();
    assert!(success.status.success(),"{}\n{}",String::from_utf8_lossy(&success.stderr),detail.chars().take(4096).collect::<String>());
    assert!(String::from_utf8_lossy(&success.stdout).contains("telemetry: local capture committed"));
    let store=coreq::telemetry::Store::open(&good.join("telemetry.log/universal.sqlite3")).unwrap();
    assert!(!store.export(0,0,1024).unwrap()["events"].as_array().unwrap().is_empty());
    drop(store);fs::remove_dir_all(root).unwrap();
}
