use std::{fs, process::Command};

// Chain: unavailable receipts -> Resume refuses before mutation, Pause remains usable, then durable recovery.
#[test]
fn focus_cli_requires_receipts_without_blocking_safe_pause() {
    let root=std::env::temp_dir().join(format!("qc-receipt-{}",std::process::id()));
    let install=root.join("i");fs::create_dir_all(&install).unwrap();fs::create_dir_all(root.join(".taskq")).unwrap();
    fs::write(root.join(".taskq/focus"),"off").unwrap();fs::write(root.join(".taskq/focus.paused"),"all").unwrap();
    fs::write(install.join("hold"),"owner hold").unwrap();
    fs::write(install.join("telemetry.log"),"store unavailable").unwrap();
    let invoke=|command:&[&str]|Command::new(env!("CARGO_BIN_EXE_taskq")).arg("service").args(command)
        .current_dir(&root).env("TASKQ_ROOT",&root).env("TASKQ_INSTALL",&install).env("TASKQ_INCIDENT_REPORTS","off").output().unwrap();
    let refused=invoke(&["resume"]);
    assert!(!refused.status.success(),"Resume accepted without a committed receipt");
    assert_eq!(fs::read_to_string(root.join(".taskq/focus")).unwrap(),"off");
    assert!(String::from_utf8_lossy(&refused.stderr).contains("command not applied: receipt unavailable"));
    fs::write(root.join(".taskq/focus"),"all").unwrap();
    let paused=invoke(&["pause"]);
    assert!(!paused.status.success(),"unrecorded Pause must report uncertainty");
    assert_eq!(fs::read_to_string(root.join(".taskq/focus")).unwrap(),"off","receipt failure blocked safe Pause");
    assert!(String::from_utf8_lossy(&paused.stderr).contains("applied locally; durable receipt unknown"));
    assert!(!invoke(&["focus","all"]).status.success());
    assert_eq!(fs::read_to_string(root.join(".taskq/focus")).unwrap(),"off");
    fs::write(root.join(".taskq/focus"),"all").unwrap();
    let off=invoke(&["focus","off"]);assert!(!off.status.success());
    assert_eq!(fs::read_to_string(root.join(".taskq/focus")).unwrap(),"off");
    assert!(String::from_utf8_lossy(&off.stderr).contains("applied locally; durable receipt unknown"));
    fs::remove_file(install.join("telemetry.log")).unwrap();
    let resumed=invoke(&["resume"]);assert!(resumed.status.success(),"{}",String::from_utf8_lossy(&resumed.stderr));
    assert!(String::from_utf8_lossy(&resumed.stdout).contains("service observation pending"));
    assert_eq!(fs::read_to_string(root.join(".taskq/focus")).unwrap(),"all");
    assert_eq!(fs::read_to_string(install.join("hold")).unwrap(),"owner hold");
    let reader=coreq::telemetry::Reader::open(&install.join("telemetry.log/universal.sqlite3")).unwrap();
    let page=reader.export(0,0,1024).unwrap();
    let output=String::from_utf8_lossy(&resumed.stdout);
    let printed=output.lines().find_map(|line|line.strip_prefix("Command applied locally; service observation pending; receipt ")).unwrap();
    assert!(page["events"].as_array().unwrap().iter().any(|row|row["id"]==printed&&row["observation"]["kind"]=="command.applied"));
    let receipts:Vec<_>=page["events"].as_array().unwrap().iter().map(|r|&r["observation"]).filter(|r|r["source"]=="command").collect();
    assert_eq!(receipts.len(),2);assert_eq!(receipts[0]["kind"],"command.received");assert_eq!(receipts[1]["kind"],"command.applied");
    assert_eq!(receipts[0]["span"],receipts[1]["span"]);assert_eq!(receipts[0]["operation"],"taskq.resume");
    drop(reader);fs::remove_dir_all(root).unwrap();
}

