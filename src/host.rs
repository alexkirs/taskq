//! Host policy, occupied slots and project supervision (taskq.md § 3, § 4, § 9).
use crate::{Res, service::{self, Capacity, Gate}};
use serde_json::{Value, json};
use coreq::telemetry::capture::CommandReceipt;
use std::{collections::{BTreeMap, BTreeSet}, fs, path::{Path, PathBuf}, sync::{Arc, Mutex}, time::Instant};

thread_local!(static ACTIVE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) });
const CAPACITY_COMMAND:&str="_taskq_capacity_command";
pub fn active() -> bool { ACTIVE.get() }
pub fn enter(root: &Path, install: &Path) {
    ACTIVE.set(true);
    crate::PROJECT_ROOT.set(Some(root.into()));
    crate::platform::project_log(install.join(format!("{}.log", crate::project_name(root))));
}

#[derive(Clone, Debug, PartialEq)]
pub struct Policy {
    pub raw: Value,
    pub cap: Capacity,
    pub me: String,
    pub has: Vec<String>,
    pub codex: Vec<String>,
    pub permission_mode: String,
    pub update_every: u64,
    pub rollback_after: u64,
}

impl Policy {
    pub fn apply(&self, cfg: &mut service::Config) {
        cfg.me = self.me.clone(); cfg.has = self.has.clone(); cfg.codex = self.codex.clone();
        cfg.permission_mode = self.permission_mode.clone(); cfg.update_every = self.update_every; cfg.rollback_after = self.rollback_after;
    }
    pub fn parse(v: Value, hostname: &str) -> Res<Self> {
        if !v.is_object() || !v["capacity_hosts"].is_null() || !v["admission"]["mode"].is_null() {
            return Err("host.json: expected host policy, without capacity_hosts or mode".into());
        }
        if !matches!(v["admission"], Value::Null | Value::Object(_)) { return Err("host.json: admission must be an object".into()); }
        for key in ["hosts", "has"] {
            if !matches!(v[key], Value::Null | Value::Object(_)) { return Err(format!("host.json: {key} must be an object")); }
        }
        if let Some(m) = v["hosts"].as_object() {
            if m.values().any(|v| v.as_str().is_none_or(str::is_empty)) { return Err("host.json: invalid host mapping".into()); }
        }
        let strings = |v: &Value, key: &str| -> Res<Vec<String>> {
            v.as_array().ok_or(format!("host.json: {key} must be an array"))?.iter()
                .map(|s| s.as_str().map(String::from).ok_or(format!("host.json: {key} must contain strings"))).collect()
        };
        if let Some(m) = v["has"].as_object() { for x in m.values() { strings(x, "has")?; } }
        let me = std::env::var("TASKQ_HOST").ok().filter(|s| !s.is_empty())
            .or_else(|| v["hosts"][hostname].as_str().map(String::from)).unwrap_or_else(|| hostname.into());
        let has = if v["has"][&me].is_null() { vec![] } else { strings(&v["has"][&me], "has")? };
        let codex = if v["codex"].is_null() { vec!["-s".into(), "danger-full-access".into()] } else { strings(&v["codex"], "codex")? };
        let permission_mode = match &v["permission_mode"] {
            Value::Null => "dontAsk".into(), Value::String(s) if !s.is_empty() => s.clone(),
            _ => return Err("host.json: permission_mode must be a nonempty string".into()),
        };
        let timer = |k: &str, d| if v[k].is_null() { Ok(d) } else { v[k].as_u64().filter(|n| *n > 0).ok_or(format!("host.json: {k} must be positive seconds")) };
        let (update_every, rollback_after) = (timer("update_every", 60)?, timer("rollback_after", 600)?);
        let mut c = v.clone();
        if c["admission"].is_null() { c["admission"] = json!({}); }
        c["admission"]["mode"] = json!("auto");
        // The host policy allows the owner to disable the gate, without a Fixed-mode bypass.
        let off = c["load_gate"]["enabled"] == false;
        if off { c["load_gate"]["enabled"] = json!(true); }
        let mut cap = service::capacity(&c, &me)?;
        if off { cap.gate.enabled = false; }
        let policy=Self { raw: v, cap, me, has, codex, permission_mode, update_every, rollback_after };
        policy.command()?;
        Ok(policy)
    }

    fn command(&self)->Res<Option<CommandReceipt>> {
        let Some(value)=self.raw.get(CAPACITY_COMMAND) else {return Ok(None)};
        let invalid="host capacity command unsupported or corrupt; file kept";
        let object=value.as_object().ok_or(invalid)?;
        if object.len()!=4+usize::from(object.contains_key("request")) || !["schema","host","policy","receipt"].iter().all(|key|object.contains_key(*key))
            || value["schema"]!=1 || value["host"].as_str().is_none_or(str::is_empty) {
            return Err(invalid.into());
        }
        if object.contains_key("request") && !value["request"].as_str().is_some_and(crate::resources::id) {return Err(invalid.into());}
        let snapshot=value["policy"].as_object().ok_or(invalid)?;
        if snapshot.len()!=3 || !["limits","admission","load_gate"].iter().all(|key|snapshot.contains_key(*key))
            || value["policy"]["admission"]["mode"]!="auto" {return Err(invalid.into());}
        let mut policy=value["policy"].clone();
        policy["admission"].as_object_mut().ok_or(invalid)?.remove("mode");
        if Self::parse(policy,&self.me)?.cap.json()!=value["policy"] {return Err(invalid.into());}
        let text=value["receipt"].as_str().ok_or(invalid)?;
        let receipt=CommandReceipt::decode(text).map_err(|_|invalid)?;
        let metadata:Value=serde_json::from_str(text).map_err(|_|invalid)?;
        if metadata["product"]!="taskq" || metadata["operation"]!="taskq.host.policy.write" {return Err(invalid.into());}
        Ok(Some(receipt))
    }

    pub fn revision(&self)->Res<String> {
        if self.raw[CAPACITY_COMMAND]["host"]==self.me && self.raw[CAPACITY_COMMAND]["policy"]==self.cap.json()
            && let Some(command)=self.command()? {return command.event_id("observed");}
        Ok(ring::digest::digest(&ring::digest::SHA256,&serde_json::to_vec(&json!([self.me,self.cap.json()])).map_err(|e|e.to_string())?).as_ref().iter().map(|v|format!("{v:02x}")).collect())
    }

    pub fn read(install: &Path) -> Res<Self> {
        match fs::symlink_metadata(install.join("host.json.pending")) {
            Err(e) if e.kind()==std::io::ErrorKind::NotFound=>{},
            Err(e)=>return Err(format!("host policy prepare unknown: {e}")),
            Ok(_)=>return Err("host policy interrupted write retained; admission held for reconciliation".into()),
        }
        let v = fs::read(install.join("host.json")).map_err(|e| format!("host.json unknown: {e}"))?;
        Self::parse(serde_json::from_slice(&v).map_err(|e| format!("host.json unknown: {e}"))?, &crate::hostname())
    }

    pub fn project(&self, v: &Value) -> Res<Capacity> {
        let mut c = self.cap.clone();
        if !matches!(v["limits"], Value::Null | Value::Object(_)) { return Err("project limits: expected an object".into()); }
        for (rt, n) in v["limits"].as_object().into_iter().flatten() {
            let n = n.as_u64().and_then(|n| u32::try_from(n).ok()).ok_or(format!("project limits.{rt}: invalid count"))?;
            if self.cap.limits.get(rt).is_some_and(|host| n > *host) { return Err(format!("project limits.{rt} raises host ceiling")); }
            c.limits.insert(rt.clone(), n);
        }
        Ok(c)
    }
}

/// Existing resource-control lock serializes policy writers with grants and migration.
pub fn write_capacity(install:&Path, host:&str, edit:&crate::CapEdit, expect:Option<&Value>)->Res<(Capacity,Capacity,CommandReceipt)> {
    crate::resources::locked(install,||write_capacity_locked(install,host,edit,expect,None))
}

#[tracing::instrument(name="host.policy.write",skip_all)]
fn write_capacity_locked(install:&Path, host:&str, edit:&crate::CapEdit, expect:Option<&Value>,mut local:Option<(&mut crate::resources::State,&Value)>)->Res<(Capacity,Capacity,CommandReceipt)> {
    if std::env::var_os("TASKQ_ATTEMPT").is_some() {return Err("a worker cannot change host policy".into());}
        let migration=install.join("host-policy-migration");
        if migration.join("plan.json").try_exists().map_err(|e|e.to_string())?
            && !migration.join("complete").try_exists().map_err(|e|e.to_string())? {
            return Err("host policy migration incomplete; edits held until verified readback".into());
        }
        let path=install.join("host.json");
        let original=fs::read_to_string(&path).map_err(|e|format!("host.json unknown: {e}"))?;
        let hostname=crate::hostname();
        let old=Policy::parse(serde_json::from_str(&original).map_err(|e|format!("host.json unknown: {e}"))?,&hostname)?;
        if host!=old.me {return Err("host policy edits require this local host; remote edits are unsupported".into());}
        if let Some((state,request))=local.as_mut() {
            let id=request["id"].as_str().ok_or("capacity request ID missing")?;
            if let Some(op)=state.ops.get_mut(id) {
                if op.bound!=json!({"local_capacity":request}) {return Err("capacity request ID reused with another payload".into());}
                if op.receipt["state"]=="rejected" {return Err(op.receipt["reason"].as_str().unwrap_or("capacity request rejected").into());}
                let command=if op.receipt["state"]=="saved" {
                    let command=CommandReceipt::decode(op.receipt["command"].as_str().ok_or("capacity receipt corrupt")?)?;
                    if op.receipt["saved_revision"]!=command.event_id("observed")? {return Err("capacity receipt corrupt".into());}
                    command
                } else if old.raw[CAPACITY_COMMAND]["request"]==id && old.raw[CAPACITY_COMMAND]["policy"]==old.cap.json() {
                    old.command()?.ok_or("capacity request outcome unknown")?
                } else {return Err("capacity request outcome unknown; mutation will not be repeated".into());};
                op.writing=false;op.sent=true;
                op.receipt=json!({"state":"saved","saved_revision":command.event_id("observed")?,"command":command.encode()});
                return Ok((old.cap.clone(),old.cap,command));
            }
            if state.ops.len()>=crate::resources::MAX_OPS {return Err("capacity operation store full; no new request taken".into());}
            let now=wall()?;
            if request["issued_at"].as_u64().is_none_or(|at|at>now.saturating_add(30)) || request["expires_at"].as_u64().is_none_or(|at|now>=at) {return Err("capacity request expired or clock unknown".into());}
            let revision=old.revision()?;
            if request["ops"].as_array().ok_or("capacity operations missing")?.iter().any(|op|op["expected_revision"]!=revision) {return Err("host policy changed; review current values before applying".into());}
        }
        if edit.mode.is_some() {return Err("host policy has no mode selector; use optional limits and the load gate".into());}
        if expect.is_some_and(|v|*v!=old.cap.json()) {return Err("host policy changed; refresh before applying".into());}
        let mut next=old.raw.clone();
        for rt in edit.limits.keys().chain(&edit.unset) {
            if !service::RUNTIMES.contains(&rt.as_str()) {return Err(format!("unknown runtime {rt}"));}
        }
        if !edit.limits.is_empty() {
            if next["limits"].is_null() {next["limits"]=json!({});}
            for (rt,n) in &edit.limits {next["limits"][rt]=json!(n);}
        }
        for rt in &edit.unset {if let Some(limits)=next["limits"].as_object_mut() {limits.remove(rt);}}
        if let Some(max)=edit.max {if next["admission"].is_null() {next["admission"]=json!({});}next["admission"]["max"]=json!(max);}
        if edit.gate.is_some()||edit.cpu_max.is_some()||edit.memory_min.is_some() {
            if next["load_gate"].is_null() {next["load_gate"]=json!({});}
            if let Some(value)=edit.gate {next["load_gate"]["enabled"]=json!(value);}
            if let Some(value)=edit.cpu_max {next["load_gate"]["cpu_max"]=json!(value);}
            if let Some(value)=edit.memory_min {next["load_gate"]["memory_min"]=json!(value);}
        }
        let candidate=Policy::parse(next,&hostname)?;
        let command=CommandReceipt::current()?;
        command.checkpoint("received")?;
        let mut next=candidate.raw;
        next[CAPACITY_COMMAND]=json!({"schema":1,"host":candidate.me,"policy":candidate.cap.json(),"receipt":command.encode()});
        if let Some((state,request))=local.as_mut() {
            let id=request["id"].as_str().ok_or("capacity request ID missing")?;
            next[CAPACITY_COMMAND]["request"]=json!(id);
            state.ops.insert(id.into(),crate::resources::Op {bound:json!({"local_capacity":request}),receipt:json!({"state":"unknown"}),
                expires_at:request["expires_at"].as_u64().ok_or("capacity expiry missing")?,saved_at:None,writing:true,sent:true});
            crate::resources::save(install,state)?;
            #[cfg(test)]
            if std::env::var("TASKQ_HOST_POLICY_PHASE").is_ok_and(|phase|phase=="before-write") {
                crate::platform::write_atomic(&install.join("ack"),"intent-saved")?;loop {std::thread::park();}
            }
            if wall()?>=request["expires_at"].as_u64().ok_or("capacity expiry missing")? {
                let op=state.ops.get_mut(id).unwrap();op.writing=false;op.receipt=json!({"state":"rejected","reason":"expired"});
                return Err("capacity request expired before host policy write".into());
            }
        }
        let candidate=Policy::parse(next,&hostname)?;
        replace_owned(&path,&original,&(serde_json::to_string_pretty(&candidate.raw).map_err(|e|e.to_string())?+"\n"))?;
        let saved=Policy::read(install)?;
        if saved.raw!=candidate.raw {return Err("host policy write uncertain: readback differs".into());}
        if let Some((state,request))=local.as_mut() {
            let op=state.ops.get_mut(request["id"].as_str().unwrap()).unwrap();op.writing=false;op.saved_at=Some(wall()?);
            op.receipt=json!({"state":"saved","saved_revision":command.event_id("observed")?,"command":command.encode()});
        }
        #[cfg(test)]
        if std::env::var("TASKQ_HOST_POLICY_PHASE").is_ok_and(|phase|phase=="before-applied") {
            crate::platform::write_atomic(&install.join("ack"),"policy-saved")?;
            loop {std::thread::park();}
        }
        command.checkpoint("applied").map_err(|e|format!("host policy saved; command receipt unknown: {e}"))?;
        crate::platform::write_atomic(&install.join("poke"),"").map_err(|e|format!("host policy saved; host notification unknown: {e}"))?;
        Ok((old.cap,saved.cap,command))
}

fn wall()->Res<u64> {std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d|d.as_secs()).map_err(|_|"capacity clock unknown".into())}

