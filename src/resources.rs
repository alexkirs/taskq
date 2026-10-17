//! Resource policy host adapter (docs/design/shared-resource-controls.md § Host adapter, `taskq.resources/1`): the
//! host operator's own resource grants, opaque project IDs and policy revisions, and the exchange that applies
//! DashQ-delivered operations through the canonical capacity writer. No listener, shell or second task queue: the
//! enrolled service posts its receipts and receives operations. State: `<install>/resources/` (kept by sweep).

use crate::{CapEdit, Res, enrollment, snapshot::Snapshot};
use coreq::enroll::{Https, Wire};
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs, path::{Path, PathBuf}, thread::JoinHandle};

pub const SCHEMA: &str = "taskq.resources/1";
/// Relative to the connection's destination (`https://<dashq>/machine`): DashQ serves `/machine/resources/v1/exchange`.
pub const EXCHANGE: &str = "/resources/v1/exchange";
const FILE: &str = "state.json";
/// v1 bounds (§ Bounds and receipts).
const BODY: usize = 16 << 10;
const ITEMS: usize = 8;
const SPACING: u64 = 5;
/// Between exchanges with nothing outstanding.
const IDLE: u64 = 30;
const FRESH: u64 = 60;
const AHEAD: u64 = 30;
const RECONCILE: u64 = 300;
const RETAIN: u64 = 24 * 3600;
pub(crate) const MAX_OPS: usize = 4096;
/// Longest host grant, in hours.
pub const MAX_HOURS: u64 = 720;
const RUNTIMES: [&str; 3] = crate::service::RUNTIMES;

#[derive(Default, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct State {
    #[serde(default)]
    pub grants: BTreeMap<String, Grant>,
    #[serde(default)]
    pub projects: BTreeMap<String, Project>,
    #[serde(default)]
    pub ops: BTreeMap<String, Op>,
}

/// The host operator's resource grant: only operations naming this grant ID and actor, for this installation and
/// generation and within this scope, can write. DashQ's own grant is necessary, never sufficient.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Grant {
    pub actor_id: String,
    pub installation_id: String,
    pub generation: u64,
    pub project_ids: Vec<String>,
    pub runtime_ids: Vec<String>,
    pub host_policy: bool,
    pub expires_at: u64,
    pub revoked: bool,
    /// Provenance: the local OS account that wrote it and when.
    pub by: String,
    pub at: u64,
}

/// One registered project root: its opaque ID never moves to another root and is never reused.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Project {
    pub root: String,
    /// Opaque; new on every change of `policy` and on every canonical write.
    pub revision: String,
    /// This host's capacity (`service::Capacity::json`) at `revision`; null when the config is unreadable.
    pub policy: Value,
    pub since: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Op {
    /// The parsed submit (or the cancellation that came first): immutable once recorded.
    pub bound: Value,
    pub receipt: Value,
    pub expires_at: u64,
    /// Time of the canonical write; a readback must be at least this new.
    #[serde(default)]
    pub saved_at: Option<u64>,
    /// Set before the write and cleared after: found set, the outcome is unknown and the write is never repeated.
    #[serde(default)]
    pub writing: bool,
    /// DashQ took this receipt.
    #[serde(default)]
    pub sent: bool,
}

fn local(o:&Op)->bool {o.bound.get("local_capacity").is_some()||o.bound.get("local_restart").is_some()||o.bound.get("local_stop").is_some()}

pub fn dir(install: &Path) -> PathBuf {
    install.join("resources")
}

fn read(install: &Path) -> Res<State> {
    match fs::symlink_metadata(dir(install).join("state.json.pending")) {
        Ok(_)=>return Err("resources state prepare retained; authority unknown".into()),
        Err(e) if e.kind()==std::io::ErrorKind::NotFound=>{},
        Err(e)=>return Err(format!("resources state prepare unknown: {e}")),
    }
    match fs::read_to_string(dir(install).join(FILE)) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(State::default()),
        Err(e) => Err(format!("resources: {e}")),
        // a corrupt store stays for the operator, never overwritten
        Ok(t) => serde_json::from_str(&t).map_err(|e| format!("resources state unreadable ({e}); file kept")),
    }
}

pub(crate) fn save(install: &Path, s: &State) -> Res<()> {
    let text = serde_json::to_string_pretty(s).map_err(|e| e.to_string())? + "\n";
    coreq::write_durable(&dir(install).join(FILE), &text)?;
    if read(install)? != *s {
        return Err("resources: readback differs".into());
    }
    Ok(())
}

/// The host lock, held until the file drops: resource writes and changes of the DashQ connection take turns under it.
fn lock(install: &Path) -> Res<coreq::file_lock::Lock> {
    fs::create_dir_all(dir(install)).map_err(|e| format!("resources: {e}"))?;
    coreq::file_lock::lock(&dir(install).join("lock")).map_err(|e|format!("resources lock: {e}"))
}

/// `f` changes the DashQ connection under the host lock (`taskq disconnect`), so it never lands between an
/// operation's final connection check and its write.
pub fn locked<T>(install: &Path, f: impl FnOnce() -> Res<T>) -> Res<T> {
    let _lock = lock(install)?;
    f()
}

/// coreq's wire for calls whose reply can replace the active connection: once an ack (pairing activates a new one)
/// or a 401 (DashQ revoked it) arrives, it takes the host lock before coreq saves, and holds it until dropped.
pub struct Fenced<W> {
    wire: W,
    install: PathBuf,
    held: Option<coreq::file_lock::Lock>,
}

impl<W: Wire> Fenced<W> {
    pub fn new(install: &Path, wire: W) -> Self {
        Fenced { wire, install: install.to_path_buf(), held: None }
    }
}

impl<W: Wire> Wire for Fenced<W> {
    fn post(&mut self, dest: &str, route: &str, auth: Option<&str>, body: &Value) -> Result<(u16, Value), String> {
        let r = self.wire.post(dest, route, auth, body)?;
        if self.held.is_none() && (route == "/connect/ack" || r.0 == 401) {
            self.held = Some(lock(&self.install)?);
        }
        Ok(r)
    }
    fn now(&self) -> u64 { self.wire.now() }
    fn sleep(&mut self, secs: u64) { self.wire.sleep(secs) }
    fn show(&mut self, uri: &str, code: &str) { self.wire.show(uri, code) }
}

/// `f` on the state under the host lock; written back when changed.
pub(crate) fn with<T>(install: &Path, f: impl FnOnce(&mut State) -> Res<T>) -> Res<T> {
    let _lock = lock(install)?;
    let mut s = read(install)?;
    let before = s.clone();
    let r = f(&mut s);
    if s != before {
        save(install, &s)?;
    }
    r
}

/// Host/UI observers retry on a later tick; they must not wait behind a policy writer.
pub(crate) fn try_with<T>(install:&Path,f:impl FnOnce(&mut State)->Res<T>)->Res<T> {
    try_locked(install,|| {
        let mut state=read(install)?;let before=state.clone();let result=f(&mut state);
        if state!=before {save(install,&state)?;}result
    })
}
pub(crate) fn try_locked<T>(install:&Path,f:impl FnOnce()->Res<T>)->Res<T> {
    fs::create_dir_all(dir(install)).map_err(|e|format!("resources: {e}"))?;
    let _lock=coreq::file_lock::try_lock(&dir(install).join("lock")).map_err(|e|format!("resources lock unavailable: {e}"))?;
    f()
}

fn token(prefix: &str) -> String {
    let mut b = [0u8; 12];
    SystemRandom::new().fill(&mut b).expect("system random");
    format!("{prefix}-{}", b.iter().map(|x| format!("{x:02x}")).collect::<String>())
}

