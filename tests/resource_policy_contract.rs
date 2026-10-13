//! Chain: shared-resource-controls resource-policy-v1. A transport-free conformance oracle;
//! not an implementation of HTTPS, durable storage, the canonical writer or admission.
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;

const SCHEMA: &str = "taskq.resources/1";
const CORPUS: &str = include_str!("fixtures/resource-policy-v1.json");

// Parse every object before Value canonicalization can erase duplicate keys.
struct UniqueJson(Value);
impl<'de> Deserialize<'de> for UniqueJson {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = UniqueJson;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result { f.write_str("JSON without duplicate keys") }
            fn visit_map<M: serde::de::MapAccess<'de>>(self, mut m: M) -> Result<Self::Value, M::Error> {
                let mut object = serde_json::Map::new();
                while let Some((key, UniqueJson(value))) = m.next_entry::<String, UniqueJson>()? {
                    if object.insert(key, value).is_some() { return Err(serde::de::Error::custom("duplicate key")); }
                }
                Ok(UniqueJson(Value::Object(object)))
            }
            fn visit_seq<S: serde::de::SeqAccess<'de>>(self, mut s: S) -> Result<Self::Value, S::Error> {
                let mut array = Vec::new();
                while let Some(UniqueJson(value)) = s.next_element()? { array.push(value); }
                Ok(UniqueJson(Value::Array(array)))
            }
            fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<Self::Value, E> { Ok(UniqueJson(json!(v))) }
            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Self::Value, E> { Ok(UniqueJson(json!(v))) }
            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Self::Value, E> { Ok(UniqueJson(json!(v))) }
            fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<Self::Value, E> { Ok(UniqueJson(json!(v))) }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> { Ok(UniqueJson(json!(v))) }
            fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> { Ok(UniqueJson(Value::Null)) }
        }
        d.deserialize_any(Visitor)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct Target {
    installation_id: String,
    generation: u64,
    project_id: Option<String>,
    runtime_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Submit {
    schema: String,
    op_id: String,
    actor_id: String,
    grant_id: String,
    target: Target,
    policy_schema: String,
    expected_revision: String,
    issued_at: u64,
    expires_at: u64,
    changes: BTreeMap<String, Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Grant {
    grant_id: String,
    actor_id: String,
    installation_id: String,
    generation: u64,
    project_ids: Vec<String>,
    runtime_ids: Vec<String>,
    host_policy: bool,
    expires_at: u64,
    revoked: bool,
}

fn id(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn count(v: &Value) -> bool {
    v.as_u64().is_some_and(|n| n <= u32::MAX as u64)
}

fn typed(v: &Value) -> Result<Submit, &'static str> {
    let r: Submit = serde_json::from_value(v.clone()).map_err(|_| "invalid")?;
    if !["installation_id", "generation", "project_id", "runtime_id"].iter().all(|k| v["target"].get(k).is_some()) {
        return Err("invalid");
    }
    if r.schema != SCHEMA || ![&r.op_id, &r.actor_id, &r.grant_id, &r.expected_revision].into_iter().all(|s| id(s))
        || r.target.installation_id.len() != 64
        || !r.target.installation_id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        || r.target.generation == 0 || r.target.project_id.as_deref().is_some_and(|s| !id(s))
        || r.target.runtime_id.as_deref().is_some_and(|s| !["claude", "codex", "codex-app"].contains(&s))
        || (r.target.runtime_id.is_some() && r.target.project_id.is_none())
        || r.expires_at <= r.issued_at || r.expires_at - r.issued_at > 300 || r.changes.is_empty()
    { return Err("invalid"); }
    let auto = match r.policy_schema.as_str() {
        "taskq.capacity/1" => false,
        "taskq.admission/1" => true,
        _ => return Err("unsupported"),
    };
    for (key, value) in &r.changes {
        let valid = match key.as_str() {
            "runtime_limit" => r.target.runtime_id.is_some() && (count(value) || (auto && value.is_null())),
            "maximum" => auto && r.target.runtime_id.is_none() && (value.is_null() || count(value)),
            "mode" => auto && r.target.runtime_id.is_none() && (value == "auto" || value == "fixed"),
            "load_gate" => r.target.project_id.is_some() && r.target.runtime_id.is_none() && value.as_object().is_some_and(|g| {
                g.len() == 3 && g.get("enabled").is_some_and(Value::is_boolean)
                    && g.get("cpu_max").and_then(Value::as_u64).is_some_and(|n| (10..=100).contains(&n))
                    && g.get("memory_min").and_then(Value::as_u64).is_some_and(|n| (1..=95).contains(&n))
            }),
            _ => false,
        };
        if !valid { return Err("invalid"); }
    }
    if r.changes.get("mode") == Some(&json!("auto")) && r.changes.get("load_gate").is_some_and(|g| g["enabled"] == false)
    { return Err("invalid"); }
    Ok(r)
}

struct Authority {
    now: u64,
    credential: &'static str,
    revoked: bool,
    registered: bool,
    supported: bool,
    generation: u64,
    grant: Grant,
    revision: String,
    operations: BTreeMap<String, (Value, Value)>,
    writes: usize,
}

impl Authority {
    fn new() -> Self {
        let corpus: Value = serde_json::from_str(CORPUS).unwrap();
        Self { now: 100, credential: "resource", revoked: false, registered: true, supported: true, generation: 1,
            grant: serde_json::from_value(corpus["grant"].clone()).unwrap(),
            revision: "r1".into(), operations: BTreeMap::new(), writes: 0 }
    }

    fn authorize(&self, r: &Submit) -> Result<(), &'static str> {
        if self.credential != "resource" || r.actor_id != self.grant.actor_id || r.grant_id != self.grant.grant_id { return Err("unauthorized"); }
        if self.revoked || self.grant.revoked || self.now >= self.grant.expires_at { return Err("revoked"); }
        if !self.registered || r.target.generation != self.generation
            || r.target.generation != self.grant.generation || r.target.installation_id != self.grant.installation_id
            || r.target.project_id.as_ref().is_some_and(|p| !self.grant.project_ids.contains(p))
            || r.target.runtime_id.as_ref().is_some_and(|rt| !self.grant.runtime_ids.contains(rt)) { return Err("scope"); }
        if r.target.project_id.is_none() && !self.grant.host_policy { return Err("scope"); }
        if !self.supported { return Err("unsupported"); }
        Ok(())
    }

    fn submit_raw(&mut self, raw: &str) -> String {
        if raw.len() > 16384 { return "invalid".into(); }
        match serde_json::from_str::<UniqueJson>(raw) {
            Ok(UniqueJson(wire)) => self.submit(&wire),
            Err(_) => "invalid".into(),
        }
    }

    fn submit(&mut self, wire: &Value) -> String {
        if serde_json::to_vec(wire).unwrap().len() > 16384 { return "invalid".into(); }
        let r = match typed(wire) { Ok(r) => r, Err(e) => return e.into() };
        if let Err(e) = self.authorize(&r) { return e.into(); }
        if let Some((bound, receipt)) = self.operations.get(&r.op_id) {
            return if bound == wire { outcome(receipt) } else { "replay_mismatch".into() };
        }
        if r.issued_at > self.now + 30 { return "clock_unknown".into(); }
        if self.now >= r.expires_at { return "expired".into(); }
        // This fixture grant covers one actor/installation; eight is its stricter pending bound.
        if self.operations.len() >= 4096 || self.operations.values().filter(|(_, p)| p["state"] == "pending").count() >= 8 {
            return "busy".into();
        }
        let receipt = json!({"schema":SCHEMA,"op_id":r.op_id,"target":wire["target"],"state":"pending",
            "reason":null,"saved_revision":null,"effective_revision":null,"observed_at":null});
        self.operations.insert(r.op_id, (wire.clone(), receipt));
        "pending".into()
    }

    fn apply(&mut self, op: &str) -> String {
        let (wire, receipt) = &self.operations[op];
        if receipt["state"] != "pending" { return outcome(receipt); }
        let r = typed(wire).unwrap();
        let reason = self.authorize(&r).err().or_else(|| {
            if self.now >= r.expires_at { Some("expired") }
            else if r.expected_revision != self.revision { Some("conflict") }
            else { None }
        });
        let receipt = &mut self.operations.get_mut(op).unwrap().1;
        if let Some(reason) = reason {
            receipt["state"] = json!(if reason == "expired" { "expired" } else { "rejected" });
            receipt["reason"] = json!(if reason == "expired" { None } else { Some(reason) });
        } else {
            self.writes += 1;
            self.revision = format!("r{}", self.writes + 1);
            receipt["state"] = json!("saved");
            receipt["saved_revision"] = json!(self.revision);
            receipt["effective_revision"] = json!("r1");
            receipt["observed_at"] = json!(self.now);
        }
        outcome(receipt)
    }

    fn cancel(&mut self, cancel: &Value) -> String {
        let op = cancel["op_id"].as_str().unwrap();
        let (wire, receipt) = &self.operations[op];
        if cancel.as_object().unwrap().len() != 5 || ["schema", "actor_id", "grant_id", "target"].iter().any(|k| cancel[k] != wire[k]) {
            return "replay_mismatch".into();
        }
        if let Err(e) = self.authorize(&typed(wire).unwrap()) { return e.into(); }
        if receipt["state"] == "pending" { self.operations.get_mut(op).unwrap().1["state"] = json!("cancelled"); }
        outcome(&self.operations[op].1)
    }

    fn readback(&mut self, op: &str, target: &Value, revision: &Value, at: u64) -> String {
        let r = &mut self.operations.get_mut(op).unwrap().1;
        if r["saved_revision"].is_null() { return outcome(r); }
        if at > self.now || self.now - at > 60 { r["state"] = json!("unknown"); }
        else if target == &r["target"] {
            r["effective_revision"] = revision.clone(); r["observed_at"] = json!(at);
            r["state"] = json!(if revision == &r["saved_revision"] { "applied" } else { "saved" });
        }
        outcome(r)
    }
}

fn outcome(r: &Value) -> String {
    r["reason"].as_str().or(r["state"].as_str()).unwrap().into()
}

// Chain: review -> confirm -> delivery -> canonical write -> readback. Risk: optimistic apply, duplicate write or stale authorization.
#[test]
fn client_server_traces() {
    let corpus: Value = serde_json::from_str(CORPUS).unwrap();
    for trace in corpus["traces"].as_array().unwrap() {
        let mut host = Authority::new();
        let mut draft = corpus["submit"].clone();
        let mut reviewed = None;
        let mut frozen = None;
        let mut client = "draft".to_string();
        for step in trace["steps"].as_array().unwrap() {
            client = match step["action"].as_str().unwrap() {
                "review" => { reviewed = Some(draft.clone()); "reviewed".into() }
                "edit" => { draft["changes"]["runtime_limit"] = json!(1); reviewed = None; "draft".into() }
                "confirm" => if reviewed.as_ref() == Some(&draft) {
                    frozen = Some(draft.clone()); host.submit_raw(&serde_json::to_string(&draft).unwrap())
                } else { client },
                "apply" => host.apply("op-1"),
                "readback" => host.readback("op-1", &draft["target"], &step["revision"], step["at"].as_u64().unwrap()),
                "external_edit" => { host.revision = "owner-r2".into(); client }
                "revoke" => { host.revoked = true; client }
                "time" => { host.now = step["at"].as_u64().unwrap(); client }
                "lose_reply" => "unknown".into(),
                "retry" => host.submit(frozen.as_ref().unwrap()),
                "mismatch" => { let mut changed = frozen.clone().unwrap(); changed["changes"]["runtime_limit"] = json!(1); host.submit(&changed) }
                "cancel" => if frozen.is_some() { host.cancel(&corpus["cancel"]) } else { "cancelled".into() },
                a => panic!("unknown fixture action {a}"),
            };
            assert_eq!(client, step["expect"].as_str().unwrap(), "{}: {step}", trace["name"]);
        }
        assert!(host.writes <= 1, "{} duplicated mutation", trace["name"]);
        if !host.operations.is_empty() && host.operations["op-1"].1["saved_revision"] == "r2" {
            assert_eq!(host.operations["op-1"].0["changes"]["runtime_limit"], 0, "zero is preserved");
        }
    }
}

// Chain: wire -> separate resource grant -> registered target. Risk: observation/answer credentials or malicious fields grant writes.
#[test]
fn refuses_untrusted_scope_fields_and_bounds() {
    let c: Value = serde_json::from_str(CORPUS).unwrap();
    let wire = &c["submit"];
    let raw = serde_json::to_string(wire).unwrap();
    let duplicate = raw.replacen("{", "{\"op_id\":\"other\",", 1);
    assert!(serde_json::from_str::<Submit>(&duplicate).is_err(), "duplicate identity refuses before canonicalization");
    for credential in ["observation", "answer"] {
        let mut h = Authority::new(); h.credential = credential;
        assert_eq!(h.submit(wire), "unauthorized"); assert_eq!(h.writes, 0);
    }
    for field in ["shell", "path", "permissions", "daemon", "tasks", "executable"] {
        let mut bad = wire.clone(); bad["changes"][field] = json!("forbidden");
        assert_eq!(Authority::new().submit(&bad), "invalid");
        let mut bad = wire.clone(); bad[field] = json!("forbidden");
        assert_eq!(Authority::new().submit(&bad), "invalid");
    }
    for value in [json!(-1), json!(1.5), json!(4294967296u64), json!("2")] {
        let mut bad = wire.clone(); bad["changes"]["runtime_limit"] = value;
        assert_eq!(Authority::new().submit(&bad), "invalid");
    }
    let mut h = Authority::new(); assert_eq!(h.submit(wire), "pending");
    h.registered = false; assert_eq!(h.apply("op-1"), "scope"); assert_eq!(h.writes, 0);
    let mut h = Authority::new(); h.generation = 2; assert_eq!(h.submit(wire), "scope");
    let mut h = Authority::new(); h.grant.expires_at = 100; assert_eq!(h.submit(wire), "revoked");
    let mut h = Authority::new(); assert_eq!(h.submit(wire), "pending");
    h.grant.expires_at = 101; h.now = 101; assert_eq!(h.apply("op-1"), "revoked"); assert_eq!(h.writes, 0);
    let mut h = Authority::new(); assert_eq!(h.submit(wire), "pending");
    h.supported = false; assert_eq!(h.apply("op-1"), "unsupported"); assert_eq!(h.writes, 0);
    let mut missing = wire.clone(); missing["target"].as_object_mut().unwrap().remove("runtime_id");
    assert!(typed(&missing).is_err(), "required null fields cannot be silently omitted");
    let mut wrong = wire.clone(); wrong["target"]["installation_id"] = json!("b".repeat(64));
    assert_eq!(Authority::new().submit(&wrong), "scope");
    let mut h = Authority::new(); assert_eq!(h.submit(wire), "pending"); h.apply("op-1");
    assert_eq!(h.readback("op-1", &wrong["target"], &json!("r2"), 100), "saved");
    let mut too_long = wire.clone(); too_long["expires_at"] = json!(401);
    assert_eq!(Authority::new().submit(&too_long), "invalid");
    let mut future = wire.clone(); future["issued_at"] = json!(131); future["expires_at"] = json!(431);
    assert_eq!(Authority::new().submit(&future), "clock_unknown");
    let mut h = Authority::new();
    for i in 0..8 { let mut w = wire.clone(); w["op_id"] = json!(format!("op-{i}")); assert_eq!(h.submit(&w), "pending"); }
    let mut ninth = wire.clone(); ninth["op_id"] = json!("op-9"); assert_eq!(h.submit(&ninth), "busy");
    assert_eq!(h.submit(wire), "pending", "existing binding survives full queue");
    let mut huge = wire.clone(); huge["padding"] = json!("x".repeat(16384)); assert_eq!(h.submit(&huge), "invalid");
    let mut legacy = wire.clone(); legacy["policy_schema"] = json!("taskq.capacity/1");
    assert!(typed(&legacy).is_ok()); legacy["changes"]["runtime_limit"] = Value::Null;
    assert!(typed(&legacy).is_err(), "legacy null cannot become unlimited");
    let mut project = wire.clone(); project["target"]["runtime_id"] = Value::Null;
    project["changes"] = json!({"mode":"auto", "maximum":null});
    assert!(typed(&project).is_ok(), "Auto optional maximum has no copied numerical default");
    project["changes"] = json!({"mode":"fixed", "maximum":2}); assert_eq!(Authority::new().submit(&project), "pending");
    let mut host_scope = project.clone(); host_scope["target"]["project_id"] = Value::Null;
    assert_eq!(Authority::new().submit(&host_scope), "scope");
    let mut h = Authority::new(); h.grant.host_policy = true;
    assert_eq!(h.submit(&host_scope), "pending", "explicit host grant permits host scope");
    project["changes"] = json!({"mode":"auto", "load_gate":{"enabled":false,"cpu_max":85,"memory_min":15}});
    assert!(typed(&project).is_err(), "Auto never silently disables gating");
    project["changes"] = json!({"load_gate":{"shell":true,"cpu_max":85,"memory_min":15}});
    assert!(typed(&project).is_err(), "malformed nested object refuses without panic");
    let mut host = wire.clone(); host["target"]["project_id"] = Value::Null; host["target"]["runtime_id"] = Value::Null;
    host["changes"] = json!({"mode":"auto", "maximum":0});
    let mut h = Authority::new(); assert_eq!(h.submit(&host), "scope");
    h.grant.host_policy = true; assert_eq!(h.submit(&host), "pending", "only explicit host grant permits host edits");
    let mut h = Authority::new();
    let receipt = c["receipt"].clone();
    for i in 0..4096 { h.operations.insert(format!("old-{i}"), (wire.clone(), receipt.clone())); }
    assert_eq!(h.submit(wire), "busy", "retained bindings cannot be evicted to accept a new ID");
}

fn allowlisted(value: &Value, kind: &str) -> Value {
    let fields: &[&str] = match kind {
        "projection" => &["target", "service_version", "supported", "current_revision", "effective_revision", "current_policy", "effective_policy", "runtime_usage", "sample", "blockers", "completeness"],
        "target" | "scope" => &["installation_id", "generation", "project_id", "runtime_id"],
        "supported" => &["policy_schema", "modes", "fields", "scopes"],
        "current_policy" | "effective_policy" => &["mode", "maximum", "runtime_limit", "load_gate", "sources"],
        "sources" => &["mode", "maximum", "runtime_limit", "load_gate"],
        "load_gate" => &["enabled", "cpu_max", "memory_min"],
        "runtime_usage" => &["runtime_id", "used", "limit", "limit_kind"],
        "sample" => &["source", "sampled_at", "age_seconds", "cadence_seconds", "window_seconds", "cpu_busy_percent", "memory_available_percent", "memory_available_gib", "pressure"],
        "blockers" => &["code", "scope", "knowledge"],
        _ => return if value.is_object() || value.is_array() { Value::Null } else { value.clone() },
    };
    if let Some(array) = value.as_array() {
        return Value::Array(array.iter().map(|v| allowlisted(v, kind)).collect());
    }
    let Some(object) = value.as_object() else { return Value::Null; };
    Value::Object(object.iter().filter(|(key, _)| fields.contains(&key.as_str())).map(|(key, value)| {
        let projected = if ["modes", "fields", "scopes"].contains(&key.as_str()) {
            Value::Array(value.as_array().map(|a| a.iter().map(|v| if v.is_string() { v.clone() } else { Value::Null }).collect()).unwrap_or_default())
        } else { allowlisted(value, key) };
        (key.clone(), projected)
    }).collect())
}

fn project(source: &Value, observed_at: u64, now: u64) -> Value {
    let mut p = allowlisted(source, "projection");
    if !p["blockers"].is_array() { p["blockers"] = json!([]); p["completeness"] = json!("unknown"); }
    for blocker in p["blockers"].as_array_mut().unwrap() {
        if !["cpu", "memory", "pressure", "sample", "config", "cooldown", "spacing", "max", "version", "host", "runtime_disabled", "pause", "focus", "dependency", "routing", "access", "provider", "unknown"].contains(&blocker["code"].as_str().unwrap_or("")) {
            blocker["code"] = json!("unknown"); blocker["knowledge"] = json!("unknown");
        }
    }
    let sampled = p["sample"]["sampled_at"].as_u64();
    let age = sampled.filter(|at| *at <= observed_at)
        .and_then(|_| p["sample"]["age_seconds"].as_u64())
        .and_then(|age| now.checked_sub(observed_at).and_then(|elapsed| age.checked_add(elapsed)));
    if !p["sample"].is_null() { p["sample"]["age_seconds"] = json!(age); }
    if age.is_none_or(|age| age > 60) {
        let target = p["target"].clone();
        p["blockers"].as_array_mut().unwrap().push(json!({"code":"sample", "scope":target, "knowledge":"unknown"}));
        p["completeness"] = json!("unknown");
    }
    p
}

// Chain: source projection -> resource view. Risk: saved becomes effective, unknown becomes zero, blockers disappear or samples lose semantics.
#[test]
fn projection_preserves_source_knowledge() {
    let c: Value = serde_json::from_str(CORPUS).unwrap();
    let p = &c["projection"]["targets"][0];
    let fields = ["target", "service_version", "supported", "current_revision", "effective_revision", "current_policy", "effective_policy", "runtime_usage", "sample", "blockers", "completeness"];
    assert_eq!(p.as_object().unwrap().len(), fields.len());
    assert!(fields.iter().all(|f| p.get(f).is_some()));
    assert_ne!(p["current_revision"], p["effective_revision"]);
    assert_eq!(p["current_policy"]["runtime_limit"], 0);
    assert_eq!(p["runtime_usage"][0]["used"], 3); assert_eq!(p["runtime_usage"][0]["limit"], 2);
    assert!(p["runtime_usage"][1]["used"].is_null()); assert_eq!(p["runtime_usage"][1]["limit_kind"], "unknown");
    assert_eq!(p["sample"]["source"], "taskq-admission-macos"); assert!(p["sample"]["memory_available_percent"].is_null());
    assert_eq!(p["blockers"].as_array().unwrap().iter().map(|b| b["code"].as_str().unwrap()).collect::<Vec<_>>(), ["cpu", "max", "provider"]);
    assert_eq!(p["completeness"], "partial");
    let mut source = p.clone(); source["path"] = json!("private");
    source["blockers"][2]["code"] = json!("future-producer-reason");
    let stale = project(&source, 100, 151);
    assert!(stale.get("path").is_none());
    assert_eq!(stale["sample"]["age_seconds"], 61);
    assert_eq!(stale["sample"]["cpu_busy_percent"], 95.0);
    assert_eq!(stale["blockers"][2]["code"], "unknown");
    assert_eq!(stale["blockers"][3]["knowledge"], "unknown");
    assert_eq!(stale["completeness"], "unknown");
    assert!(project(&source, 100, 89)["sample"]["age_seconds"].is_null());
    let receipt = &c["receipt"];
    assert_eq!(receipt["state"], "saved"); assert_ne!(receipt["saved_revision"], receipt["effective_revision"]);
}

// Chain: raw delivery/projection -> validated model. Risk: nested leakage, duplicate changes or stale samples appear fresh.
#[test]
fn reviewer_regression_fixtures() {
    let c: Value = serde_json::from_str(CORPUS).unwrap();
    for case in c["regressions"]["legacy_runtime_scope"].as_array().unwrap() {
        let mut wire = c["submit"].clone();
        wire["policy_schema"] = json!("taskq.capacity/1");
        wire["target"]["project_id"] = case["project_id"].clone();
        wire["target"]["runtime_id"] = case["runtime_id"].clone();
        let mut host = Authority::new();
        assert_eq!(host.submit_raw(&serde_json::to_string(&wire).unwrap()), case["expect"].as_str().unwrap());
        if case["expect"] == "pending" {
            assert_eq!(host.apply("op-1"), "saved");
            assert_eq!(host.writes, 1);
        } else {
            assert_eq!(host.writes, 0);
            assert!(host.operations.is_empty());
        }
    }
    for raw in c["regressions"]["duplicate_json"].as_array().unwrap() {
        let mut host = Authority::new();
        assert_eq!(host.submit_raw(raw.as_str().unwrap()), "invalid");
        assert_eq!(host.writes, 0);
        assert!(host.operations.is_empty());
    }
    let mut original = c["projection"]["targets"][0].clone();
    original["current_policy"]["load_gate"] = json!({"enabled":true,"cpu_max":85,"memory_min":15});
    let mut source = original.clone();
    for path in c["regressions"]["private_paths"].as_array().unwrap() {
        let mut field = &mut source;
        for part in path.as_array().unwrap() {
            field = if let Some(index) = part.as_u64() { &mut field[index as usize] } else { &mut field[part.as_str().unwrap()] };
        }
        *field = json!("private");
    }
    assert_eq!(project(&source, 100, 100), original, "nested private fields are stripped");
    for case in c["regressions"]["ages"].as_array().unwrap() {
        let mut source = original.clone();
        source["sample"]["age_seconds"] = case["producer_age"].clone();
        let p = project(&source, 100, case["now"].as_u64().unwrap());
        assert_eq!(p["sample"]["age_seconds"], case["expected_age"]);
        assert_eq!(p["completeness"], "unknown");
        assert_eq!(p["sample"]["cpu_busy_percent"], original["sample"]["cpu_busy_percent"]);
        assert_eq!(p["blockers"][3]["knowledge"], "unknown");
    }
}