/// Typed native request; OS-owned local controls do not grant the remote exchange any authority.
pub fn capacity_request(request:&Value)->Res<crate::CapEdit> {
    use crate::resources::{keys,count,id};
    let invalid="invalid host capacity request";
    if !keys(request,&["id","issued_at","expires_at","ops"]) || !request["id"].as_str().is_some_and(id)
        || serde_json::to_vec(request).map_err(|e|e.to_string())?.len()>16<<10 {return Err(invalid.into());}
    let issued=request["issued_at"].as_u64().ok_or(invalid)?;let expires=request["expires_at"].as_u64().ok_or(invalid)?;
    if expires<=issued || expires-issued>300 {return Err(invalid.into());}
    let ops=request["ops"].as_array().filter(|ops|!ops.is_empty()&&ops.len()<=4).ok_or(invalid)?;
    let mut seen=BTreeSet::new();let mut edit=crate::CapEdit::default();
    for op in ops {
        if !keys(op,&["target","policy_schema","expected_revision","changes"]) || op["policy_schema"]!="taskq.host-capacity/1"
            || !op["expected_revision"].as_str().is_some_and(id) || !keys(&op["target"],&["installation_id","generation","project_id","runtime_id"])
            || op["target"]["generation"]!=1 || !op["target"]["project_id"].is_null() {return Err(invalid.into());}
        let rt=op["target"]["runtime_id"].as_str();
        if (!op["target"]["runtime_id"].is_null() && rt.is_none_or(|rt|!service::RUNTIMES.contains(&rt))) || !seen.insert(rt) {return Err(invalid.into());}
        let changes=op["changes"].as_object().filter(|c|!c.is_empty()).ok_or(invalid)?;
        for (key,value) in changes {
            match key.as_str() {
                "runtime_limit" if rt.is_some() && (value.is_null()||count(value))=> {
                    let rt=rt.unwrap().to_string();if value.is_null() {edit.unset.insert(rt);}else {edit.limits.insert(rt,value.as_u64().unwrap() as u32);}
                }
                "maximum" if rt.is_none() && (value.is_null()||count(value))=>edit.max=Some(value.as_u64().map(|n|n as u32)),
                "load_gate" if rt.is_none() && keys(value,&["enabled","cpu_max","memory_min"]) && value["enabled"].is_boolean()
                    && value["cpu_max"].as_u64().is_some_and(|n|(10..=100).contains(&n)) && value["memory_min"].as_u64().is_some_and(|n|(1..=95).contains(&n))=> {
                        edit.gate=value["enabled"].as_bool();edit.cpu_max=value["cpu_max"].as_u64().map(|n|n as u32);edit.memory_min=value["memory_min"].as_u64().map(|n|n as u32);
                    }
                _=>return Err(invalid.into()),
            }
        }
    }
    Ok(edit)
}

pub fn local_capacity(install:&Path,host:&str,request:&Value)->Res<Value> {
    let edit=capacity_request(request)?;
    crate::resources::with(install,|state| {
        crate::resources::prepare(state,install,wall()?)?;
        let installation=match fs::read_to_string(install.join("installation-id")) {
            Ok(id) if id.len()==64 && id.bytes().all(|b|b.is_ascii_digit()||(b'a'..=b'f').contains(&b))=>json!(id),
            Err(e) if e.kind()==std::io::ErrorKind::NotFound=>Value::Null,
            _=>return Err("local installation identity unknown".into()),
        };
        if request["ops"].as_array().unwrap().iter().any(|op|op["target"]["installation_id"]!=installation) {return Err("capacity request belongs to another installation".into());}
        let result=write_capacity_locked(install,host,&edit,None,Some((state,request)));
        let mut receipt=state.ops.get(request["id"].as_str().unwrap()).filter(|op|op.bound==json!({"local_capacity":request})).map(|op|op.receipt.clone()).unwrap_or(json!({"state":"rejected"}));
        if let Err(error)=result {receipt["reason"]=json!(error);}
        receipt.as_object_mut().ok_or("local capacity receipt corrupt")?.remove("command");
        Ok(receipt)
    })
}

/// The host already loaded this snapshot; a later writer cannot borrow its observation.
pub fn observe_capacity(install:&Path,loaded:&Policy)->Res<Option<String>> {
    let Some(command)=loaded.command()? else {return Ok(None)};
    coreq::telemetry::capture::operation("host.policy.observe",||crate::resources::try_locked(install,|| {
        let current=Policy::read(install)?;
        let binding=&loaded.raw[CAPACITY_COMMAND];
        if current.raw!=loaded.raw || binding["host"]!=loaded.me || binding["policy"]!=loaded.cap.json() {
            return Err("capacity command changed or no longer matches the loaded policy; observation unknown".into());
        }
        command.checkpoint("applied")?;
        command.checkpoint("observed").map(Some)
    }),|_|"command-observation")
}

/// Read the exact durable acknowledgment, never infer it from equal values or wall-clock freshness.
pub fn capacity_observed(install:&Path,command:&CommandReceipt)->Res<bool> {
    let current=Policy::read(install)?;
    if current.command()?.is_none_or(|saved|saved.encode()!=command.encode())
        || current.raw[CAPACITY_COMMAND]["host"]!=current.me || current.raw[CAPACITY_COMMAND]["policy"]!=current.cap.json() {
        return Err("host policy saved; command superseded or manually changed".into());
    }
    let metadata:Value=serde_json::from_str(&command.encode()).map_err(|e|e.to_string())?;
    let id=command.event_id("observed")?;
    let reader=coreq::telemetry::Reader::open(&install.join("telemetry.log/universal.sqlite3"))?;
    let page=reader.export_kinds(&["command.observed"],metadata["started_ms"].as_i64().ok_or("capacity command time unknown")?,0,1024)?;
    let found=page["events"].as_array().ok_or("capacity receipt page invalid")?.iter().any(|event|event["id"]==id);
    if !found && page["more"]==true {return Err("capacity receipt page limit; observation unknown".into());}
    Ok(found)
}

/// Reuse migration's native replacement so a settings edit preserves owner/group/ACL and file permissions.
fn replace_owned(path:&Path, original:&str, next:&str)->Res<()> {
    use std::io::Write;
    let metadata=fs::symlink_metadata(path).map_err(|e|e.to_string())?;
    if !metadata.is_file() || metadata.permissions().readonly() {return Err("state file is not a writable regular file; kept".into());}
    let owner=ownership(path)?;
    let stage=path.with_file_name(&{let mut name=path.file_name().ok_or("state file name missing")?.to_os_string();name.push(".pending");name});
    let mut file=fs::OpenOptions::new().write(true).create_new(true).open(&stage)
        .map_err(|e|format!("state file prepare refused; interrupted state kept: {e}"))?;
    let result=(|| {
        stage_owner(&stage,&owner)?;
        file.set_permissions(metadata.permissions()).map_err(|e|e.to_string())?;
        file.write_all(next.as_bytes()).map_err(|e|e.to_string())?;
        file.sync_all().map_err(|e|e.to_string())?;
        drop(file);
        if fs::read_to_string(path).map_err(|e|e.to_string())?!=original || ownership(path)?!=owner {
            return Err("state file changed while saving; current file kept".into());
        }
        replace_project(path,&stage)?;
        #[cfg(windows)] stage_owner(path,&owner)?;
        #[cfg(unix)] fs::File::open(path.parent().ok_or("state file parent missing")?).and_then(|f|f.sync_all()).map_err(|e|e.to_string())?;
        if ownership(path)?!=owner {return Err("state file ownership readback differs".into());}
        Ok(())
    })();
    if result.is_err() && let Err(cleanup)=fs::remove_file(&stage) && cleanup.kind()!=std::io::ErrorKind::NotFound {
        return Err(format!("state file write uncertain: {}; temporary cleanup failed: {cleanup}",result.unwrap_err()));
    }
    result.map_err(|e:String|format!("{} write uncertain: {e}",path.display()))
}

// The atomic registry publisher reserves projects.pending for its own bytes.
pub(crate) const INSTALL_PENDING:&str="projects.installing";

pub type Shared = Arc<Mutex<Host>>;
type Slot = (PathBuf, u64, u64);
fn slot(project: &Path, task: u64, attempt: u64) -> Slot {
    // Before claim there is no fencing token; only that provisional slot uses the issue number.
    (project.into(), if attempt == 0 { task } else { 0 }, attempt)
}
pub enum Maintenance {Update(Option<String>),Installation(String),Inventory(Res<Vec<String>>)}

pub struct Host {
    pub instance: String,
    pub policy: Res<Policy>,
    pub capacity_command:Option<String>,
    slots: BTreeMap<Slot, String>,
    pub gate: Gate,
    began: Instant,
    last: Option<u64>,
    /// Registered ready projects, rotated only after a grant.
    ready: BTreeSet<PathBuf>,
    turn: Option<PathBuf>,
    pub held: Option<String>,
    pub restart: bool,
    pub stopping: bool,
    pub stop_controller:Option<crate::host_control::Control>,
    /// Each thread reports active turns/children before the host considers an image switch.
    pub busy: BTreeMap<PathBuf, bool>,
    pub prepared: BTreeSet<PathBuf>,
    pub recovered: BTreeSet<PathBuf>,
    pub updating:Option<std::thread::JoinHandle<Res<Maintenance>>>,
    pub draining: BTreeSet<PathBuf>,
    pub request_since: u64,
    pub hour_start: BTreeMap<String, u64>,
    pub last_hour: Option<BTreeMap<String, u64>>,
    pub rebuilding: bool,
    rebuilt: BTreeSet<PathBuf>,
    published: BTreeMap<PathBuf, Instant>,
}

impl Host {
    pub fn new(policy: Res<Policy>) -> Self {
        Self { instance:crate::runtime::uuid(),policy, capacity_command:None, slots: BTreeMap::new(), gate: Gate::default(), began: Instant::now(), last: None,
            ready: BTreeSet::new(), turn: None, held: None, restart: false, stopping:false,stop_controller:None,busy: BTreeMap::new(), prepared: BTreeSet::new(), recovered:BTreeSet::new(),updating:None, draining: BTreeSet::new(),
            request_since: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()), hour_start: crate::requests::totals(), last_hour: None,
            rebuilding: false, rebuilt: BTreeSet::new(), published: BTreeMap::new() }
    }

    pub fn reload(&mut self, policy: Res<Policy>) {
        if self.policy!=policy {self.capacity_command=None;}
        if let Ok(p) = &policy { self.gate.configure(p.cap.gate); }
        self.policy = policy;
    }

    /// A newly attached project may carry old occupancy, even if its root was drained earlier.
    fn attach(&mut self,root:&Path) {
        self.busy.insert(root.into(),true);
        self.prepared.remove(root);self.recovered.remove(root);self.rebuilt.remove(root);
        self.rebuilding=true;
    }

    pub fn sync(&mut self, project: &Path, held: Vec<crate::admission::Res>) -> usize {
        self.slots.retain(|(p, _, _), _| p != project);
        for (n, a, rt) in held { self.slots.insert(slot(project, n, a), rt); }
        self.slots.len()
    }

    /// A poisoned table is unusable until every active project reports its still-owned handles and pending grants.
    pub fn rebuild_slots(&mut self, project: &Path, held: Vec<crate::admission::Res>) -> bool {
        if !self.rebuilding {
            self.rebuilding = true; self.rebuilt.clear(); self.slots.clear(); self.ready.clear();
            self.gate = Gate::default(); self.last = Some(self.elapsed());
        }
        self.sync(project, held);
        self.rebuilt.insert(project.into());
        let complete = self.busy.keys().all(|root| self.rebuilt.contains(root));
        if complete { self.rebuilding = false; }
        complete
    }

    pub fn ready(&mut self, project: &Path, ready: bool) {
        if ready { self.ready.insert(project.into()); } else { self.ready.remove(project); }
    }

    pub fn publication(&mut self, project: &Path, qualified: bool) {
        if qualified { self.published.insert(project.into(), Instant::now()); } else { self.published.remove(project); }
    }

    pub fn publications_ready(&self, roots: &[PathBuf]) -> bool {
        self.policy.is_ok() && self.held.is_none() && !self.rebuilding && roots.iter().all(|root|
            self.published.get(root).is_some_and(|at| at.elapsed().as_secs() < 60))
    }

    pub fn handoff_ready(&self) -> bool {
        self.restart && !self.rebuilding && self.held.is_none()&&self.updating.is_none()
            &&(!self.stopping||(self.slots.is_empty()&&self.busy.values().all(|busy|!*busy)))
            && self.busy.keys().all(|root|self.prepared.contains(root))
            && self.slots.keys().all(|(root,_,_)|self.prepared.contains(root))
    }

    /// Once all projects prepared, none can re-enter before the coordinator switches images.
    pub fn begin_pass(&mut self, root:&Path) -> bool {
        if self.handoff_ready() {return false;}
        self.prepared.remove(root);
        self.recovered.remove(root);
        self.busy.insert(root.into(),true);
        true
    }

    pub fn reserve(&mut self, project: &Path, n: u64, a: u64, rt: &str, cap: &Capacity, queue: bool, now: u64) -> Res<usize> {
        let p = self.policy.as_ref().map_err(Clone::clone)?;
        if self.rebuilding {return Err("host occupancy recovery pending".into());}
        if let Some(reason) = &self.held { return Err(reason.clone()); }
        if self.draining.contains(project) && queue { return Err("project draining".into()); }
        if self.restart { return Err("host restart pending".into()); }
        if self.slots.contains_key(&slot(project, n, a)) {
            if p.cap.limits.get(rt) == Some(&0) || cap.limits.get(rt) == Some(&0) { return Err("runtime disabled".into()); }
            return Ok(self.slots.len());
        }
        if queue && !self.ready.is_empty() {
            let next = self.ready.iter().find(|p| self.turn.as_ref().is_none_or(|t| *p > t)).or_else(|| self.ready.first());
            if next.is_some_and(|p| p != project) { return Err("round-robin: another project is ready".into()); }
        }
        if !self.gate.may_launch(now) || self.last.is_some_and(|t| now.saturating_sub(t) < 30) { return Err("host load gate or 30 s launch spacing".into()); }
        let used = self.slots.values().filter(|r| *r == rt).count();
        let own = self.slots.iter().filter(|((root, _, _), r)| root == project && *r == rt).count();
        if p.cap.max.is_some_and(|m| self.slots.len() >= m as usize)
            || p.cap.limits.get(rt).is_some_and(|m| used >= *m as usize)
            || cap.limits.get(rt).is_some_and(|m| own >= *m as usize) { return Err("host or project capacity full".into()); }
        self.slots.insert(slot(project, n, a), rt.into());
        if queue { self.turn = Some(project.into()); }
        self.last = Some(now);
        self.gate.launch(now);
        Ok(self.slots.len())
    }

    pub fn claimed(&mut self, project: &Path, n: u64, a: u64) {
        if let Some(rt) = self.slots.remove(&slot(project, n, 0)) { self.slots.insert(slot(project, n, a), rt); }
    }

    pub fn remaining(&self, project: &Path, rt: &str, cap: &Capacity) -> usize {
        if self.rebuilding {return 0;}
        let Ok(p) = &self.policy else { return 0 };
        let host = (p.cap.limits.get(rt).copied().unwrap_or(u32::MAX) as usize)
            .saturating_sub(self.slots.values().filter(|r| *r == rt).count());
        let own = (cap.limits.get(rt).copied().unwrap_or(u32::MAX) as usize)
            .saturating_sub(self.slots.iter().filter(|((root, _, _), r)| root == project && *r == rt).count());
        let max = (p.cap.max.unwrap_or(u32::MAX) as usize).saturating_sub(self.slots.len());
        host.min(own).min(max)
    }

    pub fn elapsed(&self) -> u64 { self.began.elapsed().as_secs() }

    pub fn used(&self)->BTreeMap<String,usize> {
        let mut used=BTreeMap::new();
        for runtime in self.slots.values() {*used.entry(runtime.clone()).or_default()+=1;}
        used
    }
}

