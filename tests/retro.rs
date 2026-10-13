use std::{fs, process::Command};
use serde_json::json;

// Chain: § 4 request log → § 5 retro CLI. Risk: a new project loses PM proposals or panics without a log.
#[test]
fn missing_request_log_still_prints_a_proposal_recommendation() {
    let root = std::env::temp_dir().join(format!("taskq-retro-cli-{}", std::process::id()));
    fs::create_dir_all(root.join("docs/use-cases")).unwrap();
    assert!(!root.join(".taskq").exists());
    assert!(!root.join(".taskq/requests.log").exists());
    fs::write(root.join("docs/use-cases/README.md"), "# Use cases\n\n## Proposals\n- 2026-10-14 Answer all asks with one command\n").unwrap();
    let runtime = root.join(if cfg!(windows) { "retro-runtime.cmd" } else { "retro-runtime" });
    fs::write(&runtime, if cfg!(windows) {
        "@echo off\r\nmore > brief.txt\r\ncopy /y ..\\reply.json \"%~6\" >nul\r\n"
    } else {
        "#!/bin/sh\ncat > brief.txt\ncp ../reply.json \"$6\"\n"
    }).unwrap();
    #[cfg(unix)] {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700)).unwrap();
    }
    fs::write(root.join("taskq.json"), json!({"repo": "example/fixture", "limits": {runtime.to_str().unwrap(): 1}, "incident_reports": false}).to_string()).unwrap();
    fs::write(root.join("reply.json"), json!({"recommendations": [{
        "kind": "quick command", "title": "Answer all asks", "text": "Answer every ask in one command",
        "from": ["p1"], "goal": "Fewer steps", "acceptance": ["One command answers all asks"]
    }]}).to_string()).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_taskq")).arg("retro").current_dir(&root)
        .env("TASKQ_ROOT", &root).env("TASKQ_INCIDENT_REPORTS", "off").output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("0 requests") && text.contains("1 proposals"), "{text}");
    assert!(text.contains("1. [quick command] Answer all asks") && text.contains("From: p1"), "{text}");
    assert!(fs::read_to_string(root.join(".taskq/brief.txt")).unwrap().contains("p1 2026-10-14 pm: Answer all asks with one command"));
    assert!(!root.join(".taskq/requests.log").exists());
    fs::remove_dir_all(root).unwrap();
}
