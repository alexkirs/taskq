use serde_json::{Value,json};
use std::{fs,process::Command,time::{Duration,SystemTime,UNIX_EPOCH}};

// H-A2: actual CLI -> authoritative host edit/readback; project caps and private owner fields stay intact.
#[test]
fn capacity_command_changes_host_policy_not_project_settings() {
    let stamp=SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let base=std::env::temp_dir().join(format!("taskq-host-policy-{}-{stamp}",std::process::id()));
    let root=base.join("project");
    let install=base.join("install");fs::create_dir_all(root.join(".taskq")).unwrap();fs::create_dir(&install).unwrap();
    let project=json!({"repo":"fixture/project","limits":{"codex":1},"admission":{"mode":"auto"},"owner_field":"preserve"}).to_string();
    fs::write(root.join("taskq.json"),&project).unwrap();
    fs::write(install.join("host.json"),json!({"limits":{"codex":2},"codex":["-s","workspace-write"],"permission_mode":"dontAsk","owner_field":"host-private"}).to_string()).unwrap();
    #[cfg(unix)] {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(install.join("host.json"),fs::Permissions::from_mode(0o600)).unwrap();
    }
    let at=SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()+10;
    let host=json!({"limits":{"codex":3},"load_gate":{"enabled":true,"cpu_max":90,"memory_min":10},"admission":{"mode":"auto","max":null}});
    // Include the explicit host policy alongside the project-effective snapshot; no running service or board.
    let mut capacity=host.clone();capacity["host_policy"]=host;capacity["limits"]["codex"]=json!(1);
    fs::write(root.join(".taskq/status.json"),json!({"host":"fixture-host","at":at,"capacity":capacity}).to_string()).unwrap();
    fs::write(install.join("projects"),format!("{}\n",root.display())).unwrap();
    let run_in=|cwd:&std::path::Path,args:&[&str]|match coreq::process::output("test.host-policy",Command::new(env!("CARGO_BIN_EXE_taskq"))
        .args(args)
        .env("TASKQ_INSTALL",&install).env_remove("TASKQ_ROOT").env("TASKQ_HOST","fixture-host")
        .env("TASKQ_INCIDENT_REPORTS","off").env_remove("TASKQ_ATTEMPT").env_remove("TASKQ_TASK")
        .env_remove("COREQ_TRACE_CONTEXT").current_dir(cwd),Duration::from_secs(10)) {
            Ok(out)|Err(coreq::process::Failure::Exit(out))=>out,
            Err(error)=>panic!("{error}"),
        };
    let run=|args:&[&str]|run_in(&root,args);
    let rootless=run_in(&install,&["service","capacity"]);
    assert!(rootless.status.success(),"host policy command required a project cwd: {}",String::from_utf8_lossy(&rootless.stderr));
    assert!(String::from_utf8_lossy(&rootless.stdout).contains("no fresh capacity status"),"future status was treated as current");
    let (status_path,policy_path,mut readback)=(root.join(".taskq/status.json"),install.join("host.json"),capacity);
    let publisher=std::thread::spawn(move|| {
        let until=std::time::Instant::now()+Duration::from_secs(8);
        while std::time::Instant::now()<until {
            let saved:Value=fs::read(&policy_path).ok().and_then(|b|serde_json::from_slice(&b).ok()).unwrap_or(Value::Null);
            if saved["limits"]["codex"]==3 {
                readback["host_policy"]["limits"]["codex"]=json!(3);
                let at=SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
                fs::write(status_path,json!({"host":"fixture-host","at":at,"capacity":readback}).to_string()).unwrap();
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("host-policy fixture did not observe the saved edit");
    });
    let output=run_in(&install,&["service","capacity","--host","fixture-host","--codex","3"]);
    publisher.join().unwrap();
    let after=fs::read_to_string(root.join("taskq.json")).unwrap();
    let saved:Value=serde_json::from_str(&fs::read_to_string(install.join("host.json")).unwrap()).unwrap();
    assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));
    assert!(String::from_utf8_lossy(&output.stdout).contains("Applied"),"host policy readback was confused with the narrowing project cap");
    assert_eq!(saved["limits"]["codex"],3,"CLI edited a different policy than the host service reads");
    assert_eq!(after,project,"host edit changed narrowing project settings");
    assert_eq!(saved["codex"],json!(["-s","workspace-write"]));assert_eq!(saved["owner_field"],"host-private");
    let read=run(&["service","capacity"]);assert!(read.status.success());
    assert!(String::from_utf8_lossy(&read.stdout).contains("Codex ?/3"),"host settings readback used the project's ceiling: {}",String::from_utf8_lossy(&read.stdout));
    #[cfg(unix)] {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(install.join("host.json")).unwrap().permissions().mode()&0o777,0o600);
    }
    let valid=fs::read(install.join("host.json")).unwrap();
    let mut worker=Command::new(env!("CARGO_BIN_EXE_taskq"));
    worker.args(["service","capacity","--host","fixture-host","--codex","4"])
        .env("TASKQ_INSTALL",&install).env("TASKQ_HOST","fixture-host").env_remove("TASKQ_ROOT").env("TASKQ_ATTEMPT","1")
        .env("TASKQ_INCIDENT_REPORTS","off").env_remove("COREQ_TRACE_CONTEXT").current_dir(&install);
    let denied=coreq::process::output("test.host-policy-worker",&mut worker,Duration::from_secs(10));
    let Err(coreq::process::Failure::Exit(denied))=denied else {panic!("worker policy write was not refused");};
    assert!(String::from_utf8_lossy(&denied.stderr).contains("a worker cannot change host policy"));
    assert_eq!(fs::read(install.join("host.json")).unwrap(),valid);

    for args in [vec!["service","capacity","--host","other-host","--codex","4"],
        vec!["service","capacity","--host","fixture-host","--mode","fixed"]] {
        assert!(!run(&args).status.success());
        assert_eq!(fs::read(install.join("host.json")).unwrap(),valid);
        assert_eq!(fs::read_to_string(root.join("taskq.json")).unwrap(),project);
    }
    fs::write(install.join("host.json.pending"),"retained interruption").unwrap();
    assert!(!run(&["service","capacity","--host","fixture-host","--codex","4"]).status.success());
    assert_eq!(fs::read_to_string(install.join("host.json.pending")).unwrap(),"retained interruption");
    assert_eq!(fs::read(install.join("host.json")).unwrap(),valid);
    fs::remove_file(install.join("host.json.pending")).unwrap();
    fs::write(install.join("host.json"),"{").unwrap();
    let retro=run(&["retro"]);
    assert!(!retro.status.success());
    assert!(String::from_utf8_lossy(&retro.stderr).contains("host.json unknown"),"operator silently used legacy project policy: {}",String::from_utf8_lossy(&retro.stderr));
    assert!(!root.join(".taskq/retro.out").exists(),"invalid host policy reached a native runtime");
    assert!(!run(&["service","capacity","--host","fixture-host","--codex","4"]).status.success());
    assert_eq!(fs::read_to_string(install.join("host.json")).unwrap(),"{");
    assert_eq!(fs::read_to_string(root.join("taskq.json")).unwrap(),project);
    fs::remove_dir_all(&base).unwrap();
}