#[derive(serde::Serialize,serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ServiceRecord {
    schema:u8,pid:u32,started:String,instance:String,at:u64,version:String,
    #[serde(default)] restart_protocol:Option<u8>,
    #[serde(default)] stop_protocol:Option<u8>,
    #[serde(default)] uninstall_protocol:Option<u8>,
    #[serde(default)] install_protocol:Option<u8>,
}
enum OwnerRecord { Legacy(u32), Current(ServiceRecord) }

fn service_record(install:&Path)->Res<Option<OwnerRecord>> {
    use std::io::Read;
    let file=match fs::File::open(install.join("service")) {
        Err(e) if e.kind()==std::io::ErrorKind::NotFound=>return Ok(None),
        Err(e)=>return Err(format!("host service owner unknown: {e}")),
        Ok(file)=>file,
    };
    let mut bytes=Vec::new();file.take(1025).read_to_end(&mut bytes).map_err(|e|format!("host service owner unknown: {e}"))?;
    if bytes.len()>1024 {return Err("host service owner unknown: oversized record".into());}
    let text=String::from_utf8(bytes).map_err(|_|"host service owner unknown: invalid text")?;
    if let Ok(pid)=text.trim().parse::<u32>() {
        if pid==0||pid>i32::MAX as u32 {return Err("process liveness unknown: invalid pid".into());}
        return Ok(Some(OwnerRecord::Legacy(pid)));
    }
    let r:ServiceRecord=serde_json::from_str(&text).map_err(|_|"host service owner unknown: malformed record")?;
    if r.schema!=1||r.pid==0||r.pid>i32::MAX as u32||r.started.is_empty()||r.started.len()>128
        ||!r.started.bytes().all(|c|c.is_ascii_alphanumeric()||c==b'-')||!crate::resources::id(&r.instance)||r.version.is_empty()||r.version.len()>80||r.restart_protocol.is_some_and(|v|v!=1)||r.stop_protocol.is_some_and(|v|v!=1)||r.uninstall_protocol.is_some_and(|v|v!=1)||r.install_protocol.is_some_and(|v|v!=1) {
        return Err("host service owner unknown: unsupported record; kept".into());
    }
    Ok(Some(OwnerRecord::Current(r)))
}

fn publish_service(install:&Path,native:&coreq::process::Identity,instance:&str)->Res<()> {
    let record=ServiceRecord {schema:1,pid:native.pid,started:native.started.clone(),instance:instance.into(),at:wall()?,version:crate::platform::VERSION.into(),restart_protocol:Some(1),stop_protocol:Some(1),uninstall_protocol:Some(1),install_protocol:Some(1)};
    crate::platform::write_atomic(&install.join("service"),&(serde_json::to_string(&record).map_err(|e|e.to_string())?+"\n"))
}

/// A prior recorded owner must have exited; age never proves that. The native lock still owns admission.
pub fn other(install: &Path) -> Res<Option<u32>> {
    match service_record(install)? {
        None=>Ok(None),
        Some(OwnerRecord::Legacy(pid))=>Ok((pid!=std::process::id()&&crate::runtime::liveness(pid)?&&!installation::legacy_projection(install,pid,None)?).then_some(pid)),
        Some(OwnerRecord::Current(r))=> {
            let live=coreq::process::identity(r.pid).map_err(|e|format!("host service owner unknown: {e}"))?;
            Ok((r.pid!=std::process::id()&&live.as_ref().is_some_and(|p|p.started==r.started)&&!installation::legacy_projection(install,r.pid,Some(&r.started))?).then_some(r.pid))
        }
    }
}

/// Native process state and projection freshness are separate; neither releases any owned execution.
pub fn service_status(install:&Path)->Value {
    let result=coreq::telemetry::capture::operation("host.service.observe",||->Res<Value> {
        let record=service_record(install)?.ok_or("host service owner record missing")?;
        let OwnerRecord::Current(r)=record else {return Err("legacy PID-only owner record; host identity unknown".into());};
        let native=coreq::process::identity(r.pid).map_err(|e|format!("host service owner unknown: {e}"))?;
        let live=native.as_ref().is_some_and(|p|p.started==r.started);
        let age=wall()?.checked_sub(r.at);
        let fresh=live&&age.is_some_and(|age|age<=60);
        Ok(json!({"state":if live {"running"}else{"exited"},"pid":r.pid,"started":r.started,"instance":r.instance,"version":r.version,
            "at":r.at,"age_seconds":age,"fresh":fresh,"restart_protocol":r.restart_protocol,"stop_protocol":r.stop_protocol,"uninstall_protocol":r.uninstall_protocol,"install_protocol":r.install_protocol,"reason":if !live {Some("recorded host process exited; worker outcomes remain separate")}else if !fresh {Some("host observation stale or clock unknown")}else{None}}))
    },|_|"owner-unknown");
    result.unwrap_or_else(|reason|json!({"state":"unknown","fresh":false,"reason":reason}))
}

pub fn service_line(status:&Value)->String {
    match status["state"].as_str() {
        Some("running")=>format!("Service: running (host, pid {}); observation {}",status["pid"],if status["fresh"]==true {"fresh"}else{"stale or unknown"}),
        Some("exited")=>format!("Service: recorded host process exited (pid {}); worker outcomes unknown here",status["pid"]),
        _=>format!("Service: unknown ({})",status["reason"].as_str().unwrap_or("no native observation")),
    }
}

/// Legacy roots are never served concurrently, even when their status has gone stale. Unknown holds admission.
pub fn project_service(root: &Path) -> Res<Option<u32>> {
    match crate::data::json(&crate::dot(root).join("status.json")) {
            Err(e) => return Err(format!("legacy liveness unknown: {e}")),
            Ok(None) => Ok(None),
            Ok(Some(v)) => {
                let pid = v["pid"].as_u64().and_then(|n| u32::try_from(n).ok()).ok_or("legacy liveness unknown: missing pid")?;
                if pid == 0 { return Err("process liveness unknown: invalid pid".into()); }
    Ok((pid != std::process::id() && crate::runtime::liveness(pid)?).then_some(pid))
            }
    }
}

fn legacy(install:&Path,roots: &[PathBuf]) -> Res<()> {
    if !installation::legacy_definitions(install)?.is_empty() {return Err("handoff: legacy native definitions remain; host admission held".into());}
    for root in roots {
        if project_service(root)?.is_some() { return Err("handoff: legacy service still running".into()); }
    }
    Ok(())
}

fn startup_status(install: &Path, root: &Path, error: &str,instance:&str) -> Res<()> {
    if project_service(root)?.is_some() { return Err("legacy ownership retained; startup status not written".into()); }
    let mut status = coreq::status::common(&crate::platform::tool(install), "unknown", &crate::project_name(root),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()), Some(error.into()), None);
    fs::create_dir_all(crate::dot(root)).map_err(|e| e.to_string())?;
    status["host_instance"]=json!(instance);
    coreq::status::write(&crate::platform::tool(install), root, &status)
}

fn owner(install:&Path)->Res<Option<coreq::file_lock::Lock>> {
    if other(install)?.is_some() {return Ok(None);}
    match coreq::file_lock::try_lock(&install.join("service.lock")) {
        Ok(lock)=>Ok(Some(lock)),
        Err(std::fs::TryLockError::WouldBlock)=>Ok(None),
        Err(e)=>Err(format!("host service owner lock unknown: {e}")),
    }
}

fn native_inventory_ready(inventory:&Res<Vec<String>>,checked:Option<Instant>)->Res<()> {
    let names=inventory.as_ref().map_err(Clone::clone)?;
    if let Some(name)=names.first() {return Err(format!("handoff: {} native legacy unit(s) remain, including {}",names.len(),crate::compact(name,128)));}
    if checked.is_none_or(|at|at.elapsed().as_secs()>=60) {return Err("native legacy inventory stale; host admission held".into());}
    Ok(())
}

fn native_inventory_due(inventory:&Res<Vec<String>>,checked:Option<Instant>)->bool {
    checked.is_none_or(|at|at.elapsed().as_secs()>=if inventory.as_ref().is_ok_and(Vec::is_empty) {60}else{5})
}

