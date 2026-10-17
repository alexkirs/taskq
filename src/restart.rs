//! Exact host restart intent and outcome in the existing local operation store (H-A5).
use crate::{Res,host,resources};
use coreq::telemetry::capture::CommandReceipt;
use serde::{Deserialize,Serialize};
use serde_json::{Value,json};
use std::{collections::BTreeSet,fs,path::{Path,PathBuf},time::{Instant,SystemTime}};

#[derive(Clone,Debug,Deserialize,Serialize,PartialEq)]
#[serde(deny_unknown_fields)]
struct Request {id:String,instance:String,roots:Vec<PathBuf>,issued_at:u64,expires_at:u64}

#[derive(Clone)]
pub struct Pending {request:Request,command:CommandReceipt,phase:String}
impl Pending {
    pub fn origin(&self)->&str {&self.request.instance}
    pub fn id(&self)->&str {&self.request.id}
}

fn now()->Res<u64> {SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d|d.as_secs()).map_err(|_|"restart clock unknown".into())}
fn scope(install:&Path)->Res<Vec<PathBuf>> {host::projects(install)}
fn typed(value:&Value)->Res<Request> {
    if serde_json::to_vec(value).map_err(|e|e.to_string())?.len()>16<<10 {return Err("restart request too large".into());}
    let request:Request=serde_json::from_value(value.clone()).map_err(|_|"invalid restart request")?;
    if !resources::id(&request.id)||!resources::id(&request.instance)||request.expires_at<=request.issued_at
        ||request.expires_at-request.issued_at>300||request.roots.iter().any(|p|!p.is_absolute())
        ||request.roots.iter().cloned().collect::<BTreeSet<_>>().into_iter().collect::<Vec<_>>()!=request.roots {
        return Err("invalid restart request".into());
    }
    Ok(request)
}
fn parse(id:&str,op:&resources::Op)->Res<Pending> {
    let invalid="local restart binding unsupported or corrupt; state kept";
    if !resources::keys(&op.bound,&["local_restart"])||!resources::keys(&op.bound["local_restart"],&["request","command"]) {return Err(invalid.into());}
    let request=typed(&op.bound["local_restart"]["request"])?;
    if request.id!=id||op.writing||!op.sent {return Err(invalid.into());}
    let text=op.bound["local_restart"]["command"].as_str().ok_or(invalid)?;
    let command=CommandReceipt::decode(text)?;
    let metadata:Value=serde_json::from_str(text).map_err(|_|invalid)?;
    if metadata["product"]!="taskq"||metadata["operation"]!="taskq.host.restart.request" {return Err(invalid.into());}
    let phase=op.receipt["state"].as_str().filter(|p|matches!(*p,"queued"|"applied"|"observed")).ok_or(invalid)?.to_owned();
    let keys:&[&str]=match phase.as_str() {
        "queued"=>&["id","state","received_receipt"],"applied"=>&["id","state","received_receipt","applied_receipt"],
        _=>&["id","state","received_receipt","applied_receipt","observed_receipt","service_instance"],
    };
    let mut keys=keys.to_vec();
    if op.receipt.get("reason").is_some() {
        if op.receipt["reason"].as_str().is_none_or(|s|s.len()>256) {return Err(invalid.into());}keys.push("reason");
    }
    if !resources::keys(&op.receipt,&keys) {return Err(invalid.into());}
    if op.receipt["id"]!=id||op.receipt["received_receipt"]!=command.event_id("received")?
        ||(phase!="queued"&&op.receipt["applied_receipt"]!=command.event_id("applied")?)
        ||(phase=="observed"&&(op.receipt["observed_receipt"]!=command.event_id("observed")?
            ||op.receipt["service_instance"].as_str().is_none_or(|s|!resources::id(s)||s==request.instance))) {return Err(invalid.into());}
    Ok(Pending {request,command,phase})
}
fn active(state:&resources::State)->Res<Option<Pending>> {
    let mut pending=None;
    for (id,op) in &state.ops {
        if op.bound.get("local_restart").is_none() {continue;}
        let candidate=parse(id,op)?;
        if candidate.phase!="observed" {
            if pending.is_some() {return Err("multiple local restart intents; reconcile before continuing".into());}
            pending=Some(candidate);
        }
    }
    Ok(pending)
}
pub fn pending(install:&Path)->Res<Option<Pending>> {resources::try_with(install,|state|active(state))}

