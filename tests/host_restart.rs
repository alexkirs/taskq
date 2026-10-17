//! Source-process fixture: real CLI -> host re-exec -> native generation -> original durable receipts.
//! No board, model, native service unit, grant or installed host is used. Windows' parent supplies exit75 restart.
use std::{fs,path::Path,process::{Child,Command,Stdio},time::{Duration,Instant,SystemTime,UNIX_EPOCH}};
use serde_json::Value;

struct Owner(Option<Child>);
impl Drop for Owner {fn drop(&mut self){if let Some(child)=&mut self.0 {if child.try_wait().ok().flatten().is_none(){let _=child.kill();}let _=child.wait();}}}
fn json(path:&Path)->Option<Value> {serde_json::from_slice(&fs::read(path).ok()?).ok()}
fn command(install:&Path,bin:&Path,cwd:&Path,args:&[&str])->Command {
    let mut c=Command::new(env!("CARGO_BIN_EXE_taskq"));
    c.args(args).current_dir(cwd).env("TASKQ_INSTALL",install)
        .env("PATH",std::env::join_paths(std::iter::once(bin.to_path_buf()).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()))).unwrap())
        .env_remove("TASKQ_ROOT").env_remove("TASKQ_ATTEMPT").env_remove("TASKQ_TASK").env_remove("COREQ_TRACE_CONTEXT")
        .env("TASKQ_INCIDENT_REPORTS","off").env("TASKQ_SELFTEST_OFFLINE","1");c
}
fn run(install:&Path,bin:&Path,cwd:&Path,args:&[&str])->Value {
    let output=coreq::process::output("test.restart-cli",&mut command(install,bin,cwd,args),Duration::from_secs(20))
        .unwrap_or_else(|e|panic!("restart CLI {args:?}: {e}"));
    serde_json::from_slice(&output.stdout).unwrap_or_else(|e|panic!("{e}: {}",String::from_utf8_lossy(&output.stdout)))
}