pub fn run(install: &Path) -> Res<()> {
    fs::create_dir_all(install).map_err(|e| e.to_string())?;
    let mut reported=String::new();
    let _owner=loop {
        let message=match owner(install) {
            Ok(Some(owner))=>break owner,
            Ok(None)=>"host service owner already active; waiting".into(),
            Err(e)=>format!("{e}; waiting"),
        };
        if message!=reported {crate::platform::log(&message);reported=message;}
        std::thread::sleep(std::time::Duration::from_secs(1));
    };
    let shared = Arc::new(Mutex::new(Host::new(Policy::read(install))));
    shared.lock().map_err(|_|"host occupancy recovery unknown")?.rebuilding=true;
    let native=coreq::process::identity(std::process::id()).map_err(|e|e.to_string())?.ok_or("host process identity missing")?;
    let instance=shared.lock().map_err(|_|"host owner unknown")?.instance.clone();
    let mut threads: BTreeMap<PathBuf, std::thread::JoinHandle<Res<()>>> = BTreeMap::new();
    let mut roots = vec![];
    let mut stopped = BTreeSet::new();
    let mut drained_roots = BTreeSet::new();
    let mut checked: Option<Instant> = None;
    let mut installation_checked:Option<Instant>=None;
    let mut native_checked:Option<Instant>=None;
    let mut native_legacy:Res<Vec<String>>=Err("native legacy inventory pending; host admission held".into());
    let mut maintenance_result=String::new();
    let began = Instant::now();
    let mut hour = (Instant::now(), crate::requests::totals());
    let mut tray = None;
    let mut registry = Registry::default();
    let mut restart=crate::lifecycle::Monitor::default();
    loop {
        if let Err(e) = crate::profile::prune(install) { eprintln!("taskq: telemetry unknown: {e}"); }
        let publication=publish_service(install,&native,&instance).err();
        if let Some(error)=&publication {
            let message=format!("host publication unknown: {error}");if reported!=message {crate::platform::log(&message);reported=message;}
        }
        registry.refresh(install, began.elapsed().as_secs());
        restart.refresh(install);
        roots.clone_from(&registry.roots);
        let old = legacy(install,&roots).and_then(|_|native_inventory_ready(&native_legacy,native_checked));
        let policy = match &old { Ok(()) => migrate(install, &roots).and_then(|_| Policy::read(install)), Err(e) => Err(e.clone()) };
        let removal=restart.current.as_ref().ok().and_then(Option::as_ref).and_then(|p|p.removal().map(Path::to_path_buf));
        drained_roots.retain(|root|removal.as_ref()==Some(root));
        {
            let mut h = shared.lock().unwrap_or_else(|e| e.into_inner());
            h.reload(policy);
            h.held = installation::admission_reason(install).or_else(||registry.error.clone()).or_else(|| old.as_ref().err().cloned()).or_else(||restart.current.as_ref().err().cloned())
                .or_else(||publication.as_ref().map(|e|format!("host publication unavailable: {e}")));
            if let Some(p)=restart.current.as_ref().ok().and_then(Option::as_ref) {
                if p.stops() {
                    if !h.stopping {h.prepared.clear();}
                    h.restart=true;h.stopping=true;
                    if p.origin()!=instance {h.held=Some("Stop belongs to another host generation; reconcile original owner via service status --request".into());}
                } else if p.action()==crate::lifecycle::Action::Restart&&p.origin()==instance {h.restart=true;}
            }
            let now = h.elapsed();
            if h.gate.last.is_none_or(|l| now.saturating_sub(l.0) >= 30) {
                h.gate.host_mem = crate::platform::host_memory();
                let wall = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
                h.gate.observe(now, wall, crate::platform::cpu_ticks(), crate::platform::memory_available());
            }
            for root in threads.keys().filter(|p| !roots.contains(p)) { h.draining.insert(root.clone()); }
            for root in &roots {
                if removal.as_ref()==Some(root) {h.draining.insert(root.clone());}else{h.draining.remove(root);}
                if threads.contains_key(root) || stopped.contains(root)||drained_roots.contains(root) { continue; }
                h.attach(root);
                let root = root.clone();
                let install = install.to_path_buf();
                let host = shared.clone();
                threads.insert(root.clone(), std::thread::spawn(move || {
                    enter(&root, &install);
                    loop {
                        // Never replace another process's ownership evidence, including on startup/config failure.
                        match project_service(&root) {
                            Ok(None) => {},
                            Ok(Some(_)) | Err(_) => { std::thread::sleep(std::time::Duration::from_secs(1)); continue; }
                        }
                        // A broken project config holds only this thread; no board or child exists yet.
                        let context = crate::load(&root).and_then(|(cfg, raw)| {
                            let board = cfg.board.open(&cfg.web_host, &cfg.repo, raw["label_schema"].as_str(), &crate::platform::tool(&install))?;
                            Ok(crate::Ctx { cfg, raw, board })
                        });
                        match context {
                            Ok(c) => return crate::service_run(&c, Some(host),None),
                            Err(e) => {
                                if let Ok(mut h) = host.lock() {
                                    // No board/native reconciliation exists while configuration is invalid.
                                    h.busy.insert(root.clone(), true);
                                    h.prepared.remove(&root);
                                }
                                let instance=host.lock().map(|h|h.instance.clone()).unwrap_or_default();
                                let _ = startup_status(&install, &root, &e,&instance);
                                crate::platform::log(&e);
                                std::thread::sleep(std::time::Duration::from_secs(1));
                            }
                        }
                    }
                }));
            }
        }
        {let mut h=shared.lock().unwrap_or_else(|e|e.into_inner());if roots.is_empty()&&h.busy.is_empty()&&h.held.is_none() {h.rebuilding=false;}}
        let loaded={let h=shared.lock().unwrap_or_else(|e|e.into_inner());
            h.capacity_command.is_none().then(||h.policy.as_ref().ok().cloned()).flatten()};
        if let Some(policy)=loaded {
            match observe_capacity(install,&policy) {
                Ok(command)=> {
                    let mut h=shared.lock().unwrap_or_else(|e|e.into_inner());
                    if h.policy.as_ref().ok()==Some(&policy) {h.capacity_command=command;}
                }
                Err(e)=> {if reported!=e {crate::platform::log(&e);reported=e;}}
            }
        }
        // Joining is nonblocking: failed projects do not stop unrelated loops, and their occupied slots stay held.
        let ended: Vec<_> = threads.iter().filter(|(_, t)| t.is_finished()).map(|(p, _)| p.clone()).collect();
        for root in ended {
            let outcome = threads.remove(&root).unwrap().join();
            let drained = matches!(&outcome, Ok(Err(e)) if e == "project drained");
            let mut h = shared.lock().unwrap_or_else(|e| e.into_inner());
            h.publication(&root, false);
            if drained { h.busy.remove(&root); h.prepared.remove(&root);h.recovered.remove(&root);drained_roots.insert(root.clone()); }
            else {
                // A reduced loop failed: its attempts expire on other hosts, and local launches hold for inspection.
                stopped.insert(root.clone());
                h.prepared.remove(&root);
                h.recovered.remove(&root);
                h.ready(&root, false);
                append_project(install, &root, "project thread stopped; leases expire on other hosts; retained slots have unknown liveness");
            }
        }
        if let Some(p)=restart.current.as_ref().ok().and_then(Option::as_ref).filter(|p|p.action()==crate::lifecycle::Action::Uninstall&&!p.stops()) {
            let drained=p.removal().is_some_and(|root|drained_roots.contains(root));
            if drained||p.removal().is_some_and(|root|!roots.iter().any(|p|p==root)) {
                if let Err(error)=crate::lifecycle::finish_project_uninstall(install,p.id(),&instance,drained) {
                    let _=crate::lifecycle::failed(install,p.id(),&error);if reported!=error {crate::platform::log(&error);reported=error;}
                }
            }
        }
        let h = shared.lock().unwrap_or_else(|e| e.into_inner());
        let recovered=h.publications_ready(&roots)&&roots.iter().all(|r|h.recovered.contains(r));
        drop(h);
        if let Err(e)=crate::lifecycle::observe(install,&instance,recovered) {if reported!=e {crate::platform::log(&e);reported=e;}}
        let mut h=shared.lock().unwrap_or_else(|e|e.into_inner());
        let finished=h.stop_controller.as_mut().and_then(|control|control.poll().map(|result|(control.id.clone(),result)));
        if let Some((id,result))=finished {
            drop(h);
            let error=result.err().unwrap_or_else(||"controller exited without an observed host stop".into());
            let _=crate::lifecycle::failed(install,&id,&error);crate::platform::log(&format!("Stop controller: {}",crate::compact(&error,153)));
            h=shared.lock().unwrap_or_else(|e|e.into_inner());
        }
        let timers = h.policy.as_ref().map(|p| (p.update_every, p.rollback_after)).unwrap_or((60, 600));
        if h.updating.as_ref().is_some_and(|job|job.is_finished()) {
            let updated=h.updating.take().unwrap().join().unwrap_or_else(|_|Err("host maintenance panicked; partial effects unknown".into()));
            if let Ok(Maintenance::Inventory(result))=&updated {native_legacy=result.clone();}
            let text=match &updated {Ok(Maintenance::Update(Some(v)))=>format!("update: {v}"),Ok(Maintenance::Update(None))=>"update: up to date".into(),Ok(Maintenance::Installation(result))=>format!("installation: {result}"),Ok(Maintenance::Inventory(Ok(names)))=>format!("native legacy inventory: {} unit(s)",names.len()),Ok(Maintenance::Inventory(Err(e)))=>format!("native legacy inventory unknown: {e}"),Err(e)=>format!("maintenance: {e}")};
            if maintenance_result!=text {crate::platform::log(&text);for root in &roots {append_project(install,root,&text);}maintenance_result=text;}
            if crate::installed(install)&&matches!(updated,Ok(Maintenance::Update(Some(_)))) {h.restart=true;}
        }
        if h.updating.is_none()&&native_inventory_due(&native_legacy,native_checked) {
            native_checked=Some(Instant::now());native_legacy=Err("native legacy inventory pending; host admission held".into());
            let install=install.to_path_buf();
            let lifecycle_controller=restart.current.as_ref().ok().and_then(Option::as_ref).filter(|p|p.stops()&&p.prepared()).map(|p|p.id().to_string());
            let job=coreq::telemetry::capture::operation("host.legacy.inventory.dispatch",||std::thread::Builder::new().name("taskq-native-inventory".into()).spawn(coreq::telemetry::capture::bind(move || {
                let result=(|| {let mut controllers:Vec<_>=lifecycle_controller.into_iter().collect();controllers.extend(installation::controller_id(&install)?);crate::host_control::legacy_inventory(&crate::platform::tool(&install),&controllers)})();
                Ok(Maintenance::Inventory(result))
            })),|_|"spawn");
            match job {Ok(job)=>h.updating=Some(job),Err(error)=>crate::platform::log(&format!("native inventory not started: {error}"))}
        }
        let transition_ready=crate::platform::transition_ready(install).is_ok();
        if !transition_ready&&h.updating.is_none()&&installation_checked.is_none_or(|at|at.elapsed().as_secs()>=5) {
            installation_checked=Some(Instant::now());
            let install=install.to_path_buf();
            let job=coreq::telemetry::capture::operation("host.install.recover.dispatch",||std::thread::Builder::new().name("taskq-install-recovery".into()).spawn(coreq::telemetry::capture::bind(move || {
                coreq::telemetry::capture::operation("host.install.recover",||installation::recover(&install).map(Maintenance::Installation),|_|"install-pending")
            })),|_|"spawn");
            match job {Ok(job)=>h.updating=Some(job),Err(error)=>crate::platform::log(&format!("installation recovery not started: {error}"))}
        }
        let asked=transition_ready&&!h.restart&&h.updating.is_none()&&(fs::remove_file(install.join("update")).is_ok()||roots.iter().any(|r|fs::remove_file(crate::dot(r).join("update")).is_ok()));
        if transition_ready&&!h.restart&&h.updating.is_none()&&(asked || checked.is_none_or(|t| t.elapsed().as_secs() >= timers.0)) {
            checked = Some(Instant::now());
            let install=install.to_path_buf();
            let job=coreq::telemetry::capture::operation("host.update.dispatch",||std::thread::Builder::new().name("taskq-update".into()).spawn(coreq::telemetry::capture::bind(move || {
                coreq::telemetry::capture::operation("host.update",|| {
                    (if crate::installed(&install) {crate::platform::service_update(&install)}else{crate::platform::update(&install,true)}).map(Maintenance::Update)
                },|_|"update-failed")
            })),|_|"spawn");
            match job {Ok(job)=>h.updating=Some(job),Err(error)=>crate::platform::log(&format!("update not started: {error}"))}
        }
        if install.join("restart").exists() || roots.iter().any(|r| crate::dot(r).join("restart").exists()) { h.restart = true; }
        if h.restart {
            for root in &roots {
                if let Err(e) = crate::poke(root) { append_project(install, root, &e); }
            }
            if h.updating.is_some() {
                drop(h);
                if let Err(e)=crate::lifecycle::update_wait(install,&instance) {if reported!=e {crate::platform::log(&e);reported=e;}}
                h=shared.lock().unwrap_or_else(|e|e.into_inner());
            }
            if h.handoff_ready() {
                if h.stopping&&h.stop_controller.is_some() {drop(h);std::thread::sleep(std::time::Duration::from_secs(1));continue;}
                let stopping=h.stopping;
                drop(h);
                if let Err(e)=crate::lifecycle::prepare(install,&instance) {
                    if stopping&&let Ok(Some(p))=crate::lifecycle::pending(install) {let _=crate::lifecycle::failed(install,p.id(),&e);}
                    if reported!=e {crate::platform::log(&e);reported=e;}
                    std::thread::sleep(std::time::Duration::from_secs(1));continue;
                }
                if stopping {
                    match crate::lifecycle::pending(install).and_then(|p|p.filter(|p|p.stops()).ok_or("prepared Stop binding unavailable".into())) {
                        Ok(p)=>match crate::host_control::dispatch(install,&p) {
                        Ok(control)=>{shared.lock().unwrap_or_else(|e|e.into_inner()).stop_controller=Some(control);},
                        Err(e)=>{let _=crate::lifecycle::failed(install,p.id(),&e);crate::platform::log(&e);}
                        },
                        Err(e)=>crate::platform::log(&e),
                    }
                    std::thread::sleep(std::time::Duration::from_secs(1));continue;
                }
                let _ = fs::remove_file(install.join("restart"));
                for root in &roots { let _ = fs::remove_file(crate::dot(root).join("restart")); }
                #[cfg(unix)] {
                    use std::os::unix::process::CommandExt;
                    let mut command = std::process::Command::new(crate::launcher(install));
                    command.args(["service", "run"]).current_dir(install).env_remove("TASKQ_ROOT");
                    coreq::telemetry::capture::before_exec();
                    let e = command.exec();
                    return Err(format!("host re-exec: {e}"));
                }
                #[cfg(not(unix))] std::process::exit(75);
            }
        }
        drop(h);
        if crate::installed(install) {
            crate::keep_tray(install, &mut tray);
            if began.elapsed().as_secs() >= timers.1 && shared.lock().is_ok_and(|h| h.publications_ready(&roots)) {
                if let Err(e) = fs::write(install.join("releases").join(crate::platform::VERSION).join("healthy"), "") {
                    crate::platform::log(&format!("release health unknown: {e}"));
                }
            }
        }
        if hour.0.elapsed().as_secs() >= 3600 {
            let total = crate::requests::totals();
            let usage = crate::requests::diff(&total, &hour.1);
            crate::requests::record("service", &usage);
            for root in &roots { append_project(install, root, &format!("requests 1h: {}", crate::requests::summary(&usage, ""))); }
            let mut h = shared.lock().unwrap_or_else(|e| e.into_inner());
            h.last_hour = Some(usage); h.hour_start = total.clone();
            hour = (Instant::now(), total);
        }
        // Watch <install>/poke every second; the registry is otherwise polled once a minute.
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}

fn append_project(install: &Path, root: &Path, text: &str) {
    use std::io::Write;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    let _ = fs::OpenOptions::new().create(true).append(true).open(install.join(format!("{}.log", crate::project_name(root))))
        .and_then(|mut f| writeln!(f, "{}", crate::platform::log_line(now, text)));
}

fn project_text(install:&Path)->Res<Option<String>> {
    let prepare=format!("{INSTALL_PENDING}.pending");
    for name in ["projects.pending",prepare.as_str()] {
        match fs::symlink_metadata(install.join(name)) {
            Ok(_)=>return Err(format!("{name} retained; registry publication unknown")),
            Err(e) if e.kind()==std::io::ErrorKind::NotFound=>{},
            Err(e)=>return Err(format!("projects prepare unknown: {e}")),
        }
    }
    match fs::symlink_metadata(install.join(INSTALL_PENDING)) {
        Ok(_)=>return installation::registered_projects(install),
        Err(e) if e.kind()==std::io::ErrorKind::NotFound=>{},
        Err(e)=>return Err(format!("installation prepare unknown: {e}")),
    }
    let path=install.join("projects");
    match fs::read_to_string(&path) {
        Ok(text)=>Ok(Some(text)),
        Err(e) if e.kind()==std::io::ErrorKind::NotFound=>Ok(None),
        Err(e)=>Err(format!("{}: {e}",path.display())),
    }
}

/// Keep configured spelling for existing resource IDs; canonical identity owns execution and aliases.
fn project_entries(text:&str)->Res<Vec<(PathBuf,PathBuf)>> {
    text.lines().filter(|line|!line.is_empty()).map(|line| {
        let path=PathBuf::from(line);
        if !path.is_absolute() {return Err("projects unknown: a root is not absolute".into());}
        let canonical=path.canonicalize().map_err(|e|format!("projects unknown: {e}"))?;
        if !canonical.is_dir() {return Err("projects unknown: a root is not a directory".into());}
        Ok((path,canonical))
    }).collect()
}

pub fn project_paths(install:&Path)->Res<Vec<PathBuf>> {
    let text=project_text(install)?.ok_or_else(||format!("{}: project registry missing",install.join("projects").display()))?;
    Ok(project_entries(&text)?.into_iter().map(|(path,_)|path).collect())
}

/// Read only the explicit registry. Canonical identity prevents duplicate threads for aliases.
pub fn projects(install: &Path) -> Res<Vec<PathBuf>> {
    let text=project_text(install)?.ok_or("projects unknown: registry missing")?;
    Ok(project_entries(&text)?.into_iter().map(|(_,canonical)|canonical).collect::<BTreeSet<_>>().into_iter().collect())
}

/// One owner retains the preflight snapshot and native lock until the registry commit or refusal.
pub struct ProjectEdit {
    install:PathBuf, original:Option<String>, next:String, removed:Vec<PathBuf>, adding:bool,
    _lock:coreq::file_lock::Lock,
}
impl ProjectEdit {
    #[tracing::instrument(name="host.projects.prepare",skip_all)]
    pub fn new(install:&Path,root:&Path,adding:bool)->Res<Self> {
        Self::with_guard(install,root,adding,||crate::lifecycle::registry_guard(install))
    }
    pub fn for_uninstall(install:&Path,root:&Path,id:&str)->Res<Self> {
        Self::with_guard(install,root,false,||crate::lifecycle::uninstall_guard(install,id,root))
    }
    fn with_guard(install:&Path,root:&Path,adding:bool,guard:impl FnOnce()->Res<()>)->Res<Self> {
        fs::create_dir_all(install).map_err(|e|e.to_string())?;
        let lock=coreq::file_lock::try_lock(&install.join("projects.lock")).map_err(|e|format!("project registry busy or lock unavailable: {e}"))?;
        guard()?;
        crate::platform::transition_ready(install)?;
        let path=install.join("projects");
        let original=project_text(install)?;
        if original.is_none() && !adding {return Err(format!("{}: project registry missing; nothing changed",path.display()));}
        match fs::symlink_metadata(&path) {
            Ok(meta)=> {
                if !meta.is_file() || meta.permissions().readonly() {return Err("project registry is not a writable regular file; kept".into());}
                ownership(&path)?;
            }
            Err(e) if e.kind()==std::io::ErrorKind::NotFound && original.is_none()=>{},
            Err(e)=>return Err(format!("project registry metadata unknown: {e}")),
        }
        let target=root.canonicalize().map_err(|e|format!("project root unknown: {e}"))?;
        let target_text=target.to_str().ok_or("project root is not UTF-8")?;
        if target_text.contains(['\n','\r']) {return Err("project root cannot be represented by the line registry".into());}
        let entries=project_entries(original.as_deref().unwrap_or(""))?;
        let removed:Vec<PathBuf>=entries.iter().filter(|(_,canonical)|*canonical==target).map(|(path,_)|path.clone()).collect();
        let mut next=original.clone().unwrap_or_default();
        if adding && removed.is_empty() {
            if !next.is_empty()&&!next.ends_with('\n') {next.push('\n');}
            next.push_str(target_text);next.push('\n');
        } else if !adding && !removed.is_empty() {
            next=entries.iter().filter(|(_,canonical)|*canonical!=target).map(|(path,_)|format!("{}\n",path.display())).collect();
        }
        Ok(Self {install:install.into(),original,next,removed:if removed.is_empty(){vec![target]}else{removed},adding,_lock:lock})
    }