pub fn registry_guard(install:&Path)->Res<()> {
    if let Some(p)=pending(install)? {return Err(format!("host restart {} pending; project registry unchanged",p.id()));}
    Ok(())
}

/// Construct a review, not a command. Old PID-only/protocol-less services cannot take this operation.
pub fn review(install:&Path,id:&str)->Res<Value> {
    let service=host::service_status(install);
    if service["state"]!="running"||service["fresh"]!=true||service["restart_protocol"]!=1 {return Err("host restart protocol unavailable or observation stale; check service status".into());}
    let at=now()?;
    Ok(json!({"id":id,"instance":service["instance"],"roots":scope(install)?,"issued_at":at,"expires_at":at+300}))
}
fn public(op:&resources::Op)->Value {
    let mut value=op.receipt.clone();value["scope"]=json!("host");value["projects"]=op.bound["local_restart"]["request"]["roots"].clone();value
}
fn required(op:&mut resources::Op,command:&CommandReceipt,phase:&str)->Res<()> {
    if let Err(error)=command.checkpoint(phase) {
        op.receipt["reason"]=json!(format!("required restart receipt unavailable: {}; see host log",error.chars().take(48).collect::<String>()));
        return Err(error);
    }
    op.receipt.as_object_mut().ok_or("restart receipt corrupt")?.remove("reason");Ok(())
}
pub fn receipt(install:&Path,id:&str)->Res<Option<Value>> {
    if !resources::id(id) {return Err("invalid restart request ID".into());}
    resources::try_with(install,|state| {
        let Some(op)=state.ops.get(id) else {return Ok(None)};
        parse(id,op)?;Ok(Some(public(op)))
    })
}
pub fn status(install:&Path,id:Option<&str>)->Value {
    let result=if let Some(id)=id {receipt(install,id)}else {resources::try_with(install,|state| {
        if let Some(p)=active(state)? {return Ok(Some(public(&state.ops[p.id()])));}
        let mut last=None;
        for (id,op) in &state.ops {
            if op.bound.get("local_restart").is_none() {continue;}
            parse(id,op)?;
            let text=op.bound["local_restart"]["command"].as_str().ok_or("restart command missing")?;
            let metadata:Value=serde_json::from_str(text).map_err(|_|"restart command corrupt")?;
            let at=metadata["started_ms"].as_i64().ok_or("restart clock unknown")?;
            if last.as_ref().is_none_or(|(old,_)|at>*old) {last=Some((at,public(op)));}
        }
        Ok(last.map(|(_,value)|value))
    })};
    match result {Ok(Some(value))=>value,Ok(None)=>json!({"state":"unknown","reason":"no retained restart receipt"}),Err(reason)=>json!({"state":"unknown","reason":reason})}
}

#[tracing::instrument(name="host.restart.request",skip_all)]
pub fn submit(install:&Path,value:&Value)->Res<Value> {
    if std::env::var_os("TASKQ_ATTEMPT").is_some() {return Err("a worker cannot restart the host service".into());}
    let request=typed(value)?;
    let _projects=coreq::file_lock::try_lock(&install.join("projects.lock")).map_err(|e|format!("project registry busy: {e}"))?;
    let result=resources::with(install,|state| {
        if let Some(op)=state.ops.get(&request.id) {
            let old=parse(&request.id,op)?;
            if old.request!=request {return Err("restart request ID reused with another payload".into());}
            return Ok(public(op));
        }
        if let Some(p)=active(state)? {return Err(format!("host restart {} already pending; no second request taken",p.id()));}
        resources::prepare(state,install,now()?)?;
        if state.ops.len()>=resources::MAX_OPS {return Err("operation store full; restart not requested".into());}
        let at=now()?;
        if request.issued_at>at.saturating_add(30)||at>=request.expires_at {return Err("restart review expired or clock unknown".into());}
        let service=host::service_status(install);
        if service["state"]!="running"||service["fresh"]!=true||service["restart_protocol"]!=1||service["instance"]!=request.instance
            ||scope(install)?!=request.roots {return Err("host or project scope changed; review Restart again".into());}
        let command=CommandReceipt::current()?;command.checkpoint("received")?;
        let op=resources::Op {bound:json!({"local_restart":{"request":request,"command":command.encode()}}),
            receipt:json!({"id":request.id,"state":"queued","received_receipt":command.event_id("received")?}),
            expires_at:request.expires_at,saved_at:Some(at),writing:false,sent:true};
        let result=public(&op);state.ops.insert(request.id.clone(),op);Ok(result)
    })?;
    if let Err(error)=crate::platform::write_atomic(&install.join("poke"),"") {
        let mut result=result;result["reason"]=json!(format!("restart saved; host notification unknown: {error}"));return Ok(result);
    }
    Ok(result)
}