#[test]
fn real_host_restart_preserves_request_after_the_caller_exits() {
    let nonce=SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let base=std::env::temp_dir().join(format!("taskq-restart-native-{}-{nonce}",std::process::id()));
    let install=base.join("install");let bin=base.join("bin");let outside=base.join("outside");
    for dir in [&install,&bin,&outside] {fs::create_dir_all(dir).unwrap();}
    fs::write(install.join("projects"),"").unwrap();fs::write(install.join("host.json"),"{}").unwrap();
    // Update transport is deliberately unavailable; this fixture never contacts a release server.
    #[cfg(unix)] {
        use std::os::unix::fs::PermissionsExt;
        fs::write(bin.join("curl"),"#!/bin/sh\nexit 42\n").unwrap();fs::set_permissions(bin.join("curl"),fs::Permissions::from_mode(0o755)).unwrap();
    }
    #[cfg(windows)] {
        let source=bin.join("refuse.rs");fs::write(&source,"fn main(){std::process::exit(42)}").unwrap();
        coreq::process::output("test.restart-transport",Command::new("rustc").arg("--crate-name").arg("restart_transport").arg(&source).arg("-o").arg(bin.join("curl.exe")),Duration::from_secs(30)).unwrap();
    }
    let warm=run(&install,&bin,&outside,&["service","status","--json"]);assert_eq!(warm["service"]["state"],"unknown");
    let log=base.join("service.log");
    let spawn=||command(&install,&bin,&install,&["service","run"])
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(fs::File::options().create(true).append(true).open(&log).unwrap()).spawn().unwrap();
    let mut owner=Owner(Some(spawn()));
    let until=Instant::now()+Duration::from_secs(20);
    let original=loop {
        if let Some(service)=json(&install.join("service")) {if service["restart_protocol"]==1 {break service;}}
        assert!(owner.0.as_mut().unwrap().try_wait().unwrap().is_none(),"fixture host exited: {}",fs::read_to_string(&log).unwrap());
        assert!(Instant::now()<until,"host publication deadline: {}",fs::read_to_string(&log).unwrap());std::thread::sleep(Duration::from_millis(10));
    };
    let saved=install.join("service.fixture-original");fs::rename(install.join("service"),&saved).unwrap();fs::create_dir(install.join("service")).unwrap();
    loop {
        if fs::read_to_string(&log).unwrap().contains("host publication unknown:") {break;}
        assert!(owner.0.as_mut().unwrap().try_wait().unwrap().is_none(),"publication failure abandoned the host");
        assert!(Instant::now()<until,"publication failure was not reported");std::thread::sleep(Duration::from_millis(10));
    }
    assert!(owner.0.as_mut().unwrap().try_wait().unwrap().is_none());
    assert_eq!(run(&install,&bin,&outside,&["service","status","--json"])["service"]["state"],"unknown");
    fs::remove_dir(install.join("service")).unwrap();fs::rename(saved,install.join("service")).unwrap();
    let until=Instant::now()+Duration::from_secs(20);
    let id="native-restart";
    let denied=coreq::process::output("test.restart-worker",command(&install,&bin,&outside,&["service","restart","--request","denied-worker","--json"])
        .env("TASKQ_ATTEMPT","42").env("TASKQ_TASK","1"),Duration::from_secs(20));
    let denied=match denied {Err(coreq::process::Failure::Exit(out))=>out,other=>panic!("worker restart did not refuse: {other:?}")};
    assert!(String::from_utf8_lossy(&denied.stderr).contains("worker cannot restart"));
    assert!(json(&install.join("resources/state.json")).is_none_or(|s|s["ops"].get("denied-worker").is_none()));
    let accepted=run(&install,&bin,&outside,&["service","restart","--request",id,"--json"]);
    assert_eq!(accepted["id"],id);assert_eq!(accepted["state"],"queued");
    let final_receipt=loop {
        #[cfg(windows)] if let Some(status)=owner.0.as_mut().unwrap().try_wait().unwrap() {
            assert_eq!(status.code(),Some(75),"host restart must request the documented manager action");
            owner.0=Some(spawn());
        }
        if let Some(state)=json(&install.join("resources/state.json")) {
            let receipt=&state["ops"][id]["receipt"];if receipt["state"]=="observed" {break receipt.clone();}
        }
        #[cfg(unix)] assert!(owner.0.as_mut().unwrap().try_wait().unwrap().is_none(),"fixture host exited: {}",fs::read_to_string(&log).unwrap());
        assert!(Instant::now()<until,"restart observation deadline: {}",fs::read_to_string(&log).unwrap());std::thread::sleep(Duration::from_millis(10));
    };
    let current=json(&install.join("service")).unwrap();
    assert_ne!(current["instance"],original["instance"]);assert_eq!(final_receipt["service_instance"],current["instance"]);
    #[cfg(unix)] assert_eq!(current["pid"],original["pid"],"Unix changes images while retaining the native PID");
    let pid=current["pid"].as_u64().unwrap() as u32;let native=coreq::process::identity(pid).unwrap().unwrap();assert_eq!(current["started"],native.started);
    let replay=run(&install,&bin,&outside,&["service","restart","--request",id,"--json"]);
    assert_eq!(replay["state"],"observed");assert_eq!(replay["received_receipt"],accepted["received_receipt"]);
    assert_eq!(replay["observed_receipt"],final_receipt["observed_receipt"]);
    let report=run(&install,&bin,&outside,&["service","status","--request",id,"--json"]);
    assert_eq!(report["restart"]["state"],"observed");assert_eq!(report["restart"]["id"],id);
    assert_eq!(report["service"]["instance"],current["instance"]);
    let reader=coreq::telemetry::Reader::open(&install.join("telemetry.log/universal.sqlite3")).unwrap();
    let exported=reader.export_kinds(&["command.received","command.applied","command.observed"],0,0,1024).unwrap();
    let rows=exported["events"].as_array().unwrap();
    for field in ["received_receipt","applied_receipt","observed_receipt"] {assert!(rows.iter().any(|r|r["id"]==replay[field]),"missing durable {field}");}
    let command=rows.iter().find(|r|r["id"]==replay["received_receipt"]).unwrap();
    for field in ["applied_receipt","observed_receipt"] {
        let event=rows.iter().find(|r|r["id"]==replay[field]).unwrap();assert_eq!(event["observation"]["span"],command["observation"]["span"]);assert_eq!(event["observation"]["trace"],command["observation"]["trace"]);
    }
    drop(reader);
    println!("Native host restart: source generation {} -> {}; original request/trace and all three durable receipts retained",original["instance"],current["instance"]);
    drop(owner);fs::remove_dir_all(base).unwrap();
}