    #[tracing::instrument(name="host.projects.commit",skip_all)]
    pub fn commit(self)->Res<()> {
        let path=self.install.join("projects");
        let write=|| {
            if self.original.as_deref()!=Some(self.next.as_str()) {
                match &self.original {
                    Some(original)=>replace_owned(&path,original,&self.next)?,
                    None=>coreq::write_durable(&path,&self.next)?,
                }
            }
            if fs::read_to_string(&path).map_err(|e|e.to_string())?!=self.next {return Err("project registry write unknown: readback differs".into());}
            Ok(())
        };
        if self.adding {crate::resources::locked(&self.install,write)?;}
        else {crate::resources::unregister(&self.install,&self.removed,write)?;}
        crate::platform::write_atomic(&self.install.join("poke"),"").map_err(|e|format!("project registry saved; host notification unknown: {e}"))
    }
}

#[path="host_install.rs"]
mod installation;
pub use installation::install_project;
pub use installation::install_acknowledgement;
pub use installation::installation_status;
pub use installation::{legacy_install,prepare_legacy,legacy_latched,dispatch_legacy,complete_legacy};

fn installation_observed(service:&Value,status:Option<&Value>,version:&str)->bool {
    service["state"]=="running"&&service["fresh"]==true&&service["version"]==version
        &&service["pid"].as_u64().is_some_and(|pid|pid>0)
        &&service["instance"].as_str().is_some_and(crate::resources::id)
        &&status.is_some_and(|status|status["pid"]==service["pid"]&&status["host_instance"]==service["instance"])
}

#[derive(Default)]
struct Registry {
    roots: Vec<PathBuf>,
    poke: Option<std::time::SystemTime>,
    checked: Option<u64>,
    error: Option<String>,
}
impl Registry {
    fn refresh(&mut self, install: &Path, now: u64) {
        let poke = fs::metadata(install.join("poke")).and_then(|m| m.modified()).ok();
        if self.checked.is_some_and(|t| now.saturating_sub(t) < 60) && self.poke == poke { return; }
        self.poke = poke; self.checked = Some(now);
        match projects(install) {
            Ok(roots) => { self.roots = roots; self.error = None; }
            Err(e) => { if self.error.as_ref() != Some(&e) { crate::platform::log(&e); } self.error = Some(e); }
        }
    }
}

/// Compare all copies before any write. The durable plan is a configuration backup, never task state (R1).
pub fn migrate(install: &Path, roots: &[PathBuf]) -> Res<()> {
    crate::resources::try_locked(install,||migrate_locked(install,roots))
}

fn migrate_locked(install: &Path, roots: &[PathBuf]) -> Res<()> {
    let backup = install.join("host-policy-migration");
    let manifest = backup.join("plan.json");
    if backup.join("complete").exists() { Policy::read(install)?; return Ok(()); }
    if !manifest.exists() && install.join("host.json").exists() { Policy::read(install)?; return Ok(()); }
    let plan: Value = if manifest.exists() {
        serde_json::from_slice(&fs::read(&manifest).map_err(|e| e.to_string())?).map_err(|e| format!("migration backup unknown: {e}"))?
    } else {
        if roots.is_empty() { return Err("host.json unknown: no registered policy inputs".into()); }
        let mut files = vec![];
        let mut policies = vec![];
        for root in roots {
            if root.join("taskq.json.host-policy.tmp").exists() { return Err("migration staging path already exists; originals unchanged".into()); }
            let owner = ownership(&root.join("taskq.json")).map_err(|e| format!("migration input unknown: {e}"))?;
            let original = fs::read_to_string(root.join("taskq.json")).map_err(|e| format!("migration input unknown: {e}"))?;
            let mut v: Value = serde_json::from_str(&original).map_err(|e| format!("migration input unknown: {e}"))?;
            let host = std::env::var("TASKQ_HOST").ok().filter(|s| !s.is_empty()).or_else(|| v["hosts"][crate::hostname()].as_str().map(String::from)).unwrap_or_else(crate::hostname);
            let cap = service::capacity(&v, &host)?;
            if !cap.auto { return Err(format!("migration owner choice: {} uses Fixed; originals unchanged", v["repo"].as_str().unwrap_or("project"))); }
            let mut policy = cap.json();
            policy["admission"].as_object_mut().unwrap().remove("mode");
            for (key, default) in [("hosts", json!({})), ("has", json!({})), ("codex", json!(["-s", "danger-full-access"])),
                ("permission_mode", json!("dontAsk")), ("update_every", json!(60)), ("rollback_after", json!(600))] {
                policy[key] = v.get(key).cloned().unwrap_or(default);
            }
            Policy::parse(policy.clone(), &crate::hostname())?;
            policies.push((v["repo"].as_str().unwrap_or("project").to_string(), policy));
            for key in ["capacity_hosts", "admission", "load_gate", "hosts", "has", "codex", "permission_mode", "update_every", "rollback_after"] {
                v.as_object_mut().ok_or("migration input is not an object")?.remove(key);
            }
            // Identical effective policies need no project cap; top-level limits may be overridden locally.
            v.as_object_mut().unwrap().remove("limits");
            files.push(json!({"root": root, "owner": owner, "original": original, "next": serde_json::to_string_pretty(&v).unwrap() + "\n"}));
        }
        let first = &policies[0].1;
        let differences: Vec<_> = policies.iter().filter(|(_, p)| p != first).map(|(repo, p)| {
            let fields: Vec<_> = first.as_object().unwrap().keys().filter(|k| first[*k] != p[*k]).cloned().collect();
            format!("{repo}: {}", fields.join(", "))
        }).collect();
        if !differences.is_empty() { return Err(format!("migration owner choice: differing {}; originals unchanged", differences.join("; "))); }
        let plan = json!({"policy": first, "files": files});
        fs::create_dir_all(&backup).map_err(|e| e.to_string())?;
        for (i, f) in files.iter().enumerate() {
            fs::copy(Path::new(f["root"].as_str().unwrap()).join("taskq.json"), backup.join(format!("{i}.json"))).map_err(|e| e.to_string())?;
        }
        crate::platform::write_atomic(&manifest, &serde_json::to_string_pretty(&plan).unwrap())?;
        plan
    };
    let policy = plan.get("policy").ok_or("migration plan missing policy")?;
    Policy::parse(policy.clone(), &crate::hostname())?;
    let files = plan["files"].as_array().ok_or("migration plan missing files")?;
    let planned: BTreeSet<PathBuf> = files.iter().map(|f| f["root"].as_str().map(PathBuf::from).ok_or("migration plan invalid root")).collect::<Result<_, _>>()?;
    if planned != roots.iter().cloned().collect() || planned.len() != files.len() { return Err("migration registry changed; originals kept, launches held".into()); }
    // Validate every input before resuming any partial replacement; never overwrite a later owner edit.
    for f in files {
        let root = Path::new(f["root"].as_str().ok_or("migration plan invalid root")?);
        let current = fs::read_to_string(root.join("taskq.json")).map_err(|e| e.to_string())?;
        if current != f["original"].as_str().ok_or("migration plan invalid original")? && current != f["next"].as_str().ok_or("migration plan invalid next")? {
            return Err("migration interrupted by changed input; originals kept in backup, launches held".into());
        }
        if ownership(&root.join("taskq.json"))? != f["owner"] { return Err("migration ownership changed; originals kept, launches held".into()); }
    }
    // Stage every replacement with its final metadata before any target changes. Plan ownership makes retry safe.
    let mut staged = vec![];
    for (i, f) in files.iter().enumerate() {
        let path = Path::new(f["root"].as_str().unwrap()).join("taskq.json");
        let stage = path.with_file_name("taskq.json.host-policy.tmp");
        let metadata = fs::metadata(backup.join(format!("{i}.json"))).map_err(|e| e.to_string())?;
        let next = f["next"].as_str().unwrap();
        if fs::read_to_string(&path).map_err(|e| e.to_string())? == next {
            // A completed replacement still verifies/restores metadata after an interrupted prior run.
            fs::set_permissions(&path, metadata.permissions()).map_err(|e| e.to_string())?;
            staged.push(None); continue;
        }
        if stage.exists() { fs::remove_file(&stage).map_err(|e| e.to_string())?; }
        fs::copy(&path, &stage).map_err(|e| e.to_string())?;
        let mut writable = fs::metadata(&stage).map_err(|e| e.to_string())?.permissions();
        #[cfg(windows)] writable.set_readonly(false);
        #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; writable.set_mode(writable.mode() | 0o200); }
        fs::set_permissions(&stage, writable).map_err(|e| e.to_string())?;
        fs::write(&stage, next).map_err(|e| e.to_string())?;
        fs::set_permissions(&stage, metadata.permissions()).map_err(|e| e.to_string())?;
        stage_owner(&stage, &f["owner"])?;
        if ownership(&stage)? != f["owner"] { return Err("migration staging ownership differs; originals unchanged".into()); }
        if fs::read_to_string(&stage).map_err(|e| e.to_string())? != next { return Err("migration staging readback differs".into()); }
        staged.push(Some(stage));
    }
    crate::platform::write_atomic(&install.join("host.json"), &(serde_json::to_string_pretty(policy).unwrap() + "\n"))?;
    for (i, f) in files.iter().enumerate() {
        let path = Path::new(f["root"].as_str().unwrap()).join("taskq.json");
        let next = f["next"].as_str().unwrap();
        if let Some(stage) = &staged[i] {
            replace_project(&path, stage)?;
            // ReplaceFileW merges DACL inheritance flags; restore the recorded descriptor before readback.
            #[cfg(windows)]
            stage_owner(&path, &f["owner"])?;
        }
        if fs::read_to_string(&path).map_err(|e| e.to_string())? != next || ownership(&path)? != f["owner"] { return Err("migration content/ownership readback differs; launches held".into()); }
    }
    if Policy::read(install)?.raw != *policy { return Err("host.json migration readback differs".into()); }
    crate::platform::write_atomic(&backup.join("complete"), "verified\n")?;
    Ok(())
}

#[cfg(unix)]
pub(crate) fn ownership(path: &Path) -> Res<Value> {
    use std::os::unix::fs::MetadataExt;
    let m = fs::metadata(path).map_err(|e| e.to_string())?;
    if m.uid() != unsafe { libc::geteuid() } { return Err("migration needs owner: cannot preserve another user's file ownership".into()); }
    Ok(json!({"uid": m.uid(), "gid": m.gid()}))
}
#[cfg(unix)]
fn stage_owner(path: &Path, owner: &Value) -> Res<()> {
    use std::os::unix::ffi::OsStrExt;
    let p = std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
    let uid = owner["uid"].as_u64().and_then(|n| u32::try_from(n).ok()).ok_or("migration owner unknown")?;
    let gid = owner["gid"].as_u64().and_then(|n| u32::try_from(n).ok()).ok_or("migration group unknown")?;
    if unsafe { libc::chown(p.as_ptr(), uid, gid) } != 0 { return Err(format!("migration cannot preserve ownership: {}", std::io::Error::last_os_error())); }
    Ok(())
}
#[cfg(unix)]
fn replace_project(path: &Path, stage: &Path) -> Res<()> { fs::rename(stage, path).map_err(|e| e.to_string()) }

#[cfg(windows)]
pub(crate) fn ownership(path: &Path) -> Res<Value> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "advapi32")]
    unsafe extern "system" { fn GetFileSecurityW(path: *const u16, info: u32, descriptor: *mut u8, size: u32, needed: *mut u32) -> i32; }
    let path: Vec<_> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut size = 0;
    unsafe { GetFileSecurityW(path.as_ptr(), 7, std::ptr::null_mut(), 0, &mut size); }
    if size == 0 { return Err(format!("migration ownership unknown: {}", std::io::Error::last_os_error())); }
    let mut descriptor = vec![0; size as usize];
    if unsafe { GetFileSecurityW(path.as_ptr(), 7, descriptor.as_mut_ptr(), size, &mut size) } == 0 { return Err(format!("migration ownership unknown: {}", std::io::Error::last_os_error())); }
    // SetFileSecurityW clears AUTO_INHERITED bookkeeping even with byte-identical owner, group and DACL.
    *descriptor.get_mut(3).ok_or("migration ownership descriptor truncated")? &= !0x04;
    Ok(json!(descriptor))
}
#[cfg(windows)]
fn stage_owner(path: &Path, owner: &Value) -> Res<()> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "advapi32")]
    unsafe extern "system" { fn SetFileSecurityW(path: *const u16, info: u32, descriptor: *const u8) -> i32; }
    let descriptor: Vec<u8> = owner.as_array().ok_or("migration ownership unknown")?.iter()
        .map(|v| v.as_u64().and_then(|n| u8::try_from(n).ok()).ok_or("migration ownership corrupt")).collect::<Result<_, _>>()?;
    let control = descriptor.get(2..4).ok_or("migration ownership corrupt")?;
    let protected = u16::from_le_bytes([control[0], control[1]]) & 0x1000 != 0;
    let path: Vec<_> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    if unsafe { SetFileSecurityW(path.as_ptr(), 7 | if protected { 0x80000000 } else { 0x20000000 }, descriptor.as_ptr()) } == 0 {
        return Err(format!("migration cannot preserve ownership: {}", std::io::Error::last_os_error()));
    }
    Ok(())
}
#[cfg(windows)]
fn replace_project(path: &Path, stage: &Path) -> Res<()> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    unsafe extern "system" { fn ReplaceFileW(path: *const u16, stage: *const u16, backup: *const u16, flags: u32, exclude: *mut std::ffi::c_void, reserved: *mut std::ffi::c_void) -> i32; }
    let wide = |p: &Path| p.as_os_str().encode_wide().chain(Some(0)).collect::<Vec<_>>();
    if unsafe { ReplaceFileW(wide(path).as_ptr(), wide(stage).as_ptr(), std::ptr::null(), 0, std::ptr::null_mut(), std::ptr::null_mut()) } == 0 { return Err(format!("migration replace: {}", std::io::Error::last_os_error())); }
    Ok(())
}