// Chain: independent command handoff -> real CLI refusal -> causal history, with owner hold preserved.
#[test]
fn independent_cli_preserves_the_dispatch_trace_on_failure() {
    let root=std::env::temp_dir().join(format!("qc-link-{}",std::process::id()));
    let install=root.join("i");fs::create_dir_all(&install).unwrap();fs::create_dir_all(root.join(".taskq")).unwrap();
    fs::write(install.join("hold"),"owner-held fixture").unwrap();
    fs::write(root.join(".taskq/focus"),"off").unwrap();
    let trace=format!("{}.1","a".repeat(32));let parent=format!("{}.2","b".repeat(32));
    let out=Command::new(env!("CARGO_BIN_EXE_taskq")).args(["service","resume"])
        .current_dir(&root).env("TASKQ_ROOT",&root).env("TASKQ_INSTALL",&install)
        .env("TASKQ_INCIDENT_REPORTS","off").env("COREQ_TRACE_CONTEXT",format!("1 {trace} {parent}"))
        .output().unwrap();
    assert!(!out.status.success());assert!(String::from_utf8_lossy(&out.stderr).contains("no saved focus"));
    assert_eq!(fs::read_to_string(root.join(".taskq/focus")).unwrap(),"off");
    assert_eq!(fs::read_to_string(install.join("hold")).unwrap(),"owner-held fixture");
    let reader=coreq::telemetry::Reader::open(&install.join("telemetry.log/universal.sqlite3")).unwrap();
    let exported=reader.export(0,0,1024).unwrap();
    let rows:Vec<_>=exported["events"].as_array().unwrap().iter().map(|r|&r["observation"]).collect();
    let terminal=rows.iter().find(|r|r["operation"]=="taskq.resume"&&r["kind"]=="method").unwrap();
    assert_eq!(terminal["trace"],trace,"CLI refusal lost its dispatch identity");
    assert_eq!(terminal["outcome"],"failed");
    assert!(rows.iter().any(|r|r["operation"]=="taskq.continuation"&&r["kind"]=="method.start"&&r["trace"]==trace&&r["parent"]==parent));
    drop(reader);fs::remove_dir_all(root).unwrap();
}

