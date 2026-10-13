use super::*;
use coreq::incident::{Comment, Issue};
fn tool() -> coreq::Tool { coreq::Tool::new("taskq", "TaskQ", "taskqorg/taskq", "1.2.3") }
fn fault_at(site: &str) -> Fault<'_> {
    Fault { code: "dispatch.panic", fields: &[], board: "github", runtime: "none", site,
        revision: "0000000000000000000000000000000000000000", contract: "0123456789ab",
        evidence: &["exit:101"], observed: &["exit:101"], inferred: &["dispatch.panic"] }
}
fn root(name: &str) -> std::path::PathBuf {
    let r = std::env::temp_dir().join(format!("taskq-incident-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&r); r
}
struct Net { issue: Option<Issue>, notes: Vec<Comment>, writes: usize, more: bool, fail: bool, lost: bool }
impl Net {
    fn new() -> Self { Self { issue: None, notes: vec![], writes: 0, more: false, fail: false, lost: false } }
    fn existing(f: &Fault, open: bool) -> Self {
        let r = coreq::incident::build(&tool(), f, "11111111-1111-4111-8111-111111111111");
        let body = format!("<!-- taskq-incident/1 {} -->\n{r}", r["fingerprint"].as_str().unwrap());
        Self { issue: Some(Issue { number: 1, url: "https://github.com/taskqorg/taskq/issues/1".into(), open, author: "other".into(), body }), ..Self::new() }
    }
}
impl Transport for Net {
    fn login(&mut self) -> Result<String, String> { if self.fail { Err("offline".into()) } else { Ok("tester".into()) } }
    fn search(&mut self, _: &str, _: u32) -> Result<(Vec<Issue>, bool), String> {
        Ok((self.issue.iter().map(|i| Issue { number:i.number,url:i.url.clone(),open:i.open,author:i.author.clone(),body:i.body.clone() }).collect(), self.more))
    }
    fn comments(&mut self, _: u64, _: u32) -> Result<(Vec<Comment>, bool), String> {
        Ok((self.notes.iter().map(|c| Comment { author:c.author.clone(),body:c.body.clone() }).collect(), false))
    }
    fn create(&mut self, _: &str, _: &str) -> Result<Issue, String> {
        self.writes += 1;
        if self.lost { Err("lost".into()) } else { Ok(Issue { number:1,url:"https://github.com/taskqorg/taskq/issues/1".into(),open:true,author:"tester".into(),body:String::new() }) }
    }
    fn comment(&mut self, _: u64, body: &str) -> Result<(), String> {
        self.writes += 1; self.notes.push(Comment {author:"tester".into(),body:body.into()}); Ok(())
    }
}
#[test]
fn report_golden_and_unknown_values() {
    let mut f = fault_at("src/main.rs:7"); f.fields = &["src/main.rs:7"];
    let r = coreq::incident::build(&tool(), &f, "11111111-1111-4111-8111-111111111111");
    let os = coreq::update::target();
    let fp = match os.as_str() { "windows-x86_64"=>"f4de2ee79f530c01", "linux-x86_64"=>"077b5e1ee230f2c8", "macos-aarch64"=>"f713507b0d71cab5", _=>panic!("add golden for {os}") };
    assert_eq!(r, json!({"schema":"taskq-incident/1","fault":"dispatch.panic","fingerprint":fp,"report_id":"11111111-1111-4111-8111-111111111111","release":"1.2.3","revision":"0000000000000000000000000000000000000000","contract":"0123456789ab","board":"github","runtime":"none","os":os,"site":"src/main.rs:7","evidence":["exit:101"],"observed":["exit:101"],"inferred":["dispatch.panic"]}));
    assert!(r.to_string().len()<=4096); assert!(coreq::incident::check(&r).is_ok());
    let mut extra=r.clone(); extra["project"]=json!("private"); assert!(coreq::incident::check(&extra).is_err());
    let root=root("unsafe"); let mut net=Net::new();
    for bad in ["private-user","path:/private/project","exit:666","line\nbreak"] {
        let evidence=[bad]; let bad_f=Fault {evidence:&evidence,..fault_at("src/main.rs:7")}; let o=report(&tool(),&root,Some(true),&bad_f,Some(&mut net));
        assert_eq!((o.state,o.reason.as_deref()),("local",Some("unsafe")));
    }
    let huge="x".repeat(5000); let evidence=[huge.as_str()]; let bad_f=Fault {evidence:&evidence,..fault_at("src/main.rs:7")};
    assert_eq!(report(&tool(),&root,Some(true),&bad_f,Some(&mut net)).reason.as_deref(),Some("unsafe"));
    assert_eq!(net.writes,0); assert!(!root.exists());
}
#[test]
fn fake_routes_dedup_and_unknown_delivery() {
    let root=root("routes"); let t=tool();
    let f=fault_at("src/main.rs:1001"); let mut net=Net::existing(&f,true);
    for _ in 0..2 { assert_eq!(report(&t,&root,Some(true),&f,Some(&mut net)).state,"sent"); }
    assert_eq!(net.writes,1);
    let f=Fault {code:CATALOG[1],..fault_at("src/main.rs:1002")}; let mut closed=Net::existing(&f,false);
    assert_eq!(report(&t,&root,Some(true),&f,Some(&mut closed)).state,"sent"); assert_eq!(closed.writes,0);
    let f=Fault {code:CATALOG[2],..fault_at("src/main.rs:1003")}; let mut incomplete=Net {more:true,..Net::new()};
    assert_eq!(report(&t,&root,Some(true),&f,Some(&mut incomplete)).reason.as_deref(),Some("search incomplete")); assert_eq!(incomplete.writes,0);
    let f=Fault {code:CATALOG[3],..fault_at("src/main.rs:1004")}; let mut lost=Net {lost:true,..Net::new()};
    assert_eq!(report(&t,&root,Some(true),&f,Some(&mut lost)).reason.as_deref(),Some("unknown delivery"));
    assert_eq!(report(&t,&root,Some(true),&f,Some(&mut lost)).state,"local"); assert_eq!(lost.writes,1);
    let f=Fault {code:CATALOG[4],..fault_at("src/main.rs:1005")}; assert_eq!(report(&t,&root,Some(true),&f,None).state,"unsupported-auth");
    let f=Fault {code:CATALOG[6],..fault_at("src/main.rs:1006")}; let mut offline=Net {fail:true,..Net::new()};
    assert_eq!(report(&t,&root,Some(true),&f,Some(&mut offline)).state,"local"); assert_eq!(offline.writes,0);
    assert_eq!(report(&t,&root.join("off"),Some(false),&f,Some(&mut net)).state,"disabled"); assert!(!root.join("off").exists());
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn catalog_main_and_launcher_detection_points() {
    take_detected(); let bad=crate::core::GOLDEN.replacen("\"ready\"","\"doing\"",1);
    assert!(crate::golden_selftest(&bad,true).is_err()); assert_eq!(take_detected(),["invariant.golden"]);
    assert!(crate::golden_selftest(&bad,false).is_err()); assert!(take_detected().is_empty());
    let mut since=None;
    crate::restart_watch(&mut since,true,0,10); crate::restart_watch(&mut since,true,19,10); assert!(take_detected().is_empty());
    crate::restart_watch(&mut since,true,20,10); assert_eq!(take_detected(),["dispatch.reexec"]);
    crate::restart_watch(&mut since,true,40,10); assert!(take_detected().is_empty());
    let dir=root("launcher"); fs::create_dir_all(dir.join("releases/1.2.3")).unwrap();
    fs::write(dir.join("current"),"1.2.3").unwrap(); fs::write(dir.join("previous"),"1.2.2").unwrap();
    let t=crate::platform::tool(&dir); for _ in 0..3 { coreq::launcher::pick(&t,true).unwrap(); }
    assert_eq!(take_detected(),["update.rollback"]); launcher(&t,&fault_at("dep")); assert_eq!(take_detected(),["dispatch.panic"]);
    fs::remove_file(dir.join("previous")).unwrap(); fs::write(dir.join("current"),"1.2.3").unwrap();
    coreq::launcher::pick(&t,true).unwrap(); assert!(take_detected().is_empty());
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn off_child() {
    if std::env::var("TASKQ_OFF_CHILD").is_err() { return; }
    let root=root("env-off"); let mut net=Net::new();
    assert_eq!(report(&tool(),&root,Some(true),&fault_at("dep"),Some(&mut net)).state,"disabled");
    assert_eq!(net.writes,0); assert!(!root.exists());
}

#[test]
fn environment_off_writes_nothing() {
    let o=std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact","incident::tests::off_child"])
        .env("TASKQ_OFF_CHILD","1").env("TASKQ_INCIDENT_REPORTS","off").output().unwrap();
    assert!(o.status.success(),"{}",String::from_utf8_lossy(&o.stderr));
}

#[test]
fn panic_hook_child() {
    if std::env::var_os("TASKQ_TEST_PANIC_REPORT").is_some() { install_panic_hook(); }
    else if std::env::var_os("TASKQ_TEST_PANIC_BASELINE").is_none() { return; }
    panic!("fixture panic, no private data");
}

#[test]
fn first_real_panic_reports_once_without_changing_panic_output_or_exit() {
    let root = root("panic-child");
    let output = std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact", "incident::tests::panic_hook_child", "--nocapture"])
        .env("TASKQ_TEST_PANIC_REPORT", &root).env_remove("TASKQ_INCIDENT_REPORTS").output().unwrap();
    let baseline = std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact", "incident::tests::panic_hook_child", "--nocapture"])
        .env("TASKQ_TEST_PANIC_BASELINE", "1").output().unwrap();
    assert!(!output.status.success()); assert_eq!(output.status.code(), baseline.status.code());
    let text = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    assert!(text.contains("fixture panic, no private data"), "{text}");
    assert!(!text.contains("cannot modify") && !text.contains("panic in a destructor") && !text.contains("thread caused non-unwinding panic"), "{text}");
    let files: Vec<_> = fs::read_dir(root.join(".taskq/incidents")).unwrap().flatten().collect();
    assert_eq!(files.len(), 1);
    let r: Value = serde_json::from_slice(&fs::read(files[0].path()).unwrap()).unwrap();
    assert_eq!(r["fault"], "dispatch.panic");
    assert!(!r.to_string().contains("fixture panic"));
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn operator_lines_show_all_states() {
    assert_eq!(line(&Value::Null),"Incidents: none"); assert_eq!(line(&json!({"state":"disabled"})),"Incidents: disabled");
    for state in ["sent","local","unsupported-auth"] { assert!(line(&json!({"state":state,"fault":"dispatch.panic","at":now()})).starts_with(&format!("Incident: {state} dispatch.panic"))); }
}
