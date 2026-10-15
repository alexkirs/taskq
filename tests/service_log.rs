use std::process::Command;

// Chain: § 4 service log lines on a failed start. Risk: a config error before the loop reaches the unit log unstamped
// and split over lines, repeated on each unit restart.
#[test]
fn service_run_start_error_is_one_stamped_line() {
    let root = std::env::temp_dir().join(format!("q-log-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_taskq")).args(["service", "run"]).env("TASKQ_ROOT", &root).env("TASKQ_INSTALL", root.join("install")).current_dir(&root).output().unwrap();
    let _ = std::fs::remove_dir_all(&root);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert_eq!(err.lines().count(), 1, "{err}");
    let (at, text) = err.trim_end().split_once(' ').unwrap();
    assert!(at.len() == 20 && at.ends_with('Z') && at.as_bytes()[10] == b'T', "{err}");
    assert!(text.starts_with("Error: ") && text.contains(" Next: "), "{err}");
}