// Chain: emergency state -> all registered wakeups -> partial failure, without undoing safety state.
#[test]
fn emergency_state_survives_failed_wakeup_and_notifies_other_projects() {
    let root=std::env::temp_dir().join(format!("qh-{}",std::process::id()));
    let install=root.join("i"); let good=root.join("good"); let bad=root.join("bad");
    fs::create_dir_all(&install).unwrap();
    fs::create_dir_all(bad.join(".taskq/poke")).unwrap();
    fs::write(install.join("projects"),format!("{}\n{}\n",bad.display(),good.display())).unwrap();
    let invoke=|command|Command::new(env!("CARGO_BIN_EXE_taskq")).args(["service",command])
        .current_dir(&root).env("TASKQ_ROOT",&root).env("TASKQ_INSTALL",&install)
        .env("TASKQ_INCIDENT_REPORTS","off").output().unwrap();
    for command in ["hold","release"] {
        let out=invoke(command);
        assert!(!out.status.success());
        assert!(String::from_utf8_lossy(&out.stderr).contains("1 service wake(s) failed"));
        assert_eq!(install.join("hold").is_file(),command=="hold");
        assert!(good.join(".taskq/poke").is_file(),"first failure prevented notification of the next project");
        fs::remove_file(good.join(".taskq/poke")).unwrap();
    }
    fs::remove_file(install.join("projects")).unwrap();
    fs::create_dir(install.join("projects")).unwrap();
    let out=invoke("hold");
    assert!(!out.status.success()); assert!(install.join("hold").is_file());
    assert!(String::from_utf8_lossy(&out.stderr).contains("project registry unreadable"));
    let store=coreq::telemetry::Reader::open(&install.join("telemetry.log/universal.sqlite3")).unwrap();
    let rows=store.export(0,0,1024).unwrap();
    assert!(rows["events"].as_array().unwrap().iter().any(|r|r["observation"]["error"]=="registry-unreadable"));
    drop(store); fs::remove_dir_all(root).unwrap();
}

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
    fs::write(root.join(".taskq/focus"),"off").unwrap();
    fs::remove_file(root.join(".taskq/poke")).unwrap();
    fs::create_dir(root.join(".taskq/poke")).unwrap();
    let undelivered=invoke();
    assert!(!undelivered.status.success(),"Resume reported success when its wake signal could not be written");
    assert!(String::from_utf8_lossy(&undelivered.stderr).contains("service wake failed"));
    assert!(String::from_utf8_lossy(&undelivered.stderr).contains("os error"),"caller lost the original filesystem error");
    assert_eq!(fs::read_to_string(root.join(".taskq/focus")).unwrap(),"off","an unavailable command boundary must refuse before mutation");
    assert_eq!(fs::read_to_string(install.join("hold")).unwrap(),"owner-held fixture");
    let exported=Command::new(env!("CARGO_BIN_EXE_taskq")).args(["profile","export"])
        .current_dir(&root).env("TASKQ_ROOT",&root).env("TASKQ_INSTALL",&install)
        .env("TASKQ_INCIDENT_REPORTS","off").output().unwrap();
    assert!(exported.status.success(),"{}",String::from_utf8_lossy(&exported.stderr));
    let export=String::from_utf8(exported.stdout).unwrap();
    let rows:Vec<serde_json::Value>=export.lines().map(|line|serde_json::from_str(line).unwrap()).collect();
    assert_eq!(rows[0]["schema"],2);assert_eq!(rows[0]["coverage"],"committed-import-cursor-snapshot");
    let observations:Vec<_>=rows[1..].iter().map(|r|&r["observation"]).collect();
    let terminals:Vec<_>=observations.iter().filter(|r|r["operation"]=="taskq.resume"&&r["kind"]=="method").collect();
    assert_eq!(terminals.len(),3);
    assert_eq!(terminals[0]["outcome"],"failed");assert_eq!(terminals[1]["outcome"],"ok");
    assert_eq!(terminals[2]["outcome"],"failed");
    assert!(observations.iter().any(|r|r["operation"]=="taskq.operation.wake.lock"&&r["kind"]=="method"&&r["error"]=="wake-open"&&r["trace"]==terminals[2]["trace"]));
    assert_ne!(terminals[0]["trace"],terminals[1]["trace"]);
    for terminal in terminals {
        let phases:Vec<_>=observations.iter().filter(|r|r["source"]=="command"&&r["trace"]==terminal["trace"]).map(|r|r["kind"].as_str().unwrap()).collect();
        assert_eq!(phases,["command.received",if terminal["outcome"]=="ok" {"command.applied"} else {"command.failed"}]);
        assert!(terminal["duration_us"].as_u64().is_some());
        assert!(observations.iter().any(|r|r["kind"]=="method.start"&&r["span"]==terminal["span"]&&r["trace"]==terminal["trace"]));
        assert!(observations.iter().any(|r|r["span"]==terminal["parent"]&&r["trace"]==terminal["trace"]));
    }
    assert!(!export.contains("owner-held fixture"));
    assert!(!export.contains(root.to_str().unwrap()));
    #[cfg(target_os="macos")]
    {
        use std::io::BufRead;
        struct Owner(std::process::Child);
        impl Drop for Owner {fn drop(&mut self){let _=self.0.kill();let _=self.0.wait();}}
        let mut owner=Owner(Command::new(std::env::current_exe().unwrap())
            .args(["--exact","resident_analysis_owner","--ignored","--nocapture"])
            .env("TASKQ_ANALYSIS_OWNER",install.join("telemetry.log/universal.sqlite3"))
            .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).spawn().unwrap());
        let mut ready=std::io::BufReader::new(owner.0.stdout.take().unwrap());
        loop {let mut line=String::new();assert_ne!(ready.read_line(&mut line).unwrap(),0,"collector stopped before ready");if line.trim()=="ANALYSIS_READY" {break;}}
        let policy=format!("(version 1)(allow default)(deny file-write* (subpath {}))",serde_json::to_string(install.canonicalize().unwrap().to_str().unwrap()).unwrap());
        let denied=Command::new("/usr/bin/sandbox-exec").args(["-p",&policy,"/usr/bin/touch"])
            .arg(install.join("write-probe")).output().unwrap();
        assert!(!denied.status.success(),"test sandbox did not deny writes");
        let read=Command::new("/usr/bin/sandbox-exec").args(["-p",&policy,env!("CARGO_BIN_EXE_taskq"),"profile","export"])
            .current_dir(&root).env("TASKQ_ROOT",&root).env("TASKQ_INSTALL",&install)
            .env("TASKQ_INCIDENT_REPORTS","off").output().unwrap();
        assert!(read.status.success(),"{}",String::from_utf8_lossy(&read.stderr));
        assert!(String::from_utf8_lossy(&read.stdout).contains("taskq.resume"));
        assert!(!String::from_utf8_lossy(&read.stderr).contains("Telemetry: unknown"));
        assert!(!install.join("write-probe").exists());
        drop(owner.0.stdin.take());assert!(owner.0.wait().unwrap().success());
        let snapshot=coreq::telemetry::Reader::open(&install.join("telemetry.log/universal.sqlite3")).unwrap();
        let page=snapshot.export(0,0,1024).unwrap();
        assert!(page["events"].as_array().unwrap().iter().filter(|r|r["observation"]["operation"]=="taskq.telemetry.export"&&r["observation"]["kind"]=="method"&&r["observation"]["outcome"]=="ok").count()>=2,"protected analysis did not leave its own committed terminal observation");
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
#[ignore="resident shared collector subprocess for the protected analysis chain"]
fn resident_analysis_owner() {
    use std::io::Write;
    let path=std::path::PathBuf::from(std::env::var_os("TASKQ_ANALYSIS_OWNER").unwrap());
    let dir=path.parent().unwrap();let install=dir.parent().unwrap();
    let budget=coreq::telemetry::Budget {lock:install.join("taskq-profile.lock"),
        peers:vec![dir.join("telemetry.sqlite3"),dir.join("telemetry.sqlite3-wal"),dir.join("telemetry.sqlite3-shm"),install.join("taskq.log")],
        total:64<<20,reserved:(4<<20)+(64<<10)+(2<<20)};
    let _runtime=coreq::telemetry::capture::Runtime::install_bounded(&path,"fixtureq","1",budget).unwrap();
    println!("ANALYSIS_READY");std::io::stdout().flush().unwrap();
    let mut line=String::new();std::io::stdin().read_line(&mut line).unwrap();
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
        if success {
            // Installed bootstrap must collect in the executable's root before launcher/bundle dispatch.
            let home=root.join("home");let local=root.join("local");
            fs::create_dir(&home).unwrap();fs::create_dir(&local).unwrap();
            let release=install.join("releases").join(env!("CARGO_PKG_VERSION")).join(format!("taskq{}",std::env::consts::EXE_SUFFIX));
            fs::create_dir_all(release.parent().unwrap()).unwrap();
            fs::write(install.join("current"),env!("CARGO_PKG_VERSION")).unwrap();
            let mut entrypoints=vec![install.join(format!("taskq{}",std::env::consts::EXE_SUFFIX)),release];
            #[cfg(target_os="macos")]
            entrypoints.push(install.join("TaskQ.app/Contents/MacOS/taskq"));
            for executable in &entrypoints {
                fs::create_dir_all(executable.parent().unwrap()).unwrap();fs::copy(env!("CARGO_BIN_EXE_taskq"),executable).unwrap();
            }
            fs::remove_dir_all(install.join("telemetry.log")).unwrap();
            for executable in entrypoints {
                let mut command=Command::new(executable);
                command.arg("help").current_dir(&root).env_remove("TASKQ_INSTALL").env_remove("COREQ_TRACE_CONTEXT")
                    .env("HOME",&home).env("LOCALAPPDATA",&local).env("TASKQ_ROOT",&root).env("TASKQ_INCIDENT_REPORTS","off");
                let out=coreq::process::output("test.installed-capture",&mut command,std::time::Duration::from_secs(15)).unwrap();
                assert!(String::from_utf8_lossy(&out.stdout).starts_with("taskq agents"));
                let reader=coreq::telemetry::Reader::open(&install.join("telemetry.log/universal.sqlite3")).unwrap();
                let history=reader.export(0,0,1024).unwrap();
                assert!(history["events"].as_array().unwrap().iter().any(|r|r["observation"]["operation"]=="taskq.run"&&r["observation"]["kind"]=="method"));
                drop(reader);fs::remove_dir_all(install.join("telemetry.log")).unwrap();
                assert!(!home.join(".local").exists()&&!local.join("taskq").exists(),"installed bootstrap created a second installation");
            }
        }
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
    drop(store);
    #[cfg(unix)] {
        use std::{io::Write,process::Stdio,time::{Duration,Instant}};
        fs::write(good.join("telemetry.log/diagnostic.json"),"{broken control").unwrap();
        let invalid_context=root.join("invalid-context");
        for (install,context) in [(&blocked,""),(&good,""),(&invalid_context,"invalid-process-context")] {
            // The shell waits until the reader is closed, then execs the actual product once.
            let mut command=Command::new("/bin/sh");
            command.args(["-c","read -r go || exit 2; exec \"$@\"","owned-capture-fixture",env!("CARGO_BIN_EXE_taskq"),"help"])
                .current_dir(&root).env("TASKQ_INSTALL",install).env("TASKQ_ROOT",&root).env("TASKQ_INCIDENT_REPORTS","off").env_remove("COREQ_TRACE_CONTEXT")
                .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
            if !context.is_empty() {command.env("COREQ_TRACE_CONTEXT",context);}
            let mut child=command.spawn().unwrap();
            drop(child.stderr.take());child.stdin.take().unwrap().write_all(b"go\n").unwrap();
            let until=Instant::now()+Duration::from_secs(15);
            while child.try_wait().unwrap().is_none()&&Instant::now()<until {std::thread::sleep(Duration::from_millis(10));}
            let expired=child.try_wait().unwrap().is_none();if expired {child.kill().unwrap();}
            let result=child.wait_with_output().unwrap();
            assert!(!expired&&result.status.success(),"optional reporting changed ordinary help: {result:?}");
            assert!(String::from_utf8_lossy(&result.stdout).starts_with("taskq agents"));
        }
        assert_eq!(fs::read_to_string(good.join("telemetry.log/diagnostic.json")).unwrap(),"{broken control");
    }
    fs::remove_dir_all(root).unwrap();
}

