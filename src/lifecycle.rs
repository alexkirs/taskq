//! Exact host lifecycle intent and outcome in the existing local operation store (H-A5).
use crate::{Res,host,resources};
use coreq::telemetry::capture::CommandReceipt;
use serde::{Deserialize,Serialize};
use serde_json::{Value,json};
use std::{collections::BTreeSet,fs,path::{Path,PathBuf},time::{Instant,SystemTime}};

#[derive(Clone,Copy,Debug,Default,Deserialize,Serialize,PartialEq)]
#[serde(rename_all="lowercase")]
pub enum Action {#[default] Restart,Stop,Uninstall}
impl Action {
    fn restart(&self)->bool {*self==Self::Restart}
    fn key(self)->&'static str {match self {Self::Restart=>"local_restart",Self::Stop=>"local_stop",Self::Uninstall=>"local_uninstall"}}
    pub fn title(self)->&'static str {match self {Self::Restart=>"Restart",Self::Stop=>"Stop",Self::Uninstall=>"Uninstall"}}
}
#[derive(Clone,Debug,Deserialize,Serialize,PartialEq)]
#[serde(deny_unknown_fields)]
struct Request {id:String,instance:String,roots:Vec<PathBuf>,issued_at:u64,expires_at:u64,#[serde(default,skip_serializing_if="Action::restart")] action:Action,
    #[serde(default,skip_serializing_if="Option::is_none")] project:Option<PathBuf>}
impl Request {
    fn stops(&self)->bool {self.action==Action::Stop||self.action==Action::Uninstall&&(self.project.is_none()||self.roots.len()==1)}
    fn after(&self)->Vec<PathBuf> {self.roots.iter().filter(|root|Some(*root)!=self.project.as_ref()).cloned().collect()}
}

#[derive(Clone)]
pub struct Pending {request:Request,command:CommandReceipt,phase:String}
impl Pending {
    pub fn origin(&self)->&str {&self.request.instance}
    pub fn id(&self)->&str {&self.request.id}
    pub fn action(&self)->Action {self.request.action}
    pub fn stops(&self)->bool {self.request.stops()}
    pub fn removal(&self)->Option<&Path> {self.request.project.as_deref()}
    pub fn prepared(&self)->bool {self.phase=="applied"}
    pub fn context(&self)->Res<String> {let m:Value=serde_json::from_str(&self.command.encode()).map_err(|e|e.to_string())?;coreq::telemetry::capture::TraceContext::decode(&format!("1 {} {}",m["trace"].as_str().ok_or("trace missing")?,m["span"].as_str().ok_or("span missing")?)).map(|c|c.encode())}
}

fn now()->Res<u64> {SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d|d.as_secs()).map_err(|_|"lifecycle clock unknown".into())}
fn scope(install:&Path)->Res<Vec<PathBuf>> {host::projects(install)}
fn typed(value:&Value)->Res<Request> {
    if serde_json::to_vec(value).map_err(|e|e.to_string())?.len()>16<<10 {return Err("lifecycle request too large".into());}
    let request:Request=serde_json::from_value(value.clone()).map_err(|_|"invalid lifecycle request")?;
    if !resources::id(&request.id)||!resources::id(&request.instance)||request.expires_at<=request.issued_at
        ||request.expires_at-request.issued_at>300||request.roots.iter().any(|p|!p.is_absolute())
        ||request.project.as_ref().is_some_and(|root|request.action!=Action::Uninstall||!request.roots.contains(root))
        ||request.roots.iter().cloned().collect::<BTreeSet<_>>().into_iter().collect::<Vec<_>>()!=request.roots {
        return Err("invalid lifecycle request".into());
    }
    Ok(request)
}
fn binding(op:&resources::Op)->Option<&Value> {op.bound.get("local_restart").or_else(||op.bound.get("local_stop")).or_else(||op.bound.get("local_uninstall"))}
fn parse(id:&str,op:&resources::Op)->Res<Pending> {
    let invalid="local lifecycle binding unsupported or corrupt; state kept";
    let bound=binding(op).ok_or(invalid)?;
    let request=typed(&bound["request"])?;
    if !resources::keys(&op.bound,&[request.action.key()])||!resources::keys(bound,&["request","command"]) {return Err(invalid.into());}
    if request.id!=id||op.writing||!op.sent {return Err(invalid.into());}
    let text=bound["command"].as_str().ok_or(invalid)?;
    let command=CommandReceipt::decode(text)?;
    let metadata:Value=serde_json::from_str(text).map_err(|_|invalid)?;
    if metadata["product"]!="taskq"||metadata["operation"]!=match request.action {Action::Restart=>"taskq.host.restart.request",Action::Stop=>"taskq.host.stop.request",Action::Uninstall=>"taskq.host.uninstall.request"} {return Err(invalid.into());}
    let phase=op.receipt["state"].as_str().filter(|p|matches!(*p,"queued"|"applied"|"observed")).ok_or(invalid)?.to_owned();
    let keys:&[&str]=match phase.as_str() {
        "queued"=>&["id","state","received_receipt"],"applied"=>&["id","state","received_receipt","applied_receipt"],
        _=>&["id","state","received_receipt","applied_receipt","observed_receipt","service_instance"],
    };
    let mut keys=keys.to_vec();
    if request.action==Action::Uninstall&&phase!="queued" {
        if request.stops() {if phase=="observed" {keys.push("unit_removed");if op.receipt["unit_removed"]!=true {return Err(invalid.into());}}}
        else {keys.push("project_drained");if op.receipt["project_drained"]!=true {return Err(invalid.into());}}
        if phase=="observed"&&request.project.is_some() {keys.push("unregistered");if op.receipt["unregistered"]!=true {return Err(invalid.into());}}
    }
    if request.stops()&&phase!="queued" {
        if phase=="observed" {keys.retain(|key|*key!="service_instance");keys.push("native_exit");if op.receipt["native_exit"]!=true {return Err(invalid.into());}}
        keys.extend(["native_pid","native_started","unit_revision"]);
        let pid=op.receipt["native_pid"].as_u64().filter(|n|*n>0&&*n<=i32::MAX as u64).ok_or(invalid)?;
        let started=op.receipt["native_started"].as_str().ok_or(invalid)?;
        if pid==0||started.is_empty()||started.len()>128||!started.bytes().all(|c|c.is_ascii_alphanumeric()||c==b'-')
            ||op.receipt["unit_revision"].as_str().is_none_or(|s|s.len()!=64||!s.bytes().all(|b|b.is_ascii_hexdigit()&&!b.is_ascii_uppercase())) {return Err(invalid.into());}
        if let Some(launcher)=op.receipt.get("launcher") {launcher_scope(launcher)?;keys.push("launcher");}
    }
    if op.receipt.get("reason").is_some() {
        if op.receipt["reason"].as_str().is_none_or(|s|s.len()>256) {return Err(invalid.into());}keys.push("reason");
    }
    if !resources::keys(&op.receipt,&keys) {return Err(invalid.into());}
    if op.receipt["id"]!=id||op.receipt["received_receipt"]!=command.event_id("received")?
        ||(phase!="queued"&&op.receipt["applied_receipt"]!=command.event_id("applied")?)
        ||(phase=="observed"&&(op.receipt["observed_receipt"]!=command.event_id("observed")?
            ||(request.action==Action::Restart&&op.receipt["service_instance"].as_str().is_none_or(|s|!resources::id(s)||s==request.instance)))) {return Err(invalid.into());}
    Ok(Pending {request,command,phase})
}
fn active(state:&resources::State)->Res<Option<Pending>> {
    let mut pending=None;
    for (id,op) in &state.ops {
        if binding(op).is_none() {continue;}
        let candidate=parse(id,op)?;
        if candidate.phase!="observed" {
            if pending.is_some() {return Err("multiple local lifecycle intents; reconcile before continuing".into());}
            pending=Some(candidate);
        }
    }
    Ok(pending)
}
pub fn pending(install:&Path)->Res<Option<Pending>> {resources::try_with(install,|state|active(state))}

pub fn registry_guard(install:&Path)->Res<()> {
    if let Some(p)=pending(install)? {return Err(format!("host lifecycle {} pending; project registry unchanged",p.id()));}
    Ok(())
}

pub fn uninstall_guard(install:&Path,id:&str,root:&Path)->Res<()> {
    let p=pending(install)?.ok_or("Uninstall intent missing; registry kept")?;
    if p.id()!=id||p.action()!=Action::Uninstall||p.removal()!=Some(root)||p.phase!="applied" {return Err("Uninstall registry binding differs; kept".into());}
    let roots=scope(install)?;
    if roots!=p.request.roots&&roots!=p.request.after() {return Err("Uninstall registry changed; kept".into());}
    Ok(())
}

/// Construct a review, not a command. Old PID-only/protocol-less services cannot take this operation.
pub fn review(install:&Path,id:&str)->Res<Value> {review_action(install,id,Action::Restart)}
pub fn stop_review(install:&Path,id:&str)->Res<Value> {review_action(install,id,Action::Stop)}
pub fn uninstall_review(install:&Path,id:&str,project:Option<&Path>)->Res<Value> {
    let mut value=review_action(install,id,Action::Uninstall)?;
    if let Some(project)=project {value["project"]=json!(project.canonicalize().map_err(|e|e.to_string())?);}
    typed(&value)?;Ok(value)
}
fn review_action(install:&Path,id:&str,action:Action)->Res<Value> {
    let service=host::service_status(install);
    if service["state"]!="running"||service["fresh"]!=true||service["restart_protocol"]!=1 {return Err("host restart protocol unavailable or observation stale; check service status".into());}
    if action==Action::Stop {if service["stop_protocol"]!=1 {return Err("host Stop protocol unavailable; check service status".into());}crate::host_control::revision(install)?;}
    if action==Action::Uninstall&&service["uninstall_protocol"]!=1 {return Err("host Uninstall protocol unavailable; check service status".into());}
    let at=now()?;
    serde_json::to_value(Request {id:id.into(),instance:service["instance"].as_str().ok_or("host generation unknown")?.into(),roots:scope(install)?,issued_at:at,expires_at:at+300,action,project:None}).map_err(|e|e.to_string())
}
fn public(op:&resources::Op)->Value {
    let mut value=op.receipt.clone();value["scope"]=json!("host");if let Some(bound)=binding(op) {value["projects"]=bound["request"]["roots"].clone();value["origin_instance"]=bound["request"]["instance"].clone();value["action"]=bound["request"].get("action").cloned().unwrap_or(json!("restart"));if let Some(root)=bound["request"].get("project") {value["scope"]=json!("project");value["project"]=root.clone();}}
    if value["action"]=="stop" {value["restart_condition"]=json!(if cfg!(target_os="linux") {"explicit service install or user-manager startup"}else{"next logon or explicit service install"});}value
}
fn required(op:&mut resources::Op,command:&CommandReceipt,phase:&str)->Res<()> {
    if let Err(error)=command.checkpoint(phase) {
        op.receipt["reason"]=json!(format!("required lifecycle receipt unavailable: {}; see host log",error.chars().take(48).collect::<String>()));
        return Err(error);
    }
    op.receipt.as_object_mut().ok_or("lifecycle receipt corrupt")?.remove("reason");Ok(())
}
pub fn receipt(install:&Path,id:&str)->Res<Option<Value>> {
    if !resources::id(id) {return Err("invalid lifecycle request ID".into());}
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
            if binding(op).is_none() {continue;}
            parse(id,op)?;
            let text=binding(op).ok_or("lifecycle binding missing")?["command"].as_str().ok_or("restart command missing")?;
            let metadata:Value=serde_json::from_str(text).map_err(|_|"restart command corrupt")?;
            let at=metadata["started_ms"].as_i64().ok_or("lifecycle clock unknown")?;
            if last.as_ref().is_none_or(|(old,_)|at>*old) {last=Some((at,public(op)));}
        }
        Ok(last.map(|(_,value)|value))
    })};
    match result {Ok(Some(value))=>value,Ok(None)=>json!({"state":"unknown","reason":"no retained lifecycle receipt"}),Err(reason)=>json!({"state":"unknown","reason":reason})}
}

