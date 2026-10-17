//! Source-process fixture: real CLI -> host re-exec -> native generation -> original durable receipts.
//! No board, model, native service unit, grant or installed host is used. Windows' parent supplies exit75 restart.
use std::{fs,io::{Read,Write},net::{TcpListener,TcpStream},path::Path,process::{Child,Command,Stdio},sync::{Arc,atomic::{AtomicBool,AtomicUsize,Ordering}},time::{Duration,Instant,SystemTime,UNIX_EPOCH}};
use serde_json::Value;

struct Owner(Option<Child>);
impl Drop for Owner {fn drop(&mut self){if let Some(child)=&mut self.0 {if child.try_wait().ok().flatten().is_none(){let _=child.kill();}let _=child.wait();}}}
// Native executable search differs by OS; deny release transport at its local network boundary.
struct Proxy {url:String,hits:Arc<AtomicUsize>,stop:Arc<AtomicBool>,worker:Option<std::thread::JoinHandle<()>>}
impl Proxy {
    fn new()->Self {
        let listener=TcpListener::bind("127.0.0.1:0").unwrap();let address=listener.local_addr().unwrap();
        let hits=Arc::new(AtomicUsize::new(0));let stop=Arc::new(AtomicBool::new(false));let (count,done)=(hits.clone(),stop.clone());
        let worker=std::thread::spawn(move || {
            for stream in listener.incoming() {
                if done.load(Ordering::Acquire) {break;}
                let mut stream=stream.unwrap();stream.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
                stream.set_write_timeout(Some(Duration::from_secs(1))).unwrap();let mut request=[0;4096];
                if stream.read(&mut request).is_ok_and(|n|n>0) {
                    stream.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
                    count.fetch_add(1,Ordering::Release);
                }
            }
        });Self {url:format!("http://{address}"),hits,stop,worker:Some(worker)}
    }
}
impl Drop for Proxy {fn drop(&mut self){self.stop.store(true,Ordering::Release);let address=self.url.trim_start_matches("http://").parse().unwrap();let _=TcpStream::connect_timeout(&address,Duration::from_secs(1));if let Some(worker)=self.worker.take(){let _=worker.join();}}}
fn json(path:&Path)->Option<Value> {serde_json::from_slice(&fs::read(path).ok()?).ok()}
fn command(install:&Path,proxy:&Proxy,cwd:&Path,args:&[&str])->Command {
    let mut c=Command::new(env!("CARGO_BIN_EXE_taskq"));
    c.args(args).current_dir(cwd).env("TASKQ_INSTALL",install)
        .env_remove("TASKQ_ROOT").env_remove("TASKQ_ATTEMPT").env_remove("TASKQ_TASK").env_remove("COREQ_TRACE_CONTEXT")
        .env("TASKQ_INCIDENT_REPORTS","off").env("TASKQ_SELFTEST_OFFLINE","1").env("CURL_HOME",cwd).env("NO_PROXY","").env("no_proxy","");
    for key in ["HTTP_PROXY","HTTPS_PROXY","ALL_PROXY","http_proxy","https_proxy","all_proxy"] {c.env(key,&proxy.url);}c
}
fn run(install:&Path,proxy:&Proxy,cwd:&Path,args:&[&str])->Value {
    let output=coreq::process::output("test.restart-cli",&mut command(install,proxy,cwd,args),Duration::from_secs(20))
        .unwrap_or_else(|e|panic!("restart CLI {args:?}: {e}"));
    serde_json::from_slice(&output.stdout).unwrap_or_else(|e|panic!("{e}: {}",String::from_utf8_lossy(&output.stdout)))
}

#[test]
fn real_host_restart_preserves_request_after_the_caller_exits() {
    let nonce=SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let base=std::env::temp_dir().join(format!("taskq-restart-native-{}-{nonce}",std::process::id()));
    let install=base.join("install");let outside=base.join("outside");let proxy=Proxy::new();
    for dir in [&install,&outside] {fs::create_dir_all(dir).unwrap();}
    fs::write(install.join("projects"),"").unwrap();fs::write(install.join("host.json"),"{}").unwrap();
    let warm=run(&install,&proxy,&outside,&["service","status","--json"]);assert_eq!(warm["service"]["state"],"unknown");
    let log=base.join("service.log");
    let spawn=||command(&install,&proxy,&install,&["service","run"])
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
    assert_eq!(run(&install,&proxy,&outside,&["service","status","--json"])["service"]["state"],"unknown");
    fs::remove_dir(install.join("service")).unwrap();fs::rename(saved,install.join("service")).unwrap();
    let until=Instant::now()+Duration::from_secs(20);
    let id="native-restart";
    for action in ["restart","stop","uninstall"] {
        let denied=coreq::process::output("test.lifecycle-worker",command(&install,&proxy,&outside,&["service",action,"--request","denied-worker","--json"])
            .env("TASKQ_ATTEMPT","42").env("TASKQ_TASK","1"),Duration::from_secs(20));
        let denied=match denied {Err(coreq::process::Failure::Exit(out))=>out,other=>panic!("worker {action} did not refuse: {other:?}")};
        assert!(String::from_utf8_lossy(&denied.stderr).contains("worker cannot restart"));
        assert!(json(&install.join("resources/state.json")).is_none_or(|s|s["ops"].get("denied-worker").is_none()));
    }
    let accepted=run(&install,&proxy,&outside,&["service","restart","--request",id,"--json"]);
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
    let replay=run(&install,&proxy,&outside,&["service","restart","--request",id,"--json"]);
    assert_eq!(replay["state"],"observed");assert_eq!(replay["received_receipt"],accepted["received_receipt"]);
    assert_eq!(replay["observed_receipt"],final_receipt["observed_receipt"]);
    let report=run(&install,&proxy,&outside,&["service","status","--request",id,"--json"]);
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
    #[cfg(windows)] {
        // The fixture manager stops after orderly exit75; forced teardown can abandon owned update IO.
        let cleanup=run(&install,&proxy,&outside,&["service","restart","--request","fixture-cleanup","--json"]);
        assert_eq!(cleanup["state"],"queued");
        let until=Instant::now()+Duration::from_secs(20);
        loop {
            if let Some(status)=owner.0.as_mut().unwrap().try_wait().unwrap() {assert_eq!(status.code(),Some(75));break;}
            assert!(Instant::now()<until,"fixture manager did not observe orderly exit75");std::thread::sleep(Duration::from_millis(10));
        }
    }
    let until=Instant::now()+Duration::from_secs(20);
    while proxy.hits.load(Ordering::Acquire)<2 {
        assert!(Instant::now()<until,"both service generations must reach the owned rejecting proxy");std::thread::sleep(Duration::from_millis(10));
    }
    drop(owner);fs::remove_dir_all(&base).unwrap_or_else(|e|panic!("fixture cleanup {}: {e}",base.display()));
    println!("Native host restart: source generation {} -> {}; original request/trace and all three durable receipts retained",original["instance"],current["instance"]);
}