// Chain: actual maintenance CLI bypasses collector bootstrap and refuses an existing live owner.
#[test]
fn migration_cli_uses_quiesced_owner_and_preserves_legacy_text() {
    let root=std::env::temp_dir().join(format!("qm-{}",std::process::id()));
    let install=root.join("i");fs::create_dir_all(&install).unwrap();
    let line=r#"taskq-profile {"op":"spawn","us":7,"error":"none","at_ms":1}"#;
    fs::write(install.join("taskq.log"),format!("{line}\n")).unwrap();
    let invoke=||Command::new(env!("CARGO_BIN_EXE_taskq")).args(["profile","migrate"])
        .current_dir(&root).env("TASKQ_INSTALL",&install).env_remove("TASKQ_ROOT")
        .env("TASKQ_INCIDENT_REPORTS","off").output().unwrap();
    let path=install.join("telemetry.log/universal.sqlite3");
    use std::io::BufRead;
    struct Owner(std::process::Child);
    impl Drop for Owner {fn drop(&mut self){let _=self.0.kill();let _=self.0.wait();}}
    let mut owner=Owner(Command::new(std::env::current_exe().unwrap())
        .args(["--exact","resident_analysis_owner","--ignored","--nocapture"])
        .env("TASKQ_ANALYSIS_OWNER",&path).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).spawn().unwrap());
    let mut ready=std::io::BufReader::new(owner.0.stdout.take().unwrap());
    loop {let mut line=String::new();assert_ne!(ready.read_line(&mut line).unwrap(),0,"collector stopped before ready");if line.trim()=="ANALYSIS_READY" {break;}}
    let blocked=invoke();assert!(!blocked.status.success());
    assert!(String::from_utf8_lossy(&blocked.stderr).contains("requires quiesced IPC writer"));
    assert!(!install.join("telemetry.log/telemetry.sqlite3").exists());
    drop(owner.0.stdin.take());assert!(owner.0.wait().unwrap().success());
    let result=invoke();assert!(result.status.success(),"{}",String::from_utf8_lossy(&result.stderr));
    assert!(String::from_utf8_lossy(&result.stdout).contains("Migrated 1 legacy records"));
    let again=invoke();assert!(again.status.success(),"{}",String::from_utf8_lossy(&again.stderr));
    assert!(String::from_utf8_lossy(&again.stdout).contains("Migrated 0 legacy records"));
    assert_eq!(fs::read_to_string(install.join("taskq.log")).unwrap(),format!("{line}\n"));
    let reader=coreq::telemetry::Reader::open(&path).unwrap();assert!(reader.import_cursor("taskq.legacy").unwrap().is_some());
    let page=reader.export_kinds(&["operation"],0,0,1024).unwrap();assert_eq!(page["events"].as_array().unwrap().len(),1);
    drop(reader);fs::remove_dir_all(root).unwrap();
}
