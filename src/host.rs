//! Host policy, occupied slots and project supervision (taskq.md § 3, § 4, § 9).
use crate::{Res, service::{self, Capacity, Gate}};
use serde_json::{Value, json};
use std::{collections::{BTreeMap, BTreeSet}, fs, path::{Path, PathBuf}, sync::{Arc, Mutex}, time::Instant};

thread_local!(static ACTIVE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) });
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
        Ok(Self { raw: v, cap, me, has, codex, permission_mode, update_every, rollback_after })
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
#[tracing::instrument(name="host.policy.write",skip_all)]
pub fn write_capacity(install:&Path, host:&str, edit:&crate::CapEdit, expect:Option<&Value>)->Res<(Capacity,Capacity)> {
    if std::env::var_os("TASKQ_ATTEMPT").is_some() {return Err("a worker cannot change host policy".into());}
    crate::resources::locked(install,|| {
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
        replace_owned(&path,&original,&(serde_json::to_string_pretty(&candidate.raw).map_err(|e|e.to_string())?+"\n"))?;
        let saved=Policy::read(install)?;
        if saved.raw!=candidate.raw {return Err("host policy write uncertain: readback differs".into());}
        Ok((old.cap,saved.cap))
    })
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

pub type Shared = Arc<Mutex<Host>>;
type Slot = (PathBuf, u64, u64);
fn slot(project: &Path, task: u64, attempt: u64) -> Slot {
    // Before claim there is no fencing token; only that provisional slot uses the issue number.
    (project.into(), if attempt == 0 { task } else { 0 }, attempt)
}
pub struct Host {
    pub policy: Res<Policy>,
    slots: BTreeMap<Slot, String>,
    pub gate: Gate,
    began: Instant,
    last: Option<u64>,
    /// Registered ready projects, rotated only after a grant.
    ready: BTreeSet<PathBuf>,
    turn: Option<PathBuf>,
    pub held: Option<String>,
    pub restart: bool,
    /// Each thread reports active turns/children before the host considers an image switch.
    pub busy: BTreeMap<PathBuf, bool>,
    pub prepared: BTreeSet<PathBuf>,
    pub draining: BTreeSet<PathBuf>,
    pub request_since: u64,
    pub hour_start: BTreeMap<String, u64>,
    pub last_hour: Option<BTreeMap<String, u64>>,
    rebuilding: bool,
    rebuilt: BTreeSet<PathBuf>,
    published: BTreeMap<PathBuf, Instant>,
}

impl Host {
    pub fn new(policy: Res<Policy>) -> Self {
        Self { policy, slots: BTreeMap::new(), gate: Gate::default(), began: Instant::now(), last: None,
            ready: BTreeSet::new(), turn: None, held: None, restart: false, busy: BTreeMap::new(), prepared: BTreeSet::new(), draining: BTreeSet::new(),
            request_since: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()), hour_start: crate::requests::totals(), last_hour: None,
            rebuilding: false, rebuilt: BTreeSet::new(), published: BTreeMap::new() }
    }

    pub fn reload(&mut self, policy: Res<Policy>) {
        if let Ok(p) = &policy { self.gate.configure(p.cap.gate); }
        self.policy = policy;
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

    pub fn reserve(&mut self, project: &Path, n: u64, a: u64, rt: &str, cap: &Capacity, queue: bool, now: u64) -> Res<usize> {
        let p = self.policy.as_ref().map_err(Clone::clone)?;
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

/// A prior recorded owner must have exited; record age never proves that. The PID record is a projection.
pub fn other(install: &Path) -> Res<Option<u32>> {
    let text=match fs::read_to_string(install.join("service")) {
        Err(e) if e.kind()==std::io::ErrorKind::NotFound=>return Ok(None),
        Err(e)=>return Err(format!("host service pid unknown: {e}")),
        Ok(text)=>text,
    };
    let pid=text.trim().parse::<u32>().map_err(|_|"host service pid unknown: malformed record")?;
    if pid==0 {return Err("process liveness unknown: invalid pid".into());}
    Ok((pid!=std::process::id() && crate::runtime::liveness(pid)?).then_some(pid))
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

fn legacy(roots: &[PathBuf]) -> Res<()> {
    for root in roots {
        if project_service(root)?.is_some() { return Err("handoff: legacy service still running".into()); }
    }
    Ok(())
}

fn startup_status(install: &Path, root: &Path, error: &str) -> Res<()> {
    if project_service(root)?.is_some() { return Err("legacy ownership retained; startup status not written".into()); }
    let status = coreq::status::common(&crate::platform::tool(install), "unknown", &crate::project_name(root),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()), Some(error.into()), None);
    fs::create_dir_all(crate::dot(root)).map_err(|e| e.to_string())?;
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
    let mut threads: BTreeMap<PathBuf, std::thread::JoinHandle<Res<()>>> = BTreeMap::new();
    let mut roots = vec![];
    let mut stopped = BTreeSet::new();
    let mut checked: Option<Instant> = None;
    let began = Instant::now();
    let mut hour = (Instant::now(), crate::requests::totals());
    let mut tray = None;
    let mut registry = Registry::default();
    loop {
        if let Err(e) = crate::profile::prune(install) { eprintln!("taskq: telemetry unknown: {e}"); }
        crate::platform::write_atomic(&install.join("service"), &format!("{}\n", std::process::id()))?;
        registry.refresh(install, began.elapsed().as_secs());
        roots.clone_from(&registry.roots);
        let old = legacy(&roots);
        let policy = match &old { Ok(()) => migrate(install, &roots).and_then(|_| Policy::read(install)), Err(e) => Err(e.clone()) };
        {
            let mut h = shared.lock().unwrap_or_else(|e| e.into_inner());
            h.reload(policy);
            h.held = registry.error.clone().or_else(|| old.as_ref().err().cloned());
            let now = h.elapsed();
            if h.gate.last.is_none_or(|l| now.saturating_sub(l.0) >= 30) {
                h.gate.host_mem = crate::platform::host_memory();
                let wall = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
                h.gate.observe(now, wall, crate::platform::cpu_ticks(), crate::platform::memory_available());
            }
            for root in threads.keys().filter(|p| !roots.contains(p)) { h.draining.insert(root.clone()); }
            for root in &roots {
                h.draining.remove(root);
                if threads.contains_key(root) || stopped.contains(root) { continue; }
                h.busy.insert(root.clone(), true);
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
                            Ok(c) => return crate::service_run(&c, Some(host)),
                            Err(e) => {
                                if let Ok(mut h) = host.lock() {
                                    if h.draining.contains(&root) { return Err("project drained".into()); }
                                    h.busy.insert(root.clone(), false);
                                    if h.restart { h.prepared.insert(root.clone()); }
                                }
                                let _ = startup_status(&install, &root, &e);
                                crate::platform::log(&e);
                                std::thread::sleep(std::time::Duration::from_secs(1));
                            }
                        }
                    }
                }));
            }
        }
        // Joining is nonblocking: failed projects do not stop unrelated loops, and their occupied slots stay held.
        let ended: Vec<_> = threads.iter().filter(|(_, t)| t.is_finished()).map(|(p, _)| p.clone()).collect();
        for root in ended {
            let outcome = threads.remove(&root).unwrap().join();
            let drained = matches!(&outcome, Ok(Err(e)) if e == "project drained");
            let mut h = shared.lock().unwrap_or_else(|e| e.into_inner());
            h.publication(&root, false);
            if drained { h.busy.remove(&root); h.prepared.remove(&root); }
            else {
                // A reduced loop failed: its attempts expire on other hosts, and local launches hold for inspection.
                stopped.insert(root.clone());
                h.ready(&root, false);
                append_project(install, &root, "project thread stopped; leases expire on other hosts; retained slots have unknown liveness");
            }
        }
        let mut h = shared.lock().unwrap_or_else(|e| e.into_inner());
        let timers = h.policy.as_ref().map(|p| (p.update_every, p.rollback_after)).unwrap_or((60, 600));
        let asked = fs::remove_file(install.join("update")).is_ok() || roots.iter().any(|r| fs::remove_file(crate::dot(r).join("update")).is_ok());
        if !h.restart && (asked || checked.is_none_or(|t| t.elapsed().as_secs() >= timers.0)) {
            checked = Some(Instant::now());
            drop(h);
            let updated = if crate::installed(install) { crate::platform::service_update(install) } else { crate::platform::update(install, true) };
            let text = match &updated { Ok(Some(v)) => format!("update: {v}"), Ok(None) => "update: up to date".into(), Err(e) => format!("update: {e}") };
            for root in &roots { append_project(install, root, &text); }
            h = shared.lock().unwrap_or_else(|e| e.into_inner());
            if crate::installed(install) && matches!(updated, Ok(Some(_))) { h.restart = true; }
        }
        if install.join("restart").exists() || roots.iter().any(|r| crate::dot(r).join("restart").exists()) { h.restart = true; }
        if h.restart {
            for root in &roots {
                if let Err(e) = crate::poke(root) { append_project(install, root, &e); }
            }
            if h.busy.keys().all(|r| h.prepared.contains(r)) {
                drop(h);
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
    match fs::symlink_metadata(install.join("projects.pending")) {
        Ok(_)=>return Err("projects.pending retained; project membership unknown".into()),
        Err(e) if e.kind()==std::io::ErrorKind::NotFound=>{},
        Err(e)=>return Err(format!("projects prepare unknown: {e}")),
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
        fs::create_dir_all(install).map_err(|e|e.to_string())?;
        let lock=coreq::file_lock::try_lock(&install.join("projects.lock")).map_err(|e|format!("project registry busy or lock unavailable: {e}"))?;
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
    crate::resources::locked(install,||migrate_locked(install,roots))
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
fn ownership(path: &Path) -> Res<Value> {
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
fn ownership(path: &Path) -> Res<Value> {
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
    fn temp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("taskq-host-{name}-{}", std::process::id()));
        fs::create_dir_all(&p).unwrap(); p
    }
    fn policy() -> Policy {
        Policy::parse(json!({"limits": {"codex": 2}, "admission": {"max": 2}, "load_gate": {"enabled": false}}), "invented").unwrap()
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

    #[test]
    #[ignore="owned native process for one_host_owner_before_heartbeat_publication"]
    fn owner_probe() {
        let install=PathBuf::from(std::env::var_os("TASKQ_OWNER_INSTALL").unwrap());
        let claim=owner(&install).unwrap();
        let marker=PathBuf::from(std::env::var_os("TASKQ_OWNER_MARKER").unwrap());
        crate::platform::write_atomic(&marker,if claim.is_some(){"claimed"}else{"blocked"}).unwrap();
        if std::env::var_os("TASKQ_OWNER_HOLD").is_some() && claim.is_some() {loop {std::thread::park();}}
    }

    // H-A3: acknowledged policy replacement survives killing its writer, preserving unrelated policy fields.
    #[test]
    fn acknowledged_policy_survives_writer_termination() {
        let root=temp("policy-commit");
        fs::write(root.join("host.json"),json!({"limits":{"codex":2},"owner_field":"preserve"}).to_string()).unwrap();
        let mut child=std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact","host::tests::policy_writer","--ignored"])
            .env("TASKQ_HOST_POLICY_FIXTURE",&root).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().unwrap();
        let until=Instant::now()+std::time::Duration::from_secs(10);
        while !root.join("ack").is_file() && child.try_wait().unwrap().is_none() && Instant::now()<until {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let acknowledged=root.join("ack").is_file();
        if child.try_wait().unwrap().is_none() {child.kill().unwrap();child.wait().unwrap();}
        let read=Policy::read(&root);
        fs::remove_dir_all(root).unwrap();
        assert!(acknowledged,"writer did not acknowledge before the fixture deadline");
        let read=read.unwrap();assert_eq!(read.cap.limits["codex"],3);assert_eq!(read.raw["owner_field"],"preserve");
    }

    #[test]
    #[ignore="owned subprocess used by acknowledged_policy_survives_writer_termination"]
    fn policy_writer() {
        let root=PathBuf::from(std::env::var_os("TASKQ_HOST_POLICY_FIXTURE").unwrap());
        let owner=Policy::read(&root).unwrap().me;
        write_capacity(&root,&owner,&crate::CapEdit {limits:BTreeMap::from([("codex".into(),3)]),..Default::default()},None).unwrap();
        fs::write(root.join("ack"),"committed").unwrap();
        loop {std::thread::park();}
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
        fs::remove_file(&path).unwrap();fs::create_dir(&path).unwrap();
        assert!(!crate::publish_status(&tool,&root,&status,Some(&host)));
        assert!(!host.lock().unwrap().publications_ready(&roots));
        assert_eq!(host.lock().unwrap().slots.len(),1,"publication must not lose child ownership");
        fs::remove_dir(&path).unwrap();
        assert!(crate::publish_status(&tool,&root,&status,Some(&host)));
        for failed in [json!({"complete":false,"tasks":[]}),json!({"complete":true,"tasks":[],"error":"board unavailable"}),
            json!({"complete":true,"tasks":[],"focus_error":"focus command pending"})] {
            assert!(!crate::publish_status(&tool,&root,&failed,Some(&host)));
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
        assert!(startup_status(&install, &install, "invented malformed config").is_err());
        assert_eq!(fs::read_to_string(crate::dot(&install).join("status.json")).unwrap(), legacy_status, "startup failure cannot overwrite a live legacy pid");
        assert!(legacy(&[install.clone()]).is_err());
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