pub(crate) fn id(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// This host's capacity of the project at `root`.
fn policy(root: &Path) -> Res<(String, Value)> {
    let (cfg, v) = crate::load(root)?;
    Ok((cfg.me.clone(), crate::service::capacity(&v, &cfg.me)?.json()))
}

fn registered(install: &Path, root: &str) -> Res<bool> {
    Ok(crate::host::project_paths(install)?.iter().any(|r| r == Path::new(root)))
}

/// Retire old scopes before dropping roots. A failed registry write remains restrictive, never revives an ID.
pub fn unregister(install: &Path, roots: &[PathBuf], write: impl FnOnce() -> Res<()>) -> Res<()> {
    let _lock=lock(install)?;
    let mut s=read(install)?;
    let before=s.projects.len();
    s.projects.retain(|_, p| !roots.contains(&PathBuf::from(&p.root)));
    let retired=s.projects.len()!=before;
    if retired {save(install,&s)?;}
    write().map_err(|e|if retired {format!("resource scope retired; registry write failed: {e}")} else {e})
}

/// Registered roots get an opaque ID once; an unregistered root loses it for good. A capacity that differs from the
/// recorded one, including a hand edit, gets a new revision.
fn sync(s: &mut State, install: &Path, now: u64) -> Res<()> {
    let roots: Vec<String> = crate::host::project_paths(install)?.into_iter().map(|r| r.display().to_string()).collect();
    s.projects.retain(|_, p| roots.contains(&p.root));
    for r in roots {
        if !s.projects.values().any(|p| p.root == r) {
            s.projects.insert(token("p"), Project { root: r, revision: String::new(), policy: Value::Null, since: now });
        }
    }
    for p in s.projects.values_mut() {
        let now_policy = policy(Path::new(&p.root)).map(|x| x.1).unwrap_or(Value::Null);
        if p.revision.is_empty() || now_policy != p.policy {
            *p = Project { root: p.root.clone(), revision: token("r"), policy: now_policy, since: now };
        }
    }
    Ok(())
}

fn receipt(target: &Value, op: &str, state: &str, reason: Option<&str>, saved: Option<&str>, effective: Option<&str>, at: Option<u64>) -> Value {
    json!({"schema": SCHEMA, "op_id": op, "target": target, "state": state, "reason": reason,
        "saved_revision": saved, "effective_revision": effective, "observed_at": at})
}

pub(crate) fn keys(v: &Value, want: &[&str]) -> bool {
    v.as_object().is_some_and(|o| o.len() == want.len() && want.iter().all(|k| o.contains_key(*k)))
}

fn target_ok(t: &Value) -> bool {
    keys(t, &["installation_id", "generation", "project_id", "runtime_id"])
        && t["installation_id"].as_str().is_some_and(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
        && t["generation"].as_u64().is_some_and(|g| g > 0)
        && (t["project_id"].is_null() || t["project_id"].as_str().is_some_and(id))
        && (t["runtime_id"].is_null() || t["runtime_id"].as_str().is_some_and(|r| RUNTIMES.contains(&r)))
        && (t["runtime_id"].is_null() || !t["project_id"].is_null())
}

pub(crate) fn count(v: &Value) -> bool {
    v.as_u64().is_some_and(|n| n <= u32::MAX as u64)
}

/// The typed submit rules of `taskq.resources/1`: exact keys and types, ID grammar, lifetime and per-schema fields.
pub fn typed(v: &Value) -> Result<(), &'static str> {
    let ids = ["op_id", "actor_id", "grant_id", "expected_revision"];
    if !keys(v, &["schema", "op_id", "actor_id", "grant_id", "target", "policy_schema", "expected_revision", "issued_at", "expires_at", "changes"])
        || v["schema"] != SCHEMA || !ids.iter().all(|k| v[k].as_str().is_some_and(id)) || !target_ok(&v["target"])
        || !v["policy_schema"].is_string()
    {
        return Err("invalid");
    }
    let (Some(issued), Some(expires)) = (v["issued_at"].as_u64(), v["expires_at"].as_u64()) else { return Err("invalid") };
    let Some(changes) = v["changes"].as_object().filter(|c| !c.is_empty()) else { return Err("invalid") };
    if expires <= issued || expires - issued > 300 {
        return Err("invalid");
    }
    let auto = match v["policy_schema"].as_str() {
        Some("taskq.capacity/1") => false,
        Some("taskq.admission/1") => true,
        _ => return Err("unsupported"),
    };
    let (project, runtime) = (!v["target"]["project_id"].is_null(), !v["target"]["runtime_id"].is_null());
    for (k, x) in changes {
        let valid = match k.as_str() {
            "runtime_limit" => runtime && (count(x) || (auto && x.is_null())),
            "maximum" => auto && !runtime && (x.is_null() || count(x)),
            "mode" => auto && !runtime && (x == "auto" || x == "fixed"),
            "load_gate" => project && !runtime && keys(x, &["enabled", "cpu_max", "memory_min"]) && x["enabled"].is_boolean()
                && x["cpu_max"].as_u64().is_some_and(|n| (10..=100).contains(&n)) && x["memory_min"].as_u64().is_some_and(|n| (1..=95).contains(&n)),
            _ => false,
        };
        if !valid {
            return Err("invalid");
        }
    }
    if changes.get("mode") == Some(&json!("auto")) && changes.get("load_gate").is_some_and(|g| g["enabled"] == false) {
        return Err("invalid");
    }
    Ok(())
}

/// The connection this exchange speaks for: `(installation, generation)`.
type Conn<'a> = (&'a str, u64);

/// The host grant check, against the state as it is now (enqueue and again immediately before the write).
fn authorize(s: &State, v: &Value, conn: Conn, now: u64) -> Result<String, &'static str> {
    let t = &v["target"];
    let g = s.grants.get(v["grant_id"].as_str().unwrap_or_default()).filter(|g| g.actor_id == v["actor_id"]).ok_or("unauthorized")?;
    if g.revoked || now >= g.expires_at {
        return Err("revoked");
    }
    let (inst, generation) = conn;
    if t["installation_id"] != inst || t["generation"] != generation || g.installation_id != inst || g.generation != generation {
        return Err("scope");
    }
    let Some(pid) = t["project_id"].as_str() else {
        // TaskQ keeps capacity per project config; no host-wide store exists to write
        return if g.host_policy { Err("unsupported") } else { Err("scope") };
    };
    if !s.projects.contains_key(pid) || !g.project_ids.iter().any(|x| x == pid) {
        return Err("scope");
    }
    if t["runtime_id"].as_str().is_some_and(|rt| !g.runtime_ids.iter().any(|x| x == rt)) {
        return Err("scope");
    }
    Ok(pid.to_string())
}

/// The edit of a typed change set; `v["limits"]`: the project-wide limits a removed override would fall back to.
fn edit(changes: &Value, runtime: Option<&str>, project_limits: &Value) -> Result<CapEdit, &'static str> {
    let mut e = CapEdit::default();
    for (k, x) in changes.as_object().into_iter().flatten() {
        match k.as_str() {
            "runtime_limit" => {
                let rt = runtime.ok_or("invalid")?.to_string();
                match x.as_u64() {
                    Some(n) => { e.limits.insert(rt, n as u32); }
                    // `null` removes this host's override; a project-wide limit would stay, so that is unsupported here
                    None if project_limits[&rt].is_null() => { e.unset.insert(rt); }
                    None => return Err("unsupported"),
                }
            }
            "mode" => e.mode = Some(x == "auto"),
            "maximum" => e.max = Some(x.as_u64().map(|n| n as u32)),
            "load_gate" => {
                e.gate = x["enabled"].as_bool();
                e.cpu_max = x["cpu_max"].as_u64().map(|n| n as u32);
                e.memory_min = x["memory_min"].as_u64().map(|n| n as u32);
            }
            _ => return Err("invalid"),
        }
    }
    Ok(e)
}

/// What `decide` re-checks immediately before the write: the time now (never earlier than the checks before) and
/// whether the connection the operation came over is still the active one.
struct Fence<'a> {
    clock: &'a dyn Fn() -> u64,
    live: &'a dyn Fn() -> bool,
}