pub fn submit(install:&Path,value:&Value)->Res<Value> {
    match typed(value)?.action {Action::Restart=>submit_restart(install,value),Action::Stop=>submit_stop(install,value),Action::Uninstall=>submit_uninstall(install,value)}
}
#[tracing::instrument(name="host.restart.request",skip_all)]
fn submit_restart(install:&Path,value:&Value)->Res<Value> {submit_inner(install,value)}
#[tracing::instrument(name="host.stop.request",skip_all)]
fn submit_stop(install:&Path,value:&Value)->Res<Value> {submit_inner(install,value)}
#[tracing::instrument(name="host.uninstall.request",skip_all)]
fn submit_uninstall(install:&Path,value:&Value)->Res<Value> {submit_inner(install,value)}
fn submit_inner(install:&Path,value:&Value)->Res<Value> {
    if std::env::var_os("TASKQ_ATTEMPT").is_some() {return Err("a worker cannot restart the host service or request Stop".into());}
    let request=typed(value)?;
    let _projects=coreq::file_lock::try_lock(&install.join("projects.lock")).map_err(|e|format!("project registry busy: {e}"))?;
    let result=resources::with(install,|state| {
        if let Some(op)=state.ops.get(&request.id) {
            let old=parse(&request.id,op)?;
            if old.request!=request {return Err("lifecycle request ID reused with another payload".into());}
            return Ok(public(op));
        }
        if let Some(p)=active(state)? {return Err(format!("host restart {} already pending; no second request taken",p.id()));}
        resources::prepare(state,install,now()?)?;
        if state.ops.len()>=resources::MAX_OPS {return Err("operation store full; lifecycle not requested".into());}
        let at=now()?;
        if request.issued_at>at.saturating_add(30)||at>=request.expires_at {return Err("lifecycle review expired or clock unknown".into());}
        let service=host::service_status(install);
        if service["state"]!="running"||service["fresh"]!=true||service["restart_protocol"]!=1||service["instance"]!=request.instance
            ||(request.action==Action::Stop&&service["stop_protocol"]!=1)||(request.action==Action::Uninstall&&service["uninstall_protocol"]!=1)||scope(install)?!=request.roots {return Err("host or project scope changed; review lifecycle action again".into());}
        if request.stops() {crate::host_control::revision(install)?;}
        let command=CommandReceipt::current()?;command.checkpoint("received")?;
        let op=resources::Op {bound:json!({(request.action.key()):{"request":request,"command":command.encode()}}),
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
    let unit=if pending(install)?.is_some_and(|p|p.stops()) {Some(crate::host_control::revision(install)?)}else{None};
    prepare_with(install,instance,unit.as_deref())
}
fn prepare_with(install:&Path,instance:&str,unit:Option<&str>)->Res<()> {
    resources::try_with(install,|state| {
        let Some(p)=active(state)? else {return Ok(())};
        if p.origin()!=instance {return Err("original restart already has another service generation; reconcile before another switch".into());}
        if p.action()==Action::Uninstall&&!p.stops() {return Err("project Uninstall requires a qualified project drain; compatibility preparation refused".into());}
        if scope(install)?!=p.request.roots {return Err("restart scope changed; restore the reviewed registry before continuing".into());}
        let native=if p.stops() {
            let revision=unit.ok_or("Stop unit definition unavailable")?;
            let native=coreq::process::identity(std::process::id()).map_err(|e|e.to_string())?.ok_or("native owner missing")?;
            let receipt=&state.ops[p.id()].receipt;
            if p.phase=="applied"&&(receipt["unit_revision"]!=revision||receipt["native_pid"]!=native.pid||receipt["native_started"]!=native.started) {return Err("prepared Stop identity changed; outcome unknown".into());}
            Some((native,revision))
        }else{None};
        let op=state.ops.get_mut(p.id()).unwrap();required(op,&p.command,"applied")?;
        if let Some((native,revision))=native {op.receipt["native_pid"]=json!(native.pid);op.receipt["native_started"]=json!(native.started);op.receipt["unit_revision"]=json!(revision);}
        op.receipt["state"]=json!("applied");op.receipt["applied_receipt"]=json!(p.command.event_id("applied")?);
        Ok(())
    })
}

/// Only the new native owner after full project recovery may acknowledge the original command.
pub fn observe(install:&Path,instance:&str,ready:bool)->Res<()> {
    if !ready {return Ok(());}
    resources::try_with(install,|state| {
        let Some(p)=active(state)? else {return Ok(())};
        if p.action()!=Action::Restart||p.origin()==instance {return Ok(());}
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

pub fn failed(install:&Path,id:&str,error:&str)->Res<()> {
    resources::try_with(install,|state| {
        if let Some(op)=state.ops.get_mut(id) {
            let p=parse(id,op)?;
            if op.receipt["state"]!="observed" {
                let detail:String=error.char_indices().take_while(|(offset,ch)|offset+ch.len_utf8()<=112).map(|(_,ch)|ch).collect();
                op.receipt["reason"]=json!(format!("{} outcome unknown: {detail}; retry service {} --request {id}",p.action().title(),if p.action()==Action::Uninstall {"uninstall"}else{"stop"}));
            }
        }
        Ok(())
    })
}

#[tracing::instrument(name="host.stop.complete",skip_all)]
pub fn complete_stop(install:&Path,id:&str)->Res<Value> {
    let uninstall=pending(install)?.is_some_and(|p|p.id()==id&&p.action()==Action::Uninstall);
    let result=complete_stop_with(install,id,|native,revision| {
        if uninstall {crate::host_control::remove(install,id,native,revision)?;}else{crate::host_control::stop(install,id,native,revision)?;}
        crate::host_control::retire_controller(install,id)
    });
    if let Err(error)=&result {if let Err(save)=failed(install,id,error) {crate::platform::log(&format!("Stop failure readback unavailable: {save}"));}}
    result
}

fn launcher_scope(value:&Value)->Res<()> {
    if !resources::keys(value,&["pid","started","instance"])||value["pid"].as_u64().is_none_or(|n|n==0||n>i32::MAX as u64)
        ||value["started"].as_str().is_none_or(|s|s.is_empty()||s.len()>20||s.parse::<u64>().is_err())
        ||value["instance"].as_str().is_none_or(|s| {let s=if s.len()==38&&s.starts_with('{')&&s.ends_with('}') {&s[1..37]}else{s};s.len()!=36||s.bytes().enumerate().any(|(i,b)|if [8,13,18,23].contains(&i){b!=b'-'}else{!b.is_ascii_hexdigit()})}) {
        return Err("native launcher scope invalid; state kept".into());
    }
    Ok(())
}
#[cfg(any(windows,test))]
pub fn saved_launcher(install:&Path,id:&str,native:&coreq::process::Identity)->Res<Option<Value>> {
    resources::try_with(install,|state| {
        let op=state.ops.get(id).ok_or("Stop binding missing")?;let p=parse(id,op)?;
        if !p.stops()||p.phase!="applied"||op.receipt["native_pid"]!=native.pid||op.receipt["native_started"]!=native.started {return Err("Stop launcher binding differs".into());}
        Ok(op.receipt.get("launcher").cloned())
    })
}
#[cfg(any(windows,test))]
pub fn save_launcher(install:&Path,id:&str,native:&coreq::process::Identity,launcher:&Value)->Res<()> {
    launcher_scope(launcher)?;
    resources::try_with(install,|state| {
        let op=state.ops.get_mut(id).ok_or("Stop binding missing")?;let p=parse(id,op)?;
        if !p.stops()||p.phase!="applied"||op.receipt["native_pid"]!=native.pid||op.receipt["native_started"]!=native.started {return Err("Stop launcher binding differs".into());}
        if op.receipt.get("launcher").is_some_and(|old|old!=launcher) {return Err("native launcher identity changed; original scope kept".into());}
        op.receipt["launcher"]=launcher.clone();Ok(())
    })
}

fn complete_stop_with(install:&Path,id:&str,effect:impl FnOnce(&coreq::process::Identity,&str)->Res<()>)->Res<Value> {
    if std::env::var_os("TASKQ_ATTEMPT").is_some() {return Err("a worker cannot complete host Stop".into());}
    if !resources::id(id) {return Err("invalid Stop request ID".into());}
    let candidate=pending(install)?;
    let edit=match candidate.as_ref().filter(|p|p.id()==id&&p.action()==Action::Uninstall).and_then(Pending::removal) {
        Some(root)=>Some(host::ProjectEdit::for_uninstall(install,root,id)?),None=>None,
    };
    let _projects=if edit.is_none() {Some(coreq::file_lock::try_lock(&install.join("projects.lock")).map_err(|e|format!("Stop registry busy: {e}"))?)}else{None};
    let (pending,original)=resources::try_with(install,|state| {
        let op=state.ops.get(id).ok_or("Stop request not retained")?;
        let pending=parse(id,op)?;
        if !pending.stops()||pending.phase=="queued" {return Err("Stop has no qualified applied receipt; native effect refused".into());}
        Ok((pending,op.clone()))
    })?;
    if pending.phase=="observed" {return Ok(public(&original));}
    let roots=scope(install)?;
    if roots!=pending.request.roots&&!(pending.action()==Action::Uninstall&&roots==pending.request.after()) {return Err("Stop registry changed; native effect refused".into());}
    let native=coreq::process::Identity {pid:original.receipt["native_pid"].as_u64().ok_or("Stop pid unknown")? as u32,started:original.receipt["native_started"].as_str().ok_or("Stop identity unknown")?.into()};
    let service=host::service_status(install);
    if service["instance"]!=pending.origin()||service["pid"]!=native.pid||service["started"]!=native.started||service["state"]=="unknown" {
        return Err("Stop host incarnation differs or is unknown; reconcile the original owner".into());
    }
    effect(&native,original.receipt["unit_revision"].as_str().ok_or("Stop unit revision missing")?)?;
    if coreq::process::identity(native.pid).map_err(|e|e.to_string())?.as_ref()==Some(&native) {return Err("Stop native exit not observed".into());}
    let _owner=coreq::file_lock::try_lock(&install.join("service.lock")).map_err(|e|format!("host owner still held after Stop: {e}"))?;
    let service=host::service_status(install);
    if service["state"]!="exited"||service["instance"]!=pending.origin() {return Err("host changed after Stop; outcome unknown".into());}
    if let Some(edit)=edit {edit.commit()?;}
    resources::try_with(install,|state| {
        let op=state.ops.get_mut(id).ok_or("Stop request lost; outcome unknown")?;
        let current=parse(id,op)?;
        if current.request!=pending.request||op.bound!=original.bound {return Err("Stop binding changed; outcome unknown".into());}
        required(op,&pending.command,"applied")?;required(op,&pending.command,"observed")?;
        op.receipt["state"]=json!("observed");op.receipt["observed_receipt"]=json!(pending.command.event_id("observed")?);op.receipt["native_exit"]=json!(true);
        if pending.action()==Action::Uninstall {op.receipt["unit_removed"]=json!(true);if pending.removal().is_some() {op.receipt["unregistered"]=json!(true);}}
        op.expires_at=now()?;
        Ok(public(op))
    })
}

/// The host calls this only after a qualified project-thread drain, or an already-applied removal readback.
pub fn finish_project_uninstall(install:&Path,id:&str,instance:&str,drained:bool)->Res<()> {
    let p=pending(install)?.filter(|p|p.id()==id&&p.action()==Action::Uninstall&&!p.stops()).ok_or("project Uninstall intent missing")?;
    let root=p.removal().ok_or("project Uninstall root missing")?;
    let service=host::service_status(install);
    if service["state"]!="running"||service["fresh"]!=true||service["instance"]!=instance||service["pid"]!=std::process::id() {return Err("Uninstall observer is not the current native owner".into());}
    let roots=scope(install)?;
    if roots!=p.request.roots&&roots!=p.request.after() {return Err("Uninstall scope changed; registry kept".into());}
    if !drained&&!(p.phase=="applied"&&roots==p.request.after()) {return Err("project drain not observed; registry kept".into());}
    resources::try_with(install,|state| {
        let op=state.ops.get_mut(id).ok_or("Uninstall binding lost")?;
        if parse(id,op)?.request!=p.request {return Err("Uninstall binding changed".into());}
        required(op,&p.command,"applied")?;op.receipt["state"]=json!("applied");op.receipt["project_drained"]=json!(true);op.receipt["applied_receipt"]=json!(p.command.event_id("applied")?);Ok(())
    })?;
    host::ProjectEdit::for_uninstall(install,root,id)?.commit()?;
    if scope(install)?!=p.request.after() {return Err("Uninstall registry readback differs".into());}
    resources::try_with(install,|state| {
        let op=state.ops.get_mut(id).ok_or("Uninstall binding lost")?;
        if parse(id,op)?.request!=p.request {return Err("Uninstall binding changed".into());}
        required(op,&p.command,"observed")?;op.receipt["state"]=json!("observed");op.receipt["observed_receipt"]=json!(p.command.event_id("observed")?);op.receipt["unregistered"]=json!(true);op.receipt["service_instance"]=json!(instance);op.expires_at=now()?;Ok(())
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

    fn fixture_with_other()->(PathBuf,PathBuf,PathBuf,PathBuf) {
        let (dir,install,root)=fixture();let (dir,install,root)=(dir.canonicalize().unwrap(),install.canonicalize().unwrap(),root.canonicalize().unwrap());let other=dir.join("other");fs::create_dir_all(&other).unwrap();
        fs::write(other.join("taskq.json"),r#"{"repo":"fixture/other"}"#).unwrap();
        let other=other.canonicalize().unwrap();fs::write(install.join("projects"),format!("{}\n{}\n",root.display(),other.display())).unwrap();
        (dir,install,root,other)
    }
    fn publish(install:&Path,instance:&str) {
        let process=coreq::process::identity(std::process::id()).unwrap().unwrap();
        fs::write(install.join("service"),json!({"schema":1,"pid":process.pid,"started":process.started,"instance":instance,"at":now().unwrap(),"version":"fixture","restart_protocol":1,"stop_protocol":1,"uninstall_protocol":1}).to_string()).unwrap();
    }

    // H-A5: a non-last root remains registered until one drained thread has an applied receipt; a crash after
    // registry removal finalizes the original receipt without re-draining or recreating project work.
    #[test]
    fn project_uninstall_drains_before_registry_removal_and_replays_exact_receipt() {
        let (dir,install,root,other)=fixture_with_other();let (capture,guard)=crate::profile::tests::collecting(&install);
        let request=uninstall_review(&install,"drain-project",Some(&root)).unwrap();let received=submit(&install,&request).unwrap();
        assert_eq!(received["state"],"queued");assert!(prepare(&install,"original").unwrap_err().contains("qualified project drain"));
        assert_eq!(fs::read_to_string(install.join("projects")).unwrap(),format!("{}\n{}\n",root.display(),other.display()));
        assert!(finish_project_uninstall(&install,"drain-project","original",false).unwrap_err().contains("project drain not observed"));

        let db=rusqlite::Connection::open(install.join("telemetry.log/universal.sqlite3")).unwrap();
        db.execute_batch("CREATE TRIGGER refuse_uninstall_applied BEFORE INSERT ON events WHEN NEW.kind='command.applied' BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
        assert!(finish_project_uninstall(&install,"drain-project","original",true).is_err());
        assert_eq!(receipt(&install,"drain-project").unwrap().unwrap()["state"],"queued");
        assert_eq!(fs::read_to_string(install.join("projects")).unwrap(),format!("{}\n{}\n",root.display(),other.display()),"receipt failure before drain acknowledgement keeps registration");
        db.execute_batch("DROP TRIGGER refuse_uninstall_applied").unwrap();
        db.execute_batch("CREATE TRIGGER refuse_uninstall_observed BEFORE INSERT ON events WHEN NEW.kind='command.observed' BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
        assert!(finish_project_uninstall(&install,"drain-project","original",true).is_err());
        assert_eq!(receipt(&install,"drain-project").unwrap().unwrap()["state"],"applied");
        assert_eq!(fs::read_to_string(install.join("projects")).unwrap(),format!("{}\n",other.display()),"applied registry removal preserves unrelated roots");
        db.execute_batch("DROP TRIGGER refuse_uninstall_observed").unwrap();drop(db);
        finish_project_uninstall(&install,"drain-project","original",false).unwrap();
        let observed=receipt(&install,"drain-project").unwrap().unwrap();
        assert_eq!(observed["state"],"observed");assert_eq!(observed["received_receipt"],received["received_receipt"]);
        assert_eq!(submit(&install,&request).unwrap(),observed,"same ID returns the original terminal receipt");
        assert!(capture.flush().unwrap_err().contains("injected"));drop(guard);drop(capture);fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn project_uninstall_refuses_foreign_scope_without_registry_effect() {
        let (dir,install,root,other)=fixture_with_other();let request=uninstall_review(&install,"foreign-project",Some(&root)).unwrap();
        let (capture,guard)=crate::profile::tests::collecting(&install);submit(&install,&request).unwrap();
        let outsider=dir.join("outsider");fs::create_dir_all(&outsider).unwrap();fs::write(outsider.join("taskq.json"),r#"{"repo":"fixture/outsider"}"#).unwrap();
        let changed=format!("{}\n{}\n{}\n",root.display(),other.display(),outsider.display());fs::write(install.join("projects"),&changed).unwrap();
        assert!(finish_project_uninstall(&install,"foreign-project","original",true).unwrap_err().contains("scope changed"));
        assert_eq!(fs::read_to_string(install.join("projects")).unwrap(),changed);
        capture.flush().unwrap();drop(guard);drop(capture);fs::remove_dir_all(dir).unwrap();
    }

    // H-A5: Stop and both native Uninstall scopes retain the original receipt across owner exit and replay.
    #[test]
    fn stop_keeps_original_receipts_and_requires_attributable_native_exit() {
        use std::{process::{Command,Stdio,Child},time::Duration};
        struct Owner(Child);impl Drop for Owner {fn drop(&mut self){let _=self.0.kill();let _=self.0.wait();}}
        for mode in ["stop","project","host"] {
        let (dir,install,_)=if mode=="host" {let (d,i,r,_)=fixture_with_other();(d,i,r)}else{fixture()};fs::remove_file(install.join("service")).unwrap();
        let registry=fs::read(install.join("projects")).unwrap();
        let ready=dir.join("prepared");
        let mut child=Owner(Command::new(std::env::current_exe().unwrap()).args(["--exact","lifecycle::tests::stop_owner_probe","--ignored"])
            .env("TASKQ_STOP_FIXTURE",&install).env("TASKQ_STOP_MODE",mode).env("TASKQ_STOP_READY",&ready).env("HOME",install.join("home")).env_remove("TASKQ_ATTEMPT").env_remove("COREQ_TRACE_CONTEXT")
            .stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap());
        let until=Instant::now()+Duration::from_secs(10);
        while !ready.is_file() {assert!(child.0.try_wait().unwrap().is_none(),"Stop owner fixture exited before preparation");assert!(Instant::now()<until);std::thread::sleep(Duration::from_millis(10));}
        let (capture,guard)=crate::profile::tests::collecting(&install);
        let original=receipt(&install,"exact-stop").unwrap().unwrap();assert_eq!(original["action"],if mode=="stop" {"stop"}else{"uninstall"});assert_eq!(original["state"],"applied");
        if mode=="stop" {
            assert!(original["restart_condition"].as_str().unwrap().contains("service install"));
            assert_eq!(original["restart_condition"].as_str().unwrap().contains("logon"),!cfg!(target_os="linux"));
        }
        let native=coreq::process::identity(child.0.id()).unwrap().unwrap();
        assert!(saved_launcher(&install,"exact-stop",&native).unwrap().is_none());
        let launcher=json!({"pid":native.pid,"started":"1234567890","instance":"{01234567-89ab-cdef-0123-456789abcdef}"});
        let mut invalid=launcher.clone();invalid["instance"]=json!("{{01234567-89ab-cdef-0123-456789abcdef}}");
        assert!(save_launcher(&install,"exact-stop",&native,&invalid).is_err());
        assert!(saved_launcher(&install,"exact-stop",&native).unwrap().is_none());
        save_launcher(&install,"exact-stop",&native,&launcher).unwrap();save_launcher(&install,"exact-stop",&native,&launcher).unwrap();
        let mut replacement=launcher.clone();replacement["started"]=json!("1234567891");
        assert!(save_launcher(&install,"exact-stop",&native,&replacement).is_err());
        let foreign=coreq::process::Identity {pid:native.pid,started:format!("{}-foreign",native.started)};
        assert!(saved_launcher(&install,"exact-stop",&foreign).is_err());
        assert!(save_launcher(&install,"exact-stop",&foreign,&launcher).is_err());
        assert_eq!(saved_launcher(&install,"exact-stop",&native).unwrap(),Some(launcher));
        #[cfg(target_os="macos")] {
            // The test harness refuses service CLI args; its failed controller must still be owned and reaped.
            let mut control=crate::host_control::dispatch(&install,&pending(&install).unwrap().unwrap()).unwrap();
            let until=Instant::now()+Duration::from_secs(10);
            let result=loop {if let Some(result)=control.poll() {break result;}assert!(Instant::now()<until);std::thread::sleep(Duration::from_millis(10));};
            assert!(result.is_err());assert!(control.poll().is_none(),"controller completion is delivered once");
        }
        assert!(complete_stop_with(&install,"exact-stop",|_,_|Err("native effect failed".into())).unwrap_err().contains("native effect failed"));
        assert_eq!(fs::read(install.join("projects")).unwrap(),registry);
        assert!(complete_stop_with(&install,"exact-stop",|_,_|Ok(())).unwrap_err().contains("native exit"));
        assert_eq!(fs::read(install.join("projects")).unwrap(),registry);
        assert_eq!(status(&install,None)["state"],"applied");
        let lock=coreq::file_lock::try_lock(&install.join("projects.lock")).unwrap();
        assert!(complete_stop_with(&install,"exact-stop",|_,_|panic!("busy registry cannot stop a native owner")).is_err());drop(lock);
        observe(&install,"replacement",true).unwrap();assert_eq!(status(&install,None)["state"],"applied","Restart observer cannot complete Stop");
        let final_receipt=complete_stop_with(&install,"exact-stop",|native,revision| {
            assert_eq!(native.pid,child.0.id());assert_eq!(revision,original["unit_revision"].as_str().unwrap());
            child.0.kill().unwrap();child.0.wait().unwrap();Ok(())
        }).unwrap();
        assert_eq!(final_receipt["state"],"observed");assert_eq!(final_receipt["native_exit"],true);assert_eq!(final_receipt["received_receipt"],original["received_receipt"]);
        assert!(final_receipt.get("service_instance").is_none(),"native exit cannot claim a new service generation");
        assert_eq!(fs::read(install.join("projects")).unwrap(),if mode=="project" {Vec::new()}else{registry});
        if mode!="stop" {assert_eq!(final_receipt["unit_removed"],true);if mode=="project" {assert_eq!(final_receipt["unregistered"],true);}}
        assert_eq!(complete_stop_with(&install,"exact-stop",|_,_|panic!("observed Stop must never repeat the native effect")).unwrap(),final_receipt);
        capture.flush().unwrap();drop(guard);drop(capture);drop(child);fs::remove_dir_all(dir).unwrap();
        }
    }

    #[test]
    #[ignore="owned native service/receipt fixture; no OS service registration"]
    fn stop_owner_probe() {
        use std::io::Read;
        let install=PathBuf::from(std::env::var_os("TASKQ_STOP_FIXTURE").unwrap());
        let _owner=coreq::file_lock::try_lock(&install.join("service.lock")).unwrap();publish(&install,"original");
        let unit=if cfg!(windows) {install.join("TaskQ.xml")}else {let home=PathBuf::from(std::env::var_os("HOME").unwrap());assert_eq!(home,install.join("home"));home.join(if cfg!(target_os="macos") {"Library/LaunchAgents/com.taskq.service.plist"}else{".config/systemd/user/taskq.service"})};
        assert!(stop_review(&install,"before-unit").is_err(),"missing host unit cannot accept Stop");
        fs::create_dir_all(unit.parent().unwrap()).unwrap();fs::write(&unit,"owned fixture unit").unwrap();
        let (capture,guard)=crate::profile::tests::collecting(&install);
        let mode=std::env::var("TASKQ_STOP_MODE").unwrap_or_else(|_|"stop".into());
        let request=if mode=="stop" {stop_review(&install,"exact-stop")}else{let root=scope(&install).unwrap().remove(0);uninstall_review(&install,"exact-stop",if mode=="project" {Some(&root)}else{None})}.unwrap();let queued=submit(&install,&request).unwrap();assert_eq!(queued["state"],"queued");
        assert_eq!(submit(&install,&request).unwrap(),queued);
        let mut changed=request.clone();changed["action"]=json!("restart");assert!(submit(&install,&changed).is_err(),"same ID cannot change lifecycle action");
        assert!(complete_stop_with(&install,"exact-stop",|_,_|panic!("unprepared Stop cannot have an effect")).is_err());
        prepare_with(&install,"original",Some(&crate::host_control::revision(&install).unwrap())).unwrap();
        capture.flush().unwrap();drop(guard);drop(capture);
        fs::write(PathBuf::from(std::env::var_os("TASKQ_STOP_READY").unwrap()),"prepared").unwrap();
        let _=std::io::stdin().read(&mut [0]);
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
        assert!(status(&install,None)["reason"].as_str().unwrap().contains("required lifecycle receipt unavailable"));
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