// H-A1: a registered project cannot start a project scheduler beside the already live host owner.
#[test]
fn project_run_waits_for_the_host_owner() {
    let stamp=SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let root=std::env::temp_dir().join(format!("taskq-host-entry-{}-{stamp}",std::process::id()));
    let (project,install)=(root.join("project"),root.join("install"));
    fs::create_dir_all(&project).unwrap();fs::create_dir_all(&install).unwrap();
    // An unrelated ancestor project must not choose the installed service's scope.
    fs::write(root.join("taskq.json"),json!({"repo":"fixture/unregistered-ancestor"}).to_string()).unwrap();
    fs::write(project.join("taskq.json"),json!({"repo":"fixture/project"}).to_string()).unwrap();
    fs::write(install.join("projects"),format!("{}\n",project.display())).unwrap();
    fs::write(install.join("service"),std::process::id().to_string()).unwrap();
    for cwd in [&project,&install] {
        let log=root.join("entry.log");
        let mut child=Command::new(env!("CARGO_BIN_EXE_taskq")).args(["service","run"])
            .env("TASKQ_INSTALL",&install).env_remove("TASKQ_ROOT").env_remove("TASKQ_ATTEMPT")
            .env_remove("TASKQ_TASK").env_remove("COREQ_TRACE_CONTEXT").env("TASKQ_INCIDENT_REPORTS","off")
            .current_dir(cwd).stdout(std::process::Stdio::null()).stderr(fs::File::create(&log).unwrap()).spawn().unwrap();
        let until=std::time::Instant::now()+Duration::from_secs(10);
        let observed=loop {
            let text=fs::read_to_string(&log).unwrap();
            if text.lines().any(|line|line.contains("host service")&&line.contains("waiting")) {break true;}
            if child.try_wait().unwrap().is_some()||std::time::Instant::now()>=until {break false;}
            std::thread::sleep(Duration::from_millis(10));
        };
        if child.try_wait().unwrap().is_none() {child.kill().unwrap();child.wait().unwrap();}
        let text=fs::read_to_string(&log).unwrap();
        assert!(observed,"{} selected a different owner: {text}",cwd.display());
        assert!(!project.join(".taskq/status.json").exists());
        assert_eq!(fs::read_to_string(install.join("service")).unwrap(),std::process::id().to_string());
    }
    fs::remove_dir_all(root).unwrap();
}