/// Per-project restart/quarantine clock. Children stay outside the unwind boundary in the thread's Service.
#[derive(Default)]
pub struct Panics { times: Vec<u64>, pub until: u64, pub reason: Option<String> }
impl Panics {
    pub fn quarantined(&self) -> bool { self.times.len() >= 3 }
    pub fn record(&mut self, now: u64, msg: &str) {
        self.times.retain(|t| now.saturating_sub(*t) <= 600);
        self.times.push(now);
        let class = crate::incident::panic_class(msg);
        let delay = [1, 10, 60][self.times.len().min(3) - 1];
        self.until = now + delay;
        self.reason = Some(if self.quarantined() { format!("quarantined: {class}") } else { format!("{class}; restarting in {delay} s") });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // H-A5: a busy maintenance owner cannot prolong an old native absence observation.
    #[test]
    fn stale_native_inventory_holds_new_work_without_releasing_owned_slots() {
        let p=policy();let mut h=Host::new(Ok(p.clone()));
        let root=Path::new("owned");h.sync(root,vec![(1,7,"codex".into())]);
        let (send,receive)=std::sync::mpsc::channel();
        h.updating=Some(std::thread::spawn(move || {receive.recv().map_err(|e|e.to_string())?;Ok(Maintenance::Update(None))}));
        let inventory=Ok(vec![]);
        let checked=Instant::now()-std::time::Duration::from_secs(61);
        h.policy=native_inventory_ready(&inventory,Some(checked)).map(|_|p.clone());
        assert!(h.reserve(root,2,8,"codex",&p.cap,true,0).unwrap_err().contains("stale"));
        assert_eq!(h.used()["codex"],1,"missing freshness cannot release owned work");
        assert!(!h.updating.as_ref().unwrap().is_finished());
        send.send(()).unwrap();h.updating.take().unwrap().join().unwrap().unwrap();
        assert!(native_inventory_ready(&inventory,None).is_err());
        assert!(native_inventory_ready(&Err("native read denied".into()),Some(Instant::now())).unwrap_err().contains("denied"));
        assert!(native_inventory_ready(&Ok(vec!["retained-unit".into()]),Some(Instant::now())).unwrap_err().contains("retained-unit"));
        let retained=Ok(vec!["retained-unit".into()]);
        let retry=Some(Instant::now()-std::time::Duration::from_secs(6));
        assert!(native_inventory_due(&retained,retry),"retired legacy units need fresh native proof before lifecycle can proceed");
        assert!(!native_inventory_due(&inventory,retry),"fresh empty inventory keeps the ordinary cadence");
        assert!(native_inventory_ready(&retained,retry).is_err(),"a refresh being due cannot release the native barrier");
        h.policy=native_inventory_ready(&inventory,Some(Instant::now())).map(|_|p.clone());
        assert!(h.reserve(root,2,8,"codex",&p.cap,true,0).is_ok());
    }

    // H-A4/H-A5: first launch waits for every project's occupancy, including unknown native ownership.
    #[test]
    fn cold_host_accounts_every_project_before_admission() {
        let p=policy();let mut h=Host::new(Ok(p.clone()));h.gate.configure(p.cap.gate);
        let a=PathBuf::from("a");let b=PathBuf::from("b");h.busy.insert(a.clone(),true);h.busy.insert(b.clone(),true);h.rebuilding=true;
        assert!(h.reserve(&a,1,0,"codex",&p.cap,true,0).is_err());
        assert!(!h.rebuild_slots(&a,vec![(1,7,"codex".into())]));
        assert_eq!(h.remaining(&a,"codex",&p.cap),0);assert!(h.reserve(&a,2,0,"codex",&p.cap,true,0).is_err());
        assert!(h.rebuild_slots(&b,vec![(2,8,"codex".into())]));assert!(!h.rebuilding);
        assert_eq!(h.used()["codex"],2);assert!(h.reserve(&a,3,0,"codex",&p.cap,true,0).is_err());
        h.sync(&b,vec![]);assert!(h.reserve(&a,3,0,"codex",&p.cap,true,0).is_ok());

        h.sync(&a,vec![(1,7,"codex".into())]);
        let c=PathBuf::from("later");h.attach(&c);
        assert_eq!(h.reserve(&a,4,0,"codex",&p.cap,true,30).unwrap_err(),"host occupancy recovery pending","registration may carry unresolved prior occupancy");
        assert!(!h.rebuild_slots(&a,vec![(1,7,"codex".into())]));
        assert!(h.rebuild_slots(&c,vec![(4,9,"codex".into())]));
        assert_eq!(h.used()["codex"],2);
        h.sync(&c,vec![]);h.busy.remove(&c);h.attach(&c);
        assert!(!h.rebuild_slots(&a,vec![(1,7,"codex".into())]),"re-registration cannot reuse the earlier root's recovery acknowledgment");
        assert!(h.rebuild_slots(&c,vec![]));assert!(!h.rebuilding);
    }

    // H-A5: the final handover barrier covers orphaned slots and closes before re-entry.
    #[test]
    fn handover_preparation_latches_only_after_every_owned_project() {
        let a=PathBuf::from("a");let b=PathBuf::from("b");
        let mut h=Host::new(Err("fixture".into()));
        assert!(h.begin_pass(&a));h.restart=true;h.prepared.insert(a.clone());
        h.sync(&b,vec![(1,7,"codex".into())]);
        assert!(!h.handoff_ready(),"an unrepresented slot is not an idle project");
        assert!(h.begin_pass(&a));assert!(!h.prepared.contains(&a));
        h.prepared.insert(a.clone());assert!(h.begin_pass(&b));
        h.prepared.insert(b.clone());
        h.held=Some("registry unknown".into());assert!(!h.handoff_ready());
        h.held=None;assert!(h.handoff_ready());
        for _ in 0..3 {
            assert!(!h.begin_pass(&a));assert!(!h.begin_pass(&b));assert!(h.handoff_ready());
        }
        h.prepared.remove(&b);assert!(!h.handoff_ready(),"failed ownership cannot keep an old acknowledgment");
        assert!(h.begin_pass(&a));assert!(!h.prepared.contains(&a));
        h.prepared.insert(a.clone());h.prepared.insert(b.clone());
        let (send,receive)=std::sync::mpsc::channel();
        h.updating=Some(std::thread::spawn(move || {receive.recv().map_err(|e|e.to_string())?;Ok(Maintenance::Update(None))}));
        assert!(!h.handoff_ready(),"an update owner must finish before image replacement");
        assert!(h.begin_pass(&a),"projects keep heartbeat/native monitoring while the update waits");
        h.prepared.insert(a);send.send(()).unwrap();h.updating.take().unwrap().join().unwrap().unwrap();assert!(h.handoff_ready());
        h.stopping=true;
        assert!(!h.handoff_ready(),"Stop cannot reuse a CLI handover acknowledgment while a native slot remains");
        h.sync(&b,vec![]);assert!(!h.handoff_ready(),"Stop still waits for active project/operator ownership");
        for busy in h.busy.values_mut() {*busy=false;}
        assert!(h.handoff_ready(),"only the prepared host with no occupied work may stop");

    }
    fn temp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("taskq-host-{name}-{}", std::process::id()));
        fs::create_dir_all(&p).unwrap(); p
    }
    fn policy() -> Policy {
        Policy::parse(json!({"limits": {"codex": 2}, "admission": {"max": 2}, "load_gate": {"enabled": false}}), "invented").unwrap()
    }

    // H-A5: native owner + exact host generation acknowledge install; project PID or freshness alone cannot.
    #[test]
    fn installation_observed_requires_current_native_owner_and_project_generation() {
        let service=json!({"state":"running","fresh":true,"version":crate::platform::VERSION,"pid":123,"instance":"owned-host"});
        let project=json!({"pid":123,"host_instance":"owned-host"});
        assert!(installation_observed(&service,Some(&project),crate::platform::VERSION));
        for (key,value) in [("state",json!("unknown")),("fresh",json!(false)),("version",json!("older")),("pid",json!(0)),("instance",json!(""))] {
            let mut other=service.clone();other[key]=value;assert!(!installation_observed(&other,Some(&project),crate::platform::VERSION),"{key}");
        }
        for other in [None,Some(json!({"pid":124,"host_instance":"owned-host"})),Some(json!({"pid":123,"host_instance":"prior-host"}))] {
            assert!(!installation_observed(&service,other.as_ref(),crate::platform::VERSION));
        }
    }

    // H-A5: refusal precedes native installation, focus changes, release replacement and registry publication.
    #[test]
    fn host_install_refuses_unresolved_legacy_owner_without_effects() {
        let base=temp("install-legacy-owner");let install=base.join("install");let root=base.join("project");
        fs::create_dir_all(&install).unwrap();fs::create_dir_all(crate::dot(&root)).unwrap();
        fs::write(install.join("projects"),format!("{}\n",root.display())).unwrap();
        fs::write(crate::dot(&root).join("focus"),"retained-focus").unwrap();
        let mut child=std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact","runtime::tests::stop_child","--ignored"])
            .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().unwrap();
        fs::write(crate::dot(&root).join("status.json"),json!({"pid":child.id(),"kids":[{"task":7}]}).to_string()).unwrap();
        let before=fs::read(install.join("projects")).unwrap();
        let result=install_project(&install,&root);
        child.kill().unwrap();child.wait().unwrap();
        assert!(result.unwrap_err().contains("legacy project owner"));
        assert_eq!(fs::read(install.join("projects")).unwrap(),before);
        assert_eq!(fs::read_to_string(crate::dot(&root).join("focus")).unwrap(),"retained-focus");
        assert!(!install.join("releases").exists()&&!install.join("poke").exists());
        fs::write(install.join("service"),"{broken").unwrap();
        assert!(install_project(&install,&root).unwrap_err().contains("malformed"));
        assert_eq!(fs::read(install.join("projects")).unwrap(),before);
        fs::remove_dir_all(base).unwrap();
    }

    // H-A5: one editor preserves unrelated roots/permissions, rejects interrupted state and removes canonical aliases.
    #[test]
    fn registry_editor_preserves_roots_and_refuses_unknown() {
        let base=temp("registry-editor");let install=base.join("install");let a=base.join("a");let b=base.join("b");
        for path in [&install,&a,&b] {fs::create_dir_all(path).unwrap();}
        assert!(ProjectEdit::new(&install,&a,false).is_err(),"only registration initializes a missing registry");
        ProjectEdit::new(&install,&a,true).unwrap().commit().unwrap();
        let path=install.join("projects");
        let before=fs::read_to_string(&path).unwrap();
        assert_eq!(before,format!("{}\n",a.canonicalize().unwrap().display()));
        assert!(install.join("poke").exists());
        #[cfg(unix)] {use std::os::unix::fs::PermissionsExt;fs::set_permissions(&path,fs::Permissions::from_mode(0o640)).unwrap();}
        let owner=ownership(&path).unwrap();
        ProjectEdit::new(&install,&b,true).unwrap().commit().unwrap();
        assert_eq!(ownership(&path).unwrap(),owner);
        #[cfg(unix)] {use std::os::unix::fs::PermissionsExt;assert_eq!(fs::metadata(&path).unwrap().permissions().mode()&0o777,0o640);}
        let saved=fs::read(&path).unwrap();
        let mut registry=Registry::default();registry.refresh(&install,0);
        let roots=registry.roots.clone();
        fs::write(install.join("projects.pending"),"interrupted").unwrap();
        registry.refresh(&install,60);
        assert!(registry.error.is_some() && registry.roots==roots);
        assert!(ProjectEdit::new(&install,&a,false).is_err());
        assert_eq!(fs::read(&path).unwrap(),saved);
        fs::remove_file(install.join("projects.pending")).unwrap();
        let alias=a.join(".");
        fs::write(&path,format!("{}{}\n{}\n",String::from_utf8(saved).unwrap(),alias.display(),b.display())).unwrap();
        ProjectEdit::new(&install,&a,false).unwrap().commit().unwrap();
        assert_eq!(projects(&install).unwrap(),vec![b.canonicalize().unwrap()]);
        assert_eq!(project_paths(&install).unwrap().len(),2,"unrelated configured spelling is retained");
        fs::write(&path,b"\xff").unwrap();
        assert!(ProjectEdit::new(&install,&a,true).is_err());
        assert_eq!(fs::read(&path).unwrap(),b"\xff");
        fs::remove_dir_all(base).unwrap();
    }

    // H-A1: an owner before PID publication excludes a second process, survives sweep, and releases on exit.
    #[test]
    fn one_host_owner_before_heartbeat_publication() {
        let base=temp("host-owner");let install=base.join("install");fs::create_dir(&install).unwrap();
        let (first,second)=(base.join("first"),base.join("second"));
        let mut holder=std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact","host::tests::owner_probe","--ignored"])
            .env("TASKQ_OWNER_INSTALL",&install).env("TASKQ_OWNER_MARKER",&first).env("TASKQ_OWNER_HOLD","1")
            .stdout(std::process::Stdio::null()).stderr(fs::File::create(base.join("owner.stderr")).unwrap()).spawn().unwrap();
        let until=Instant::now()+std::time::Duration::from_secs(5);
        while !first.is_file() && holder.try_wait().unwrap().is_none() && Instant::now()<until {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let entered=fs::read_to_string(&first).unwrap_or_default();
        crate::platform::sweep(&install,&base.join("absent-shim"));
        let probe=coreq::process::output("test.host-owner",std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact","host::tests::owner_probe","--ignored"])
            .env("TASKQ_OWNER_INSTALL",&install).env("TASKQ_OWNER_MARKER",&second).env_remove("TASKQ_OWNER_HOLD"),
            std::time::Duration::from_secs(5));
        if holder.try_wait().unwrap().is_none() {holder.kill().unwrap();holder.wait().unwrap();}
        let observed=fs::read_to_string(&second).unwrap_or_default();
        let released=owner(&install).unwrap();let freed=released.is_some();
        drop(released);
        let diagnostic=fs::read_to_string(base.join("owner.stderr")).unwrap_or_default();
        fs::remove_dir_all(base).unwrap();
        assert_eq!(entered,"claimed","first owner never reached the controlled startup boundary: {diagnostic}");
        probe.unwrap();
        assert_eq!(observed,"blocked","two processes claimed the host before its PID was published");
        assert!(freed,"native owner exit did not release ownership");
    }

    // H-A1/H-A5: exact native incarnation, launch generation and observation age have independent meanings.
    #[test]
    fn host_owner_projection_separates_identity_generation_and_freshness() {
        let install=temp("owner-observation");
        let marker=install.join("published");
        let mut child=std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact","host::tests::owner_probe","--ignored"])
            .env("TASKQ_OWNER_INSTALL",&install).env("TASKQ_OWNER_MARKER",&marker).env("TASKQ_OWNER_PUBLISH","first")
            .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().unwrap();
        let until=Instant::now()+std::time::Duration::from_secs(5);
        while !marker.exists()&&Instant::now()<until&&child.try_wait().unwrap().is_none() {std::thread::yield_now();}
        assert_eq!(fs::read_to_string(marker).unwrap(),"claimed");
        let native=coreq::process::identity(child.id()).unwrap().unwrap();
        let status=service_status(&install);
        assert_eq!(status["state"],"running");assert_eq!(status["fresh"],true);assert_eq!(status["instance"],"first");
        assert!(service_line(&status).contains("running (host, pid"));
        assert_eq!(other(&install).unwrap(),Some(child.id()));
        let original=fs::read_to_string(install.join("service")).unwrap();
        let mut record:Value=serde_json::from_str(&original).unwrap();
        for at in [wall().unwrap()-120,wall().unwrap()+120] {
            record["at"]=json!(at);fs::write(install.join("service"),record.to_string()).unwrap();
            let stale=service_status(&install);assert_eq!(stale["state"],"running");assert_eq!(stale["fresh"],false);
            assert!(service_line(&stale).contains("observation stale or unknown"));
            assert_eq!(other(&install).unwrap(),Some(child.id()),"stale/future projection released a live owner");
        }
        publish_service(&install,&native,"second").unwrap();
        assert_eq!(service_status(&install)["instance"],"second","same native PID may have a new host launch");
        record=serde_json::from_str(&original).unwrap();record["started"]=json!(format!("{}-older",native.started));
        fs::write(install.join("service"),record.to_string()).unwrap();
        assert_eq!(service_status(&install)["state"],"exited","another incarnation must not look like the recorded daemon");
        assert_eq!(other(&install).unwrap(),None);assert!(coreq::process::identity(child.id()).unwrap().is_some());
        assert!(owner(&install).unwrap().is_none(),"record mismatch bypassed the native ownership lock");
        record["schema"]=json!(2);
        for text in [record.to_string(),"{".into(),"x".repeat(1025)] {
            fs::write(install.join("service"),&text).unwrap();
            assert_eq!(service_status(&install)["state"],"unknown");assert!(other(&install).is_err());
            assert_eq!(fs::read_to_string(install.join("service")).unwrap(),text);
        }
        fs::write(install.join("service"),child.id().to_string()).unwrap();
        assert_eq!(service_status(&install)["state"],"unknown","PID-only compatibility cannot certify a native host owner");
        assert_eq!(other(&install).unwrap(),Some(child.id()));
        publish_service(&install,&native,"second").unwrap();
        child.kill().unwrap();child.wait().unwrap();
        assert_eq!(service_status(&install)["state"],"exited");assert_eq!(other(&install).unwrap(),None);
        fs::remove_dir_all(install).unwrap();
    }

    #[test]
    #[ignore="owned native process for one_host_owner_before_heartbeat_publication"]
    fn owner_probe() {
        let install=PathBuf::from(std::env::var_os("TASKQ_OWNER_INSTALL").unwrap());
        let claim=owner(&install).unwrap();
        let marker=PathBuf::from(std::env::var_os("TASKQ_OWNER_MARKER").unwrap());
        if claim.is_some()&&let Ok(instance)=std::env::var("TASKQ_OWNER_PUBLISH") {
            use std::io::Read;
            let native=coreq::process::identity(std::process::id()).unwrap().unwrap();
            publish_service(&install,&native,&instance).unwrap();
            crate::platform::write_atomic(&marker,"claimed").unwrap();
            let _=std::io::stdin().read(&mut [0]); // Parent pipe closure also retires a failed fixture.
            return;
        }
        crate::platform::write_atomic(&marker,if claim.is_some(){"claimed"}else{"blocked"}).unwrap();
        if std::env::var_os("TASKQ_OWNER_HOLD").is_some() && claim.is_some() {loop {std::thread::park();}}
    }

    // H-A3: committed policy and its original command survive writer death, even before the applied receipt.
    #[test]
    fn acknowledged_policy_survives_writer_termination() {
        for phase in ["committed","before-applied"] {
        let root=temp(&format!("policy-commit-{phase}"));
        fs::write(root.join("host.json"),json!({"limits":{"codex":2},"owner_field":"preserve"}).to_string()).unwrap();
        let mut child=std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact","host::tests::policy_writer","--ignored"])
            .env("TASKQ_HOST_POLICY_FIXTURE",&root).env("TASKQ_HOST_POLICY_PHASE",phase)
            .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().unwrap();
        let until=Instant::now()+std::time::Duration::from_secs(10);
        while !root.join("ack").is_file() && child.try_wait().unwrap().is_none() && Instant::now()<until {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let acknowledged=fs::read_to_string(root.join("ack")).unwrap_or_default();
        if child.try_wait().unwrap().is_none() {child.kill().unwrap();child.wait().unwrap();}
        assert_eq!(acknowledged,if phase=="committed" {"committed"} else {"policy-saved"},"writer did not reach {phase}");
        let read=Policy::read(&root).unwrap();assert_eq!(read.cap.limits["codex"],3);assert_eq!(read.raw["owner_field"],"preserve");
        let receipt=read.command().unwrap().unwrap();
        let before=fs::read(root.join("host.json")).unwrap();
        let (capture,guard)=crate::profile::tests::collecting(&root);
        assert!(!capacity_observed(&root,&receipt).unwrap());
        assert_eq!(observe_capacity(&root,&read).unwrap(),Some(receipt.event_id("observed").unwrap()));
        assert!(capacity_observed(&root,&receipt).unwrap());
        assert_eq!(fs::read(root.join("host.json")).unwrap(),before,"recovery replayed a settings mutation");
        capture.flush().unwrap();drop(guard);drop(capture);fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    #[ignore="owned subprocess used by acknowledged_policy_survives_writer_termination"]
    fn policy_writer() {
        let root=PathBuf::from(std::env::var_os("TASKQ_HOST_POLICY_FIXTURE").unwrap());
        let (_capture,_guard)=crate::profile::tests::collecting(&root);
        let owner=Policy::read(&root).unwrap().me;
        if let Ok(request)=std::env::var("TASKQ_HOST_POLICY_REQUEST") {
            local_capacity(&root,&owner,&serde_json::from_str(&request).unwrap()).unwrap();
        } else {write_capacity(&root,&owner,&crate::CapEdit {limits:BTreeMap::from([("codex".into(),3)]),..Default::default()},None).unwrap();}
        crate::platform::write_atomic(&root.join("ack"),"committed").unwrap();
        loop {std::thread::park();}
    }

    fn local_fixture(name:&str)->(PathBuf,PathBuf,String,Value) {
        let base=temp(name);let install=base.join("install");let root=base.join("project");
        fs::create_dir_all(&install).unwrap();fs::create_dir_all(&root).unwrap();
        fs::write(install.join("host.json"),"{\"limits\":{\"codex\":2}}").unwrap();
        fs::write(root.join("taskq.json"),"{\"repo\":\"fixture/project\"}").unwrap();
        fs::write(install.join("projects"),format!("{}\n",root.display())).unwrap();
        let p=Policy::read(&install).unwrap();let now=wall().unwrap();
        let request=json!({"id":"local-capacity","issued_at":now,"expires_at":now+300,"ops":[{
            "target":{"installation_id":null,"generation":1,"project_id":null,"runtime_id":"codex"},
            "policy_schema":"taskq.host-capacity/1","expected_revision":p.revision().unwrap(),"changes":{"runtime_limit":3}}]});
        (base,install,p.me,request)
    }

    // H-A2: immutable same-ID replay, current revision and typed local scope hold before policy effects.
    #[test]
    fn local_capacity_request_replays_without_mutation_and_refuses_foreign_scope() {
        let (base,install,owner,request)=local_fixture("local-request");
        let (capture,guard)=crate::profile::tests::collecting(&install);
        let saved=local_capacity(&install,&owner,&request).unwrap();assert_eq!(saved["state"],"saved");
        let bytes=fs::read(install.join("host.json")).unwrap();
        assert_eq!(local_capacity(&install,&owner,&request).unwrap(),saved);
        for change in ["payload","stale","expired","installation","generation","project","runtime","schema","type","future","lifetime","empty","extra","large"] {
            let mut other=request.clone();
            if change!="payload" {other["id"]=json!(change);}
            match change {
                "payload"=>other["ops"][0]["changes"]["runtime_limit"]=json!(4),
                "expired"=>{let now=wall().unwrap();other["issued_at"]=json!(now-301);other["expires_at"]=json!(now-1);}
                "installation"=>other["ops"][0]["target"]["installation_id"]=json!("a".repeat(64)),
                "generation"=>other["ops"][0]["target"]["generation"]=json!(2),
                "project"=>other["ops"][0]["target"]["project_id"]=json!("project"),
                "runtime"=>other["ops"][0]["target"]["runtime_id"]=json!("unknown"),
                "schema"=>other["ops"][0]["policy_schema"]=json!("taskq.capacity/1"),
                "type"=>other["ops"][0]["changes"]["runtime_limit"]=json!(true),
                "future"=>{let now=wall().unwrap();other["issued_at"]=json!(now+60);other["expires_at"]=json!(now+300);}
                "lifetime"=>other["expires_at"]=json!(request["issued_at"].as_u64().unwrap()+301),
                "empty"=>other["ops"]=json!([]),
                "extra"=>other["set"]=json!({"codex":4}),
                "large"=>other["ops"][0]["changes"]["runtime_limit"]=json!("x".repeat(16<<10)),
                _=>{},
            }
            let result=local_capacity(&install,&owner,&other);
            assert!(result.is_err()||result.unwrap()["state"]=="rejected","{change} accepted");
            assert_eq!(fs::read(install.join("host.json")).unwrap(),bytes,"{change} changed policy");
        }
        let current=Policy::read(&install).unwrap();observe_capacity(&install,&current).unwrap();
        assert_eq!(local_capacity(&install,&owner,&request).unwrap()["saved_revision"],current.revision().unwrap());
        let mut newer=request.clone();newer["id"]=json!("newer");newer["ops"][0]["expected_revision"]=json!(current.revision().unwrap());
        let next=local_capacity(&install,&owner,&newer).unwrap();assert_eq!(next["state"],"saved");assert_ne!(next["saved_revision"],saved["saved_revision"]);
        let bytes=fs::read(install.join("host.json")).unwrap();
        assert_eq!(local_capacity(&install,&owner,&request).unwrap(),saved,"a later command must not change the original receipt");
        assert_eq!(fs::read(install.join("host.json")).unwrap(),bytes);
        crate::resources::with(&install,|state| {
            let op=state.ops["local-capacity"].clone();
            for n in state.ops.len()..crate::resources::MAX_OPS {state.ops.insert(format!("retained-{n}"),op.clone());}
            Ok(())
        }).unwrap();
        newer["id"]=json!("store-full");newer["ops"][0]["expected_revision"]=json!(Policy::read(&install).unwrap().revision().unwrap());
        assert_eq!(local_capacity(&install,&owner,&newer).unwrap()["state"],"rejected");
        assert_eq!(fs::read(install.join("host.json")).unwrap(),bytes);
        fs::write(install.join("resources/state.json"),"{").unwrap();
        assert!(local_capacity(&install,&owner,&newer).unwrap_err().contains("file kept"));
        assert_eq!(fs::read_to_string(install.join("resources/state.json")).unwrap(),"{");
        assert_eq!(fs::read(install.join("host.json")).unwrap(),bytes);
        capture.flush().unwrap();drop(guard);drop(capture);fs::remove_dir_all(base).unwrap();
    }

    // H-A2/H-A3: crash before effect remains unknown; crash after replacement recovers the same receipt read-only.
    #[test]
    fn local_capacity_crashes_never_replay_an_uncertain_write() {
        for phase in ["before-write","before-applied"] {
            let (base,install,owner,request)=local_fixture(&format!("local-crash-{phase}"));
            let mut child=std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact","host::tests::policy_writer","--ignored"]).env("TASKQ_HOST_POLICY_FIXTURE",&install)
                .env("TASKQ_HOST_POLICY_REQUEST",request.to_string()).env("TASKQ_HOST_POLICY_PHASE",phase)
                .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().unwrap();
            let until=Instant::now()+std::time::Duration::from_secs(10);
            while !install.join("ack").exists() && child.try_wait().unwrap().is_none() && Instant::now()<until {std::thread::sleep(std::time::Duration::from_millis(5));}
            let signal=fs::read_to_string(install.join("ack")).unwrap_or_default();let _=child.kill();child.wait().unwrap();
            assert_eq!(signal,if phase=="before-write" {"intent-saved"}else{"policy-saved"});
            let bytes=fs::read(install.join("host.json")).unwrap();
            let (capture,guard)=crate::profile::tests::collecting(&install);
            let result=local_capacity(&install,&owner,&request).unwrap();
            assert_eq!(result["state"],if phase=="before-write" {"unknown"}else{"saved"},"{phase}: {result}");
            assert_eq!(fs::read(install.join("host.json")).unwrap(),bytes,"recovery rewrote policy at {phase}");
            if phase=="before-applied" {
                let p=Policy::read(&install).unwrap();assert_eq!(observe_capacity(&install,&p).unwrap(),result["saved_revision"].as_str().map(String::from));
            }
            capture.flush().unwrap();drop(guard);drop(capture);fs::remove_dir_all(base).unwrap();
        }
    }

    // H-A2: identical settings have different command identities; stale/manual readback cannot confirm a new edit.
    #[test]
    fn capacity_receipts_follow_the_loaded_command_not_equal_values() {
        let root=temp("capacity-receipt");
        fs::write(root.join("host.json"),"{\"limits\":{\"codex\":2}}").unwrap();
        let owner=Policy::read(&root).unwrap().me;
        let edit=crate::CapEdit {limits:BTreeMap::from([("codex".into(),3)]),gate:Some(false),..Default::default()};
        assert!(write_capacity(&root,&owner,&edit,None).is_err(),"missing required receipts must refuse before effect");
        assert_eq!(Policy::read(&root).unwrap().cap.limits["codex"],2);
        let (capture,guard)=crate::profile::tests::collecting(&root);
        let (_,_,first)=write_capacity(&root,&owner,&edit,None).unwrap();
        let loaded=Policy::read(&root).unwrap();
        assert!(!capacity_observed(&root,&first).unwrap());
        let (_,_,second)=write_capacity(&root,&owner,&edit,None).unwrap();
        let current=Policy::read(&root).unwrap();
        assert_eq!(loaded.cap,current.cap);assert_ne!(first.encode(),second.encode());
        assert!(observe_capacity(&root,&loaded).is_err(),"late old loader confirmed a different command");
        assert!(!capacity_observed(&root,&second).unwrap());
        assert_eq!(observe_capacity(&root,&current).unwrap(),Some(second.event_id("observed").unwrap()));
        assert!(capacity_observed(&root,&second).unwrap());
        assert!(capacity_observed(&root,&first).is_err());
        let mut edited=current.raw.clone();edited["limits"]["codex"]=json!(4);
        fs::write(root.join("host.json"),edited.to_string()).unwrap();
        assert!(observe_capacity(&root,&Policy::read(&root).unwrap()).is_err(),"manual edit borrowed old receipt");
        for invalid in [json!({"schema":2}),json!({"schema":1,"host":owner,"policy":current.cap.json(),"receipt":"{}"})] {
            edited[CAPACITY_COMMAND]=invalid;
            let bytes=edited.to_string();fs::write(root.join("host.json"),&bytes).unwrap();
            assert!(Policy::read(&root).is_err());assert!(write_capacity(&root,&owner,&edit,None).is_err());
            assert_eq!(fs::read_to_string(root.join("host.json")).unwrap(),bytes);
        }
        capture.flush().unwrap();drop(guard);drop(capture);fs::remove_dir_all(root).unwrap();
    }

    // Chain: published project passes -> host release qualification; failure keeps owned work and recovery can qualify.
    #[test]
    fn release_health_requires_current_successful_publications() {
        let root=std::env::temp_dir().join(format!("taskq-publication-{}",std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let tool=crate::platform::tool(&root.join("install"));
        let host=Arc::new(Mutex::new(Host::new(Ok(policy()))));
        let roots=vec![root.clone()];
        host.lock().unwrap().sync(&root,vec![(1,99,"codex".into())]);
        let mut status=coreq::status::common(&tool,"fixture","project",1,None,None);
        status["complete"]=json!(true);status["tasks"]=json!([]);
        assert!(!host.lock().unwrap().publications_ready(&roots));
        assert!(crate::publish_status(&tool,&root,&status,Some(&host)));
        assert!(host.lock().unwrap().publications_ready(&roots));
        let path=coreq::status::path(&tool,&root);
        assert_eq!(crate::data::json(&path).unwrap().unwrap()["host_instance"],host.lock().unwrap().instance);
        fs::remove_file(&path).unwrap();fs::create_dir(&path).unwrap();
        assert!(!crate::publish_status(&tool,&root,&status,Some(&host)));
        assert!(!host.lock().unwrap().publications_ready(&roots));
        assert_eq!(host.lock().unwrap().slots.len(),1,"publication must not lose child ownership");
        fs::remove_dir(&path).unwrap();
        assert!(crate::publish_status(&tool,&root,&status,Some(&host)));
        for failed in [json!({"complete":false,"tasks":[]}),json!({"complete":true,"tasks":[],"error":"board unavailable"}),
            json!({"complete":true,"tasks":[],"focus_error":"focus command pending"})] {
            assert!(!crate::publish_status(&tool,&root,&failed,Some(&host)));
            assert_eq!(crate::data::json(&path).unwrap().unwrap()["host_instance"],host.lock().unwrap().instance,"failed status lost its owner generation");
            assert!(!host.lock().unwrap().publications_ready(&roots));
        }
        assert!(crate::publish_status(&tool,&root,&status,Some(&host)));
        let mut h=host.lock().unwrap();
        h.published.insert(root.clone(),Instant::now()-std::time::Duration::from_secs(61));
        assert!(!h.publications_ready(&roots),"old evidence cannot qualify a blocked thread");
        h.publication(&root,true);
        assert!(!h.publications_ready(&[root.clone(),root.join("new-project")]));
        h.held=Some("registry unreadable".into());assert!(!h.publications_ready(&roots));
        h.held=None;assert!(h.publications_ready(&roots));
        h.publication(&root,false);assert!(!h.publications_ready(&roots),"thread exit invalidates evidence");
        drop(h);fs::remove_dir_all(root).unwrap();
    }

    // Chain A3: grant -> accepted attempt -> pending/running union -> verified exit -> new grant.
    // Risk: double-counting retained grants, or cross-project equal numbers colliding and exceeding host ceilings.
    #[test]
    fn host_slots_survive_pending_running_and_project_identity() {
        let p = policy(); let cap = p.project(&json!({"limits": {"codex": 2}})).unwrap();
        let mut h = Host::new(Ok(p));
        let (a, b) = (Path::new("a"), Path::new("b"));
        assert_eq!(h.reserve(a, 1, 0, "codex", &cap, false, 0), Ok(1));
        h.claimed(a, 1, 77);
        assert_eq!(h.sync(a, vec![(1, 77, "codex".into()), (1, 77, "codex".into())]), 1);
        assert_eq!(h.reserve(b, 1, 77, "codex", &cap, false, 30), Ok(2));
        assert_eq!(h.sync(b, vec![(1, 77, "codex".into()), (1, 77, "codex".into())]), 2);
        assert!(h.reserve(a, 2, 78, "codex", &cap, false, 60).is_err());
        assert_eq!(h.sync(a, vec![]), 1); // only a verified exit releases the attempt
        assert_eq!(h.reserve(a, 2, 78, "codex", &cap, false, 60), Ok(2));
        h.sync(a, vec![]); h.sync(b, vec![]);
        let lower = h.policy.as_ref().unwrap().project(&json!({"limits": {"codex": 1}})).unwrap();
        assert!(h.reserve(a, 1, 80, "codex", &lower, false, 90).is_ok());
        assert!(h.reserve(a, 2, 81, "codex", &lower, false, 120).is_err());
        assert!(h.reserve(b, 2, 81, "codex", &cap, false, 120).is_ok());
    }

    // Chain A3: simultaneous project requests serialize through the same authority.
    // Risk: both threads observe the final free slot and grant it; failed contenders must not reserve anything.
    #[test]
    fn concurrent_grants_never_exceed_host_limit_and_round_robin_rotates() {
        let h = Arc::new(Mutex::new(Host::new(Ok(policy()))));
        let cap = policy().cap;
        let barrier = Arc::new(std::sync::Barrier::new(3));
        let joins: Vec<_> = ["a", "b"].into_iter().map(|root| {
            let (h, cap, barrier) = (h.clone(), cap.clone(), barrier.clone());
            std::thread::spawn(move || { barrier.wait(); h.lock().unwrap().reserve(Path::new(root), 1, 1, "codex", &cap, false, 0).is_ok() })
        }).collect();
        barrier.wait();
        assert_eq!(joins.into_iter().map(|j| usize::from(j.join().unwrap())).sum::<usize>(), 1, "host spacing is serialized too");
        let mut h = h.lock().unwrap(); h.slots.clear(); h.last = None;
        h.ready(Path::new("a"), true); h.ready(Path::new("b"), true);
        assert!(h.reserve(Path::new("b"), 1, 1, "codex", &cap, true, 0).is_err());
        assert!(h.reserve(Path::new("a"), 1, 1, "codex", &cap, true, 0).is_ok());
        h.sync(Path::new("a"), vec![]);
        assert!(h.reserve(Path::new("a"), 2, 2, "codex", &cap, true, 30).is_err());
        assert!(h.reserve(Path::new("b"), 1, 1, "codex", &cap, true, 30).is_ok());
    }

    // Chain A4: six effective copies -> preflight -> durable originals -> all readbacks -> repeat.
    // Risk: conflict/unknown inputs cause partial edits, foreign-host policy leaks in, or retry changes owner policy.
    #[test]
    fn six_copy_migration_is_atomic_on_conflict_and_restart_safe() {
        let install = temp("migration");
        let _ = fs::remove_dir_all(install.join("host-policy-migration"));
        let _ = fs::remove_file(install.join("host.json"));
        let roots: Vec<_> = (0..6).map(|i| { let p = install.join(format!("project{i}")); fs::create_dir_all(&p).unwrap(); p }).collect();
        let me = std::env::var("TASKQ_HOST").ok().filter(|s| !s.is_empty()).unwrap_or_else(crate::hostname);
        let originals: Vec<_> = roots.iter().enumerate().map(|(i, r)| {
            let text = format!("  {{\"repo\":\"invented/p{i}\",\"untouched\":{{\"x\":7}},\"limits\":{{\"codex\":0}},\"capacity_hosts\":{},\"admission\":{{\"mode\":\"auto\",\"max\":2}}}}\n",
                json!({me.clone(): {"limits": {"codex": 2}}, "invented-other-host": {"limits": {"codex": 999}}}));
            fs::write(r.join("taskq.json"), &text).unwrap(); text
        }).collect();
        #[cfg(unix)] {
            use std::os::unix::fs::PermissionsExt;
            for r in &roots { fs::set_permissions(r.join("taskq.json"), fs::Permissions::from_mode(0o640)).unwrap(); }
        }
        let owners: Vec<_> = roots.iter().map(|r| ownership(&r.join("taskq.json")).unwrap()).collect();
        let path = roots[5].join("taskq.json");
        let mut conflict: Value = serde_json::from_str(&originals[5]).unwrap();
        conflict["capacity_hosts"][&me]["limits"]["codex"] = json!(1);
        let conflicting = serde_json::to_string(&conflict).unwrap(); fs::write(&path, &conflicting).unwrap();
        let e = migrate(&install, &roots).unwrap_err();
        assert!(e.contains("invented/p5: limits"), "{e}");
        assert!(!install.join("host.json").exists()); assert!(!install.join("host-policy-migration").exists());
        for (i, r) in roots.iter().enumerate() { assert_eq!(fs::read_to_string(r.join("taskq.json")).unwrap(), if i == 5 { &conflicting } else { &originals[i] }.as_str()); }
        fs::write(&path, &originals[5]).unwrap();
        fs::rename(roots[4].join("taskq.json"), roots[4].join("missing")).unwrap();
        assert!(migrate(&install, &roots).unwrap_err().contains("unknown")); assert!(!install.join("host.json").exists());
        fs::rename(roots[4].join("missing"), roots[4].join("taskq.json")).unwrap();
        migrate(&install, &roots).unwrap();
        assert_eq!(Policy::read(&install).unwrap().cap.limits["codex"], 2, "only the effective local host copy migrates");
        for (i, r) in roots.iter().enumerate() {
            assert_eq!(fs::read_to_string(install.join(format!("host-policy-migration/{i}.json"))).unwrap(), originals[i]);
            let v: Value = serde_json::from_slice(&fs::read(r.join("taskq.json")).unwrap()).unwrap();
            assert_eq!(v["untouched"], json!({"x": 7})); assert!(v["capacity_hosts"].is_null()); assert!(v["limits"].is_null());
            assert_eq!(ownership(&r.join("taskq.json")).unwrap(), owners[i]);
            #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; assert_eq!(fs::metadata(r.join("taskq.json")).unwrap().permissions().mode() & 0o777, 0o640); }
        }
        // Simulate a crash halfway through staged replacements. Resume consumes the saved mapping, not remaining copies.
        fs::remove_file(install.join("host-policy-migration/complete")).unwrap();
        let mut interrupted = fs::metadata(roots[0].join("taskq.json")).unwrap().permissions();
        #[cfg(windows)] interrupted.set_readonly(true);
        #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; interrupted.set_mode(0o600); }
        fs::set_permissions(roots[0].join("taskq.json"), interrupted).unwrap();
        fs::write(roots[2].join("taskq.json"), &originals[2]).unwrap();
        migrate(&install, &roots).unwrap();
        assert!(!fs::metadata(roots[0].join("taskq.json")).unwrap().permissions().readonly());
        #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; assert_eq!(fs::metadata(roots[0].join("taskq.json")).unwrap().permissions().mode() & 0o777, 0o640); }
        let mut edited = Policy::read(&install).unwrap().raw; edited["limits"]["codex"] = json!(1);
        fs::write(install.join("host.json"), serde_json::to_string(&edited).unwrap()).unwrap();
        migrate(&install, &roots).unwrap(); assert_eq!(Policy::read(&install).unwrap().raw, edited, "completed retry preserves later owner edits");
        fs::remove_dir_all(install).unwrap();
    }

    // Chain A4: Windows protected DACL -> staged replacement. Risk: changing file access during migration.
    #[cfg(windows)]
    #[test]
    fn migration_preserves_protected_windows_acl() {
        let root = temp("protected-acl");
        let path = root.join("taskq.json");
        fs::write(&path, r#"{"repo":"invented/protected","limits":{"codex":2},"admission":{"mode":"auto","max":2}}"#).unwrap();
        let owner = ownership(&path).unwrap();
        let mut protected = owner.clone();
        protected[3] = json!(protected[3].as_u64().unwrap() | 0x10);
        stage_owner(&path, &protected).unwrap();
        let expected = ownership(&path).unwrap();
        migrate(&root, &[root.clone()]).unwrap();
        assert_eq!(ownership(&path).unwrap(), expected);
        assert_eq!(Policy::read(&root).unwrap().cap.limits["codex"], 2);
        fs::remove_dir_all(root).unwrap();
    }

    // Chain A4: unknown or corrupt host policy -> refused launch, never fallback to project copies.
    #[test]
    fn unknown_policy_holds_without_discarding_occupied_slots() {
        let install = temp("unknown"); let _ = fs::remove_file(install.join("host.json"));
        let mut h = Host::new(Ok(policy())); let cap = policy().cap;
        h.reserve(Path::new("a"), 1, 7, "codex", &cap, false, 0).unwrap();
        h.reload(Policy::read(&install));
        assert!(h.policy.as_ref().unwrap_err().contains("unknown"));
        assert!(h.reserve(Path::new("b"), 1, 7, "codex", &cap, false, 30).is_err());
        assert_eq!(h.slots.len(), 1);
        for v in [json!({"admission": "bad"}), json!({"codex": [false]}), json!({"load_gate": {"enabled": "yes"}}), json!({"update_every": 0})] {
            assert!(Policy::parse(v, "invented").is_err());
        }
        assert!(policy().project(&json!({"limits": {"codex": 3}})).is_err());
        fs::remove_dir_all(install).unwrap();
    }

    // Chain A5: host pid ownership and registry poke -> exact-root reload. Risk: duplicate host or discovery.
    #[test]
    fn pid_check_and_poke_use_host_paths_and_unknown_is_held() {
        let install = temp("pid");
        let child = if cfg!(windows) { std::process::Command::new("ping").args(["-n", "30", "127.0.0.1"]).stdout(std::process::Stdio::null()).spawn() }
            else { std::process::Command::new("sleep").arg("30").spawn() }.unwrap();
        let mut child = child;
        fs::create_dir_all(crate::dot(&install)).unwrap();
        let legacy_status = format!("{{\"pid\":{}}}", child.id());
        fs::write(crate::dot(&install).join("status.json"), &legacy_status).unwrap();
        assert!(startup_status(&install, &install, "invented malformed config","fixture").is_err());
        assert_eq!(fs::read_to_string(crate::dot(&install).join("status.json")).unwrap(), legacy_status, "startup failure cannot overwrite a live legacy pid");
        assert!(legacy(&install,&[install.clone()]).is_err());
        fs::write(install.join("service"), child.id().to_string()).unwrap(); assert_eq!(other(&install).unwrap(), Some(child.id()));
        let stale=std::time::SystemTime::now()-std::time::Duration::from_secs(120);
        fs::OpenOptions::new().write(true).open(install.join("service")).unwrap()
            .set_times(fs::FileTimes::new().set_modified(stale)).unwrap();
        assert_eq!(other(&install).unwrap(),Some(child.id()),"stale heartbeat cannot release a live service owner");

        fs::write(install.join("service"), std::process::id().to_string()).unwrap(); assert_eq!(other(&install).unwrap(), None);
        fs::write(install.join("service"), "bad").unwrap(); assert!(other(&install).unwrap_err().contains("unknown"));
        fs::write(install.join("projects"), format!("{}\n", install.display())).unwrap();
        assert_eq!(projects(&install).unwrap(), vec![install.canonicalize().unwrap()]);
        let _ = fs::remove_file(install.join("poke"));
        let mut registry = Registry::default(); registry.refresh(&install, 0);
        fs::write(install.join("projects"), "").unwrap(); registry.refresh(&install, 1);
        assert_eq!(registry.roots.len(), 1, "no folder discovery or unnecessary registry reads");
        crate::touch(&install.join("poke")); registry.refresh(&install, 2);
        assert!(registry.roots.is_empty(), "host poke loads the changed exact-root list before the minute poll");
        fs::write(install.join("projects"), "relative\n").unwrap(); registry.refresh(&install, 62);
        assert!(registry.error.as_ref().unwrap().contains("unknown")); assert!(registry.roots.is_empty(), "malformed registry retains the last good list");
        child.kill().unwrap(); let _ = child.wait(); fs::remove_dir_all(install).unwrap();
    }
}