/// One delivered submit to its receipt; `None` keeps the original binding of a reused op ID and sends nothing.
fn decide(install: &Path, s: &mut State, v: &Value, conn: Conn, clock: bool, now: u64, fence: &Fence) -> Res<Option<String>> {
    let (Some(op), true) = (v["op_id"].as_str().filter(|o| id(o)), target_ok(&v["target"])) else {
        return Ok(Some("resources: dropped an operation without a valid ID or target".into()));
    };
    let op = op.to_string();
    if let Some(o) = s.ops.get_mut(&op) {
        if local(o) {return Ok(Some(format!("resources: {op} belongs to a local command; ignored")));}
        // the host deduplicates by op ID: the same submit (or the cancellation that came first) repeats its receipt
        let same = o.bound == *v || (o.bound.get("changes").is_none() && ["schema", "actor_id", "grant_id", "target"].iter().all(|k| o.bound[k] == v[k]));
        if !same {
            return Ok(Some(format!("resources: {op} reused with another payload; original kept")));
        }
        o.sent = false;
        return Ok(None);
    }
    let t = v["target"].clone();
    let expires = v["expires_at"].as_u64().unwrap_or(now);
    let record = |s: &mut State, state: &str, reason: Option<&str>| {
        s.ops.insert(op.clone(), Op { bound: v.clone(), receipt: receipt(&t, &op, state, reason, None, None, None), expires_at: expires, saved_at: None, writing: false, sent: false });
        Some(format!("resources: {op} {}", reason.unwrap_or(state)))
    };
    if s.ops.len() >= MAX_OPS {
        // never evict a retained binding: the new ID is not recorded and expires undelivered at DashQ
        return Ok(Some(format!("resources: {MAX_OPS} operations retained; {op} not taken")));
    }
    if let Err(r) = typed(v) {
        return Ok(record(s, "rejected", Some(r)));
    }
    let pid = match authorize(s, v, conn, now) {
        Err(r) => return Ok(record(s, "rejected", Some(r))),
        Ok(p) => p,
    };
    if !clock || v["issued_at"].as_u64().is_some_and(|i| i > now + AHEAD) {
        return Ok(record(s, "rejected", Some("clock_unknown")));
    }
    if now >= expires {
        return Ok(record(s, "expired", None));
    }
    let p = s.projects[&pid].clone();
    if v["expected_revision"] != p.revision.as_str() {
        return Ok(record(s, "rejected", Some("conflict")));
    }
    // These wire profiles own legacy project policy, never fallback authority for host.json.
    if ["host.json","host.json.pending"].iter().any(|name|match fs::symlink_metadata(install.join(name)) {
        Err(e) if e.kind()==std::io::ErrorKind::NotFound=>false,
        _=>true,
    }) {return Ok(record(s,"rejected",Some("unsupported")));}
    let root = PathBuf::from(&p.root);
    let Ok((cfg, cv)) = crate::load(&root) else { return Ok(record(s, "rejected", Some("unsupported"))) };
    let Ok(cur) = crate::service::capacity(&cv, &cfg.me) else { return Ok(record(s, "rejected", Some("unsupported"))) };
    if cur.json() != p.policy {
        return Ok(record(s, "rejected", Some("conflict")));
    }
    // taskq.capacity/1 is the legacy Fixed authority
    if v["policy_schema"] == "taskq.capacity/1" && cur.auto {
        return Ok(record(s, "rejected", Some("unsupported")));
    }
    let e = match edit(&v["changes"], v["target"]["runtime_id"].as_str(), &cv["limits"]) {
        Ok(e) => e,
        Err(r) => return Ok(record(s, "rejected", Some(r))),
    };
    let path = root.join("taskq.json");
    let Ok(text) = fs::read_to_string(&path) else { return Ok(record(s, "rejected", Some("conflict"))) };
    let Ok(out) = crate::edit_capacity(&text, &cfg.me, &cfg.me, &e, Some(&cur.json())) else { return Ok(record(s, "rejected", Some("invalid"))) };
    let want = serde_json::from_str::<Value>(&out).ok().and_then(|x| crate::service::capacity(&x, &cfg.me).ok()).map(|c| c.json());
    // the marker is durable before the write: a crash from here on reads unknown and never writes again
    record(s, "unknown", None);
    s.ops.get_mut(&op).unwrap().writing = true;
    save(install, s)?;
    s.ops.get_mut(&op).unwrap().writing = false;
    // the marker save and earlier waits took time: deadline, grant and connection again on a fresh clock
    let now = now.max((fence.clock)());
    if !(fence.live)() || !registered(install, &p.root)? {
        return Ok(record(s, "rejected", Some("scope")));
    }
    if let Err(r) = authorize(s, v, conn, now) {
        return Ok(record(s, "rejected", Some(r)));
    }
    if now >= expires {
        return Ok(record(s, "expired", None));
    }
    if fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
        return Ok(record(s, "rejected", Some("conflict")));
    }
    if let Err(x) = crate::platform::write_atomic(&path, &out) {
        return Ok(Some(format!("resources: {op} unknown: {x}")));
    }
    let wake = crate::poke(&root);
    let saved = policy(&root).ok().map(|x| x.1);
    let Some(saved) = saved.filter(|x| Some(x) == want.as_ref()) else {
        return Ok(Some(format!("resources: {op} unknown: readback differs")));
    };
    let rev = token("r");
    let o = s.ops.get_mut(&op).unwrap();
    o.receipt = receipt(&t, &op, "saved", None, Some(&rev), None, None);
    o.saved_at = Some(now);
    s.projects.insert(pid, Project { revision: rev, policy: saved, since: now, ..p });
    Ok(Some(match wake { Ok(()) => format!("resources: {op} saved"), Err(error) => format!("resources: {op} saved; {error}") }))
}

/// A cancellation: a recorded operation repeats its receipt (a completed write is never rolled back); an unknown
/// one is recorded cancelled, so a later delivery of it cannot write.
fn cancel(s: &mut State, c: &Value, now: u64) -> Option<String> {
    let valid = keys(c, &["schema", "op_id", "actor_id", "grant_id", "target"]) && c["schema"] == SCHEMA
        && ["op_id", "actor_id", "grant_id"].iter().all(|k| c[k].as_str().is_some_and(id)) && target_ok(&c["target"]);
    let op = c["op_id"].as_str().filter(|_| valid)?.to_string();
    let full = s.ops.len() >= MAX_OPS;
    if s.ops.get(&op).is_some_and(|o|local(o)) {return Some(format!("resources: cancellation of local command {op} ignored"));}
    match s.ops.get_mut(&op) {
        Some(o) if ["schema", "actor_id", "grant_id", "target"].iter().all(|k| o.bound[k] == c[k]) => o.sent = false,
        Some(_) => return Some(format!("resources: cancellation of {op} does not match it; ignored")),
        None if !full => {
            s.ops.insert(op.clone(), Op { bound: c.clone(), receipt: receipt(&c["target"], &op, "cancelled", None, None, None, None), expires_at: now + 300, saved_at: None, writing: false, sent: false });
        }
        None => return None,
    }
    Some(format!("resources: {op} cancelled"))
}