/// The coordinator already latched all project preparation before this required receipt.
pub fn prepare(install:&Path,instance:&str)->Res<()> {
    resources::try_with(install,|state| {
        let Some(p)=active(state)? else {return Ok(())};
        if p.origin()!=instance {return Err("original restart already has another service generation; reconcile before another switch".into());}
        if scope(install)?!=p.request.roots {return Err("restart scope changed; restore the reviewed registry before continuing".into());}
        let op=state.ops.get_mut(p.id()).unwrap();required(op,&p.command,"applied")?;
        op.receipt["state"]=json!("applied");op.receipt["applied_receipt"]=json!(p.command.event_id("applied")?);
        Ok(())
    })
}

/// Only the new native owner after full project recovery may acknowledge the original command.
pub fn observe(install:&Path,instance:&str,ready:bool)->Res<()> {
    if !ready {return Ok(());}
    resources::try_with(install,|state| {
        let Some(p)=active(state)? else {return Ok(())};
        if p.origin()==instance {return Ok(());}
        if scope(install)?!=p.request.roots {return Err("restart outcome unknown: reviewed registry changed".into());}
        let service=host::service_status(install);
        if service["state"]!="running"||service["fresh"]!=true||service["instance"]!=instance||service["pid"]!=std::process::id() {
            return Err("restart observer is not the current native owner".into());
        }
        let op=state.ops.get_mut(p.id()).unwrap();
        required(op,&p.command,"applied")?;required(op,&p.command,"observed")?;
        op.receipt["state"]=json!("observed");op.receipt["applied_receipt"]=json!(p.command.event_id("applied")?);
        op.receipt["observed_receipt"]=json!(p.command.event_id("observed")?);op.receipt["service_instance"]=json!(instance);
        op.expires_at=now()?;
        Ok(())
    })
}

pub fn update_wait(install:&Path,instance:&str)->Res<()> {
    resources::try_with(install,|state| {
        if let Some(p)=active(state)? && p.origin()==instance {
            state.ops.get_mut(p.id()).unwrap().receipt["reason"]=json!("host release check in progress; waiting for its bounded result");
        }
        Ok(())
    })
}

