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