/// Saved operations become applied on a fresh service readback of the saved revision (§ 4 `status.json`).
fn readback(s: &mut State, now: u64) {
    for o in s.ops.values_mut() {
        if local(o) {continue;}
        let Some(at) = o.saved_at.filter(|t| o.receipt["state"] == "saved" && now < t + RECONCILE) else { continue };
        let Some(p) = o.receipt["target"]["project_id"].as_str().and_then(|pid| s.projects.get(pid)) else { continue };
        if o.receipt["saved_revision"] != p.revision.as_str() {
            continue;
        }
        let root = PathBuf::from(&p.root);
        let st: Value = fs::read_to_string(crate::dot(&root).join("status.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        let me = crate::load(&root).map(|x| x.0.me).unwrap_or_default();
        let seen = st["at"].as_u64().filter(|t| *t >= at && *t <= now && now - t <= FRESH && st["host"] == me.as_str());
        let same = ["limits", "load_gate", "admission"].iter().all(|k| st["capacity"][k] == p.policy[k]);
        if let (Some(seen), true) = (seen, same) {
            o.receipt["state"] = json!("applied");
            o.receipt["effective_revision"] = json!(p.revision);
            o.receipt["observed_at"] = json!(seen);
            o.sent = false;
        }
    }
}

/// Before an exchange: interrupted writes read unknown, projects and revisions are current, retention is applied.
pub(crate) fn prepare(s: &mut State, install: &Path, now: u64) -> Res<()> {
    sync(s, install, now)?;
    for o in s.ops.values_mut().filter(|o| o.writing) {
        o.writing = false;
        o.receipt["state"] = json!("unknown");
        o.sent = false;
    }
    s.ops.retain(|_,o|now<o.expires_at.saturating_add(RETAIN)
        ||((o.bound.get("local_restart").is_some()||o.bound.get("local_stop").is_some())&&o.receipt["state"]!="observed"));
    readback(s, now);
    Ok(())
}

/// Whether there is anything to exchange: an active host grant for this connection, or a receipt DashQ has not taken.
fn wanted(s: &State, conn: Conn, now: u64) -> bool {
    s.grants.values().any(|g| !g.revoked && now < g.expires_at && g.installation_id == conn.0 && g.generation == conn.1)
        || s.ops.values().any(|o| !o.sent && !local(o))
}

/// One exchange with DashQ over the active connection: up to 8 receipts out, up to 8 operations and cancellations
/// in. Returns log lines and whether to come back after the minimum spacing rather than the idle interval.
pub fn exchange(install: &Path, active: &Value, w: &mut dyn Wire, now: u64) -> Res<(Vec<String>, bool)> {
    let (Some(inst), Some(generation), Some(dest), Some(cred)) = (active["installation"].as_str(), active["generation"].as_u64(), active["destination"].as_str(), active["credential"].as_str()) else {
        return Err("connection has no installation, generation or credential".into());
    };
    let conn = (inst, generation);
    // no store: no host grant was ever made, nothing to exchange or to create
    if !dir(install).join(FILE).exists() {
        read(install)?;
        return Ok((vec![], false));
    }
    let sent: Vec<Value> = with(install, |s| {
        prepare(s, install, now)?;
        Ok(if wanted(s, conn, now) { s.ops.values().filter(|o| !o.sent && !local(o)).take(ITEMS).map(|o| o.receipt.clone()).collect() } else { vec![json!(null)] })
    })?;
    if sent.first().is_some_and(Value::is_null) {
        return Ok((vec![], false));
    }
    let body = json!({"schema": SCHEMA, "installation_id": inst, "generation": generation, "receipts": sent});
    let (code, r) = w.post(dest, EXCHANGE, Some(cred), &body)?;
    if code == 429 {
        return Ok((vec![], true));
    }
    if code != 200 {
        return Err(format!("exchange refused: http {code} {}", r["reason"].as_str().filter(|x| id(x)).unwrap_or("unknown")));
    }
    let size = serde_json::to_vec(&r).map_or(usize::MAX, |b| b.len());
    let list = |k: &str| r[k].as_array().filter(|a| a.len() <= ITEMS).cloned();
    let (Some(ops), Some(cancels), Some(server)) = (list("operations"), list("cancellations"), r["server_at"].as_u64()) else {
        return Err("exchange reply invalid".into());
    };
    if size > BODY || !keys(&r, &["schema", "server_at", "operations", "cancellations"]) || r["schema"] != SCHEMA {
        return Err("exchange reply invalid".into());
    }
    let clock = now.abs_diff(server) <= AHEAD;
    // the operations belong to this connection: a disconnect, revoke or new generation during the POST drops them
    let live = || enrollment::active(install).ok().flatten().is_some_and(|a| ["installation", "generation", "destination", "credential"].iter().all(|k| a[k] == active[k]));
    let w = &*w;
    let fence = Fence { clock: &|| w.now(), live: &live };
    with(install, |s| {
        // never at or after the deadline on either clock, read after the POST and the lock wait
        let now = now.max(server).max(w.now());
        if !live() {
            return Ok((vec![format!("resources: connection changed during the exchange; {} operations not taken", ops.len())], false));
        }
        for rc in &sent {
            if let Some(o) = s.ops.get_mut(rc["op_id"].as_str().unwrap_or_default()).filter(|o| o.receipt == *rc) {
                o.sent = true;
            }
        }
        let mut log: Vec<String> = cancels.iter().filter_map(|c| cancel(s, c, now)).collect();
        for v in &ops {
            log.extend(decide(install, s, v, conn, clock, now, &fence)?);
        }
        if !clock && !ops.is_empty() {
            log.push(format!("resources: host clock differs from DashQ's by {} s", now.abs_diff(server)));
        }
        let more = !ops.is_empty() || !cancels.is_empty() || s.ops.values().any(|o| !o.sent && !local(o));
        Ok((log, more))
    })
}

/// The service's exchange: one at a time on its own thread, never blocking a pass, only from the service whose board
/// the connection approved (the host's one DashQ speaker).
#[derive(Default)]
pub struct Exchanger {
    job: Option<JoinHandle<(u64, Vec<String>)>>,
    wait_until: u64,
}

impl Exchanger {
    pub fn tick(&mut self, install: &Path, snap: Option<&Snapshot>, now: u64) -> Vec<String> {
        let mut log = vec![];
        if self.job.as_ref().is_some_and(|j| j.is_finished()) {
            let (wait, l) = self.job.take().unwrap().join().unwrap_or((60, vec!["resources: exchange failed".into()]));
            self.wait_until = now + wait;
            log = l;
        }
        if self.job.is_some() || now < self.wait_until {
            return log;
        }
        let Some(active) = enrollment::active(install).ok().flatten().filter(|a| enrollment::speaks(a, snap)) else {
            self.wait_until = now + IDLE;
            return log;
        };
        let install = install.to_path_buf();
        self.job = Some(std::thread::spawn(move || {
            let mut w = Https { browser: false };
            let now = w.now();
            match exchange(&install, &active, &mut w, now) {
                Ok((log, more)) => (if more { SPACING } else { IDLE }, log),
                Err(e) => (60, vec![format!("resources: {e}")]),
            }
        }));
        log
    }
}

/// The scope a host grant names: `project` entries are project IDs or registered names.
pub struct Scope {
    pub host_policy: bool,
    pub projects: Vec<String>,
    pub runtimes: Vec<String>,
    pub hours: u64,
}

/// `service resources --grant`: add or replace one host grant for the active connection.
pub fn grant(install: &Path, grant_id: &str, actor: &str, scope: &Scope, by: &str, now: u64) -> Res<Grant> {
    if !id(grant_id) || !id(actor) {
        return Err("grant and actor IDs: 1-64 of A-Z a-z 0-9 _ -".into());
    }
    if !(1..=MAX_HOURS).contains(&scope.hours) {
        return Err(format!("--hours: 1..{MAX_HOURS}"));
    }
    if let Some(rt) = scope.runtimes.iter().find(|r| !RUNTIMES.contains(&r.as_str())) {
        return Err(format!("unknown runtime {rt}"));
    }
    if scope.projects.is_empty() && !scope.host_policy {
        return Err("name a --project or --host-policy".into());
    }
    let a = enrollment::active(install)?.ok_or("no active DashQ connection: run taskq enroll first")?;
    let (Some(inst), Some(generation)) = (a["installation"].as_str(), a["generation"].as_u64()) else {
        return Err("connection has no installation or generation".into());
    };
    let names: BTreeMap<String, String> = crate::operator::projects(install).into_iter().map(|(n, r)| (r.display().to_string(), n)).collect();
    with(install, |s| {
        sync(s, install, now)?;
        let mut ids = vec![];
        for x in &scope.projects {
            let found: Vec<&String> = s.projects.iter().filter(|(k, p)| *k == x || names.get(&p.root) == Some(x)).map(|(k, _)| k).collect();
            match found[..] {
                [one] => ids.push(one.clone()),
                [] => return Err(format!("unknown project {x}: see taskq service resources")),
                _ => return Err(format!("project name {x} is ambiguous: give its ID")),
            }
        }
        ids.sort();
        ids.dedup();
        let mut runtimes = scope.runtimes.clone();
        runtimes.sort();
        runtimes.dedup();
        let g = Grant { actor_id: actor.into(), installation_id: inst.into(), generation, project_ids: ids, runtime_ids: runtimes,
            host_policy: scope.host_policy, expires_at: now + scope.hours * 3600, revoked: false, by: by.into(), at: now };
        s.grants.insert(grant_id.into(), g.clone());
        Ok(g)
    })
}

/// `service resources --revoke`: the grant stays, revoked, so its ID is never silently reissued.
pub fn revoke(install: &Path, grant_id: &str) -> Res<()> {
    with(install, |s| match s.grants.get_mut(grant_id) {
        Some(g) => { g.revoked = true; Ok(()) }
        None => Err(format!("no grant {grant_id}")),
    })
}

/// `service resources`: projects with IDs and revisions, grants and operation receipts.
pub fn report(install: &Path, now: u64) -> Res<Vec<String>> {
    let names: BTreeMap<String, String> = crate::operator::projects(install).into_iter().map(|(n, r)| (r.display().to_string(), n)).collect();
    let conn = enrollment::active(install).ok().flatten();
    with(install, |s| {
        prepare(s, install, now)?;
        let mut out = vec![match &conn {
            Some(a) => format!("Connection: {} generation {}", a["destination"].as_str().unwrap_or("?"), a["generation"]),
            None => "Connection: none (grants need an active DashQ connection)".into(),
        }];
        for (k, p) in &s.projects {
            let name = names.get(&p.root).map_or("?", String::as_str);
            out.push(format!("Project {name}: {k} revision {}{}", p.revision, if p.policy.is_null() { " (config unreadable)" } else { "" }));
        }
        for (k, g) in &s.grants {
            let state = if g.revoked { "revoked".into() } else if now >= g.expires_at { "expired".into() } else { format!("expires {}", crate::board::stamp(g.expires_at as i64)) };
            let scope = [g.host_policy.then(|| "host".to_string()), (!g.project_ids.is_empty()).then(|| format!("projects {}", g.project_ids.join(" "))),
                (!g.runtime_ids.is_empty()).then(|| format!("runtimes {}", g.runtime_ids.join(" ")))].into_iter().flatten().collect::<Vec<_>>().join(", ");
            out.push(format!("Grant {k}: actor {} generation {}, {scope}; {state}", g.actor_id, g.generation));
        }
        let mut states: BTreeMap<String, usize> = BTreeMap::new();
        for o in s.ops.values() {
            *states.entry(o.receipt["reason"].as_str().or(o.receipt["state"].as_str()).unwrap_or("?").into()).or_default() += 1;
        }
        out.push(if states.is_empty() { "Operations: none".into() } else { format!("Operations: {}", states.iter().map(|(k, n)| format!("{n} {k}")).collect::<Vec<_>>().join(", ")) });
        Ok(out)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const INST: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn fix() -> Value {
        serde_json::from_str(include_str!("../tests/fixtures/resource-policy-v1.json")).unwrap()
    }

    /// DashQ stand-in: queued replies (none: offline) and the exchange bodies it was sent; `at`: the host clock after
    /// the POST; `cut`: a connection file the host replaces while the POST is in flight.
    struct Dash { replies: Vec<(u16, Value)>, posts: Vec<Value>, at: u64, cut: Option<(PathBuf, String)> }
    impl Wire for Dash {
        fn post(&mut self, dest: &str, route: &str, auth: Option<&str>, body: &Value) -> Result<(u16, Value), String> {
            assert_eq!((dest, route, auth), ("https://x/machine", EXCHANGE, Some("c")));
            self.posts.push(body.clone());
            if let Some((path, text)) = self.cut.take() { fs::write(path, text).unwrap() }
            if self.replies.is_empty() { return Err("offline".into()) }
            Ok(self.replies.remove(0))
        }
        fn now(&self) -> u64 { self.at }
        fn sleep(&mut self, _: u64) {}
        fn show(&mut self, _: &str, _: &str) {}
    }

    struct T { base: PathBuf, install: PathBuf, root: PathBuf, me: String }

    /// One registered project with ID `project-1` at revision `r1` and the fixture's host grant.
    fn setup(name: &str) -> T {
        let base = std::env::temp_dir().join(format!("taskq-resources-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let (install, root) = (base.join("install"), base.join("proj"));
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&install).unwrap();
        fs::write(root.join("taskq.json"), "{\n  \"repo\": \"o/r\",\n  \"limits\": {\"codex\": 2},\n  \"custom\": {\"keep\": true}\n}\n").unwrap();
        fs::write(install.join("projects"), format!("{}\n", root.display())).unwrap();
        with(&install, |s| {
            sync(s, &install, 0)?;
            let (_, mut p) = s.projects.pop_first().unwrap();
            p.revision = "r1".into();
            s.projects.insert("project-1".into(), p);
            let mut g = fix()["grant"].clone();
            g.as_object_mut().unwrap().remove("grant_id");
            g["by"] = json!("test");
            g["at"] = json!(0);
            s.grants.insert("grant-1".into(), serde_json::from_value(g).unwrap());
            Ok(())
        }).unwrap();
        let me = crate::load(&root).unwrap().0.me;
        fs::create_dir_all(enrollment::dir(&install)).unwrap();
        fs::write(conn_file(&install), conn(Some(active())).to_string()).unwrap();
        T { base, install, root, me }
    }

    fn active() -> Value {
        json!({"installation": INST, "generation": 1, "destination": "https://x/machine", "credential": "c"})
    }

    // H-A2: legacy remote profiles cannot report saved after writing an ignored project fallback.
    #[test]
    fn host_policy_refuses_legacy_remote_project_writes() {
        let t=setup("host-profile");let before=config(&t);
        let host=json!({"limits":{"codex":2},"owner_field":"preserve"}).to_string();
        fs::write(t.install.join("host.json"),&host).unwrap();
        let fence=Fence {clock:&||100,live:&||true};
        with(&t.install,|s|decide(&t.install,s,&submit("host-profile",json!({"runtime_limit":0})),(INST,1),true,100,&fence)).unwrap();
        assert_eq!(receipt_of(&t,"host-profile")["state"],"rejected");
        assert_eq!(receipt_of(&t,"host-profile")["reason"],"unsupported");
        assert_eq!(config(&t),before);assert_eq!(fs::read_to_string(t.install.join("host.json")).unwrap(),host);
        fs::remove_dir_all(t.base).unwrap();
    }

    // H-A2: reusing the bounded store does not export local receipts or grant remote control over their IDs.
    #[test]
    fn local_capacity_receipts_never_enter_the_remote_exchange() {
        let t=setup("local-isolation");
        fs::write(t.install.join("host.json"),"{\"limits\":{\"codex\":2}}").unwrap();
        let (capture,guard)=crate::profile::tests::collecting(&t.install);
        let p=crate::host::Policy::read(&t.install).unwrap();let now=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        let request=json!({"id":"local-op","issued_at":now,"expires_at":now+300,"ops":[{"target":{"installation_id":null,"generation":1,"project_id":null,"runtime_id":"codex"},
            "policy_schema":"taskq.host-capacity/1","expected_revision":p.revision().unwrap(),"changes":{"runtime_limit":3}}]});
        assert_eq!(crate::host::local_capacity(&t.install,&p.me,&request).unwrap()["state"],"saved");
        let before=read(&t.install).unwrap().ops["local-op"].clone();let policy=fs::read(t.install.join("host.json")).unwrap();
        let mut cancel=fix()["cancel"].clone();cancel["op_id"]=json!("local-op");
        let (sent,log)=go(&t,&[submit("local-op",json!({"runtime_limit":0}))],&[cancel],100);
        assert!(sent.is_empty(),"local receipt leaked into DashQ: {sent:?}");
        assert_eq!(read(&t.install).unwrap().ops["local-op"],before);
        assert_eq!(fs::read(t.install.join("host.json")).unwrap(),policy);
        assert_eq!(log.len(),2,"remote cancel and ID reuse must both be explicitly refused");
        for (key,id) in [("local_restart","local-restart"),("local_stop","local-stop")] {
            with(&t.install,|state| {
                state.ops.insert(id.into(),Op {bound:json!({(key):{"request":{"id":id}}}),
                    receipt:json!({"id":id,"state":"queued"}),expires_at:0,saved_at:Some(0),writing:false,sent:true});Ok(())
            }).unwrap();
            let original=read(&t.install).unwrap().ops[id].clone();
            let mut cancel=fix()["cancel"].clone();cancel["op_id"]=json!(id);
            let (sent,log)=go(&t,&[submit(id,json!({"runtime_limit":0}))],&[cancel],100);
            assert!(sent.is_empty());assert_eq!(log.len(),2);assert_eq!(read(&t.install).unwrap().ops[id],original);
        }
        capture.flush().unwrap();drop(guard);drop(capture);fs::remove_dir_all(t.base).unwrap();
    }

    fn conn_file(install: &Path) -> PathBuf {
        enrollment::dir(install).join("connection.json")
    }

    fn conn(active: Option<Value>) -> Value {
        json!({"installation": INST, "pending": null, "active": active, "revoke": []})
    }

    fn submit(op: &str, changes: Value) -> Value {
        let mut v = fix()["submit"].clone();
        v["op_id"] = json!(op);
        v["changes"] = changes;
        v
    }

    /// One exchange at `now` whose reply delivers `ops` and `cancels`: the receipts sent and the log.
    fn go(t: &T, ops: &[Value], cancels: &[Value], now: u64) -> (Vec<Value>, Vec<String>) {
        let mut d = Dash { replies: vec![(200, json!({"schema": SCHEMA, "server_at": now, "operations": ops, "cancellations": cancels}))], posts: vec![], at: 0, cut: None };
        let (log, _) = exchange(&t.install, &active(), &mut d, now).unwrap();
        (d.posts.first().map(|p| p["receipts"].as_array().unwrap().clone()).unwrap_or_default(), log)
    }

    fn receipt_of(t: &T, op: &str) -> Value {
        read(&t.install).unwrap().ops.get(op).unwrap_or_else(|| panic!("{op} not recorded")).receipt.clone()
    }

    fn config(t: &T) -> String {
        fs::read_to_string(t.root.join("taskq.json")).unwrap()
    }

    // Chain: DashQ submit -> exchange -> host grant check -> canonical capacity write -> service status readback ->
    // receipt. Risk: a saved write reported applied before the service runs it, or the write losing other keys.
    #[test]
    fn saved_turns_applied_only_on_a_fresh_matching_readback() {
        let t = setup("apply");
        let (sent, log) = go(&t, &[submit("op-1", json!({"runtime_limit": 0}))], &[], 100);
        assert_eq!((sent.len(), log), (0, vec!["resources: op-1 saved".to_string()]));
        let v: Value = serde_json::from_str(&config(&t)).unwrap();
        assert_eq!((&v["repo"], &v["limits"], &v["custom"]), (&json!("o/r"), &json!({"codex": 2}), &json!({"keep": true})), "unrelated keys kept");
        assert_eq!(v["capacity_hosts"][&t.me]["limits"]["codex"], 0);
        assert!(t.root.join(".taskq/poke").exists(), "the service reloads");
        let s = read(&t.install).unwrap();
        let p = s.projects["project-1"].clone();
        let r = receipt_of(&t, "op-1");
        assert_ne!(p.revision, "r1");
        assert_eq!((&r["state"], &r["saved_revision"], &r["effective_revision"]), (&json!("saved"), &json!(p.revision), &Value::Null));
        let mut old = p.policy.clone();
        old["limits"]["codex"] = json!(2);
        let status = |at: u64, host: &str, cap: &Value| fs::write(t.root.join(".taskq/status.json"), json!({"at": at, "host": host, "capacity": cap}).to_string()).unwrap();
        let (sent, _) = go(&t, &[], &[], 101);
        assert_eq!((sent.len(), &sent[0]["state"]), (1, &json!("saved")), "no status yet: saved");
        for (at, host, cap, now) in [(90, t.me.as_str(), &p.policy, 110), (110, t.me.as_str(), &old, 110), (110, "other", &p.policy, 110), (110, t.me.as_str(), &p.policy, 171)] {
            status(at, host, cap);
            go(&t, &[], &[], now);
            assert_eq!(receipt_of(&t, "op-1")["state"], "saved", "stale, old, other host or old readback at {now}");
        }
        status(110, &t.me, &p.policy);
        let (sent, _) = go(&t, &[], &[], 111);
        let r = receipt_of(&t, "op-1");
        assert_eq!((&r["state"], &r["effective_revision"], &r["observed_at"]), (&json!("applied"), &json!(p.revision), &json!(110)));
        assert_eq!(sent, [r], "the applied receipt goes out in the same exchange");
        assert!(go(&t, &[], &[], 112).0.is_empty(), "and only once");
        fs::remove_dir_all(&t.base).unwrap();
    }

    // Chain: hand edit or duplicate/lost reply or reused op ID -> exchange -> decide. Risk: an operation writing over
    // a newer config, writing twice, or a reused ID rebinding to another payload.
    #[test]
    fn conflicts_duplicates_and_mismatches_never_write_again() {
        let t = setup("dedup");
        fs::write(t.root.join("taskq.json"), config(&t).replace("\"codex\": 2", "\"codex\": 3")).unwrap();
        let edited = config(&t);
        go(&t, &[submit("op-0", json!({"runtime_limit": 0}))], &[], 100);
        assert_eq!((receipt_of(&t, "op-0")["reason"].as_str(), config(&t)), (Some("conflict"), edited), "a hand edit gets a new revision");
        let mut op = submit("op-1", json!({"runtime_limit": 1}));
        op["expected_revision"] = json!(read(&t.install).unwrap().projects["project-1"].revision);
        go(&t, std::slice::from_ref(&op), &[], 100);
        let (saved, after) = (receipt_of(&t, "op-1"), config(&t));
        assert_eq!(saved["state"], "saved");
        // the reply was lost: the next exchange resends the receipt, DashQ redelivers the same submit
        let mut off = Dash { replies: vec![], posts: vec![], at: 0, cut: None };
        assert!(exchange(&t.install, &active(), &mut off, 101).is_err());
        let (sent, _) = go(&t, &[op.clone(), op.clone()], &[], 102);
        assert!(sent.contains(&saved), "unsent receipts go again");
        let (sent, _) = go(&t, &[], &[], 103);
        assert_eq!((sent, receipt_of(&t, "op-1"), config(&t)), (vec![saved.clone()], saved.clone(), after.clone()), "one write, the same receipt");
        let mut other = op.clone();
        other["changes"] = json!({"runtime_limit": 0});
        let (_, log) = go(&t, &[other], &[], 104);
        assert_eq!(log, ["resources: op-1 reused with another payload; original kept"]);
        assert_eq!((receipt_of(&t, "op-1"), config(&t)), (saved, after));
        assert!(go(&t, &[], &[], 105).0.is_empty(), "a mismatch sends nothing");
        fs::remove_dir_all(&t.base).unwrap();
    }

    // Chain: DashQ-approved submit outside the host grant -> exchange -> authorize. Risk: DashQ's grant alone, another
    // host or generation, a revoked or expired grant, or a path or privilege change reaching taskq.json.
    #[test]
    fn grant_scope_typing_and_lifetime_refuse_before_any_write() {
        let t = setup("scope");
        with(&t.install, |s| {
            let mut g = s.grants["grant-1"].clone();
            g.revoked = true;
            s.grants.insert("grant-r".into(), g.clone());
            // keeps the exchange going after grant-1 expires
            s.grants.insert("grant-2".into(), Grant { revoked: false, expires_at: 10_000, ..g });
            Ok(())
        }).unwrap();
        let before = config(&t);
        let rt = |c: Value| submit("x", json!({"runtime_limit": c}));
        let set = |mut v: Value, path: &[&str], x: Value| {
            let last = path.len() - 1;
            path[..last].iter().fold(&mut v, |v, k| &mut v[*k])[path[last]] = x;
            v
        };
        let cases = [
            (set(rt(json!(0)), &["grant_id"], json!("grant-x")), 100, "unauthorized"),
            (set(rt(json!(0)), &["actor_id"], json!("operator-2")), 100, "unauthorized"),
            (set(rt(json!(0)), &["grant_id"], json!("grant-r")), 100, "revoked"),
            (set(set(rt(json!(0)), &["issued_at"], json!(950)), &["expires_at"], json!(1200)), 1000, "revoked"),
            (set(rt(json!(0)), &["target", "generation"], json!(2)), 100, "scope"),
            (set(rt(json!(0)), &["target", "installation_id"], json!("b".repeat(64))), 100, "scope"),
            (set(rt(json!(0)), &["target", "project_id"], json!("project-2")), 100, "scope"),
            (set(rt(json!(0)), &["target", "runtime_id"], json!("claude")), 100, "scope"),
            (set(set(submit("x", json!({"maximum": 2})), &["target", "project_id"], Value::Null), &["target", "runtime_id"], Value::Null), 100, "scope"),
            (submit("x", json!({"runtime_limit": 0, "path": "/etc"})), 100, "invalid"),
            (submit("x", json!({"privileged": true})), 100, "invalid"),
            (set(rt(json!(0)), &["policy_schema"], json!("taskq.capacity/2")), 100, "unsupported"),
            (rt(Value::Null), 100, "unsupported"),
            (set(rt(json!(0)), &["expected_revision"], json!("r0")), 100, "conflict"),
        ];
        for (i, (mut op, now, want)) in cases.into_iter().enumerate() {
            op["op_id"] = json!(format!("op-{i}"));
            go(&t, &[op], &[], now);
            assert_eq!((receipt_of(&t, &format!("op-{i}"))["reason"].as_str(), config(&t)), (Some(want), before.clone()), "case {i}");
        }
        // a runtime without a project is no valid target: nothing to answer, nothing recorded (legacy_runtime_scope)
        let (_, log) = go(&t, &[set(submit("op-t", json!({"runtime_limit": 0})), &["target", "project_id"], Value::Null)], &[], 100);
        assert_eq!((log, config(&t)), (vec!["resources: dropped an operation without a valid ID or target".to_string()], before.clone()));
        assert!(!read(&t.install).unwrap().ops.contains_key("op-t"));
        // clock: DashQ's time 100 s away; the deadline on either clock
        let mut d = Dash { replies: vec![(200, json!({"schema": SCHEMA, "server_at": 200, "operations": [submit("op-c", json!({"runtime_limit": 0}))], "cancellations": []}))], posts: vec![], at: 0, cut: None };
        exchange(&t.install, &active(), &mut d, 100).unwrap();
        assert_eq!(receipt_of(&t, "op-c")["reason"], "clock_unknown");
        go(&t, &[submit("op-e", json!({"runtime_limit": 0}))], &[], 400);
        go(&t, &[submit("op-e", json!({"runtime_limit": 0}))], &[], 401);
        assert_eq!((receipt_of(&t, "op-e")["state"].as_str(), config(&t)), (Some("expired"), before.clone()), "redelivered after expiry");
        // a host-policy grant still has no host-wide store; capacity/1 is the Fixed authority only
        with(&t.install, |s| { s.grants.get_mut("grant-1").unwrap().host_policy = true; Ok(()) }).unwrap();
        go(&t, &[set(set(submit("op-h", json!({"maximum": 2})), &["target", "project_id"], Value::Null), &["target", "runtime_id"], Value::Null)], &[], 100);
        assert_eq!(receipt_of(&t, "op-h")["reason"], "unsupported");
        fs::write(t.root.join("taskq.json"), before.replace("\"repo\"", "\"admission\": {\"mode\": \"auto\"},\n  \"repo\"")).unwrap();
        let auto = config(&t);
        go(&t, &[], &[], 100);
        let mut op = set(submit("op-f", json!({"runtime_limit": 0})), &["policy_schema"], json!("taskq.capacity/1"));
        op["expected_revision"] = json!(read(&t.install).unwrap().projects["project-1"].revision);
        go(&t, &[op], &[], 100);
        assert_eq!((receipt_of(&t, "op-f")["reason"].as_str(), config(&t)), (Some("unsupported"), auto));
        // the project leaves the registry: its ID goes for good
        fs::write(t.install.join("projects"), "").unwrap();
        go(&t, &[submit("op-u", json!({"runtime_limit": 0}))], &[], 100);
        assert_eq!(receipt_of(&t, "op-u")["reason"], "scope");
        assert!(read(&t.install).unwrap().projects.is_empty());
        fs::remove_dir_all(&t.base).unwrap();
    }

    // Chain: revoke -> exchange. Risk: a revoked host still exchanging, or a pending operation applying after revoke.
    #[test]
    fn revoked_grant_stops_the_exchange_and_pending_writes() {
        let t = setup("revoke");
        revoke(&t.install, "grant-1").unwrap();
        let mut d = Dash { replies: vec![], posts: vec![], at: 0, cut: None };
        assert_eq!(exchange(&t.install, &active(), &mut d, 100).unwrap(), (vec![], false));
        assert!(d.posts.is_empty(), "no active grant, nothing unsent: no exchange");
        with(&t.install, |s| {
            let mut g = s.grants["grant-1"].clone();
            g.revoked = false;
            s.grants.insert("grant-2".into(), g);
            Ok(())
        }).unwrap();
        let before = config(&t);
        go(&t, &[submit("op-1", json!({"runtime_limit": 0}))], &[], 100);
        assert_eq!((receipt_of(&t, "op-1")["reason"].as_str(), config(&t)), (Some("revoked"), before));
        assert!(revoke(&t.install, "grant-9").is_err());
        fs::remove_dir_all(&t.base).unwrap();
    }

    // Chain: cancellation and submit racing through the exchange; crash between marker and write. Risk: a cancelled
    // operation writing later, a completed write rolled back, or an interrupted write repeated after restart.
    #[test]
    fn cancel_races_and_interrupted_writes_settle_once() {
        let t = setup("cancel");
        let before = config(&t);
        let mut cancel = fix()["cancel"].clone();
        go(&t, &[submit("op-1", json!({"runtime_limit": 0}))], &[cancel.clone()], 100);
        assert_eq!((receipt_of(&t, "op-1")["state"].as_str(), config(&t)), (Some("cancelled"), before.clone()), "cancel wins");
        go(&t, &[submit("op-1", json!({"runtime_limit": 0}))], &[], 101);
        assert_eq!((receipt_of(&t, "op-1")["state"].as_str(), config(&t)), (Some("cancelled"), before.clone()), "late delivery cannot write");
        go(&t, &[submit("op-2", json!({"runtime_limit": 0}))], &[], 102);
        cancel["op_id"] = json!("op-2");
        go(&t, &[], &[cancel], 103);
        assert_eq!(receipt_of(&t, "op-2")["state"], "saved", "apply wins: never rolled back");
        // the marker was durable, the write may or may not have happened: unknown, and never written again
        let after = config(&t);
        with(&t.install, |s| { s.ops.get_mut("op-2").unwrap().writing = true; Ok(()) }).unwrap();
        let (sent, _) = go(&t, &[submit("op-2", json!({"runtime_limit": 0}))], &[], 104);
        assert_eq!((sent.len(), &sent[0]["state"]), (1, &json!("unknown")));
        assert_eq!((receipt_of(&t, "op-2")["state"].as_str(), config(&t)), (Some("unknown"), after));
        fs::remove_dir_all(&t.base).unwrap();
    }

    // Chain: exchange POST in flight -> disconnect, new generation or clock past a deadline -> decide -> write.
    // Risk: an operation applying on a connection that is no longer active, or after its or its grant's deadline
    // because the time was read before the POST, the lock wait or the marker save.
    #[test]
    fn races_during_the_exchange_never_write() {
        let t = setup("race");
        let before = config(&t);
        let reply = |op: &str| (200, json!({"schema": SCHEMA, "server_at": 399, "operations": [submit(op, json!({"runtime_limit": 0}))], "cancellations": []}));
        let mut gen2 = active();
        gen2["generation"] = json!(2);
        for (op, cut) in [("op-d", conn(None)), ("op-g", conn(Some(gen2)))] {
            let mut d = Dash { replies: vec![reply(op)], posts: vec![], at: 0, cut: Some((conn_file(&t.install), cut.to_string())) };
            let (log, _) = exchange(&t.install, &active(), &mut d, 399).unwrap();
            assert_eq!(log, ["resources: connection changed during the exchange; 1 operations not taken"]);
            assert!(!read(&t.install).unwrap().ops.contains_key(op) && config(&t) == before, "{op}");
            fs::write(conn_file(&t.install), conn(Some(active())).to_string()).unwrap();
        }
        // started at 399, the operation expires at 400, the reply lands at 401
        let mut d = Dash { replies: vec![reply("op-e")], posts: vec![], at: 401, cut: None };
        exchange(&t.install, &active(), &mut d, 399).unwrap();
        assert_eq!((receipt_of(&t, "op-e")["state"].as_str(), config(&t)), (Some("expired"), before.clone()));
        // the clock passes the operation's or the grant's deadline, or the connection goes, during the marker save
        let mut late = submit("op-l", json!({"runtime_limit": 0}));
        late["issued_at"] = json!(950);
        late["expires_at"] = json!(1200);
        let cases = [(submit("op-x", json!({"runtime_limit": 0})), 399, 400, true, "expired"), (late, 990, 1000, true, "revoked"), (submit("op-c", json!({"runtime_limit": 0})), 100, 100, false, "scope")];
        for (v, now, then, up, want) in cases {
            let op = v["op_id"].as_str().unwrap().to_string();
            let fence = Fence { clock: &|| then, live: &|| up };
            with(&t.install, |s| decide(&t.install, s, &v, (INST, 1), true, now, &fence)).unwrap();
            let r = receipt_of(&t, &op);
            assert_eq!((r["reason"].as_str().or(r["state"].as_str()), config(&t)), (Some(want), before.clone()), "{op}");
        }
        fs::remove_dir_all(&t.base).unwrap();
    }

    // Chain: exchange POST in flight -> `service uninstall` -> decide -> write. Risk: a removed project still taking
    // a write because the registration was read before the POST or the lock wait, or its ID keeping authority.
    #[test]
    fn unregister_during_the_exchange_never_writes() {
        let t = setup("unregister");
        let before = config(&t);
        let reply = |op: &str| (200, json!({"schema": SCHEMA, "server_at": 399, "operations": [submit(op, json!({"runtime_limit": 0}))], "cancellations": []}));
        let mut d = Dash { replies: vec![reply("op-u")], posts: vec![], at: 0, cut: Some((t.install.join("projects"), String::new())) };
        exchange(&t.install, &active(), &mut d, 399).unwrap();
        assert_eq!((receipt_of(&t, "op-u")["reason"].as_str(), config(&t)), (Some("scope"), before.clone()));
        // the locked path drops the ID with the registration; registering the root again never revives it
        fs::write(t.install.join("projects"), format!("{}\n", t.root.display())).unwrap();
        unregister(&t.install, std::slice::from_ref(&t.root), || fs::write(t.install.join("projects"), "").map_err(|e| e.to_string())).unwrap();
        assert!(read(&t.install).unwrap().projects.is_empty());
        fs::write(t.install.join("projects"), format!("{}\n", t.root.display())).unwrap();
        go(&t, &[submit("op-v", json!({"runtime_limit": 0}))], &[], 399);
        assert_eq!((receipt_of(&t, "op-v")["reason"].as_str(), config(&t)), (Some("scope"), before));
        assert!(!read(&t.install).unwrap().projects.contains_key("project-1"));
        fs::remove_dir_all(&t.base).unwrap();
    }

    // H-A5: a failed registry commit cannot revive a retired scope on the next exchange.
    #[test]
    fn failed_registry_commit_keeps_scope_retired() {
        let t=setup("unregister-failed");
        let before=config(&t);
        let result=unregister(&t.install,std::slice::from_ref(&t.root),||Err("fixture registry write failed".into()));
        assert!(result.unwrap_err().contains("scope retired"));
        assert!(!read(&t.install).unwrap().projects.contains_key("project-1"));
        go(&t,&[submit("old-scope",json!({"runtime_limit":0}))],&[],100);
        assert_eq!(receipt_of(&t,"old-scope")["reason"],"scope");
        assert_eq!(config(&t),before);
        let state=read(&t.install).unwrap();
        assert!(!state.projects.contains_key("project-1"));
        assert!(state.grants.contains_key("grant-1") && state.ops.contains_key("old-scope"));
        fs::remove_dir_all(t.base).unwrap();
    }

    // H-A5: process death after scope retirement and before registry replacement cannot revive old grants.
    #[test]
    fn retired_scope_survives_process_death_before_registry_commit() {
        let t=setup("registry-crash");
        let registry=fs::read(t.install.join("projects")).unwrap();
        let signal=t.base.join("retired");
        let mut child=std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact","resources::tests::retire_fixture","--ignored"])
            .env("TASKQ_RESOURCE_RETIRE_FIXTURE",&t.base)
            .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().unwrap();
        let until=std::time::Instant::now()+std::time::Duration::from_secs(5);
        while !signal.exists() && child.try_wait().unwrap().is_none() && std::time::Instant::now()<until {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let retired=fs::read_to_string(signal).unwrap_or_default();
        let _=child.kill();child.wait().unwrap();
        assert_eq!(retired,"retired","fixture did not reach the crash boundary");
        assert_eq!(fs::read(t.install.join("projects")).unwrap(),registry);
        assert!(!read(&t.install).unwrap().projects.contains_key("project-1"));
        go(&t,&[submit("after-crash",json!({"runtime_limit":0}))],&[],100);
        assert_eq!(receipt_of(&t,"after-crash")["reason"],"scope");
        fs::remove_dir_all(t.base).unwrap();
    }

    #[test]
    #[ignore="owned subprocess for retired_scope_survives_process_death_before_registry_commit"]
    fn retire_fixture() {
        let base=PathBuf::from(std::env::var_os("TASKQ_RESOURCE_RETIRE_FIXTURE").unwrap());
        unregister(&base.join("install"),&[base.join("proj")],|| {
            crate::platform::write_atomic(&base.join("retired"),"retired")?;
            loop {std::thread::park();}
        }).unwrap();
    }

    // H-A5: unknown membership is not an empty authoritative list and must not erase cached scopes.
    #[test]
    fn unknown_registry_preserves_resource_authority() {
        let t=setup("registry-unknown");
        let registry=fs::read(t.install.join("projects")).unwrap();
        let state=fs::read(dir(&t.install).join(FILE)).unwrap();
        for fault in ["missing","invalid","pending"] {
            match fault {
                "missing"=>fs::remove_file(t.install.join("projects")).unwrap(),
                "invalid"=>fs::write(t.install.join("projects"),b"\xff").unwrap(),
                _=>fs::write(t.install.join("projects.pending"),"interrupted").unwrap(),
            }
            assert!(report(&t.install,100).is_err(),"{fault} membership silently accepted");
            let mut wire=Dash {replies:vec![],posts:vec![],at:0,cut:None};
            assert!(exchange(&t.install,&active(),&mut wire,100).is_err());
            assert!(wire.posts.is_empty());
            assert_eq!(fs::read(dir(&t.install).join(FILE)).unwrap(),state,"{fault} destroyed cached authority");
            fs::write(t.install.join("projects"),&registry).unwrap();
            if fault=="pending" {fs::remove_file(t.install.join("projects.pending")).unwrap();}
        }
        fs::write(dir(&t.install).join("state.json.pending"),"interrupted retirement").unwrap();
        let mut wrote=false;
        assert!(unregister(&t.install,std::slice::from_ref(&t.root),|| {wrote=true;Ok(())}).is_err());
        assert!(!wrote,"unknown resource retirement cannot commit registry removal");
        assert_eq!(fs::read(dir(&t.install).join(FILE)).unwrap(),state);
        assert_eq!(fs::read(dir(&t.install).join("state.json.pending")).unwrap(),b"interrupted retirement");
        fs::remove_dir_all(t.base).unwrap();
    }

    /// DashQ stand-in for connection calls: pairing activates generation 2, a revoke succeeds, ingest answers 401.
    struct Peer;
    impl Wire for Peer {
        fn post(&mut self, _: &str, route: &str, _: Option<&str>, _: &Value) -> Result<(u16, Value), String> {
            Ok(match route {
                "/connect/device" => (200, json!({"protocol": 1, "device_code": "d", "user_code": "U-1", "verification_uri": "https://x/v", "expires_in": 600, "interval": 1})),
                "/connect/token" => (200, json!({"protocol": 1, "installation": INST, "product": "taskq", "credential": "c2", "generation": 2})),
                "/connect/ack" => (200, json!({"protocol": 1, "state": "active", "generation": 2})),
                "/ingest" => (401, json!({"protocol": 1, "error": "revoked"})),
                _ => (200, json!({"protocol": 1})),
            })
        }
        fn now(&self) -> u64 { 0 }
        fn sleep(&mut self, _: u64) {}
        fn show(&mut self, _: &str, _: &str) {}
    }

    // Chain: decide's final connection check -> disconnect, pairing activation or DashQ's 401 revoke -> write.
    // Risk: the connection going between the final check and the write, so an operation writes after it went.
    #[test]
    fn connection_changes_wait_for_the_write() {
        let t = setup("final");
        let before = config(&t);
        let changes: [(&str, fn(&Path)); 3] = [
            ("op-d", |i| { enrollment::disconnect_with(i, &mut Peer).unwrap(); }),
            ("op-p", |i| {
                let req = coreq::enroll::Request { installation: Some(INST), product: "taskq", host_label: "h", schemas: &[enrollment::SCHEMA], scope: None, capabilities: &[] };
                coreq::enroll::pair(&enrollment::dir(i), "https://x/machine", &req, &mut Fenced::new(i, Peer)).unwrap();
            }),
            ("op-r", |i| {
                let rec = coreq::enroll::stamp(&enrollment::dir(i), enrollment::SCHEMA, "e-1", 0, json!({})).unwrap();
                assert!(coreq::enroll::deliver(&enrollment::dir(i), &[rec], &mut Default::default(), &mut Fenced::new(i, Peer)).is_err());
            }),
        ];
        for (op, change) in changes {
            fs::write(t.root.join("taskq.json"), &before).unwrap();
            fs::write(conn_file(&t.install), conn(Some(active())).to_string()).unwrap();
            with(&t.install, |s| { sync(s, &t.install, 0)?; s.projects.get_mut("project-1").unwrap().revision = "r1".into(); Ok(()) }).unwrap();
            let seen = std::cell::RefCell::new(None);
            // the final read sees the operation's connection; the change starts right after it
            let live = || {
                let up = enrollment::active(&t.install).unwrap() == Some(active());
                let (install, root) = (t.install.clone(), t.root.clone());
                seen.replace(Some(std::thread::spawn(move || { change(&install); fs::read_to_string(root.join("taskq.json")).unwrap() })));
                std::thread::sleep(std::time::Duration::from_millis(500));
                up
            };
            let fence = Fence { clock: &|| 100, live: &live };
            with(&t.install, |s| decide(&t.install, s, &submit(op, json!({"runtime_limit": 0})), (INST, 1), true, 100, &fence)).unwrap();
            let after = seen.take().expect("final check ran").join().unwrap();
            assert_eq!(receipt_of(&t, op)["state"], "saved", "{op}");
            assert_ne!(after, before, "{op}: the connection changed before the write landed");
            assert_ne!(enrollment::active(&t.install).unwrap(), Some(active()), "{op}: the change applied after the write");
        }
        fs::remove_dir_all(&t.base).unwrap();
    }

    // Chain: exchange reply -> bounds -> decide. Risk: an oversized, malformed or throttled reply applying anything,
    // or a host sending unbounded receipts.
    #[test]
    fn exchange_holds_v1_bounds() {
        let t = setup("bounds");
        let ops: Vec<Value> = (0..9).map(|i| submit(&format!("op-{i}"), json!({"runtime_limit": 0}))).collect();
        let reply = |ops: &[Value], extra: bool| {
            let mut r = json!({"schema": SCHEMA, "server_at": 100, "operations": ops, "cancellations": []});
            if extra { r["debug"] = json!(1) }
            r
        };
        for r in [(200, reply(&ops, false)), (200, reply(&ops[..1], true)), (200, json!({"schema": SCHEMA}))] {
            let mut d = Dash { replies: vec![r], posts: vec![], at: 0, cut: None };
            assert_eq!(exchange(&t.install, &active(), &mut d, 100).unwrap_err(), "exchange reply invalid");
        }
        let mut d = Dash { replies: vec![(429, json!({})), (403, json!({"reason": "revoked"}))], posts: vec![], at: 0, cut: None };
        assert_eq!(exchange(&t.install, &active(), &mut d, 100).unwrap(), (vec![], true), "busy: come back after the spacing");
        assert_eq!(exchange(&t.install, &active(), &mut d, 100).unwrap_err(), "exchange refused: http 403 revoked");
        assert!(read(&t.install).unwrap().ops.is_empty(), "nothing taken from a refused reply");
        let bad: Vec<Value> = (0..10).map(|i| { let mut o = submit(&format!("u-{i}"), json!({"runtime_limit": 0})); o["grant_id"] = json!("grant-x"); o }).collect();
        go(&t, &bad[..8], &[], 100);
        assert_eq!(go(&t, &bad[8..], &[], 100).0.len(), ITEMS, "at most 8 receipts per exchange");
        assert_eq!(go(&t, &[], &[], 101).0.len(), 2);
        fs::remove_dir_all(&t.base).unwrap();
    }

    // Chain: service resources --grant/--revoke -> report; sweep and pruning.
    // Risk: a grant naming an unknown or ambiguous project, an unbounded grant, or the store lost to cleanup.
    #[test]
    fn host_grants_resolve_projects_and_survive_cleanup() {
        let t = setup("grant");
        let fresh = t.base.join("fresh");
        let mut d = Dash { replies: vec![], posts: vec![], at: 0, cut: None };
        assert_eq!(exchange(&fresh, &active(), &mut d, 0).unwrap(), (vec![], false));
        assert!(d.posts.is_empty() && !dir(&fresh).exists(), "no grant ever made: no exchange, no store");
        let scope =|projects: &[&str], runtimes: &[&str], hours| Scope { host_policy: false, projects: projects.iter().map(|x| x.to_string()).collect(), runtimes: runtimes.iter().map(|x| x.to_string()).collect(), hours };
        fs::write(conn_file(&t.install), conn(None).to_string()).unwrap();
        assert!(grant(&t.install, "g", "op", &scope(&["proj"], &[], 1), "me", 0).unwrap_err().contains("no active DashQ connection"));
        fs::write(conn_file(&t.install), conn(Some(active())).to_string()).unwrap();
        for (id, s, want) in [("g", scope(&[], &[], 1), "name a --project"), ("g", scope(&["proj"], &[], 0), "--hours"), ("g", scope(&["proj"], &[], MAX_HOURS + 1), "--hours"),
            ("g", scope(&["proj"], &["vim"], 1), "unknown runtime"), ("g", scope(&["nope"], &[], 1), "unknown project"), ("g/1", scope(&["proj"], &[], 1), "grant and actor IDs")] {
            assert!(grant(&t.install, id, "op", &s, "me", 0).unwrap_err().contains(want), "{want}");
        }
        let g = grant(&t.install, "g", "op", &scope(&["proj", "project-1"], &["codex"], 2), "me", 10).unwrap();
        assert_eq!((g.project_ids, g.runtime_ids, g.installation_id.as_str(), g.generation, g.expires_at), (vec!["project-1".to_string()], vec!["codex".to_string()], INST, 1, 10 + 7200));
        let r = report(&t.install, 20).unwrap();
        assert!(r.iter().any(|l| l == "Project proj: project-1 revision r1"), "{r:?}");
        assert!(r.iter().any(|l| l.starts_with("Grant g: actor op generation 1, projects project-1, runtimes codex; expires")), "{r:?}");
        revoke(&t.install, "g").unwrap();
        assert!(report(&t.install, 20).unwrap().iter().any(|l| l == "Grant g: actor op generation 1, projects project-1, runtimes codex; revoked"));
        crate::platform::sweep(&t.install, &t.base.join("absent-shim"));
        crate::data::prune(&t.root, &t.install, "proj", &[]);
        assert!(read(&t.install).unwrap().grants.contains_key("g"), "the store survives sweep and pruning");
        // a corrupt store is kept and refuses everything
        fs::write(dir(&t.install).join(FILE), "{").unwrap();
        assert!(report(&t.install, 40).unwrap_err().contains("unreadable"));
        assert!(exchange(&t.install, &active(), &mut Dash { replies: vec![], posts: vec![], at: 0, cut: None }, 40).is_err());
        assert_eq!(fs::read_to_string(dir(&t.install).join(FILE)).unwrap(), "{");
        fs::remove_dir_all(&t.base).unwrap();
    }
}