pub struct Monitor {checked:Option<Instant>,modified:Res<Option<SystemTime>>,pub current:Res<Option<Pending>>}
impl Default for Monitor {fn default()->Self {Self {checked:None,modified:Ok(None),current:Ok(None)}}}
impl Monitor {
    pub fn refresh(&mut self,install:&Path) {
        let modified=match fs::metadata(resources::dir(install).join("state.json")) {
            Ok(m)=>m.modified().map(Some).map_err(|e|e.to_string()),
            Err(e) if e.kind()==std::io::ErrorKind::NotFound=>Ok(None),Err(e)=>Err(e.to_string()),
        };
        let busy=self.current.as_ref().err().is_some_and(|e|e.starts_with("resources lock unavailable:"));
        if !busy&&self.checked.is_some_and(|at|at.elapsed().as_secs()<60)&&self.modified==modified {return;}
        self.checked=Some(Instant::now());self.modified=modified;
        self.current=self.modified.as_ref().map_err(Clone::clone).and_then(|_|pending(install));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture()->(PathBuf,PathBuf,PathBuf) {
        let dir=std::env::temp_dir().join(format!("taskq-restart-{}",crate::runtime::uuid()));
        let install=dir.join("install");let root=dir.join("project");
        fs::create_dir_all(&install).unwrap();fs::create_dir_all(&root).unwrap();
        fs::write(install.join("projects"),format!("{}\n",root.canonicalize().unwrap().display())).unwrap();
        fs::write(install.join("host.json"),"{}").unwrap();fs::write(root.join("taskq.json"),r#"{"repo":"fixture/project"}"#).unwrap();
        publish(&install,"original");(dir,install,root)
    }
    fn publish(install:&Path,instance:&str) {
        let process=coreq::process::identity(std::process::id()).unwrap().unwrap();
        fs::write(install.join("service"),json!({"schema":1,"pid":process.pid,"started":process.started,"instance":instance,"at":now().unwrap(),"version":"fixture","restart_protocol":1}).to_string()).unwrap();
    }

    // H-A5: one durable request -> qualified preparation -> new owner recovery -> the original receipt.
    #[test]
    fn restart_replay_scope_and_exact_generation() {
        let (dir,install,root)=fixture();let (capture,guard)=crate::profile::tests::collecting(&install);
        let request=review(&install,"exact-restart").unwrap();
        let received=submit(&install,&request).unwrap();assert_eq!(received["state"],"queued");
        let bytes=fs::read(resources::dir(&install).join("state.json")).unwrap();
        assert_eq!(submit(&install,&request).unwrap(),received);assert_eq!(fs::read(resources::dir(&install).join("state.json")).unwrap(),bytes);
        let mut changed=request.clone();changed["instance"]=json!("other");assert!(submit(&install,&changed).is_err());
        changed["id"]=json!("another");assert!(submit(&install,&changed).is_err());
        assert!(host::ProjectEdit::new(&install,&root,false).is_err());
        let original=fs::read(install.join("projects")).unwrap();
        fs::write(install.join("projects"),"").unwrap();assert!(prepare(&install,"original").is_err());
        assert_eq!(receipt(&install,"exact-restart").unwrap().unwrap()["state"],"queued");fs::write(install.join("projects"),original).unwrap();
        prepare(&install,"original").unwrap();assert_eq!(receipt(&install,"exact-restart").unwrap().unwrap()["state"],"applied");
        observe(&install,"original",true).unwrap();assert_eq!(status(&install,None)["state"],"applied");
        publish(&install,"replacement");observe(&install,"replacement",false).unwrap();assert_eq!(status(&install,None)["state"],"applied");
        assert!(prepare(&install,"replacement").is_err(),"an already replaced origin must never trigger a second switch");
        observe(&install,"replacement",true).unwrap();let completed=receipt(&install,"exact-restart").unwrap().unwrap();
        assert_eq!(completed["state"],"observed");assert_eq!(completed["service_instance"],"replacement");
        assert_eq!(completed["received_receipt"],received["received_receipt"]);
        assert_eq!(submit(&install,&request).unwrap(),completed,"same-ID recovery cannot create another command or effect");
        drop(host::ProjectEdit::new(&install,&root,false).unwrap());
        capture.flush().unwrap();drop(guard);drop(capture);fs::remove_dir_all(dir).unwrap();
    }

    // H-A5 trust boundary: no receipt, stale review or unsupported protocol cannot create restart intent.
    #[test]
    fn restart_refuses_unqualified_or_unrecordable_requests() {
        let (dir,install,_)=fixture();let request=review(&install,"no-collector").unwrap();
        let dispatch=tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default());
        tracing::dispatcher::with_default(&dispatch,||assert!(submit(&install,&request).is_err()));
        assert!(pending(&install).unwrap().is_none());
        let (capture,guard)=crate::profile::tests::collecting(&install);
        for case in ["old-protocol","stale-instance","expired","foreign-root","extra","oversized","bad-id"] {
            let mut input=request.clone();input["id"]=json!(case);
            match case {
                "old-protocol"=>{let mut record:Value=serde_json::from_slice(&fs::read(install.join("service")).unwrap()).unwrap();record.as_object_mut().unwrap().remove("restart_protocol");fs::write(install.join("service"),record.to_string()).unwrap();}
                "stale-instance"=>input["instance"]=json!("another"),
                "expired"=>{input["issued_at"]=json!(now().unwrap()-301);input["expires_at"]=json!(now().unwrap()-1);}
                "foreign-root"=>input["roots"]=json!([dir.canonicalize().unwrap()]),
                "extra"=>input["force"]=json!(true),"oversized"=>input["instance"]=json!("x".repeat(20<<10)),
                "bad-id"=>input["id"]=json!("bad/id"),_=>unreachable!(),
            }
            assert!(submit(&install,&input).is_err(),"{case}");assert!(pending(&install).unwrap().is_none());publish(&install,"original");
        }
        let db=rusqlite::Connection::open(install.join("telemetry.log/universal.sqlite3")).unwrap();
        db.execute_batch("CREATE TRIGGER refuse_restart_receipt BEFORE INSERT ON events BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
        assert!(submit(&install,&request).is_err());assert!(pending(&install).unwrap().is_none());
        db.execute_batch("DROP TRIGGER refuse_restart_receipt").unwrap();drop(db);
        assert!(capture.flush().unwrap_err().contains("injected"));drop(guard);drop(capture);fs::remove_dir_all(dir).unwrap();
    }

    // Readback/retention must preserve a long drain, refuse corruption and recover an observer death.
    #[test]
    fn restart_pending_survives_retention_and_receipt_failure() {
        let (dir,install,_)=fixture();let (capture,guard)=crate::profile::tests::collecting(&install);
        let input=review(&install,"long-restart").unwrap();submit(&install,&input).unwrap();
        let held=coreq::file_lock::try_lock(&resources::dir(&install).join("lock")).unwrap();
        let (send,receive)=std::sync::mpsc::channel();let observed=install.clone();
        let reader=std::thread::spawn(move ||send.send(pending(&observed).map(|_|())).unwrap());
        let result=receive.recv_timeout(std::time::Duration::from_secs(2));drop(held);reader.join().unwrap();
        assert!(result.unwrap().unwrap_err().starts_with("resources lock unavailable:"),"host observer waited behind a policy writer");
        resources::with(&install,|state|resources::prepare(state,&install,now()?+3*86400)).unwrap();
        assert!(pending(&install).unwrap().is_some());prepare(&install,"original").unwrap();publish(&install,"new-owner");
        let db=rusqlite::Connection::open(install.join("telemetry.log/universal.sqlite3")).unwrap();
        db.execute_batch("CREATE TRIGGER refuse_restart_observation BEFORE INSERT ON events WHEN NEW.kind='command.observed' BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
        assert!(observe(&install,"new-owner",true).is_err());assert_eq!(status(&install,None)["state"],"applied");
        assert!(status(&install,None)["reason"].as_str().unwrap().contains("required restart receipt unavailable"));
        db.execute_batch("DROP TRIGGER refuse_restart_observation").unwrap();drop(db);
        observe(&install,"new-owner",true).unwrap();assert_eq!(status(&install,None)["state"],"observed");
        let path=resources::dir(&install).join("state.json");let bytes=fs::read(&path).unwrap();
        fs::write(&path,"{").unwrap();assert_eq!(status(&install,None)["state"],"unknown");assert!(pending(&install).is_err());assert_eq!(fs::read(&path).unwrap(),b"{");
        fs::write(&path,bytes).unwrap();fs::write(resources::dir(&install).join("state.json.pending"),"interrupted").unwrap();
        assert!(pending(&install).is_err());fs::remove_file(resources::dir(&install).join("state.json.pending")).unwrap();
        resources::with(&install,|state|resources::prepare(state,&install,now()?+3*86400)).unwrap();assert!(receipt(&install,"long-restart").unwrap().is_none());
        assert!(capture.flush().unwrap_err().contains("injected"));drop(guard);drop(capture);fs::remove_dir_all(dir).unwrap();
    }
}
