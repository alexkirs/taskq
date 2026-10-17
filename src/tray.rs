//! `taskq tray` (taskq.md § 4): the host's one menu-bar / notification-area icon over the `<root>/.taskq/status.json`
//! and `events` of every project in `<install>/projects`. A click opens `tray.html` in a wry webview; Rust pushes one
//! JSON state, the page sends actions back. No board access and no token: actions go through the same files and
//! commands as the CLI.

use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};
use tao::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use tao::event::{Event, StartCause, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tao::window::{Window, WindowBuilder};
use tray_icon::{Icon, MouseButtonState, Rect, TrayIcon, TrayIconBuilder, TrayIconEvent};
use wry::{WebView, WebViewBuilder};

use crate::icon::{self, Rgba, compact};
use crate::notify;

const HTML: &str = include_str!(concat!(env!("OUT_DIR"),"/web/tray.html"));
const FONTS: [u64; 4] = [14, 16, 18, 20];
const PERIODS: [(&str, i64); 5] = [("5m", 300), ("1h", 3600), ("1d", 86400), ("1w", 604800), ("1mo", 2592000)];

/// False when another tray wrote the heartbeat `<install>/tray` in the last 10 s (it ticks every 2 s): of two trays
/// started at once, the first to see the other's pid exits.
fn alone(install: &Path) -> bool {
    let hb = install.join("tray");
    let other = fs::read_to_string(&hb).ok().and_then(|p| p.trim().parse::<u32>().ok()).is_some_and(|p| p != std::process::id());
    !(other && crate::file_age(&hb).is_some_and(|a| a < 10))
}

enum Ev {
    Tick,
    Ipc(String),
    Evaluated(u64, &'static str),
    Click(Rect),
    Shot,
    Shot2,
    /// Add project: the picked folder classified, or Null (cancel)
    Added(Value),
    Answer(Value),
    /// A notification click: its request identifier (M14)
    Note(String),
}

/// View, text size, period and Custom icon of `<install>/tray.json`; anything else falls back to the default. A
/// `custom` period is `[start, end)` in epoch seconds with the time zone `local` or `UTC` it was typed in. View
/// `full` is Summary; `custom` needs a valid `icon` configuration (custom-icon.md CI1).
#[derive(Clone, PartialEq)]
struct Settings {
    view: String,
    font: u64,
    period: String,
    start: i64,
    end: i64,
    tz: String,
    icon: Value,
    pin: Pin,
    /// Notification modes and cursors (`notify.rs`), kept as saved
    notify: Value,
}

/// Pinned window (menu.md M13): pinned or not, the last display and the last top-left per display, physical pixels.
#[derive(Clone, PartialEq)]
struct Pin {
    pinned: bool,
    top: bool,
    size: Option<(f64, f64)>,
    display: String,
    pos: std::collections::BTreeMap<String, (i32, i32)>,
}

impl Default for Pin {
    fn default() -> Self {
        Self { pinned: false, top: true, size: None, display: String::new(), pos: Default::default() }
    }
}

impl Settings {
    fn load(install: &Path) -> Settings {
        // § 4 Data: a corrupt tray.json shows the defaults with its reason; the file stays until a save keeps a copy
        let v = crate::data::json(&install.join("tray.json")).unwrap_or_else(|e| {
            eprintln!("{e}; defaults shown");
            None
        }).unwrap_or_default();
        let pos = v["pos"].as_object().into_iter().flatten().filter_map(|(k, p)| Some((k.clone(), (i32::try_from(p[0].as_i64()?).ok()?, i32::try_from(p[1].as_i64()?).ok()?)))).collect();
        let size = v["size"][0].as_f64().zip(v["size"][1].as_f64()).filter(|(w, h)| w.is_finite() && h.is_finite() && *w >= 240.0 && *h >= 120.0);
        let pin = Pin { top: v["top"].as_bool().unwrap_or(true), size, pinned: v["pinned"] == true, display: v["display"].as_str().unwrap_or("").into(), pos };
        let mut s = Settings { view: "icon".into(), font: 16, period: "1d".into(), start: 0, end: 0, tz: "local".into(), icon: Value::Null, pin,
            notify: if v["notify"].is_object() { v["notify"].clone() } else { Value::Null } };
        // an invalid or corrupt saved icon falls back to Summary (CI4)
        if icon::validate(&v["icon"]).is_empty() {
            s.icon = v["icon"].clone();
        }
        s.set(&v);
        s
    }

    fn set(&mut self, v: &Value) {
        if let Some(x) = v["view"].as_str().filter(|x| ["icon", "full"].contains(x) || (*x == "custom" && !self.icon.is_null())) {
            self.view = x.into();
        }
        if let Some(x) = v["font"].as_u64().filter(|x| FONTS.contains(x)) {
            self.font = x;
        }
        if let Some(x) = v["period"].as_str().filter(|x| PERIODS.iter().any(|p| p.0 == *x)) {
            self.period = x.into();
        }
        // a custom range with start >= end is refused (menu.md M6)
        if let (Some("custom"), Some(a), Some(b)) = (v["period"].as_str(), v["start"].as_i64(), v["end"].as_i64())
            && a < b
        {
            (self.period, self.start, self.end) = ("custom".into(), a, b);
            self.tz = if v["tz"] == "UTC" { "UTC" } else { "local" }.into();
        }
    }

    /// `[start, end)` of the period at `now`.
    fn range(&self, now: i64) -> (i64, i64) {
        if self.period == "custom" {
            return (self.start, self.end);
        }
        rolling(now, PERIODS.iter().find(|p| p.0 == self.period).map_or(86400, |p| p.1))
    }

    fn save(&self, install: &Path) -> bool {
        let v = json!({"view": self.view, "font": self.font, "period": self.period, "start": self.start, "end": self.end, "tz": self.tz, "icon": self.icon,
            "pinned": self.pin.pinned, "top": self.pin.top, "size": self.pin.size, "display": self.pin.display, "pos": self.pin.pos, "notify": self.notify});
        let p = install.join("tray.json");
        if crate::data::json(&p).is_err() && fs::copy(&p, install.join("tray.json.corrupt")).is_err() {
            return false;
        }
        crate::platform::write_atomic(&p, &v.to_string()).is_ok()
    }
}

/// `taskq tray --icon FILE`: a draft with any schema error is refused with every error and leaves `tray.json` (so the
/// icon) unchanged; a valid one is saved as Custom and selected (custom-icon.md CI2 Validate/Save).
pub fn use_icon(install: &Path, draft: &Value) -> Result<(), Vec<String>> {
    let err = icon::validate(draft);
    if !err.is_empty() {
        return Err(err);
    }
    let mut s = Settings::load(install);
    (s.icon, s.view) = (draft.clone(), "custom".into());
    if !s.save(install) || Settings::load(install).icon != *draft {
        return Err(vec!["tray.json: write or readback failed".into()]);
    }
    Ok(())
}

/// Draft and history of AI requests (§ 4): `<install>/tray-requests.json`, apart from the view settings.
fn requests_file(install: &Path) -> Value {
    let v = crate::operator::read(&install.join("tray-requests.json"));
    json!({"draft": v["draft"].as_str().unwrap_or(""), "history": v["history"].as_array().cloned().unwrap_or_default()})
}

/// The last 20 distinct sent texts, newest first; the same text moves to the top (menu.md M7).
fn remember(history: &[Value], text: &str) -> Vec<Value> {
    std::iter::once(json!(text)).chain(history.iter().filter(|h| h.as_str() != Some(text)).cloned()).take(crate::operator::KEEP).collect()
}

/// The project whose service takes a new request: the first registered one with a fresh `status.json`.
fn target(install: &Path) -> Option<String> {
    roots(install).into_iter().find(|r| crate::file_age(&crate::dot(Path::new(r)).join("status.json")).is_some_and(|a| a < 300))
}

/// The newest request over all projects with its answer.
fn newest(install: &Path) -> Value {
    let all = roots(install).into_iter().filter_map(|r| crate::operator::ids(Path::new(&r)).pop().map(|id| (id, r)));
    let Some((id, r)) = all.max_by_key(|(id, _)| (id.len(), id.clone())) else { return Value::Null };
    let root = Path::new(&r);
    let mut chain = Vec::new();
    let mut current = id.clone();
    for _ in 0..crate::operator::KEEP {
        let q = crate::operator::read(&crate::operator::dir(root).join(format!("{current}.json")));
        if q.is_null() { break; }
        chain.push(json!({"id": current, "text": q["text"], "task": q["task"], "url": q["url"], "answer": crate::operator::read(&crate::operator::answer_path(root, &current))}));
        let Some(after) = q["after"].as_str().filter(|a| !a.is_empty() && a.bytes().all(|b| b.is_ascii_digit())) else { break };
        if chain.iter().any(|q| q["id"] == after) { break; }
        current = after.to_string();
    }
    chain.reverse();
    let mut request = chain.last().cloned().unwrap_or(Value::Null);
    request["root"] = json!(r);
    request["chain"] = json!(chain);
    request
}

fn ai(install: &Path, a: &str, m: &Value) {
    let path = install.join("tray-requests.json");
    let mut f = requests_file(install);
    match a {
        "ai-draft" => f["draft"] = json!(m["text"].as_str().unwrap_or("")),
        "ai-clear" => f["history"] = json!([]),
        "ai-send" => {
            let (Some(text), Some(r)) = (m["text"].as_str().map(str::trim).filter(|t| !t.is_empty()), target(install)) else { return };
            let onboard = m["onboard"].as_str();
            let root = Path::new(&r);
            let id = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0).to_string();
            let after = m["after"].as_str().filter(|x| !x.is_empty() && x.bytes().all(|b| b.is_ascii_digit()));
            let _ = fs::create_dir_all(crate::operator::dir(root));
            if crate::platform::write_atomic(&crate::operator::dir(root).join(format!("{id}.json")), &json!({"text": text, "after": after, "onboard": onboard}).to_string()).is_err() {
                return;
            }
            let _ = crate::poke(root);
            if onboard.is_some() {
                return;
            }
            f = json!({"draft": "", "history": remember(f["history"].as_array().map_or(&[][..], |v| v), text)});
        }
        // only a registered project and a request id: the page never names a path of its own
        "ai-confirm" => {
            let (r, id) = (m["root"].as_str().unwrap_or(""), m["id"].as_str().unwrap_or(""));
            if roots(install).iter().any(|x| x == r) && !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()) {
                let _ = fs::write(crate::operator::dir(Path::new(r)).join(format!("{id}.confirm")), m["rev"].as_u64().unwrap_or(0).to_string());
                let _ = crate::poke(Path::new(r));
            }
            return;
        }
        _ => return,
    }
    let _ = crate::platform::write_atomic(&path, &f.to_string());
}

fn roots(install: &Path) -> Vec<String> {
    fs::read_to_string(install.join("projects")).unwrap_or_default().lines().filter(|l| !l.is_empty()).map(String::from).collect()
}

/// Owner answers and tray-only fallback actions cross this bridge; the CLI owns credentials, dispatch and board readback.
fn answer_args(install: &Path, m: &Value) -> Result<Vec<String>, String> {
    let root = m["root"].as_str().ok_or("project missing")?;
    if !roots(install).iter().any(|r| r == root) { return Err("project is not registered".into()); }
    if m["board"].as_str().is_none_or(str::is_empty) { return Err("board identity missing".into()); }
    let op = &m["op"];
    let id = op["id"].as_str().filter(|id| !id.is_empty() && !id.contains(char::is_whitespace)).ok_or("operation id must be one word")?;
    let n = op["task"].as_u64().filter(|n| *n > 0).ok_or("task number missing")?;
    let revision = op["revision"].as_u64().ok_or("question revision missing")?;
    let action = op.get("action").map(|v| v.as_str().ok_or("invalid action")).transpose()?;
    let mut args: Vec<String> = if let Some(action) = action {
        if op.get("choice").is_some() { return Err("fallback actions cannot include a choice".into()); }
        let (command, text) = match action {
            "requeue" => {
                let reason = op["text"].as_str().filter(|s| !s.trim().is_empty() && !s.contains(['\r', '\n'])).ok_or("supply one requeue reason line")?;
                ("requeue", format!("Reason: {reason}"))
            }
            "later" if op.get("text").is_none() => ("later", "Reason: Parked by the owner from the tray.".into()),
            "close" if op["confirmed"] == true && op.get("text").is_none() => ("later", "Reason: Owner confirmed cancellation without publication from the tray.".into()),
            _ => return Err("unsupported or unconfirmed tray action".into()),
        };
        crate::task_text(command, &text)?;
        vec![command.into(), n.to_string(), "--text".into(), text]
    } else {
        let mut args = vec!["answer".into()];
        match (op.get("choice"), op.get("text")) {
            (Some(choice), None) => {
                let k = choice.as_u64().filter(|k| *k > 0).ok_or("invalid choice")?;
                args.push(format!("{n}.{k}"));
            }
            (None, Some(text)) => {
                let text = text.as_str().ok_or("invalid answer text")?;
                crate::task_text("answer", text)?;
                args.extend([n.to_string(), "--text".into(), text.into()]);
            }
            _ => return Err("supply one choice or literal text".into()),
        }
        args
    };
    let id = if action == Some("close") { format!("{id}.later") } else { id.into() };
    args.extend(["--json".into(), "--op-id".into(), id, "--revision".into(), revision.to_string()]);
    Ok(args)
}

fn owner_command(root: &Path) -> Result<std::process::Command, String> {
    let mut c = std::process::Command::new(std::env::current_exe().map_err(|e| e.to_string())?);
    c.current_dir(root).env("TASKQ_ROOT", root).env_remove("TASKQ_TASK").env_remove("TASKQ_ATTEMPT").env_remove("TASKQ_ROLE");
    Ok(c)
}

fn answer_snapshot(s: &Value, m: &Value) -> Result<(), String> {
    if s["complete"] != true { return Err(s["reason"].as_str().unwrap_or("board read incomplete").into()); }
    if s["board"]["url"] != m["board"] { return Err("board identity changed; refresh the project".into()); }
    let op = &m["op"];
    let row = s["tasks"].as_array().into_iter().flatten().find(|r| r["n"] == op["task"]);
    let row = row.ok_or("task is not in the current board snapshot; open it on the board")?;
    if op.get("action").is_some() {
        if row["revision"] != op["revision"] { return Err("question changed; refresh before acting".into()); }
        if op["action"] == "close" && row["state"] == "later" { return Ok(()); } // only a matching later op can succeed again
        if row["state"] != "ask" || row["question"]["choices"].as_array().is_some_and(|c| !c.is_empty()) {
            return Err("fallback actions require an ask without choices; check the board for partial cancellation".into());
        }
        return Ok(());
    }
    if !row["revision"].is_u64() { return Err("question revision unavailable".into()); }
    // An old revision still reaches dispatch so uncertain delivery can reconcile its op id.
    if row["revision"] == op["revision"] {
        if row["question"]["parsed"] != true { return Err("question is read-only; open it on the board".into()); }
        if let Some(choice) = op.get("choice") {
            let c = row["question"]["choices"].as_array().into_iter().flatten().find(|c| c["id"] == *choice).ok_or("choice not found")?;
            if c["consequential"] == true { return Err("close/publication requires review on the board".into()); }
        }
    }
    Ok(())
}

// Chain: one CLI receipt per step. Close never continues after unknown/failed parking or from another state.
fn answer_dispatch(m: &Value, args: &[String], mut run: impl FnMut(&[String]) -> Value) -> Value {
    let checked = |r: Value, id: &str| {
        if r["task"] == m["op"]["task"] && r["op_id"] == id
            && matches!(r["outcome"].as_str(), Some("accepted" | "stale" | "rejected" | "unknown")) { r }
        else { json!({"outcome": "unknown", "reason": "No matching CLI receipt", "next": "Check the board; retry the same operation id."}) }
    };
    let id = args[args.len() - 3].as_str();
    let r = checked(run(args), id);
    if m["op"]["action"] != "close" || r["outcome"] != "accepted" || r["state"] == "closed" { return r; }
    if r["state"] != "later" || !r["revision"].is_u64() {
        return json!({"outcome": "rejected", "reason": "Cancellation stopped: parking was not read back as later with a revision", "next": "Check the task on the board."});
    }
    let id = format!("{}.close", m["op"]["id"].as_str().unwrap());
    let close = vec!["close".into(), m["op"]["task"].to_string(), "--text".into(),
        "Changed: Cancelled by the owner without publication.\nChecks: Tray cancellation -- later read back before close.\nOpen: none".into(),
        "--json".into(), "--op-id".into(), id.clone(), "--revision".into(), r["revision"].to_string()];
    let mut r = checked(run(&close), &id);
    if r["outcome"] == "accepted" && r["state"] != "closed" {
        r["outcome"] = json!("unknown");
        r["reason"] = json!("Close receipt did not read back closed");
    }
    if r["outcome"] != "accepted" {
        r["reason"] = json!(format!("Parked; close not verified: {}", r["reason"].as_str().unwrap_or("unknown")));
        r["next"] = json!("Check the board; retry the same cancellation to reconcile delivery.");
    }
    r
}

fn answer(install: &Path, m: &Value) -> Value {
    let response = |state: &str, reason: &str, next: &str| json!({"answerReceipt": {"root": m["root"], "id": m["op"]["id"], "state": state, "reason": reason, "next": next}});
    let prepare = || -> Result<_, String> {
        let args = answer_args(install, m)?;
        let root = Path::new(m["root"].as_str().unwrap());
        let out = crate::platform::output("tray.answer-status",owner_command(root)?.args(["status", "--json"]),Duration::from_secs(30))?;
        let snapshot = serde_json::from_slice(&out.stdout).map_err(|_| String::from_utf8_lossy(&out.stderr).into_owned())?;
        answer_snapshot(&snapshot, m)?;
        Ok((root, args))
    };
    let (root, args) = match prepare() {
        Ok(c) => c,
        Err(e) => return response("rejected", &e, "Refresh the question or open it on the board."),
    };
    let r = answer_dispatch(m, &args, |args| {
        let out = owner_command(root).and_then(|mut c| crate::platform::output("tray.answer-command",c.args(args),Duration::from_secs(60)));
        match out {
            Ok(out) => serde_json::from_slice(&out.stdout).unwrap_or_else(|_| json!({"task": m["op"]["task"], "op_id": args[args.len()-3], "outcome": "unknown", "reason": String::from_utf8_lossy(&out.stderr), "next": "Check the board; retry the same operation id."})),
            Err(e) => json!({"task": m["op"]["task"], "op_id": args[args.len()-3], "outcome": "unknown", "reason": e, "next": "Check the board; retry the same operation id."}),
        }
    });
    let reason = r["reason"].as_str().filter(|s| !s.is_empty()).or_else(|| r["state"].as_str()).unwrap_or("");
    response(r["outcome"].as_str().unwrap_or("unknown"), reason, r["next"].as_str().unwrap_or(""))
}

/// One opaque revision of a policy: equal content, equal revision.
/// `ponytail:` a content hash, so an edit-and-revert reads as unchanged; the host adapter's revisions replace it.
fn revision(v: &Value) -> Value {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    v.to_string().hash(&mut h);
    json!(format!("r{:016x}", h.finish()))
}

/// Host policy and occupancy are distinct from this project's narrowing ceilings.
fn resource_group(cap: &Value, saved: &Value, saved_revision:&Value, project_limits: &Value, observed_at: Option<i64>, installation: Option<String>, project: &str) -> Value {
    let host = crate::service::host_capacity(cap).unwrap_or(Value::Null);
    let ok = host.is_object();
    let saved = if saved["error"].is_null() { saved } else { &Value::Null };
    let target = |p: bool, rt: Option<&str>| json!({"installation_id": installation, "generation": 1, "project_id": if p { json!(project) } else { Value::Null }, "runtime_id": rt});
    let entry = |t: Value, current: Value, effective: Value| {let project=!t["project_id"].is_null();let runtime=!t["runtime_id"].is_null();json!({"target": t, "service_version": crate::platform::VERSION,
        "supported": [{"policy_schema": "taskq.host-capacity/1", "modes": [], "fields": if project||saved.is_null() {json!([])}else if runtime {json!(["runtime_limit"])}else{json!(["maximum","load_gate"])}, "scopes": if project {json!([])}else if runtime {json!(["runtime"])}else{json!(["host"])} }],
        "current_revision": if saved.is_null() { Value::Null } else if project {revision(&current)}else{saved_revision.clone()},
        "effective_revision": if !ok {Value::Null}else if project {revision(&effective)}else{cap["host_command"].clone()},
        "current_policy": current, "effective_policy": effective,
        "sample": null, "runtime_usage": [], "blockers": [], "completeness": if ok { "complete" } else { "unknown" }})};
    let pol = |c: &Value| json!({"maximum": c["admission"]["max"], "load_gate": c["load_gate"]});
    let mut h = entry(target(false, None), pol(saved), pol(&host));
    let mut rts: Vec<String> = crate::service::RUNTIMES.iter().map(|rt| rt.to_string()).collect();
    for m in [&saved["limits"], &host["limits"], &host["used"]] {
        for k in m.as_object().into_iter().flatten().map(|(k, _)| k) { if !rts.contains(k) { rts.push(k.clone()); } }
    }
    h["runtime_usage"] = json!(rts.iter().map(|rt| {
        let l = if ok { host["limits"][rt].as_u64() } else { saved["limits"][rt].as_u64() };
        let kind = if !ok && saved.is_null() { "unknown" } else { match l { Some(0) => "disabled", Some(_) => "ceiling", None => "unbounded" } };
        json!({"runtime_id": rt, "used": host["used"][rt].as_u64(), "limit": l, "limit_kind": kind})
    }).collect::<Vec<_>>());
    if ok && host["sample_at"].is_i64() {
        h["sample"] = json!({"source": format!("taskq-admission-{}", if cfg!(windows) { "windows" } else if cfg!(target_os = "macos") { "macos" } else { "linux" }),
            "sampled_at": host["sample_at"], "age_seconds": host["sample_age_seconds"], "cadence_seconds": 30, "window_seconds": 60,
            "cpu_busy_percent": host["cpu_percent"], "memory_available_percent": host["memory_available_percent"], "memory_available_gib": null, "pressure": null});
    }
    const CODES: [&str; 18] = ["cpu", "memory", "pressure", "sample", "config", "cooldown", "spacing", "max", "version", "host", "runtime_disabled", "pause", "focus", "dependency", "routing", "access", "provider", "unknown"];
    h["blockers"] = json!(host["reasons"].as_array().into_iter().flatten().map(|r| {
        let code = match r["code"].as_str().unwrap_or("") { "hold" => "host", c if CODES.contains(&c) => c, _ => "unknown" };
        let host_scope = ["host", "max", "spacing", "version", "cpu", "memory", "pressure", "sample", "config"].contains(&code);
        json!({"code": code, "scope": target(!host_scope, None), "knowledge": if code == "sample" || code == "unknown" || (code == "host" && host["gate"] == "unknown") { "unknown" } else { "known" }})
    }).collect::<Vec<_>>());
    let mut targets = vec![h, entry(target(true, None), json!({"limits": project_limits}), json!({"limits": cap["project_limits"]}))];
    for rt in &rts {
        let lim = |c: &Value| json!({"runtime_limit": c["limits"][rt]});
        targets.push(entry(target(false, Some(rt)), lim(saved), lim(&host)));
    }
    json!({"schema": "taskq.resources/1", "observed_at": observed_at, "targets": targets})
}

/// One project for the page: its `status.json`, saved focus, labels and `events` lines `[time, kind, attempt, task]`.
fn project(install: &Path, root: &str) -> Value {
    let r = Path::new(root);
    let d = crate::dot(r);
    // § 4 Data: a corrupt status.json is unknown with its reason, never zero
    let (s, bad) = match crate::data::json(&d.join("status.json")) {
        Ok(s) => (s.unwrap_or(Value::Null), None),
        Err(e) => (Value::Null, Some(e)),
    };
    let age = crate::file_age(&d.join("status.json"));
    let saved = fs::read_to_string(d.join("focus.paused")).is_ok_and(|f| !matches!(f.trim(), "" | "off"));
    let text = fs::read_to_string(d.join("events")).unwrap_or_default();
    let since = text.lines().next().and_then(|l| l.strip_prefix("since ")?.parse::<i64>().ok());
    let events: Vec<Value> = text.lines().skip(1).filter_map(|l| {
        let w: Vec<&str> = l.split(' ').collect();
        Some(json!([w.first()?.parse::<i64>().ok()?, w.get(2)?, w.get(3)?, w.get(1)?.strip_prefix('#')?.parse::<u64>().ok()?]))
    }).collect();
    // the local host's saved capacity, for `Edit limits` and its readback (§ 4 Capacity)
    let (cap_host, cap_saved, cap_revision,board_kind, project_limits) = match crate::load(r) {
        Ok((c, raw)) => {let policy=crate::host::Policy::read(install);let rev=policy.as_ref().ok().and_then(|p|p.revision().ok());let host=policy.as_ref().map(|p|json!(p.me)).unwrap_or(Value::Null);
            (host, policy.map(|p|p.cap.json()).unwrap_or_else(|e| json!({"error": e})),json!(rev), json!(c.board.name), if raw["limits"].is_null() {json!({})} else {raw["limits"].clone()})},
        Err(e) => (Value::Null, json!({"error": e}), Value::Null,Value::Null, Value::Null),
    };
    let mut tasks = s["tasks"].as_array().cloned().unwrap_or_default();
    for task in &mut tasks { task_session(r, cap_host.as_str().unwrap_or(""), task); }
    let installation = fs::read_to_string(install.join("installation-id")).ok().filter(|i| i.len() == 64 && i.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)));
    let id: String = crate::project_name(r).chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).take(64).collect();
    let observed_at=s["at"].as_i64().filter(|at|*at<=now()&&now()-*at<=60).and(age.map(|a|now()-a as i64));
    let resources = resource_group(&s["capacity"], &cap_saved,&cap_revision, &project_limits, observed_at, installation, &id);
    json!({"root": root, "name": s["project"].as_str().map(String::from).unwrap_or_else(|| crate::project_name(r)),
        "issues": s["issues"].as_str().unwrap_or(""), "board_kind": board_kind, "closed": Value::Null,
        "log": s["log"].is_string(), "alive": age.is_some_and(|a| a < 300), "age": age,
        "complete": crate::snapshot::complete(&s) && age.is_some_and(|a| a < 300),
        "error": bad.map(Value::from).or_else(|| [&s["error"], &s["focus_error"]].into_iter().find(|e| e.is_string()).cloned()).unwrap_or(Value::Null), "focus": s["focus"].as_array().cloned().unwrap_or_default(), "saved": saved, "labels": crate::status_labels(r),
        "focus_command": s["focus_command"], "request": s["request"], "since": since, "events": events, "tasks": tasks, "host": s["host"], "version": s["version"], "update": s["update"],
        "resources": resources, "cap_host": cap_host, "cap_saved": cap_saved, "agents": s["agents"],
        "incident": s["incident"], "incident_line": s.get("incident").map(crate::incident::line), "incident_notice": s["incident_notice"],
        "incident_repo": crate::platform::RELEASE_REPO})
}

fn now() -> i64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Native adapter (M3): a Claude URL is account-scoped, so any host links it (owning host: its transcript, else the
/// owner's heartbeat via the snapshot); a Codex thread opens only on its owning host, elsewhere unavailable with a reason.
fn task_session(root: &Path, local: &str, row: &mut Value) {
    let Some(object) = row.as_object_mut() else { return };
    let published = object.remove("session_url").and_then(|u| u.as_str().filter(|u| claude_url(u)).map(String::from));
    object.remove("session_reason");
    object.insert("local_session".into(), json!(false));
    let session = row["session"].as_str().unwrap_or("").to_string();
    let words: Vec<&str> = session.split_whitespace().collect();
    let Some((rt, id)) = words.first().and_then(|s| s.split_once(':')) else { return };
    let host = words.get(1).copied().unwrap_or("");
    let own = !local.is_empty() && host == local;
    row["runtime"] = json!(rt);
    row["host"] = json!(host);
    row["local_session"] = json!(own);
    if rt != "claude" && !own {
        if matches!(rt, "codex" | "codex-app") { row["session_reason"] = json!(format!("Codex session opens only on {host}")); }
        return;
    }
    let id = if own && ((id == "-" && rt == "codex") || rt == "claude") {
        let l = crate::runtime::Launch { execution: None, task: row["n"].as_u64().unwrap_or(0), title: String::new(), attempt: row["attempt"].as_u64().unwrap_or(0),
            role: row["role"].as_str().unwrap_or("").into(), rt: rt.into(), session: id.into(), url: String::new(), resume: None };
        crate::runtime::codex_thread(&crate::runtime::log_path(root, &l)).unwrap_or_else(|| id.into())
    } else { id.into() };
    if id == "-" || id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') { return }
    row["session_url"] = match rt {
        "codex" | "codex-app" => json!(format!("codex://threads/{id}")),
        "claude" => json!(own.then(|| crate::runtime::claude_remote(&id)).flatten().or(published)),
        _ => Value::Null,
    };
}

fn claude_url(u: &str) -> bool {
    u.strip_prefix("https://claude.ai/code/session_").is_some_and(|id| !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric()))
}

/// What the icon and the history show: in work, `ask`, ready, failed over all projects (one task per board and issue), whether
/// any project admits, 12 equal bins of `[start, end)` (oldest first) counting `KINDS` and the latest `since` of the
/// projects that have history (None: none has).
#[derive(Clone)]
struct Summary {
    /// work, ask, ready, failed
    counts: [usize; 4],
    /// Every project's snapshot is complete and fresh; else the counts are unknown, never zero (§ 4).
    known: bool,
    run: bool,
    bins: Vec<[u32; 4]>,
    since: Option<i64>,
}

/// Event kinds of the history, in stacking order from the bottom (M6).
const KINDS: [&str; 4] = ["close", "failed", "ask", "blocked"];

fn summary(ps: &[Value], (start, end): (i64, i64)) -> Summary {
    let mut seen = std::collections::HashMap::new();
    for p in ps {
        for t in p["tasks"].as_array().into_iter().flatten() {
            seen.insert(format!("{}/{}", p["issues"].as_str().unwrap_or(""), t["n"]), t["state"].as_str().unwrap_or("").to_string());
        }
    }
    let mut bins = vec![[0; 4]; 12];
    let mut ev = std::collections::HashSet::new();
    for p in ps {
        for e in p["events"].as_array().into_iter().flatten() {
            let (t, kind) = (e[0].as_i64().unwrap_or(0), e[1].as_str().unwrap_or(""));
            let Some(k) = KINDS.iter().position(|x| *x == kind) else { continue };
            if t >= start && t < end && ev.insert(format!("{} {} {kind}", p["issues"], e[2])) {
                bins[(((t - start) * 12 / (end - start)) as usize).min(11)][k] += 1;
            }
        }
    }
    let since = ps.iter().filter_map(|p| p["since"].as_i64()).max();
    let t = crate::snapshot::totals(seen.values().map(String::as_str));
    let known = ps.iter().all(|p| p["complete"] == true);
    Summary { counts: [t["doing"] + t["review"], t["ask"], t["ready"], t["failed"]], known, run: ps.iter().any(|p| p["focus"].as_array().is_some_and(|f| !f.is_empty())), bins, since }
}

struct QueuedAction {
    id: String,
    root: PathBuf,
    message:Value,
    context: Option<coreq::telemetry::capture::TraceContext>,
    dispatch: tracing::Dispatch,
}
struct ActionQueue {
    sender: std::sync::mpsc::SyncSender<QueuedAction>,
    worker: std::thread::JoinHandle<()>,
    allowed:&'static [&'static str],
}
impl ActionQueue {
    fn start(install: PathBuf,allowed:&'static [&'static str],execute:fn(&Path,&Path,&Value)->crate::Res<Value>,report: impl Fn(PathBuf, String, crate::Res<Value>) + Send + 'static) -> crate::Res<Self> {
        let (sender, receiver)=std::sync::mpsc::sync_channel::<QueuedAction>(16);
        let operation=if allowed==["capacity"] {"tray.capacity.execute"}else{"tray.focus.execute"};
        let worker=std::thread::Builder::new().name(operation.into()).spawn(move || {
            while let Ok(job)=receiver.recv() {
                tracing::dispatcher::with_default(&job.dispatch,|| {
                    let _context=job.context.as_ref().map(|c|c.span().entered());
                    let result=coreq::telemetry::capture::operation(operation,||std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        if !crate::host::project_paths(&install)?.iter().any(|r|r==&job.root) {
                            return Err("Project is no longer registered; command not applied".into());
                        }
                        execute(&install,&job.root,&job.message)
                    })).unwrap_or_else(|_|Err("Command panicked; result unknown; check service status before retry".into())),|_|"command-failed");
                    report(job.root.clone(),job.id.clone(),result);
                });
            }
        }).map_err(|e|format!("Cannot start command worker: {e}"))?;
        Ok(Self{sender,worker,allowed})
    }
    fn submit(&self,message:&Value) -> crate::Res<()> {
        let action=message["a"].as_str().unwrap_or("");let id=message["id"].as_str().unwrap_or("");
        let name=match action {"pause"=>"tray.pause.dispatch","resume"=>"tray.resume.dispatch","focus"=>"tray.focus.dispatch","capacity"=>"tray.capacity.dispatch",_=>"tray.invalid.dispatch"};
        coreq::telemetry::capture::operation(name,|| {
            if !self.allowed.contains(&action) {return Err(("invalid-command","Unsupported queued command"));}
            if id.is_empty() || id.len()>64 || !id.bytes().all(|b|b.is_ascii_alphanumeric()||b==b'-') {
                return Err(("invalid-id","Invalid command correlation id; action not submitted"));
            }
            if serde_json::to_vec(message).map_or(true,|bytes|bytes.len()>20<<10) {return Err(("request-too-large","Command payload too large; action not submitted"));}
            let root=message["root"].as_str().filter(|root|!root.is_empty()).ok_or(("invalid-root","Project root missing; action not submitted"))?;
            if self.worker.is_finished() {return Err(("worker-unavailable","Command worker unavailable; action not submitted"));}
            self.sender.try_send(QueuedAction {id:id.into(),root:root.into(),message:message.clone(),
                context:coreq::telemetry::capture::TraceContext::current(),dispatch:tracing::dispatcher::get_default(Clone::clone)})
                .map_err(|error|match error {
                    std::sync::mpsc::TrySendError::Full(_)=>("queue-full","Command queue full; action not submitted"),
                    std::sync::mpsc::TrySendError::Disconnected(_)=>("worker-unavailable","Command worker unavailable; action not submitted"),
                })
        },|(code,_)|*code).map_err(|(_,message)|message.into())
    }
}

fn queued_focus(_install:&Path,root:&Path,message:&Value)->crate::Res<Value> {
    let receipt=act(message["a"].as_str().unwrap_or(""),root,message["label"].as_str())?.ok_or("Command confirmation unavailable; check service status")?;
    Ok(json!({"applied_receipt":receipt}))
}

// The page owns asynchronous work; a submission result cannot acknowledge its execution.
struct WebCalls {
    next: u64,
    pending: std::collections::BTreeMap<u64, (Instant, tracing::Span)>,
    counts: [u64; 4],
    saturated: bool,
    failed: bool,
    notified: bool,
    metadata_unknown: bool,
}
impl WebCalls {
    fn new() -> Self {
        let mut calls=Self{next:0,pending:Default::default(),counts:[0;4],saturated:false,failed:false,notified:false,metadata_unknown:false};
        calls.begin("tray.web.boot",Instant::now());
        calls
    }
    fn begin(&mut self, name: &'static str, now: Instant) -> Option<u64> {
        if self.pending.len()>=64 || self.next==u64::MAX {self.fault("pending-limit");return None;}
        let id=self.next;self.next+=1;
        self.pending.insert(id,(now,coreq::telemetry::capture::detached_span(name)));
        Some(id)
    }
    fn fault(&mut self, reason: &'static str) {
        if !self.failed {
            self.failed=true;
            eprintln!("taskq tray: interface outcomes unknown ({reason}); check service status before retry");
            let _=coreq::telemetry::capture::operation("tray.web.delivery",||Err::<(),_>(reason),|code|*code);
        }
    }
    fn finish(&mut self, id: u64, result: &'static str) {
        let Some((_,span))=self.pending.remove(&id) else {return};
        match result {
            "ok" => {span.record("coreq_result","observed");}
            "script-error"|"submit-error" => {
                span.record("coreq_result","observed");span.record("coreq_error",result);self.fault(result);
            }
            _ => self.fault(result), // no callback is not proof that the script failed or did not run
        }
    }
    fn expire(&mut self, now: Instant) {
        let expired:Vec<_>=self.pending.iter().filter(|(_, (start,_))|now.duration_since(*start)>=Duration::from_secs(30)).map(|(id,_)|*id).collect();
        for id in expired {self.finish(id,"ack-timeout");}
    }
    fn metadata(&mut self, row:&Value)->bool {
        let value=match web_observation(row){Ok(value)=>value,Err(_)=>{self.fault("invalid-web-observation");return false;}};
        if let Err(error)=coreq::telemetry::capture::observe_linked(value)&&!self.metadata_unknown {
            self.metadata_unknown=true;
            eprintln!("taskq tray: activity history incomplete ({error}); command confirmations remain separate");
            return true;
        }
        false
    }
    fn errors(&mut self, message: &Value) {
        let Some(values)=message["counts"].as_array().filter(|v|v.len()==4) else {self.fault("invalid-error-report");return;};
        let mut counts=[0;4];
        for (i,value) in values.iter().enumerate() {
            let Some(n)=value.as_u64().filter(|n|*n>=self.counts[i] && *n<=u32::MAX as u64) else {self.fault("invalid-error-report");return;};
            counts[i]=n;
        }
        let Some(saturated)=message["saturated"].as_bool() else {self.fault("invalid-error-report");return;};
        for (i,class) in ["script-error","unhandled-rejection","resource-error","evaluation-error"].iter().enumerate() {
            let delta=counts[i]-self.counts[i];
            if delta>0 {
                // Page-wide counts have no reliable initiating click; do not invent a parent.
                let span=coreq::telemetry::capture::detached_span("tray.web.error");
                if !span.is_disabled() {tracing::info!(parent:&span,calls=delta);}
                span.record("coreq_result","observed");span.record("coreq_error",*class);
            }
        }
        self.counts=counts;
        if saturated && !self.saturated {
            self.saturated=true;
            let _=coreq::telemetry::capture::operation("tray.web.error-counts",||Err::<(),_>("count-saturated"),|code|*code);
            self.fault("error-count-saturated");
        }
        else if counts.iter().any(|n|*n>0) {self.fault("page-error");}
    }
}

// Accept only metadata named by the actual bundled modules; the WebView supplies no content fields.
fn web_observation(row: &Value) -> crate::Res<Value> {
    let object=row.as_object().ok_or("invalid-web-observation")?;
    if row.to_string().len()>4096||object.keys().any(|key|!matches!(key.as_str(),"kind"|"source"|"operation"|"trace"|"span"|"parent"|"started_ms"|"duration_us"|"outcome"|"error"|"counts")) {return Err("invalid-web-observation".into());}
    static INVENTORY:std::sync::OnceLock<Vec<String>>=std::sync::OnceLock::new();
    let inventory=INVENTORY.get_or_init(||serde_json::from_str(include_str!(concat!(env!("OUT_DIR"),"/web/operations.json"))).expect("build-owned web inventory"));
    if !inventory.iter().any(|name|row["operation"].as_str()==Some(name)) {return Err("unknown-web-operation".into());}
    let (kind,source)=match row["kind"].as_str() {
        Some("ui.start")=>("method.start","action"),Some("ui")=>("method","action"),
        Some("method")=>("method","diagnostic"),Some("method.aggregate")=>("method.aggregate","aggregate"),
        Some("effect.start")=>("method.start","effect"),Some("effect")=>("method","effect"),
        Some("effect.wait")=>("effect.wait","effect"),Some("collection")=>("collection","web"),
        Some("diagnostic")=>("diagnostic","web"),_=>return Err("invalid-web-kind".into()),
    };
    if !matches!(row["outcome"].as_str(),Some("ok"|"failed"|"unknown"))||!matches!(row["error"].as_str(),Some("none"|"javascript-error"|"unowned-async"|"completion-timeout"|"collection-incomplete")) {return Err("invalid-web-outcome".into());}
    if let Some(counts)=row.get("counts") {
        let counts=counts.as_object().ok_or("invalid-web-counts")?;
        if counts.len()>6||counts.iter().any(|(key,value)|!matches!(key.as_str(),"calls"|"finished"|"failed"|"unknown"|"lost"|"pending")||value.as_u64().is_none_or(|n|n>9007199254740991)) {return Err("invalid-web-counts".into());}
    }
    let mut value=row.clone();
    value["product"]=json!("taskq");value["version"]=json!(crate::platform::VERSION);value["process_id"]=json!(std::process::id());
    value["kind"]=json!(kind);value["source"]=json!(source);
    let mut unknown=serde_json::Map::new();
    for key in ["trace","span","parent"] {
        if value[key].is_null() {value[key]=Value::Null;unknown.insert(key.into(),json!("aggregate-unlinked"));}
        else if value[key].as_str().is_none_or(|id|coreq::telemetry::capture::TraceContext::decode(&format!("1 {id} {id}")).is_err()) {return Err("invalid-web-context".into());}
    }
    if !unknown.is_empty(){value["unknown"]=Value::Object(unknown);}
    coreq::telemetry::Event::new(value.clone())?;
    Ok(value)
}

fn web_result(value: &str) -> &'static str {
    match value {"\"ok\""=>"ok","\"script-error\""=>"script-error",_=>"invalid-ack"}
}
fn web_script(script: &str) -> String {
    // eval catches syntax errors too; the callback contains only a fixed result code.
    format!("(()=>{{try{{(0,eval)({});return 'ok';}}catch(_){{try{{window.taskqWebFault('evaluation');}}catch(_){{}}return 'script-error';}}}})()",json!(script))
}
fn web_eval(wv: &WebView, script: &str, name: &'static str, proxy: &tao::event_loop::EventLoopProxy<Ev>, calls: &mut WebCalls) {
    if calls.pending.contains_key(&0) {calls.fault("page-not-ready");return;}
    let Some(id)=calls.begin(name,Instant::now()) else {return};
    let p=proxy.clone();
    if wv.evaluate_script_with_callback(&web_script(script),move |value| {let _=p.send_event(Ev::Evaluated(id,web_result(&value)));}).is_err() {
        calls.finish(id,"submit-error");
    }
}

pub fn run(install: PathBuf, snapshot: Option<PathBuf>, routes: Vec<String>) -> Result<(), String> {
    if snapshot.is_none() && !alone(&install) {
        return Err("taskq tray already runs on this host".into());
    }
    if let Some(r) = routes.iter().find(|r| r.as_str() != "icon" && !ROUTES.contains(&r.as_str())) {
        return Err(format!("unknown route {r}; known: icon, {}", ROUTES.join(", ")));
    }
    #[allow(unused_mut)]
    let mut event_loop = EventLoopBuilder::<Ev>::with_user_event().build();
    #[cfg(target_os = "macos")]
    {
        // menu bar only: no Dock icon, no app menu
        use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};
        event_loop.set_activation_policy(ActivationPolicy::Accessory);
        // a snapshot never takes the owner's focus (taskq.md R12)
        event_loop.set_activate_ignoring_other_apps(snapshot.is_none());
    }
    let proxy = event_loop.create_proxy();
    TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
        if let TrayIconEvent::Click { rect, button_state: MouseButtonState::Up, .. } = e {
            let _ = proxy.send_event(Ev::Click(rect));
        }
    }));
    let proxy = event_loop.create_proxy();
    std::thread::spawn(move || {
        while proxy.send_event(Ev::Tick).is_ok() {
            std::thread::sleep(Duration::from_secs(2));
        }
    });
    let proxy = event_loop.create_proxy();
    let replies=proxy.clone();
    let focus_actions=ActionQueue::start(install.clone(),&["pause","resume","focus"],queued_focus,move |root,id,result| {
        let mut reply=result.unwrap_or_else(|error|json!({"text":error}));reply["root"]=json!(root);reply["id"]=json!(id);
        let _=coreq::telemetry::capture::operation("tray.focus.report",||replies.send_event(Ev::Answer(json!({"focus_receipt":reply}))),|_|"ui-closed");
    })?;
    let replies=proxy.clone();
    let capacity_actions=ActionQueue::start(install.clone(),&["capacity"],capacity,move |root,id,result| {
        let mut reply=result.unwrap_or_else(|error|json!({"state":"unknown","reason":error}));reply["root"]=json!(root);reply["id"]=json!(id);
        let _=coreq::telemetry::capture::operation("tray.capacity.report",||replies.send_event(Ev::Answer(json!({"capmsg":reply}))),|_|"ui-closed");
    })?;
    let mut settings = Settings::load(&install);
    let mut ui: Option<(Window, WebView)> = None;
    let mut tray: Option<TrayIcon> = None;
    let (mut shown, mut icon_sig, mut dark) = (String::new(), String::new(), true);
    let mut hidden_at = Instant::now() - Duration::from_secs(1);
    let mut shown_at = hidden_at;
    let mut anchor: Option<Rect> = None;
    let mut step = 0usize;
    // snapshot pages wider than their window (#1073): `tray --snapshot` fails naming them
    let mut overflow: Vec<String> = Vec::new();
    let mut adding = Value::Null;
    let mut moved = false;
    let mut asked = false;
    let mut bursts = notify::Bursts::new();
    let mut shot_wait=None;
    let mut web=WebCalls::new();
    event_loop.run(move |event, target, flow| {
        crate::telemetry::diagnostic::poll(&install);
        *flow = ControlFlow::Wait;
        let native=match &event {
            Event::WindowEvent{event:WindowEvent::Resized(_),..}=>Some("tray.window.resize"),
            Event::WindowEvent{event:WindowEvent::Moved(_),..}=>Some("tray.window.move"),
            Event::WindowEvent{event:WindowEvent::Focused(_),..}=>Some("tray.window.focus"),
            Event::WindowEvent{event:WindowEvent::ThemeChanged(_),..}=>Some("tray.window.theme"),
            Event::WindowEvent{event:WindowEvent::Destroyed,..}=>Some("tray.window.destroy"),
            Event::WindowEvent{event:WindowEvent::CloseRequested,..}=>Some("tray.window.close"),
            Event::RedrawRequested(_)=>Some("tray.window.redraw-request"),_=>None,
        };
        if let Some(name)=native {let _=coreq::telemetry::capture::operation(name,||Ok::<_,&str>(()),|error|*error);}

        web.expire(Instant::now());
        if !web.pending.contains_key(&0)&&let Some((_,wv))=&ui&&let Some(selection)=crate::telemetry::diagnostic::take_web_selection(){
            web_eval(wv,&format!("taskq.receive({})",json!({"diagnose":selection})),"tray.web.diagnose",&proxy,&mut web);
        }
        if web.failed && !web.notified {
            web.notified=true;
            if let Some(t)=&tray {let _=t.set_tooltip(Some("TaskQ: interface outcomes unknown; check service status"));}
            if snapshot.is_none() && let Some((_,wv))=&ui {web_eval(wv,"window.taskqWebFault('native')","tray.web.notice",&proxy,&mut web);}
        }
        if web.failed && snapshot.is_some() && web.pending.is_empty() {std::process::exit(1);}
        let push = |web: &mut WebCalls, ui: &Option<(Window, WebView)>, st: &Value| {
            if let Some((_, wv)) = ui {
                if !web.failed {web_eval(wv,&format!("taskq.receive({st})"),"tray.web.receive",&proxy,web);}
            }
        };
        match event {
            Event::NewEvents(StartCause::Init) => {
                let b = WindowBuilder::new().with_title("taskq").with_decorations(false).with_resizable(false).with_always_on_top(settings.pin.top)
                    .with_visible(snapshot.is_some()).with_inner_size(LogicalSize::new(320.0, 400.0)).with_position(tao::dpi::LogicalPosition::new(40.0, 40.0));
                // a snapshot renders off the visible screen (taskq.md R12)
                let b = if snapshot.is_some() { b.with_position(tao::dpi::LogicalPosition::new(-20000.0, -20000.0)).with_always_on_top(false).with_focused(false) } else { b };
                #[cfg(windows)]
                let b = tao::platform::windows::WindowBuilderExtWindows::with_skip_taskbar(b, true);
                let p = proxy.clone();
                let built = b.build(target).map_err(|e| e.to_string()).and_then(|w| {
                    // only our page: a link goes to the browser through the bridge
                    WebViewBuilder::new().with_accept_first_mouse(true).with_html(HTML).with_navigation_handler(|u| !u.starts_with("http"))
                        .with_ipc_handler(move |r| {
                            let _ = p.send_event(Ev::Ipc(r.into_body()));
                        })
                        .build(&w).map(|v| (w, v)).map_err(|e| e.to_string())
                });
                match built {
                    Ok(x) => {
                        dark = is_dark(&x.0);
                        ui = Some(x);
                    }
                    Err(e) => {
                        eprintln!("taskq tray: {e}");
                        *flow = ControlFlow::Exit;
                        return;
                    }
                }
                if snapshot.is_some() {
                    return;
                }
                // macOS needs a running event loop before the icon exists
                match TrayIconBuilder::new().with_icon(rgba_icon(&compact(false, dark, 2.0))).with_tooltip("taskq").build() {
                    Ok(t) => tray = Some(t),
                    Err(e) => {
                        eprintln!("taskq tray: {e}");
                        *flow = ControlFlow::Exit;
                    }
                }
                let p = proxy.clone();
                notify::watch(move |id| {
                    let _ = p.send_event(Ev::Note(id));
                });
                // a pinned window survives a tray restart at its last position; off-screen: at the tray icon (M13)
                if let (Some((w, _)), true) = (&ui, settings.pin.pinned) {
                    let saved = w.available_monitors().find(|m| key(m) == settings.pin.display).zip(settings.pin.pos.get(&settings.pin.display))
                        .filter(|(m, p)| on_screen(**p, (m.position().x, m.position().y, m.size().width as i32, m.size().height as i32)));
                    match (saved, tray.as_ref().and_then(TrayIcon::rect)) {
                        (Some((_, p)), _) => w.set_outer_position(PhysicalPosition::new(p.0, p.1)),
                        (None, Some(r)) => place(w, r),
                        _ => {}
                    }
                    w.set_visible(true);
                }
            }
            Event::WindowEvent { event: WindowEvent::Moved(p), .. } if settings.pin.pinned && snapshot.is_none() => {
                let Some((w, _)) = &ui else { return };
                let Some(m) = w.current_monitor() else { return };
                let s = w.outer_size();
                let c = if cfg!(target_os = "macos") { clamp((p.x, p.y), (s.width as i32, s.height as i32), work_area(&m)) } else { (p.x, p.y) };
                if c != (p.x, p.y) {
                    w.set_outer_position(PhysicalPosition::new(c.0, c.1));
                }
                settings.pin.display = key(&m);
                settings.pin.pos.insert(settings.pin.display.clone(), c);
                moved = true;
            }
            Event::WindowEvent { event: WindowEvent::Focused(false), .. } if snapshot.is_none() && !settings.pin.pinned => {
                if let Some((w, _)) = &ui {
                    if blur_refocuses(shown_at.elapsed()) {
                        w.set_focus();
                        return;
                    }
                    w.set_visible(false);
                    hidden_at = Instant::now();
                }
            }
            Event::WindowEvent { event: WindowEvent::ThemeChanged(_), .. } => {
                if let Some((w, _)) = &ui {
                    dark = is_dark(w);
                    icon_sig.clear();
                    shown.clear();
                }
            }
            Event::UserEvent(Ev::Tick) if snapshot.is_none() => {
                // another version became current: exit, a service starts the current one (§ 4)
                if !alone(&install) || crate::platform::tool(&install).current().is_some_and(|v| v != crate::platform::VERSION) {
                    *flow = ControlFlow::Exit;
                    return;
                }
                let _ = fs::write(install.join("tray"), std::process::id().to_string());
                // a drag sends many moves: save the last one per tick
                if std::mem::take(&mut moved) {
                    settings.save(&install);
                }
                let st = state(&install, &settings, dark, None, &adding);
                if !web.failed {refresh_icon(&tray, &st, &settings, dark, &mut icon_sig);}
                notes(&install, &mut settings, &st, &mut asked, &mut bursts);
                let mut comparable=st.clone();comparable.as_object_mut().expect("tray snapshot object").remove("now");
                let sig=comparable.to_string();
                if sig != shown && !web.pending.contains_key(&0) {
                    shown = sig;
                    push(&mut web,&ui, &st);
                }
            }
            Event::UserEvent(Ev::Click(rect)) => {
                let Some((w, _)) = &ui else { return };
                anchor = Some(rect);
                // pinned: the click raises the window, never closes it (M13)
                if settings.pin.pinned {
                    w.set_visible(true);
                    w.set_focus();
                    return;
                }
                // a click on the icon while open first blurs the popover: that click only closes it
                if w.is_visible() || hidden_at.elapsed() < Duration::from_millis(300) {
                    w.set_visible(false);
                    return;
                }
                place(w, rect);
                w.set_visible(true);
                w.set_focus();
                shown_at = Instant::now();
            }
            Event::UserEvent(Ev::Answer(receipt)) => push(&mut web,&ui, &receipt),
            // a notification click opens the menu (or raises the pinned window) on that card (M14)
            Event::UserEvent(Ev::Note(id)) => {
                let Ok(id) = serde_json::from_str::<Value>(&id) else { return };
                let Some((w, _)) = &ui else { return };
                if !settings.pin.pinned && let Some(r) = anchor.or_else(|| tray.as_ref().and_then(TrayIcon::rect)) {
                    place(w, r);
                }
                w.set_visible(true);
                w.set_focus();
                if id["root"].is_string() {
                    push(&mut web,&ui, &json!({"open": {"root": id["root"], "n": id["n"], "kind": id["k"], "title": id["t"]}}));
                }
            }
            Event::UserEvent(Ev::Evaluated(id,result)) => {
                web.finish(id,result);
                if web.pending.is_empty() && let Some(next)=shot_wait.take() {let _=proxy.send_event(next);}
            }
            Event::UserEvent(Ev::Ipc(m)) => {
                let Ok(m) = serde_json::from_str::<Value>(&m) else {web.fault("invalid-ipc");return;};
                let Some((w, _)) = &ui else { return };
                if m["a"]=="web-error" {web.errors(&m);return;}
                if m["a"]=="web-observation" {
                    if web.metadata(&m["row"])&&let Some(tray)=&tray {
                        let _=tray.set_tooltip(Some("TaskQ: activity history incomplete; check each command confirmation"));
                    }
                    return;
                }
                let context=match m["context"].as_str(){Some(text)=>match coreq::telemetry::capture::TraceContext::decode(text){Ok(context)=>Some(context),Err(_)=>{web.fault("invalid-web-context");return;}},None=>None};
                let _continuation=context.as_ref().map(|context|context.span().entered());
                if web.failed {return;}
                match m["a"].as_str().unwrap_or("") {
                    "answer" if snapshot.is_none() => {
                        let (p, install) = (proxy.clone(), install.clone());
                        std::thread::spawn(coreq::telemetry::capture::bind(move || { let _ = p.send_event(Ev::Answer(answer(&install, &m))); }));
                    }
                    "overflow" if snapshot.is_some() => overflow.push(format!("{} by {} px", m["shot"].as_str().unwrap_or("?"), m["x"])),
                    "ready" => {
                        web.finish(0,"ok");
                        if snapshot.is_some() {
                            let _ = proxy.send_event(Ev::Shot);
                            return;
                        }
                        push(&mut web,&ui, &state(&install, &settings, dark, None, &adding));
                    }
                    "size" | "resize" => {
                        let Some((width, height)) = m["w"].as_f64().zip(m["h"].as_f64()).filter(|(w, h)| w.is_finite() && h.is_finite()) else { return };
                        let Some(scale) = m["d"].as_f64().filter(|k| k.is_finite() && (0.5..=8.0).contains(k)) else { return };
                        let (max_w, max_h) = w.current_monitor().or_else(|| w.primary_monitor()).map_or((1920.0, 800.0), |monitor| {
                            let area = work_area(&monitor);
                            (f64::from(area.2) / scale, f64::from(area.3) / scale)
                        });
                        let size = (width.max(240.0).min(max_w), height.max(120.0).min(max_h));
                        let measurement = json!({"w": size.0, "h": if m["a"] == "resize" { size.1 } else { height.max(120.0) }, "d": scale, "g": m["g"]});
                        let (width, height) = popover_size(&measurement, max_h * scale / w.scale_factor(), w.scale_factor());
                        w.set_inner_size(PhysicalSize::new(width.min((max_w * scale) as u32), height));
                        if m["a"] == "resize" {
                            settings.pin.size = Some(size);
                            moved = true;
                        }
                        // pinned: the window stays where it was put; a move then keeps it in the work area
                        if m["a"] == "size" && let (Some(r), false) = (anchor, settings.pin.pinned) {
                            place(w, r);
                        }
                    }
                    "resize-end" => { settings.save(&install); }
                    "hide" => w.set_visible(false),
                    "top" => {
                        settings.pin.top = !settings.pin.top;
                        w.set_always_on_top(settings.pin.top);
                        settings.save(&install);
                        push(&mut web,&ui, &state(&install, &settings, dark, None, &adding));
                    }
                    "pin" => {
                        settings.pin.pinned = !settings.pin.pinned;
                        if let (true, Ok(p), Some(m)) = (settings.pin.pinned, w.outer_position(), w.current_monitor()) {
                            settings.pin.display = key(&m);
                            settings.pin.pos.insert(settings.pin.display.clone(), (p.x, p.y));
                        }
                        // unpin: back to the popover at the tray icon
                        if let (false, Some(r)) = (settings.pin.pinned, anchor.or_else(|| tray.as_ref().and_then(TrayIcon::rect))) {
                            place(w, r);
                        }
                        settings.save(&install);
                        w.set_focus();
                        push(&mut web,&ui, &state(&install, &settings, dark, None, &adding));
                    }
                    "drag" => {
                        let _ = w.drag_window();
                    }
                    "resources" => {
                        std::thread::spawn(coreq::telemetry::capture::bind(resources));
                    }
                    "open" => {
                        if let Some(u) = m["url"].as_str().filter(|u| u.starts_with("https://") || u.starts_with("http://")) {
                            open(u);
                        }
                    }
                    "notify" => {
                        if notify::set(&mut settings.notify, &m) {
                            settings.save(&install);
                            notes(&install, &mut settings, &Value::Null, &mut asked, &mut bursts);
                        }
                        push(&mut web,&ui, &state(&install, &settings, dark, None, &adding));
                    }
                    "notify-test" if snapshot.is_none() => {
                        if let Some(k) = m["kind"].as_str().filter(|k| notify::KINDS.contains(k)) {
                            notify::post(&notify::sample(&settings.notify, k, now()));
                        }
                    }
                    "notify-os" => notify::open_settings(),
                    "set" => {
                        settings.set(&m);
                        settings.save(&install);
                        let st = state(&install, &settings, dark, None, &adding);
                        refresh_icon(&tray, &st, &settings, dark, &mut icon_sig);
                        push(&mut web,&ui, &st);
                    }
                    // `Edit limits` after the page's confirm: the same write as `service capacity` (§ 5), this host only
                    "capacity" => {
                        let root = m["root"].as_str().unwrap_or("");
                        if let Err(error)=capacity_actions.submit(&m) {
                            push(&mut web,&ui, &json!({"capmsg": {"root": root, "id": m["id"], "state":"rejected","reason":error}}));
                        }
                    }
                    // Add project (§ 4): the folder comes only from the OS picker, never from the page
                    "add" => {
                        adding = json!({"state": "picking"});
                        push(&mut web,&ui, &state(&install, &settings, dark, None, &adding));
                        let p = proxy.clone();
                        std::thread::spawn(coreq::telemetry::capture::bind(move || {
                            let v = pick().map_or(Value::Null, |d| classify(&d, selftest));
                            let _ = p.send_event(Ev::Added(v));
                        }));
                    }
                    "add-cancel" => {
                        adding = Value::Null;
                        push(&mut web,&ui, &state(&install, &settings, dark, None, &adding));
                    }
                    "add-install" if adding["kind"] == "taskq" && adding["state"] == "picked" => {
                        adding["state"] = json!("installing");
                        push(&mut web,&ui, &state(&install, &settings, dark, None, &adding));
                        let (p, a, inst) = (proxy.clone(), adding.clone(), install.clone());
                        std::thread::spawn(coreq::telemetry::capture::bind(move || {
                            let _ = p.send_event(Ev::Added(install_service(&inst, a)));
                        }));
                    }
                    "add-setup" if matches!(adding["kind"].as_str(), Some("new" | "plain")) => {
                        let text = format!("Set up {} as a taskq project", adding["name"].as_str().unwrap_or("this folder"));
                        ai(&install, "ai-send", &json!({"text": text, "onboard": adding["path"]}));
                        adding = Value::Null;
                        push(&mut web,&ui, &state(&install, &settings, dark, None, &adding));
                    }
                    a if a.starts_with("ai-") => {
                        let mut m = m.clone();
                        m["onboard"] = Value::Null;
                        ai(&install, a, &m);
                        push(&mut web,&ui, &state(&install, &settings, dark, None, &adding));
                    }
                    a => {
                        // only a registered project (§ 4): the page never names a path of its own
                        let root = m["root"].as_str().unwrap_or("");
                        if roots(&install).iter().any(|r| r == root) {
                            let result=if matches!(a,"pause"|"resume"|"focus") {
                                focus_actions.submit(&m)
                            } else {act(a,Path::new(root),m["label"].as_str()).map(|_|())};
                            if let Err(error) = result {
                                let key=if matches!(a,"pause"|"resume"|"focus") {"focus_receipt"} else {"action_error"};
                                push(&mut web,&ui, &json!({key:{"root":root,"id":m["id"],"text":error}}));
                            }
                        } else if matches!(a,"pause"|"resume"|"focus") {
                            push(&mut web,&ui,&json!({"focus_receipt":{"root":root,"id":m["id"],"text":"Project is no longer registered; command not submitted"}}));
                        }
                    }
                }
            }
            Event::UserEvent(Ev::Added(v)) => {
                adding = v;
                push(&mut web,&ui, &state(&install, &settings, dark, None, &adding));
                if let (Some((w, _)), false) = (&ui, adding.is_null()) {
                    w.set_visible(true);
                    w.set_focus();
                }
            }
            Event::UserEvent(Ev::Shot | Ev::Shot2) if web.failed => {}
            // snapshot mode: per step push a state, wait for layout, capture, next
            Event::UserEvent(Ev::Shot) => {
                let dir = snapshot.clone().unwrap_or_default();
                let steps = shots(&install, &routes);
                if step == 0 {
                    let _ = fs::create_dir_all(&dir);
                }
                if step == 0 && (routes.is_empty() || routes.iter().any(|r| r == "icon")) {
                    let ps: Vec<Value> = roots(&install).iter().map(|r| project(&install, r)).collect();
                    let s = summary(&ps, day(now()));
                    // this Mac's menu-bar height and backing scale (A5); the names record them
                    let (k, dpr) = (menu_bar_scale(), backing_scale());
                    let size = format!("{}pt@{dpr}x", 22.0 * k);
                    let mut views = vec![("summary", icon::summary())];
                    views.extend((!settings.icon.is_null()).then(|| ("custom", settings.icon.clone())));
                    for (theme, d) in [("dark", true), ("light", false)] {
                        for (mode, run) in [("run", true), ("pause", false)] {
                            let s = Summary { run, ..s.clone() };
                            let _ = fs::write(dir.join(format!("icon-compact-{mode}-{theme}-{size}.png")), compact(run, d, k * dpr).png());
                            for (name, c) in &views {
                                if let Ok(img) = draw(c, &s, d, k, dpr) {
                                    let _ = fs::write(dir.join(format!("icon-{name}-{mode}-{theme}-{size}.png")), img.png());
                                }
                            }
                        }
                    }
                }
                let Some((theme, page, font, script)) = steps.get(step).cloned() else {
                    if !web.pending.is_empty() {
                        shot_wait=Some(Ev::Shot);return;
                    }
                    println!("snapshots in {}", dir.display());
                    if !overflow.is_empty() {
                        eprintln!("horizontal overflow: {}", overflow.join(", "));
                        std::process::exit(1);
                    }
                    *flow = ControlFlow::Exit;
                    return;
                };
                let mut s = settings.clone();
                s.font = font;
                push(&mut web,&ui, &state(&install, &s, theme == "dark", Some((theme, &page)), &Value::Null));
                if let (Some((_, wv)), false) = (&ui, script.is_empty()) {
                    web_eval(wv,script,"tray.web.fixture",&proxy,&mut web);
                }
                let p = proxy.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(800));
                    let _ = p.send_event(Ev::Shot2);
                });
            }
            Event::UserEvent(Ev::Shot2) => {
                let Some((_, wv)) = &ui else { return };
                let dir = snapshot.clone().unwrap_or_default();
                let p = proxy.clone();
                let name = format!("popover-{}.png", shot_name(&install, &routes, step));
                if !web.pending.is_empty() {
                    shot_wait=Some(Ev::Shot2);return;
                }
                web_eval(wv,&format!("taskq.measure({})",json!(&name)),"tray.web.measure",&proxy,&mut web);
                step += 1;
                capture(wv, dir.join(name), move || {
                    let _ = p.send_event(Ev::Shot);
                });
            }
            _ => {}
        }
    })
}

/// Physical inner size for a `size` message in CSS px; `max` is logical. The page renders at its own pixel ratio `d`:
/// Windows Text size scales WebView2 but not the window, so CSS px become logical px by `d / scale`. A menu taller than
/// `max` scrolls; the vertical scrollbar takes `g` of the width, so the window adds it and the menu keeps its full width
/// (#1073). The width rounds up: a fraction of a pixel short also scrolls horizontally.
fn popover_size(m: &Value, max: f64, scale: f64) -> (u32, u32) {
    let (w, h) = (m["w"].as_f64().unwrap_or(320.0), m["h"].as_f64().unwrap_or(400.0));
    let k = m["d"].as_f64().filter(|d| *d > 0.0).map_or(1.0, |d| d / scale);
    let w = if h * k > max { w + m["g"].as_f64().unwrap_or(0.0) } else { w };
    ((w * k * scale).ceil() as u32, ((h * k).min(max) * scale).round() as u32)
}

/// Reports a snapshot page that scrolls horizontally or clips content inside the menu (#1073), and a button whose label
/// is wider than the button, so the text crosses its border (#1049).

/// The `actions` snapshot: a refused custom range (menu.md M6), without applying a setting.
const ACTIONS: &str = "(()=>{const q=s=>document.querySelector(s),put=(s,v)=>{const i=q(s);i.value=v;i.dispatchEvent(new Event('input',{bubbles:true}));};
q('[data-route=cadence]').click();q('[data-v=custom]').click();put('#hs','2026-10-13T10:00');put('#he','2026-10-13T09:00');q('[data-a=range]').click();})()";

/// The `scroll` snapshots: Home with each bounded AI block at its start, middle and end, realigned by the page's
/// `taskq.settle`, the step a reader's scroll runs once it settles (#1115). Called directly: an off-screen capture window may
/// deliver the scroll event after the capture.
const SCROLLS: [(&str, &str); 3] = [
    ("start", "document.querySelectorAll('[data-bound]').forEach(c=>{c.scrollTop=0;taskq.settle(c);})"),
    ("middle", "document.querySelectorAll('[data-bound]').forEach(c=>{c.scrollTop=(c.scrollHeight-c.clientHeight)/2;taskq.settle(c);})"),
    ("end", "document.querySelectorAll('[data-bound]').forEach(c=>{c.scrollTop=c.scrollHeight;taskq.settle(c);})"),
];

/// `tray --snapshot --route` names: `actions` is Home with the [`ACTIONS`] script, `scroll` Home with the [`SCROLLS`]
/// (only when named), `project` the first project's page.
const ROUTES: [&str; 13] = ["home", "actions", "scroll", "project", "projects", "attention", "settings", "appearance", "cadence", "notifications", "agents", "services", "about"];

/// Snapshot steps: theme, page (`home` or a project root), text size, a script run on the page; only `routes` unless empty.
fn shots(install: &Path, routes: &[String]) -> Vec<(&'static str, String, u64, &'static str)> {
    let first = roots(install).into_iter().next();
    ["dark", "light"].iter().flat_map(|t| {
        let mut v = vec![(*t, "home".to_string(), 16, ""), (*t, "home".to_string(), 20, ""), (*t, "home".to_string(), 16, ACTIONS)];
        for font in [16, 20] { v.extend(SCROLLS.iter().map(|(_, js)| (*t, "home".to_string(), font, *js))); }
        v.extend(first.clone().map(|r| (*t, r, 16, "")));
        for route in &ROUTES[4..] {
            for font in [16, 20] { v.push((*t, route.to_string(), font, "")); }
        }
        v
    }).filter(|(_, p, _, js)| if routes.is_empty() { route_of(p, js) != "scroll" } else { routes.iter().any(|r| r == route_of(p, js)) }).collect()
}

fn route_of<'a>(page: &'a str, script: &str) -> &'a str {
    if script == ACTIONS { "actions" } else if !script.is_empty() { "scroll" } else if Path::new(page).is_absolute() { "project" } else { page }
}

fn shot_name(install: &Path, routes: &[String], step: usize) -> String {
    shots(install, routes).get(step).map_or(step.to_string(), |(t, p, f, js)| {
        let suffix = if js.is_empty() { String::new() } else { format!("-{}", SCROLLS.iter().find(|(_, s)| s == js).map_or("actions", |(n, _)| n)) };
        let scroll = if route_of(p, js) == "scroll" { "-scroll" } else { "" };
        format!("{t}-{}{scroll}{suffix}-{f}", if Path::new(p).is_absolute() { "project" } else { p })
    })
}

fn state(install: &Path, s: &Settings, dark: bool, snap: Option<(&str, &str)>, add: &Value) -> Value {
    let ps: Vec<Value> = roots(install).iter().map(|r| project(install, r)).collect();
    let (start, end) = s.range(now());
    let sum = summary(&ps, (start, end));
    let url = |rgba: &Rgba| format!("data:image/png;base64,{}", base64(&rgba.png()));
    let mut st = json!({"platform": if cfg!(windows) { "windows" } else { "macos" }, "version": crate::platform::VERSION,
        "host": ps.iter().find_map(|p| p["host"].as_str()).unwrap_or("?"), "view": s.view, "font": s.font, "period": s.period, "pinned": s.pin.pinned, "top": s.pin.top, "size": s.pin.size,
        "now": now(), "service":crate::host::service_status(install),"projects": ps, "notify": notify_state(&s.notify, &ps), "agents": agents(&ps), "ai": {"target": target(install), "request": newest(install), "draft": requests_file(install)["draft"], "history": requests_file(install)["history"]}, "add": add, "history": {"start": start, "end": end, "tz": s.tz, "bins": sum.bins, "since": sum.since},
        // § 4 Data: a corrupt tray.json names its reason while the defaults show
        "settings_error": crate::data::json(&install.join("tray.json")).err()});
    if cfg!(target_os = "macos") {
        let day = summary(&ps, day(now()));
        st["icons"] = json!({"icon": url(&compact(sum.run, dark, 2.0)), "full": draw(&icon::summary(), &day, dark, 1.0, 2.0).map_or(Value::Null, |i| json!(url(&i)))});
    }
    if let Some((theme, page)) = snap {
        st["theme"] = json!(theme);
        st["page"] = json!(page);
    }
    st
}

/// Notifications for the page: OS status, host modes, per project override (null: inherit) and effective mode.
fn notify_state(n: &Value, ps: &[Value]) -> Value {
    let host: serde_json::Map<String, Value> = notify::KINDS.iter().map(|k| (k.to_string(), json!(notify::host(n, k)))).collect();
    let projects: serde_json::Map<String, Value> = ps.iter().filter_map(|p| p["root"].as_str()).map(|r| {
        (r.to_string(), notify::KINDS.iter().map(|k| (k.to_string(), json!([n["projects"][r][k].as_str().filter(|v| notify::MODES.contains(v)), notify::mode(n, r, k)]))).collect())
    }).collect();
    json!({"status": notify::status(), "failure": notify::failure(), "host": host, "projects": projects})
}

/// One tick of notifications (M14): the first banner setting asks the OS once; new events move the cursor, which is
/// saved before anything posts (a crash loses a notification, never repeats one); only an allowed OS posts.
fn notes(install: &Path, s: &mut Settings, st: &Value, asked: &mut bool, bursts: &mut notify::Bursts) {
    if !*asked && notify::status() == "not asked" && notify::wants_banner(&s.notify, &roots(install)) {
        *asked = true;
        notify::request();
    }
    let Some(ps) = st["projects"].as_array() else { return };
    let before = s.notify.clone();
    let posts = notify::due(&mut s.notify, ps, now(), bursts);
    if s.notify != before && !s.save(install) {
        s.notify = before;
        return;
    }
    if notify::status() == "allowed" {
        for p in &posts {
            notify::post(p);
        }
    }
}

/// Host-level `Agents` (§ 4): the rows of the freshest probe in any `status.json`, as menu lines; null when none has one.
fn agents(ps: &[Value]) -> Value {
    let Some(a) = ps.iter().map(|p| &p["agents"]).filter(|a| a["at"].is_u64()).max_by_key(|a| a["at"].as_u64()) else { return Value::Null };
    json!({"at": a["at"], "rows": a["rows"].as_array().into_iter().flatten().map(crate::runtime::agent_text).collect::<Vec<_>>()})
}

/// The last 24 h, the period of the icon histogram (icon-summary.config.json).
fn day(now: i64) -> (i64, i64) {
    rolling(now, 86400)
}

/// 12 whole epoch-aligned bins of `period / 12`, the newest holding `now` (partial): the grid moves one bin at a boundary (M6).
fn rolling(now: i64, period: i64) -> (i64, i64) {
    let w = period / 12;
    let end = (now.div_euclid(w) + 1) * w;
    (end - period, end)
}

/// Configuration `c` over `s` at `k` × 22 pt and `dpr` pixels per point through the shared renderer.
fn draw(c: &Value, s: &Summary, dark: bool, k: f32, dpr: f32) -> Result<Rgba, Vec<String>> {
    let d = icon::Data { counts: s.counts.map(|n| s.known.then_some(n)), bins: s.bins.clone(), paused: !s.run };
    icon::render(c, (22.0 * k * dpr).round() as u32, dpr, &d, dark)
}

fn refresh_icon(tray: &Option<TrayIcon>, st: &Value, s: &Settings, dark: bool, sig: &mut String) {
    let Some(t) = tray else { return };
    let sum = summary(st["projects"].as_array().map_or(&[][..], |v| v), day(now()));
    // Windows shows only the compact q (menu.md M1)
    let config = match s.view.as_str() {
        _ if cfg!(windows) => None,
        "full" => Some(icon::summary()),
        "custom" => Some(s.icon.clone()),
        _ => None,
    };
    let (k, dpr) = if config.is_some() { (menu_bar_scale(), backing_scale()) } else { (1.0, 2.0) };
    let new = format!("{config:?} {dark} {k} {dpr} {} {} {:?} {:?}", sum.run, sum.known, sum.counts, if config.is_some() { sum.bins.clone() } else { vec![] });
    if *sig == new {
        return;
    }
    *sig = new;
    let _ = crate::profile::measure("tray.render", || {
        let rendered = config.map(|c| draw(&c, &sum, dark, k, dpr));
        let failed = rendered.as_ref().is_some_and(Result::is_err);
        let img = rendered.and_then(Result::ok).unwrap_or_else(|| compact(sum.run, dark, k * dpr));
        let applied = t.set_icon(Some(rgba_icon(&img)));
        if k > 1.0 { fit_status_image(k); }
        if failed { Err("invalid icon render".to_string()) } else { applied.map_err(|e| e.to_string()) }
    });
    let [w, a, r, _] = sum.counts;
    let counts = if sum.known { format!("in work {w} · waiting for answer {a} · ready {r}") } else { "counts unknown (service error or stale status)".into() };
    let _ = t.set_tooltip(Some(format!("taskq: {counts}{}", if sum.run { "" } else { " · paused" })));
}

/// A blur this soon after the icon opened the popover takes the focus back instead of closing it:
/// on Windows 11 the hidden-icons flyout retakes the foreground right after its icon is clicked.
fn blur_refocuses(shown_for: Duration) -> bool {
    shown_for < Duration::from_millis(300)
}

/// Put the popover under a menu-bar icon or above a taskbar icon, inside the icon's monitor.
fn place(w: &Window, r: Rect) {
    let (ix, iy) = (r.position.x, r.position.y);
    let m = w.available_monitors().find(|m| {
        let (p, s) = (m.position(), m.size());
        ix >= p.x as f64 && ix < (p.x + s.width as i32) as f64 && iy >= p.y as f64 && iy < (p.y + s.height as i32) as f64
    });
    let (mp, ms) = m.map_or(((0, 0), (1920, 1080)), |m| ((m.position().x, m.position().y), (m.size().width as i32, m.size().height as i32)));
    let sz = w.outer_size();
    let (ww, wh) = (sz.width as i32, sz.height as i32);
    let x = (ix as i32 + r.size.width as i32 / 2 - ww / 2).clamp(mp.0, (mp.0 + ms.0 - ww).max(mp.0));
    let y = if (iy as i32) < mp.1 + ms.1 / 2 { iy as i32 + r.size.height as i32 + 4 } else { iy as i32 - wh - 4 };
    w.set_outer_position(PhysicalPosition::new(x, y.clamp(mp.1, (mp.1 + ms.1 - wh).max(mp.1))));
}

/// A display's name in `tray.json`; unnamed: its origin.
fn key(m: &tao::monitor::MonitorHandle) -> String {
    m.name().unwrap_or_else(|| format!("{},{}", m.position().x, m.position().y))
}

/// The top-left `p` lies on the area `(x, y, w, h)`.
fn on_screen(p: (i32, i32), (x, y, w, h): (i32, i32, i32, i32)) -> bool {
    p.0 >= x && p.0 < x + w && p.1 >= y && p.1 < y + h
}

/// Top-left that keeps a window of `size` inside the area `(x, y, w, h)`; a too large window keeps its top-left edge in.
fn clamp(p: (i32, i32), size: (i32, i32), (x, y, w, h): (i32, i32, i32, i32)) -> (i32, i32) {
    (p.0.clamp(x, (x + w - size.0).max(x)), p.1.clamp(y, (y + h - size.1).max(y)))
}

/// The display without menu bar and Dock, physical pixels.
#[cfg(target_os = "macos")]
fn work_area(m: &tao::monitor::MonitorHandle) -> (i32, i32, i32, i32) {
    use tao::platform::macos::MonitorHandleExtMacOS;
    let (p, s) = (m.position(), m.size());
    let full = (p.x, p.y, s.width as i32, s.height as i32);
    let Some(ptr) = m.ns_screen().filter(|p| !p.is_null()) else { return full };
    // SAFETY: tao hands out a live NSScreen; read on the main thread
    let screen = unsafe { &*(ptr as *const objc2_app_kit::NSScreen) };
    let (f, v, k) = (screen.frame(), screen.visibleFrame(), m.scale_factor());
    // AppKit counts from the bottom: top inset = menu bar, bottom inset = Dock
    let top = ((f.origin.y + f.size.height) - (v.origin.y + v.size.height)) * k;
    let (left, bottom) = ((v.origin.x - f.origin.x) * k, (v.origin.y - f.origin.y) * k);
    (p.x + left as i32, p.y + top as i32, (v.size.width * k) as i32, s.height as i32 - top as i32 - bottom as i32)
}

#[cfg(not(any(target_os = "macos", windows)))]
fn work_area(m: &tao::monitor::MonitorHandle) -> (i32, i32, i32, i32) {
    (m.position().x, m.position().y, m.size().width as i32, m.size().height as i32)
}

#[cfg(windows)]
fn work_area(m: &tao::monitor::MonitorHandle) -> (i32, i32, i32, i32) {
    use windows::Win32::{Foundation::POINT, Graphics::Gdi::{GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST}};
    let p = m.position();
    let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
    // SAFETY: the monitor handle and writable MONITORINFO are valid for these synchronous reads.
    if unsafe { GetMonitorInfoW(MonitorFromPoint(POINT { x: p.x, y: p.y }, MONITOR_DEFAULTTONEAREST), &mut info) }.as_bool() {
        let r = info.rcWork;
        (r.left, r.top, r.right - r.left, r.bottom - r.top)
    } else {
        let s = m.size();
        (p.x, p.y, s.width as i32, s.height as i32)
    }
}

#[tracing::instrument(name="tray.act",skip_all,fields(coreq_operation=match action {
    "pause"|"resume"|"focus"|"update"|"restart"|"log"|"folder"|"incident-report"|"incident-manual"|"stop"|"uninstall"=>action,
    _=>"invalid",
}))]
fn act(action: &str, root: &Path, label: Option<&str>) -> crate::Res<Option<String>> {
    let d = crate::dot(root);
    match action {
        "pause" => return crate::pause(root,false).map(Some),
        "resume" => {
            return crate::resume(root).map(|(_,receipt)|Some(receipt));
        }
        // a label of the project's open tasks, never `all` (§ 4)
        "focus" => {
            if let Some(l) = label.filter(|l| crate::status_labels(root).iter().any(|x| x == l)) {
                return crate::set_focus(root, l, None).map(Some);
            } else { return Err("Label is no longer available; refresh and choose again".into()); }
        }
        // `Update now` (menu.md M9): the service checks at once (§ 4 step 7); the page waits for the version in status.json
        "update" => {
            fs::write(d.join("update"), "").map_err(|e| e.to_string())?;
            crate::poke(root)?;
        }
        "restart" => {
            fs::write(d.join("restart"), "").map_err(|e| e.to_string())?;
            crate::poke(root)?;
        }
        "log" => {
            let s: Value = fs::read_to_string(d.join("status.json")).ok().and_then(|x| serde_json::from_str(&x).ok()).unwrap_or_default();
            if let Some(l) = s["log"].as_str() {
                open(l);
            }
        }
        "folder" => open(&root.display().to_string()),
        "incident-report" => {
            let s: Value = fs::read_to_string(d.join("status.json")).ok().and_then(|x| serde_json::from_str(&x).ok()).unwrap_or_default();
            if let Some(path) = incident_path(root, &s["incident"]) {
                open(&path.display().to_string());
            }
        }
        "incident-manual" => open(&format!("https://github.com/{}/issues/new", crate::platform::RELEASE_REPO)),
        // a separate process: on macOS `launchctl bootout` kills the service's process group, this tray included
        "stop" | "uninstall" => {
            let me = std::env::current_exe().map_err(|e| e.to_string())?;
            let mut c = std::process::Command::new(me);
            c.args(["service", action])
                .current_dir(root)
                .env("TASKQ_ROOT", root)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
            #[cfg(unix)]
            std::os::unix::process::CommandExt::process_group(&mut c, 0);
            coreq::telemetry::capture::inherit_context(&mut c);
            crate::platform::quiet(&mut c).spawn().map_err(|e| e.to_string())?;
        }
        _ => {}
    }
    Ok(None)
}

fn incident_path(root: &Path, incident: &Value) -> Option<PathBuf> {
    let dir = crate::dot(root).join("incidents").canonicalize().ok()?;
    let path = root.join(incident["path"].as_str()?);
    let resolved = path.canonicalize().ok()?;
    (resolved.starts_with(dir) && resolved.is_file() && resolved.extension().is_some_and(|x| x == "json")).then_some(path)
}

/// `Resources` (menu.md M5): btop in its own OS terminal window, installed first with the OS package manager when
/// missing, its output in that window; no package manager: one line saying so.
const BTOP_SH: &str = r#"#!/bin/sh
printf '\033]0;Resources\007\033[8;30;110t'
export PATH="/opt/homebrew/bin:/usr/local/bin:$PATH"
if ! command -v btop >/dev/null; then
  if ! command -v brew >/dev/null; then echo "btop is not installed and Homebrew was not found: install btop, then try again."; read _; exit 1; fi
  echo "Installing btop: brew install btop"
  brew install btop || { echo "brew install btop failed (see above)."; read _; exit 1; }
fi
exec btop
"#;
const BTOP_PS1: &str = r#"$Host.UI.RawUI.WindowTitle = 'Resources'
if (-not (Get-Command btop -ErrorAction SilentlyContinue)) {
  if (-not (Get-Command winget -ErrorAction SilentlyContinue)) { 'btop4win is not installed and winget was not found: install btop4win, then try again.'; Read-Host; exit 1 }
  'Installing btop: winget install aristocratos.btop4win'
  winget install -e --id aristocratos.btop4win --accept-source-agreements --accept-package-agreements
  $env:Path = [Environment]::GetEnvironmentVariable('Path', 'User') + ';' + [Environment]::GetEnvironmentVariable('Path', 'Machine')
  if (-not (Get-Command btop -ErrorAction SilentlyContinue)) { 'winget install aristocratos.btop4win failed (see above).'; Read-Host; exit 1 }
}
btop
"#;

fn resources() {
    let file = std::env::temp_dir().join(if cfg!(windows) { "taskq-resources.ps1" } else { "taskq-resources.command" });
    if fs::write(&file, if cfg!(windows) { BTOP_PS1 } else { BTOP_SH }).is_err() {
        return;
    }
    // not `quiet`: the terminal window is the point; waited on its own thread, no zombie
    let _ = if cfg!(windows) {
        std::process::Command::new("conhost.exe").args(["powershell", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File"]).arg(&file).status()
    } else {
        #[cfg(unix)]
        let _ = fs::set_permissions(&file, std::os::unix::fs::PermissionsExt::from_mode(0o755));
        std::process::Command::new("open").args(["-a", "Terminal"]).arg(&file).status()
    };
}

/// The OS folder picker; None on cancel.
fn pick() -> Option<PathBuf> {
    let mut c = if cfg!(windows) {
        let mut c = std::process::Command::new("powershell");
        c.args(["-NoProfile", "-STA", "-Command", "Add-Type -AssemblyName System.Windows.Forms; $d = New-Object System.Windows.Forms.FolderBrowserDialog; $d.Description = 'Add project'; if ($d.ShowDialog() -eq 'OK') { $d.SelectedPath }"]);
        c
    } else {
        let mut c = std::process::Command::new("osascript");
        c.args(["-e", "activate", "-e", "POSIX path of (choose folder with prompt \"Add project\")"]);
        c
    };
    let o = crate::platform::quiet(&mut c).output().ok().filter(|o| o.status.success())?;
    let p = String::from_utf8_lossy(&o.stdout).trim().trim_end_matches(['/', '\\']).to_string();
    (!p.is_empty()).then(|| PathBuf::from(p))
}

/// `taskq selftest` in `dir`: its first error line on failure.
fn selftest(dir: &Path) -> Result<(), String> {
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    let o = crate::platform::output("tray.selftest",std::process::Command::new(me).arg("selftest").current_dir(dir).env("TASKQ_ROOT", dir),Duration::from_secs(30))?;
    let text = String::from_utf8_lossy(&o.stderr).to_string() + &String::from_utf8_lossy(&o.stdout);
    if o.status.success() { Ok(()) } else { Err(text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("selftest failed").chars().take(160).collect()) }
}

/// The four Add project cases (menu.md M10): `taskq.json` with a passing `selftest` (taskq), with a failing one (old,
/// owner's marker), else an empty folder or a Git root (new) or files without Git (plain: not a repository).
fn classify(dir: &Path, selftest: impl Fn(&Path) -> Result<(), String>) -> Value {
    let (kind, reason) = if dir.join("taskq.json").is_file() {
        selftest(dir).map_or_else(|e| ("old", e), |_| ("taskq", String::new()))
    } else if dir.join(".git").exists() || fs::read_dir(dir).is_ok_and(|mut d| d.next().is_none()) {
        ("new", String::new())
    } else {
        ("plain", String::new())
    };
    json!({"state": "picked", "path": dir, "name": crate::project_name(dir), "kind": kind, "reason": reason})
}

/// `service install` in the picked taskq project after the page's confirm; installed only when `<install>/projects`
/// lists it (R12).
fn install_service(install: &Path, mut a: Value) -> Value {
    let dir = PathBuf::from(a["path"].as_str().unwrap_or(""));
    let r = std::env::current_exe().map_err(|e| e.to_string()).and_then(|me| {
        let mut c = std::process::Command::new(me);
        c.args(["service", "install"]).current_dir(&dir).env("TASKQ_ROOT", &dir).stdin(std::process::Stdio::null());
        // a separate process group: launchctl must not take the tray with it
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut c, 0);
        let o = crate::platform::output("tray.install", &mut c, Duration::from_secs(60))?;
        let text = String::from_utf8_lossy(&o.stderr).to_string() + &String::from_utf8_lossy(&o.stdout);
        if !o.status.success() {
            return Err(text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("service install failed").to_string());
        }
        if roots(install).iter().any(|r| Path::new(r) == dir) { Ok(()) } else { Err("installed, but this host's projects list does not show it".into()) }
    });
    a["state"] = json!(if r.is_ok() { "installed" } else { "install failed" });
    a["reason"] = json!(r.err().unwrap_or_default());
    a
}

/// Write the page's capacity edit for this host; the outcome line for the page.
fn capacity(install:&Path,_root: &Path,m: &Value) -> crate::Res<Value> {
    let mut input=m.clone();input.as_object_mut().ok_or("invalid capacity message")?.remove("context");
    if !crate::resources::keys(&input,&["a","root","id","ops","issued_at","expires_at"]) {
        return Ok(json!({"state":"rejected","reason":"Unsupported host capacity payload; refresh and review current settings"}));
    }
    let request=json!({"id":input["id"],"ops":input["ops"],"issued_at":input["issued_at"],"expires_at":input["expires_at"]});
    if let Err(error)=crate::host::capacity_request(&request) {return Ok(json!({"state":"rejected","reason":error}));}
    let policy=crate::host::Policy::read(install)?;
    crate::host::local_capacity(install,&policy.me,&request)
}

fn open(arg: &str) {
    let opener = if cfg!(windows) { "explorer" } else { "open" };
    let _ = coreq::telemetry::capture::operation("tray.open-request",||crate::platform::quiet(std::process::Command::new(opener).arg(arg)).spawn().map(|_|()),|_|"spawn");
}

/// The menu bar's theme: macOS follows the app appearance; the Windows taskbar has its own setting.
fn is_dark(_w: &Window) -> bool {
    #[cfg(windows)]
    {
        let mut c = std::process::Command::new("reg");
        c.args(["query", r"HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize", "/v", "SystemUsesLightTheme"]);
        let out = crate::platform::output("tray.theme", &mut c, Duration::from_secs(10)).map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
        !out.contains("0x1")
    }
    #[cfg(not(windows))]
    {
        _w.theme() == tao::window::Theme::Dark
    }
}

// ---------- icons: drawn by the shared renderer (src/icon.rs) ----------

fn rgba_icon(i: &Rgba) -> Icon {
    Icon::from_rgba(i.px.clone(), i.w, i.h).expect("rgba icon")
}

/// Call `f` with the status item's window and button (tray-icon keeps them private).
#[cfg(target_os = "macos")]
fn with_status_button(mut f: impl FnMut(&objc2_app_kit::NSWindow, &objc2_app_kit::NSButton)) {
    use objc2_app_kit::{NSApplication, NSButton, NSView, NSWindow};
    use objc2_foundation::MainThreadMarker;
    fn walk(w: &NSWindow, v: &NSView, f: &mut impl FnMut(&NSWindow, &NSButton)) -> bool {
        if let Some(b) = v.downcast_ref::<NSButton>() {
            f(w, b);
            return true;
        }
        v.subviews().iter().any(|s| walk(w, &s, f))
    }
    let Some(mtm) = MainThreadMarker::new() else { return };
    for w in NSApplication::sharedApplication(mtm).windows().iter() {
        if w.class().name().to_str() == Ok("NSStatusBarWindow") && w.contentView().is_some_and(|v| walk(&w, &v, &mut f)) {
            return;
        }
    }
}

/// Menu-bar height over the 22 pt the drawing assumes: 1.5 on notch Macs, 1 elsewhere.
#[cfg(target_os = "macos")]
fn menu_bar_scale() -> f32 {
    use objc2_app_kit::NSScreen;
    // no status item yet (`--snapshot`): the main screen's menu bar, its frame above the visible frame
    let mut bar = objc2_foundation::MainThreadMarker::new().and_then(NSScreen::mainScreen).map_or(22.0, |s| {
        let (f, v) = (s.frame(), s.visibleFrame());
        (f.origin.y + f.size.height - v.origin.y - v.size.height) as f32
    });
    // 0 until the item is placed: the next tick re-renders (`k` is in the icon signature)
    with_status_button(|w, _| bar = w.frame().size.height as f32);
    if (22.0..=44.0).contains(&bar) { bar / 22.0 } else { 1.0 }
}

/// Pixels per point of the main screen (2 on Retina).
#[cfg(target_os = "macos")]
fn backing_scale() -> f32 {
    objc2_foundation::MainThreadMarker::new().and_then(objc2_app_kit::NSScreen::mainScreen).map_or(2.0, |s| s.backingScaleFactor() as f32)
}

#[cfg(not(target_os = "macos"))]
fn menu_bar_scale() -> f32 {
    1.0
}

// ponytail: Windows keeps the compact q at 2x until per-monitor DPI rendering (I6) is qualified
#[cfg(not(target_os = "macos"))]
fn backing_scale() -> f32 {
    2.0
}

#[cfg(not(target_os = "macos"))]
fn fit_status_image(_: f32) {}

/// tray-icon caps the status-item image at 22 pt; grow it back to the full menu-bar height.
#[cfg(target_os = "macos")]
fn fit_status_image(k: f32) {
    use objc2_foundation::NSSize;
    with_status_button(|_, b| {
        let Some(img) = b.image() else { return };
        let sz = img.size();
        if (sz.height - 22.0).abs() < 0.5 {
            img.setSize(NSSize::new(sz.width * k as f64, 22.0 * k as f64));
            // the button takes its height from a newly set image, not from a resized one
            b.setImage(None);
            b.setImage(Some(&img));
        }
    });
}

fn base64(b: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::new();
    for c in b.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for k in 0..4 {
            s.push(if k <= c.len() { A[(n >> (18 - 6 * k) & 63) as usize] as char } else { '=' });
        }
    }
    s
}

// ---------- popover snapshot (`taskq tray --snapshot`): the webview renders its own page ----------

#[cfg(target_os = "macos")]
fn capture(wv: &WebView, path: PathBuf, done: impl Fn() + 'static) {
    use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSImage};
    use objc2_foundation::{NSDictionary, NSError};
    use wry::WebViewExtMacOS;
    let block = block2::RcBlock::new(move |img: *mut NSImage, _: *mut NSError| {
        // SAFETY: WebKit passes a valid image or null
        if let Some(img) = unsafe { img.as_ref() } {
            let png = img.TIFFRepresentation().and_then(|t| NSBitmapImageRep::imageRepWithData(&t))
                .and_then(|r| unsafe { r.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new()) });
            if let Some(p) = png {
                let _ = fs::write(&path, p.to_vec());
            }
        }
        done();
    });
    // SAFETY: main thread, a live webview
    unsafe { wv.webview().takeSnapshotWithConfiguration_completionHandler(None, &block) };
}

#[cfg(windows)]
fn capture(wv: &WebView, path: PathBuf, done: impl Fn() + 'static) {
    use webview2_com::CapturePreviewCompletedHandler;
    use webview2_com::Microsoft::Web::WebView2::Win32::COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG;
    use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_NORMAL;
    use windows::Win32::System::Com::{STGM_CREATE, STGM_WRITE};
    use windows::Win32::UI::Shell::SHCreateStreamOnFileEx;
    use wry::WebViewExtWindows;
    let p = windows::core::HSTRING::from(path.as_os_str());
    // SAFETY: a file stream handed to WebView2, released by the handler after the capture
    let Ok(stream) = (unsafe { SHCreateStreamOnFileEx(&p, (STGM_CREATE | STGM_WRITE).0, FILE_ATTRIBUTE_NORMAL.0, true, None) }) else {
        return done();
    };
    let done = std::rc::Rc::new(done);
    let (mut keep, d) = (Some(stream.clone()), done.clone());
    let handler = CapturePreviewCompletedHandler::create(Box::new(move |_| {
        keep.take();
        d();
        Ok(())
    }));
    // SAFETY: UI thread, a live webview
    if unsafe { wv.webview().CapturePreview(COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG, &stream, &handler) }.is_err() {
        done();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Chain: native delivery -> exact JS outcome or unknown -> bounded safe shared observations.
    #[test]
    fn web_execution_requires_acknowledgment_and_bounds_error_reports() {
        crate::with_test_capture(|store,_| {
            let now=Instant::now();
            let mut calls=WebCalls::new();calls.finish(0,"ok");
            for (name,result) in [("tray.web.success","ok"),("tray.web.exception","script-error"),("tray.web.malformed","invalid-ack")] {
                let id=calls.begin(name,now).unwrap();calls.finish(id,result);calls.finish(id,"ok");
            }
            let expired=calls.begin("tray.web.expired",now).unwrap();
            calls.expire(now+Duration::from_secs(29));assert!(calls.pending.contains_key(&expired));
            calls.expire(now+Duration::from_secs(30));assert!(calls.pending.is_empty());calls.finish(expired,"ok");
            for _ in 0..64 {assert!(calls.begin("tray.web.pending",now).is_some());}
            assert!(calls.begin("tray.web.overflow",now).is_none());assert_eq!(calls.pending.len(),64);
            for counts in [json!([1,0,0,0]),json!([101,1,1,1]),json!([101,1,1,1])] {
                calls.errors(&json!({"counts":counts,"saturated":false}));
            }
            for counts in [json!([1,1,1,1]),json!([-1,0,0,0]),json!([4294967296_u64,1,1,1]),json!([101]),json!(null)] {
                calls.errors(&json!({"counts":counts,"saturated":false}));assert_eq!(calls.counts,[101,1,1,1]);
            }
            calls.errors(&json!({"counts":[101,1,1,1],"saturated":true}));
            calls.errors(&json!({"counts":[101,1,1,1],"saturated":true}));
            assert!(calls.failed && calls.saturated);drop(calls);
            tracing::dispatcher::get_default(|d|d.downcast_ref::<coreq::telemetry::capture::Capture>().unwrap().flush().unwrap());
            let page=store.export(0,0,1024).unwrap();
            let rows:Vec<_>=page["events"].as_array().unwrap().iter().map(|r|&r["observation"]).filter(|r|r["kind"]=="method").collect();
            for (name,result) in [("success","ok"),("exception","failed"),("malformed","unknown"),("expired","unknown")] {
                let found:Vec<_>=rows.iter().filter(|r|r["operation"]==format!("taskq.operation.tray.web.{name}")).collect();
                assert_eq!(found.len(),1,"duplicate/late callback changed {name}");assert_eq!(found[0]["outcome"],result);
            }
            assert_eq!(rows.iter().filter(|r|r["operation"]=="taskq.operation.tray.web.pending"&&r["outcome"]=="unknown").count(),64);
            let errors:Vec<_>=rows.iter().filter(|r|r["operation"]=="taskq.operation.tray.web.error").collect();
            assert_eq!(rows.iter().filter(|r|r["operation"]=="taskq.operation.tray.web.error-counts"&&r["error"]=="count-saturated").count(),1);
            assert_eq!(errors.len(),5);assert_eq!(errors.iter().map(|r|r["calls"].as_u64().unwrap()).sum::<u64>(),104);
            assert!(errors.iter().all(|r|r["outcome"]=="failed"));
            assert_eq!(web_result("true"),"invalid-ack");assert_eq!(web_result("\"ok\""),"ok");
        });
    }

    // Chain: bounded UI submission -> owned worker -> same-trace receipt -> stale registration refusal.
    #[test]
    fn queued_focus_actions_keep_context_and_refuse_overflow_without_blocking() {
        crate::with_test_capture(|store,dir| {
            let root=dir.join("project");let install=dir.join("install");
            fs::create_dir_all(crate::dot(&root)).unwrap();fs::create_dir_all(&install).unwrap();
            fs::write(install.join("projects"),format!("{}\n",root.display())).unwrap();
            fs::write(crate::dot(&root).join("focus"),"area-maps").unwrap();
            let (result_tx,result_rx)=std::sync::mpsc::channel();
            let (release_tx,release_rx)=std::sync::mpsc::channel();
            let queue=ActionQueue::start(install.clone(),&["pause","resume","focus"],queued_focus,move |root,id,result| {
                result_tx.send((root,id,result)).unwrap();
                release_rx.recv_timeout(Duration::from_secs(3)).unwrap();
            }).unwrap();
            let message=|action:&str,id:&str|json!({"a":action,"root":root,"id":id});
            let _click=tracing::info_span!("fixture.click").entered();
            for invalid in [String::new(),"../outside".into(),"x".repeat(65)] {
                assert!(queue.submit(&message("pause",&invalid)).unwrap_err().contains("correlation id"));
                assert_eq!(crate::data::focus(&root).unwrap(),["area-maps"]);
            }
            queue.submit(&message("pause","first")).unwrap();
            let (reported,id,result)=result_rx.recv_timeout(Duration::from_secs(3)).unwrap();
            assert_eq!(reported,root);assert_eq!(id,"first");let applied=result.unwrap()["applied_receipt"].as_str().unwrap().to_string();
            let snapshot=crate::control::snapshot(&root,&mut None,&mut None).unwrap();
            assert_eq!(snapshot.command["applied_receipt"],applied);
            let status=json!({"focus_command":snapshot.command,"focus":snapshot.focus,"tasks":[],"complete":true});
            fs::write(crate::dot(&root).join("status.json"),status.to_string()).unwrap();
            assert_eq!(project(&install,root.to_str().unwrap())["focus_command"],status["focus_command"]);
            for n in 0..16 {queue.submit(&message("resume",&format!("queued-{n}"))).unwrap();}
            assert!(queue.submit(&message("resume","overflow")).unwrap_err().contains("queue full"));
            assert!(crate::data::focus(&root).unwrap().is_empty());
            // Queued intent cannot act on a project removed after submission.
            fs::write(install.join("projects"),"").unwrap();
            for n in 0..16 {
                release_tx.send(()).unwrap();
                let (reported,id,result)=result_rx.recv_timeout(Duration::from_secs(3)).unwrap();
                assert_eq!(reported,root);assert_eq!(id,format!("queued-{n}"));assert!(result.unwrap_err().contains("no longer registered"));
            }
            release_tx.send(()).unwrap();
            let ActionQueue{sender,worker,..}=queue;drop(sender);worker.join().unwrap();
            assert!(crate::data::focus(&root).unwrap().is_empty());
            let page=store.export(0,0,1024).unwrap();
            let rows:Vec<_>=page["events"].as_array().unwrap().iter().map(|r|&r["observation"]).collect();
            let receipts:Vec<_>=rows.iter().copied().filter(|r|r["source"]=="command").collect();
            assert_eq!(receipts.len(),3,"refused queued actions mutated the project");
            let click=rows.iter().find(|r|r["operation"]=="taskq.fixture.click").unwrap();
            assert_eq!(receipts[0]["trace"],click["trace"]);assert_eq!(receipts[1]["trace"],click["trace"]);
            assert_eq!(receipts[1]["kind"],"command.applied");
            assert_eq!(receipts[2]["kind"],"command.observed");
        });
    }

    // H-A2: a blocked host-policy write cannot delay the independent restrictive Pause command.
    #[test]
    fn blocked_capacity_does_not_block_pause() {
        crate::with_test_capture(|_store,dir| {
            let root=dir.join("project");let install=dir.join("install");
            fs::create_dir_all(crate::dot(&root)).unwrap();fs::create_dir_all(install.join("resources")).unwrap();
            fs::write(root.join("taskq.json"),"{\"repo\":\"fixture/project\"}").unwrap();
            fs::write(install.join("projects"),format!("{}\n",root.display())).unwrap();
            fs::write(install.join("host.json"),json!({"hosts":{crate::hostname():"capacity-host"},"limits":{"codex":2}}).to_string()).unwrap();
            fs::write(crate::dot(&root).join("focus"),"all").unwrap();
            let policy=crate::host::Policy::read(&install).unwrap();
            assert_eq!(project(&install,root.to_str().unwrap())["cap_host"],"capacity-host");
            let now=SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs();
            let message=json!({"a":"capacity","root":root,"id":"blocked","issued_at":now,"expires_at":now+300,"ops":[{
                "target":{"installation_id":null,"generation":1,"project_id":null,"runtime_id":"codex"},
                "policy_schema":"taskq.host-capacity/1","expected_revision":policy.revision().unwrap(),"changes":{"runtime_limit":3}}]});
            let lock=coreq::file_lock::lock(&install.join("resources/lock")).unwrap();
            let (capacity_tx,capacity_rx)=std::sync::mpsc::channel();
            let blocked=ActionQueue::start(install.clone(),&["capacity"],|install,root,message| {
                crate::platform::write_atomic(&install.join("capacity-entered"),"")?;
                capacity(install,root,message)
            },move |_,_,result|capacity_tx.send(result).unwrap()).unwrap();
            blocked.submit(&message).unwrap();
            let until=Instant::now()+Duration::from_secs(3);
            while !install.join("capacity-entered").exists() && Instant::now()<until {std::thread::yield_now();}
            assert!(install.join("capacity-entered").exists());assert!(capacity_rx.try_recv().is_err());
            let (pause_tx,pause_rx)=std::sync::mpsc::channel();
            let focus=ActionQueue::start(install.clone(),&["pause","resume","focus"],queued_focus,move |_,_,result|pause_tx.send(result).unwrap()).unwrap();
            focus.submit(&json!({"a":"pause","root":root,"id":"pause"})).unwrap();
            assert!(pause_rx.recv_timeout(Duration::from_secs(3)).unwrap().unwrap()["applied_receipt"].is_string());
            assert!(crate::data::focus(&root).unwrap().is_empty());assert!(capacity_rx.try_recv().is_err());
            drop(lock);
            assert_eq!(capacity_rx.recv_timeout(Duration::from_secs(3)).unwrap().unwrap()["state"],"saved");
            for queue in [blocked,focus] {let ActionQueue{sender,worker,..}=queue;drop(sender);worker.join().unwrap();}
        });
    }

    // Chain: native Resume -> shared command -> failure receipt; retries keep the selected focus.
    #[test]
    fn action_failures_reach_the_tray_caller() {
        crate::with_test_capture(|store, _| {
            let root=std::env::temp_dir().join(format!("taskq-action-receipt-{}",std::process::id()));
            fs::create_dir_all(crate::dot(&root)).unwrap();
            fs::write(crate::dot(&root).join("focus"),"off").unwrap();
            assert!(act("resume",&root,None).unwrap_err().contains("no saved focus"));
            assert_eq!(fs::read_to_string(crate::dot(&root).join("focus")).unwrap(),"off");
            fs::write(crate::dot(&root).join("focus.paused"),"telemetry-fixture").unwrap();
            act("resume",&root,None).unwrap();
            assert_eq!(fs::read_to_string(crate::dot(&root).join("focus")).unwrap(),"telemetry-fixture");
            assert!(act("focus",&root,Some("stale-label")).is_err());
            fs::remove_dir_all(&root).unwrap();
            assert!(act("pause",&root,None).is_err());
            let page=store.export(0,0,1024).unwrap();
            let receipts:Vec<_>=page["events"].as_array().unwrap().iter().map(|r|&r["observation"])
                .filter(|r|r["source"]=="command").collect();
            assert_eq!(receipts.len(),6,"native dispatch bypassed the shared receipt boundary");
            assert_eq!(receipts.iter().map(|r|r["kind"].as_str().unwrap()).collect::<Vec<_>>(),
                ["command.received","command.failed","command.received","command.applied","command.received","command.failed"]);
            assert_ne!(receipts[0]["span"],receipts[2]["span"],"separate clicks reused an identity");
            for pair in receipts.chunks_exact(2) { assert_eq!(pair[0]["span"],pair[1]["span"]); }

        });
    }

    // Chain: R12 tray evidence -> `tray --snapshot --route`. Risk: a filtered run captures the full matrix or drops a route.
    #[test]
    fn snapshot_routes_capture_only_named_pages() {
        let install = std::env::temp_dir().join(format!("taskq-tray-routes-{}", std::process::id()));
        fs::create_dir_all(&install).unwrap();
        let root = std::env::temp_dir().join("taskq-tray-routes-project");
        fs::write(install.join("projects"), root.to_str().unwrap()).unwrap();
        let r = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let names = |v: &[&str]| (0..shots(&install, &r(v)).len()).map(|i| shot_name(&install, &r(v), i)).collect::<Vec<_>>();
        assert_eq!(names(&["attention"]), ["dark-attention-16", "dark-attention-20", "light-attention-16", "light-attention-20"]);
        assert_eq!(names(&["actions", "project"]), ["dark-home-actions-16", "dark-project-16", "light-home-actions-16", "light-project-16"]);
        assert_eq!(names(&["scroll"])[..3], ["dark-home-scroll-start-16", "dark-home-scroll-middle-16", "dark-home-scroll-end-16"]);
        assert_eq!(names(&["scroll"]).len(), 12, "start, middle and end at 16 and 20 px in both themes");
        assert!(names(&["icon"]).is_empty(), "icon alone captures no popover page");
        let all = names(&[]);
        assert_eq!(all.len(), 2 * (4 + 2 * (ROUTES.len() - 4)), "no filter keeps the full matrix without scroll");
        for route in ROUTES {
            assert!(!names(&[route]).is_empty(), "{route} names a captured page");
        }
        fs::remove_dir_all(&install).ok();
    }

    // Chain: owner tray answer -> canonical CLI. Risk: IPC selects another project or inherits worker fencing.
    #[test]
    fn answer_bridge_restricts_targets_and_preserves_literal_owner_input() {
        let install = std::env::temp_dir().join(format!("taskq-tray-answer-{}", std::process::id()));
        let root = install.join("project");
        fs::create_dir_all(crate::dot(&root)).unwrap();
        fs::write(install.join("projects"), root.to_str().unwrap()).unwrap();
        let mut m = json!({"root": root, "board": "https://github.com/o/r/issues", "op": {"id": "tray.1", "task": 7, "revision": 20, "text": "2; literal $(text)"}});
        let args = answer_args(&install, &m).unwrap();
        assert_eq!(args, ["answer", "7", "--text", "2; literal $(text)", "--json", "--op-id", "tray.1", "--revision", "20"]);
        let command = owner_command(&root).unwrap();
        for key in ["TASKQ_TASK", "TASKQ_ATTEMPT", "TASKQ_ROLE"] {
            assert!(command.get_envs().any(|(k, v)| k == key && v.is_none()), "{key}");
        }
        m["op"]["choice"] = json!(2);
        assert!(answer_args(&install, &m).is_err(), "choice and text are exclusive");
        m["op"].as_object_mut().unwrap().remove("text");
        assert_eq!(answer_args(&install, &m).unwrap()[1], "7.2");
        m["root"] = json!(install.join("unregistered"));
        assert!(answer_args(&install, &m).unwrap_err().contains("not registered"));
        m["root"] = json!(root);
        m["op"]["revision"] = Value::Null;
        assert!(answer_args(&install, &m).is_err());
        fs::remove_dir_all(install).unwrap();
    }

    // Chain: live board snapshot -> tray answer. Risk: forged IPC bypasses read-only/close controls.
    #[test]
    fn answer_bridge_checks_board_and_canonical_question() {
        let card = "Done: checked\nProblem: decide\n1. close: ship ★\n2. redo";
        let row = json!({"n": 7, "revision": 20, "card": card, "question": crate::snapshot::question(card)});
        let mut s = json!({"complete": true, "board": {"url": "https://github.com/o/r/issues"}, "tasks": [row]});
        let mut m = json!({"board": "https://github.com/o/r/issues", "op": {"id": "tray.1", "task": 7, "revision": 20, "choice": 1}});
        assert!(answer_snapshot(&s, &m).unwrap_err().contains("close/publication"));
        m["op"]["choice"] = json!(2);
        assert!(answer_snapshot(&s, &m).is_ok());
        s["tasks"][0]["question"]["parsed"] = json!(false);
        assert!(answer_snapshot(&s, &m).unwrap_err().contains("read-only"));
        s["tasks"][0]["revision"] = json!(21);
        assert!(answer_snapshot(&s, &m).is_ok(), "dispatch owns stale and idempotent retry receipts");
        let rows = s["tasks"].take();
        s["tasks"] = json!([]);
        assert!(answer_snapshot(&s, &m).is_err(), "an omitted row never bypasses the consequential-choice guard");
        s["tasks"] = rows;
        s["board"]["url"] = json!("https://github.com/o/other/issues");
        assert!(answer_snapshot(&s, &m).unwrap_err().contains("identity"));
        s["complete"] = json!(false);
        s["reason"] = json!("permission denied");
        assert_eq!(answer_snapshot(&s, &m).unwrap_err(), "permission denied");
    }

    // Chain: no-choice ask -> tray IPC -> CLI dispatch -> board readback. Risk: cancellation publishes a result,
    // stale input acts on another question, or retry/partial failure duplicates a write or skips parking.
    #[test]
    fn answer_bridge_fallback_cancellation_chain() {
        use crate::board::{Board, Fake};
        let install = std::env::temp_dir().join(format!("taskq-tray-cancel-{}", std::process::id()));
        let root = install.join("project");
        fs::create_dir_all(&root).unwrap();
        fs::write(install.join("projects"), root.to_str().unwrap()).unwrap();
        fs::write(root.join("taskq.json"), r#"{"repo":"o/r"}"#).unwrap();
        let (cfg, _) = crate::load(&root).unwrap();
        let b = Fake::default();
        b.state.borrow_mut().publish = "pending".into(); // a publication attempt must not succeed
        let n = b.create("Needs a decision", "## Goal\ng", &["taskq".into()]).unwrap();
        let attempt = b.comment(n, "taskq: spawn worker codex:u mac").unwrap();
        b.comment(n, &format!("taskq: result {} attempt {attempt}", "a".repeat(40))).unwrap();
        let reviewer = b.comment(n, "taskq: spawn reviewer codex:v mac").unwrap();
        let rev = b.comment(n, &format!("taskq: ask attempt {reviewer}\nReviewer cannot continue.")).unwrap();
        let mut m = json!({"root": root, "board": "https://github.com/o/r/issues", "op": {"id":"cancel.1", "task":n, "revision":rev, "action":"close", "confirmed":true}});
        let mut snapshot = json!({"complete":true,"board":{"url":m["board"]},"tasks":[{"n":n,"state":"ask","revision":rev,"question":crate::snapshot::question("Reviewer cannot continue.")}]});
        assert!(answer_snapshot(&snapshot, &m).is_ok());
        for st in ["doing", "review", "ready", "waiting"] {
            snapshot["tasks"][0]["state"] = json!(st);
            assert!(answer_snapshot(&snapshot, &m).is_err());
        }
        snapshot["tasks"][0]["state"] = json!("ask");
        snapshot["tasks"][0]["revision"] = json!(rev + 1);
        assert!(answer_snapshot(&snapshot, &m).is_err());
        snapshot["tasks"][0]["revision"] = json!(rev);
        snapshot["tasks"][0]["question"]["choices"] = json!([{"id":1}]);
        assert!(answer_snapshot(&snapshot, &m).is_err());
        snapshot["tasks"][0]["question"]["choices"] = json!([]);
        m["op"]["confirmed"] = json!(false);
        assert!(answer_args(&install, &m).is_err());
        m["op"]["confirmed"] = json!(true);
        let args = answer_args(&install, &m).unwrap();
        assert_eq!(args[0], "later");
        assert_eq!(args[args.len()-3], "cancel.1.later");
        for outcome in ["rejected", "stale", "unknown"] {
            let mut calls = 0;
            let r = answer_dispatch(&m, &args, |_| { calls += 1; json!({"task":n,"op_id":"cancel.1.later","outcome":outcome}) });
            assert_eq!((calls, r["outcome"].as_str().unwrap()), (1, outcome));
        }
        let mut calls = Vec::new();
        let execute = |args: &[String]| {
            let op = crate::dispatch::Op { id: Some(args[args.len()-3].clone()), revision: Some(args.last().unwrap().parse().unwrap()) };
            let receipt = if args[0] == "close" { crate::dispatch::close_one(&b, &cfg, n, &args[3], &op) }
                else { crate::dispatch::act(&b, &root, n, &args[0], &[], &args[3], &op) };
            serde_json::to_value(receipt).unwrap()
        };
        let partial = answer_dispatch(&m, &args, |a| {
            calls.push(a.to_vec());
            if a[0] == "close" { json!({"task":n,"op_id":"cancel.1.close","outcome":"unknown","reason":"response lost"}) }
            else { execute(a) }
        });
        assert!(partial["reason"].as_str().unwrap().contains("Parked; close not verified"));
        assert_eq!(crate::dispatch::current(&b, n).unwrap().1.state, crate::core::State::Later);
        snapshot["tasks"][0]["state"] = json!("later");
        assert!(answer_snapshot(&snapshot, &m).is_ok(), "retry can reconcile its parking receipt");
        let r = answer_dispatch(&m, &args, |a| { calls.push(a.to_vec()); execute(a) });
        assert_eq!(r["state"], "closed");
        assert_eq!(r["outcome"], "accepted", "must cancel without publishing the stored result");
        assert_eq!(calls[0], calls[2]);
        assert_eq!(calls[1], calls[3]);
        assert_eq!(calls[1].last().unwrap(), &rev.to_string());
        let count = b.comments(n).unwrap().len();
        snapshot["tasks"] = json!([]);
        assert!(answer_snapshot(&snapshot, &m).is_err(), "an omitted row never grants cancellation authority");
        assert_eq!(answer_dispatch(&m, &args, execute)["state"], "closed");
        assert_eq!(b.comments(n).unwrap().len(), count);
        for action in ["requeue", "later"] {
            m["op"] = json!({"id":"action.1","task":n,"revision":rev,"action":action});
            if action == "requeue" { m["op"]["text"] = json!("Retry with literal $(text)"); }
            let a = answer_args(&install, &m).unwrap();
            assert_eq!(a[0], action);
            assert_eq!(a[a.len()-3], "action.1");
            let mut seen = 0;
            assert_eq!(answer_dispatch(&m, &a, |_| { seen += 1; json!({"task":n,"op_id":"action.1","outcome":"accepted","state":"ready"}) })["outcome"], "accepted");
            assert_eq!(seen, 1);
        }
        m["op"]["action"] = json!("requeue");
        m["op"]["text"] = json!("x".repeat(233));
        assert!(answer_args(&install, &m).is_err());
        m["op"]["text"] = json!("a\nb");
        assert!(answer_args(&install, &m).is_err());
        fs::remove_dir_all(install).unwrap();
    }

    // Chain: menu `size` message -> popover window size. Risk: a scrolling menu clipped on the right behind a horizontal
    // scrollbar (#1073), a short menu given an empty scrollbar strip, or Windows Text size shrinking the CSS viewport.
    #[test]
    fn scrolling_menu_window_adds_the_scrollbar_width() {
        assert_eq!(popover_size(&json!({"w": 320, "h": 500, "g": 8, "d": 2}), 600.0, 2.0), (640, 1000));
        assert_eq!(popover_size(&json!({"w": 360, "h": 900, "g": 8, "d": 1}), 600.0, 1.0), (368, 600));
        assert_eq!(popover_size(&json!({"w": 320, "h": 900}), 600.0, 1.0), (320, 600), "overlay scrollbars (macOS) take no width");
        // Text size 150 % at 100 % display scale: 320 CSS px need 480 window px; 400 CSS px tall fit, 500 scroll
        assert_eq!(popover_size(&json!({"w": 320, "h": 400, "g": 10, "d": 1.5}), 600.0, 1.0), (480, 600));
        assert_eq!(popover_size(&json!({"w": 320, "h": 300, "g": 10, "d": 1.5}), 600.0, 1.0), (480, 450));
        assert_eq!(popover_size(&json!({"w": 320, "h": 500, "g": 10, "d": 1.5}), 600.0, 1.0), (495, 600));
        // Text size 140 % at 125 % display scale: 331 CSS px are 579.25 device px, never 579
        assert_eq!(popover_size(&json!({"w": 320, "h": 900, "g": 11, "d": 1.75}), 600.0, 1.25).0, 580);
    }

    // Chain: icon click -> popover shown -> blur. Risk: opened from the Windows 11 hidden-icons flyout, the popover
    // closes 15 ms later because the flyout retakes the foreground; or a later blur no longer closes it.
    #[test]
    fn blur_right_after_opening_keeps_the_popover() {
        assert!(blur_refocuses(Duration::from_millis(15)));
        assert!(!blur_refocuses(Duration::from_millis(300)) && !blur_refocuses(Duration::from_secs(5)));
    }

    // H-A2: host policy/readback cannot be confused with a project's lower ceiling or legacy writable scope.
    #[test]
    fn capacity_projects_to_resource_targets() {
        let saved = json!({"limits":{"claude":0,"codex":3},"load_gate":{"enabled":true,"cpu_max":85,"memory_min":10},"admission":{"mode":"auto","max":null}});
        let cap = json!({"host_policy":saved,"host_runtime_used":{"claude":1,"codex":2},"project_limits":{"codex":1},
            "limits":{"codex":1},"used":{"codex":1},"load_gate":{"enabled":true,"cpu_max":85,"memory_min":10},
            "sample_at":95,"sample_age_seconds":3,"cpu_percent":12.5,"memory_available_percent":40.0,
            "reasons":[{"code":"hold"},{"code":"future"},{"code":"cpu"}]});
        let group = resource_group(&cap, &saved,&json!("command-current"), &json!({"codex":1}), Some(100), None, "taskq");
        let t = group["targets"].as_array().unwrap();
        assert_eq!(t[0]["runtime_usage"][1],json!({"runtime_id":"codex","used":2,"limit":3,"limit_kind":"ceiling"}));
        assert_eq!(t[1]["current_policy"]["limits"]["codex"],1);
        assert!(t[0]["effective_revision"].is_null(),"equal settings without the host's exact receipt cannot look applied");
        assert_eq!(t[0]["supported"][0]["fields"],json!(["maximum","load_gate"]));
        assert_eq!(t[1]["supported"][0]["fields"],json!([]),"a project ceiling has no host write authority");
        assert!(t[3]["target"]["project_id"].is_null(),"runtime ceiling belongs to the host");
        assert!(t[0]["current_policy"]["mode"].is_null());
        let codes:Vec<_>=t[0]["blockers"].as_array().unwrap().iter().map(|b|b["code"].as_str().unwrap()).collect();
        assert_eq!(codes,["host","unknown","cpu"]);
        assert_eq!(t[0]["sample"]["age_seconds"],3);
        let mut stale=cap.clone();stale.as_object_mut().unwrap().remove("host_policy");
        let unknown=resource_group(&stale,&saved,&json!("command-current"),&json!({"codex":1}),Some(100),None,"taskq");
        assert!(unknown["targets"][0]["effective_revision"].is_null());
        assert!(unknown["targets"][0]["runtime_usage"][1]["used"].is_null(),"legacy project snapshot cannot prove host use");
    }

    // H-A2: a stale page cannot repurpose a legacy project operation as an edit to every project on the host.
    #[test]
    fn capacity_bridge_refuses_unqualified_host_edits() {
        for set in [json!({"mode":"auto"}),json!({"limits":{"codex":4}}),json!({"maximum":null})] {
            assert_eq!(capacity(Path::new("/nonexistent-taskq-954"),Path::new("/nonexistent-taskq-954"),&json!({"set":set})).unwrap()["state"],"rejected");
        }
    }

    // Chain: § 4 tray opens an incident report. Risk: tray opens a file outside .taskq/incidents.
    #[test]
    fn incident_report_only_opens_json_inside_its_project() {
        let root = std::env::temp_dir().join(format!("taskq-tray-incident-{}", std::process::id()));
        let dir = crate::dot(&root).join("incidents");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("report.json"), "{}").unwrap();
        fs::write(root.join("private.json"), "{}").unwrap();
        fs::write(dir.join("program.exe"), "{}").unwrap();
        assert_eq!(incident_path(&root, &json!({"path": ".taskq/incidents/report.json"})), Some(dir.join("report.json")));
        for path in ["private.json", ".taskq/incidents/../../private.json", ".taskq/incidents/program.exe", ".taskq/incidents/missing.json"] {
            assert!(incident_path(&root, &json!({"path": path})).is_none(), "{path}");
        }
        fs::remove_dir_all(root).unwrap();
    }

    // Chain: § 4 tray clarify thread. Risk: wrong order or a loop on a cyclic thread.
    #[test]
    fn clarify_thread_is_oldest_first_and_stops_at_missing_or_cycle() {
        let install = std::env::temp_dir().join(format!("taskq-thread-{}", std::process::id()));
        let root = install.join("project");
        fs::create_dir_all(crate::operator::dir(&root)).unwrap();
        fs::write(install.join("projects"), root.to_str().unwrap()).unwrap();
        let put = |id: &str, after: &str| {
            fs::write(crate::operator::dir(&root).join(format!("{id}.json")), json!({"text": id, "after": after}).to_string()).unwrap();
            fs::write(crate::operator::answer_path(&root, id), json!({"state": "clarify", "text": format!("question {id}")}).to_string()).unwrap();
        };
        put("1", ""); put("2", "1"); put("3", "2");
        let q = newest(&install);
        assert_eq!(q["id"], "3");
        assert_eq!(q["chain"].as_array().unwrap().iter().map(|v| v["id"].as_str().unwrap()).collect::<Vec<_>>(), ["1", "2", "3"]);
        assert_eq!(q["answer"]["text"], "question 3");
        put("1", "3");
        assert_eq!(newest(&install)["chain"].as_array().unwrap().len(), 3);
        put("2", "0");
        assert_eq!(newest(&install)["chain"].as_array().unwrap().len(), 2);
        fs::remove_dir_all(install).unwrap();
    }

    // Risk: page metadata smuggles content or forges a method outside the bundled inventory.
    #[test]
    fn web_metadata_is_allowlisted_and_aggregate_identity_is_explicitly_unknown() {
        let row=json!({"operation":"js.tray.create.mode","kind":"method.aggregate","started_ms":1,"counts":{"calls":2,"finished":2,"failed":0,"unknown":0},"outcome":"ok","error":"none"});
        let accepted=web_observation(&row).unwrap();
        tracing::subscriber::with_default(tracing::subscriber::NoSubscriber::default(),||{
            let mut calls=WebCalls::new();
            assert!(calls.metadata(&row),"missing collector must be visible");
            assert!(calls.metadata_unknown);assert!(!calls.failed,"optional metadata disabled command controls");
            assert!(!calls.metadata(&row),"same outage repeated its notification");
        });
        assert_eq!(accepted["unknown"]["trace"],"aggregate-unlinked");
        for (key,value) in [("message",json!("private")),("operation",json!("js.private-data")),("trace",json!("/private/path")),("counts",json!({"private":1})),("error",json!("raw-error"))] {
            let mut invalid=row.clone();invalid[key]=value;assert!(web_observation(&invalid).is_err(),"{key}");
        }
    }

    // Chain: § 4 tray icon save. Risk: an invalid draft replaces the working icon.
    #[test]
    fn invalid_icon_draft_leaves_tray_json_unchanged() {
        let install = std::env::temp_dir().join(format!("taskq-icon-{}", std::process::id()));
        fs::create_dir_all(&install).unwrap();
        let before = r#"{"view":"full","font":18}"#;
        fs::write(install.join("tray.json"), before).unwrap();
        let mut bad = icon::summary();
        bad["version"] = json!(2);
        bad["elements"][0]["bind"] = json!("eta");
        assert_eq!(use_icon(&install, &bad).unwrap_err().len(), 2, "every schema error");
        assert_eq!(fs::read_to_string(install.join("tray.json")).unwrap(), before);
        assert_eq!(Settings::load(&install).view, "full", "the current icon stays");
        let mut good = icon::summary();
        good["elements"][0]["align"] = json!("center");
        use_icon(&install, &good).unwrap();
        let s = Settings::load(&install);
        assert!(s.view == "custom" && s.icon == good && s.font == 18);
        // a corrupt saved icon falls back to Summary
        fs::write(install.join("tray.json"), r#"{"view":"custom","icon":{"version":9}}"#).unwrap();
        assert_eq!(Settings::load(&install).view, "icon");
        fs::remove_dir_all(install).unwrap();
    }

    // Chain: tray project page and settings. Risk: corrupt status.json or tray.json reads as zero/default and is reset
    #[test]
    fn corrupt_status_and_tray_json_read_unknown_and_stay() {
        let install = std::env::temp_dir().join(format!("taskq-tray-corrupt-{}", std::process::id()));
        let root = install.join("project");
        fs::create_dir_all(crate::dot(&root)).unwrap();
        fs::write(crate::dot(&root).join("status.json"), "{\"tasks\": [{\"n\": 1").unwrap();
        let p = project(&root, root.to_str().unwrap());
        assert!(p["complete"] == false && p["tasks"] == json!([]) && p["error"].as_str().unwrap().starts_with("status.json: corrupt"), "{p}");
        assert!(!summary(&[p], (0, 1)).known, "counts unknown, never zero");
        assert_eq!(fs::read_to_string(crate::dot(&root).join("status.json")).unwrap(), "{\"tasks\": [{\"n\": 1");
        // a corrupt focus in a fresh status.json names its reason
        fs::write(crate::dot(&root).join("status.json"), r#"{"focus": null, "focus_error": "focus: not UTF-8 text; file kept"}"#).unwrap();
        assert_eq!(project(&root, root.to_str().unwrap())["error"], "focus: not UTF-8 text; file kept");
        fs::write(install.join("tray.json"), "{\"view\": \"fu").unwrap();
        let s = Settings::load(&install);
        assert!(s.view == "icon" && s.font == 16, "defaults shown");
        assert_eq!(fs::read_to_string(install.join("tray.json")).unwrap(), "{\"view\": \"fu", "a read never resets it");
        let reason = state(&install, &s, false, None, &Value::Null)["settings_error"].clone();
        assert!(reason.as_str().unwrap().starts_with("tray.json: corrupt"), "the popover names the reason: {reason}");
        assert!(HTML.contains("Settings unknown:"));
        assert!(s.save(&install));
        assert_eq!(state(&install, &s, false, None, &Value::Null)["settings_error"], Value::Null, "a save clears it");
        assert_eq!(fs::read_to_string(install.join("tray.json.corrupt")).unwrap(), "{\"view\": \"fu", "a save keeps a copy");
        assert_eq!(Settings::load(&install).font, 16);
        fs::remove_dir_all(install).unwrap();
    }

    // Chain: § 4 tray counts across projects. Risk: double counting or unknown shown as known.
    #[test]
    fn summary_counts_one_task_per_board_and_issue() {
        let p = |issues: &str, state: &str, focus: Value| json!({"issues": issues, "focus": focus, "tasks": [{"n": 1, "state": state}], "events": [[95, "close", "7"], [-250, "failed", "8"]]});
        let ps = [p("https://github.com/o/a/issues", "ask", json!([])), p("https://github.com/o/a/issues", "ask", json!([])), p("https://github.com/o/b/issues", "doing", json!(["all"]))];
        let s = summary(&ps, (-200, 100));
        assert_eq!(s.counts, [1, 1, 0, 0]);
        assert!(s.run && !summary(&ps[..2], (-200, 100)).run);
        assert_eq!(s.bins.iter().map(|b| b[0]).sum::<u32>(), 2, "same attempt on two boards counts twice, on one board once");
        assert_eq!(s.bins.iter().map(|b| b[1]).sum::<u32>(), 0, "outside the period");
        assert!(!s.known, "a project without a complete snapshot makes the counts unknown");
        let mut ok = ps.clone();
        for p in &mut ok { p["complete"] = json!(true); }
        assert!(summary(&ok, (-200, 100)).known);
    }

    // Chain: M6/D11 rolling history grid and U-A6 icon. Risk: bars drift on every refresh, or a boundary loses or moves events.
    #[test]
    fn history_grid_moves_one_bin_at_a_boundary() {
        let b = 7200 * 300_000; // a 2 h boundary of the 24 h period
        // one event per bin before b (kinds in turn), two more in the bin closing at b, one after b
        let mut ev: Vec<Value> = (1..12).map(|i| json!([b - i * 7200, KINDS[(i % 4) as usize], format!("e{i}")])).collect();
        ev.extend([json!([b - 3700, "close", "late"]), json!([b - 3601, "ask", "last"]), json!([b + 30, "failed", "new"])]);
        // the events cache at `now` holds only events up to now
        let at = |now: i64| {
            let seen: Vec<&Value> = ev.iter().filter(|e| e[0].as_i64().unwrap() <= now).collect();
            summary(&[json!({"issues": "u", "tasks": [], "focus": ["all"], "events": seen, "since": 0, "complete": true})], day(now))
        };
        let icons = |s: &Summary| [(1.0, 2.0), (16.0 / 22.0, 1.0)].map(|(k, dpr)| draw(&icon::summary(), s, true, k, dpr).unwrap().png());
        let frames = [b - 3600, b - 1, b, b + 31, b + 3600];
        let sums: Vec<Summary> = frames.iter().map(|&t| at(t)).collect();
        let pngs: Vec<_> = sums.iter().map(icons).collect();
        for (i, j) in [(0, 1), (3, 4)] {
            assert_eq!(sums[i].bins, sums[j].bins, "no movement inside a bin");
            assert!(pngs[i] == pngs[j], "44 px and 16 px icons identical inside a bin");
        }
        let (before, after) = (&sums[1].bins, &sums[2].bins);
        assert_eq!(after[..11], before[1..], "the whole grid moves one bin left at the boundary");
        assert_eq!((after[11], sums[3].bins[11]), ([0; 4], [0, 1, 0, 0]), "the new partial bin starts empty and fills in place");
        assert_eq!(before[11], [1, 1, 1, 0]);
        let tot = |bins: &[[u32; 4]]| (0..4).map(|k| bins.iter().map(|r| r[k]).sum::<u32>()).collect::<Vec<_>>();
        assert_eq!(tot(&after[..11]), tot(&before[1..]), "retained totals unchanged");
        assert_eq!((before[0], tot(after)), ([0; 4], tot(before)), "an empty oldest bin leaves: every total unchanged");
        assert!(pngs[1] != pngs[2], "the icon changes at the boundary");
        if let Ok(dir) = std::env::var("TASKQ_FRAMES") {
            // ponytail: evidence frames on demand, `TASKQ_FRAMES=DIR cargo test history_grid`
            for (t, p) in frames.iter().zip(&pngs) {
                for (size, png) in ["22pt@2x", "16px@1x"].iter().zip(p) {
                    fs::write(Path::new(&dir).join(format!("icon-summary-{:+}s-{size}.png", t - b)), png).unwrap();
                }
            }
        }
    }

    // Chain: § 4 tray settings input. Risk: webview input stores unknown values.
    #[test]
    fn settings_keep_only_known_values() {
        let mut s = Settings { view: "icon".into(), font: 16, period: "1d".into(), start: 0, end: 0, tz: "local".into(), icon: Value::Null, pin: Pin::default(), notify: Value::Null };
        s.set(&json!({"view": "full", "font": 18, "period": "1w"}));
        s.set(&json!({"view": "x", "font": 15, "period": "2y"}));
        s.set(&json!({"period": "custom", "start": 50, "end": 50, "tz": "UTC"}));
        assert!(s == Settings { view: "full".into(), font: 18, period: "1w".into(), start: 0, end: 0, tz: "local".into(), icon: Value::Null, pin: Pin::default(), notify: Value::Null });
        assert_eq!(s.range(1000), (50400 - 604800, 50400), "whole 14 h bins, the newest holds now");
        s.set(&json!({"period": "custom", "start": 40, "end": 160, "tz": "UTC"}));
        assert_eq!((s.range(1000), s.tz.as_str()), ((40, 160), "UTC"));
    }

    // Chain: § 4 pinned tray window (menu.md M13). Risk: pin or position lost on restart; window placed off-screen.
    #[test]
    fn pin_and_position_per_display_survive_restart() {
        let install = std::env::temp_dir().join(format!("taskq-pin-{}", std::process::id()));
        fs::create_dir_all(&install).unwrap();
        fs::write(install.join("tray.json"), r#"{"font":18}"#).unwrap();
        let mut s = Settings::load(&install);
        assert!(!s.pin.pinned && s.pin.top && s.pin.size.is_none(), "old settings retain popover/top defaults and content fit");
        fs::write(install.join("tray.json"), r#"{"size":[-1,0],"top":false}"#).unwrap();
        let invalid = Settings::load(&install);
        assert!(invalid.pin.size.is_none() && !invalid.pin.top, "invalid size cannot hide the header or resize grip");
        s.pin = Pin { top: false, size: Some((480.0, 600.0)), pinned: true, display: "B".into(), pos: [("A".into(), (10, 20)), ("B".into(), (-1500, 300))].into() };
        assert!(s.save(&install));
        let r = Settings::load(&install);
        assert!(r.pin == s.pin && r.font == 18);
        // M14: modes and cursors survive a restart as saved, unknown values included
        let mut s = r;
        s.notify = json!({"kinds": {"ask": "loud"}, "cursor": {"/p": "100 #7 ask 5"}});
        let st = json!({"projects": [{"root": "/p", "name": "p", "tasks": [], "events": [[100, "ask", "5", 7], [200, "failed", "6", 7]]}]});
        let mut asked = true;
        notes(&install, &mut s, &st, &mut asked, &mut notify::Bursts::new());
        let r = Settings::load(&install);
        assert_eq!((r.notify["cursor"]["/p"].as_str(), r.notify["kinds"]["ask"].as_str()), (Some("200 #7 failed 6"), Some("loud")), "cursor saved before any post");
        let area = (0, 25, 1512, 900);
        assert!(on_screen((10, 30), area) && !on_screen((-1500, 300), area) && !on_screen((1512, 30), area));
        assert_eq!(clamp((1400, 0), (320, 400), area), (1192, 25), "kept under the menu bar and inside the right edge");
        assert_eq!(clamp((100, 800), (320, 2000), area), (100, 25), "a too tall window keeps its top in");
        fs::remove_dir_all(install).unwrap();
    }

    // Chain: § 4 tray history graph. Risk: events land in the wrong bin or period.
    #[test]
    fn history_bins_are_equal_oldest_left_on_one_scale() {
        // M-A5/M-A6 fixture: 3 of each kind in [0, 120); the first and last bins are not empty; unknown kinds drop
        let ev = json!([[0, "close", "1"], [5, "close", "2"], [119, "close", "3"], [0, "failed", "4"], [60, "failed", "5"], [119, "failed", "6"], [120, "failed", "7"], [-1, "close", "8"],
            [0, "ask", "9"], [30, "ask", "10"], [119, "ask", "11"], [10, "blocked", "12"], [60, "blocked", "13"], [119, "blocked", "14"], [50, "later", "15"]]);
        let p = json!([{"issues": "u", "tasks": [], "focus": ["all"], "events": ev, "since": -10}]);
        let s = summary(p.as_array().unwrap(), (0, 120));
        assert_eq!(s.bins.len(), 12);
        assert_eq!((s.bins[0], s.bins[6], s.bins[11]), ([2, 1, 1, 0], [0, 1, 0, 1], [1, 1, 1, 1]), "[start, end): 120 and -1 are out");
        assert_eq!((0..4).map(|k| s.bins.iter().map(|b| b[k]).sum::<u32>()).collect::<Vec<_>>(), [3, 3, 3, 3]);
        assert_eq!(s.since, Some(-10));
        let ps = json!([{"issues": "u", "events": []}, {"issues": "v", "events": [], "since": 5}]);
        assert_eq!(summary(&ps.as_array().unwrap()[..1], (0, 1)).since, None, "no events cache: no since");
        assert_eq!(summary(ps.as_array().unwrap(), (0, 1)).since, Some(5));
    }

    // Chain: R12 tray agents row. Risk: stale probe shown or missing probe shown as known.
    #[test]
    fn agents_come_from_the_freshest_probe_or_are_unknown() {
        let row = |v: &str| json!({"runtime": "claude", "path": "/c", "version": v, "login": "not logged in", "account": null});
        let ps = [json!({"agents": {"at": 10, "rows": [row("1")]}}), json!({"agents": {"at": 20, "rows": [row("2")]}}), json!({})];
        assert_eq!(agents(&ps), json!({"at": 20, "rows": ["claude 2 · not logged in"]}));
        assert_eq!(agents(&ps[2..]), Value::Null);
    }

    // Chain: § 6 Add project from the tray. Risk: a folder is classified wrong and set up wrong.
    #[test]
    fn add_project_has_four_cases() {
        let d = std::env::temp_dir().join(format!("taskq-add-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        let kind = |st: &dyn Fn(&Path) -> Result<(), String>| { let v = classify(&d, st); (v["kind"].as_str().unwrap().to_string(), v["reason"].as_str().unwrap().to_string()) };
        let pass = |_: &Path| Ok(());
        assert_eq!(kind(&pass).0, "new", "empty folder");
        fs::write(d.join("notes.txt"), "x").unwrap();
        assert_eq!(kind(&pass).0, "plain", "files without Git: not a repository");
        fs::create_dir(d.join(".git")).unwrap();
        assert_eq!(kind(&pass).0, "new", "a Git root without taskq.json");
        fs::write(d.join("taskq.json"), "{}").unwrap();
        assert_eq!(kind(&pass).0, "taskq");
        assert_eq!(kind(&|_: &Path| Err("taskq.json: repo is required".into())), ("old".into(), "taskq.json: repo is required".into()), "the verified reason");
        let _ = fs::remove_dir_all(&d);
    }
    // Chain: canonical session -> native row -> shared task link (#1204). Risk: a Codex thread opens off its host,
    // a remote Claude row loses its account-scoped link, or a starting observation opens anything.
    #[test]
    fn project_row_links_claude_anywhere_codex_only_locally() {
        let url = "https://claude.ai/code/session_014abc";
        let mut row = json!({"n": 938, "session": "codex:valid-thread win"});
        task_session(Path::new("."), "win", &mut row);
        assert_eq!((row["session_url"].clone(), row["runtime"].clone()), (json!("codex://threads/valid-thread"), json!("codex")), "local codex: deep link");
        let mut codex = json!({"session": "codex:valid-thread mac"});
        task_session(Path::new("."), "win", &mut codex);
        assert!(codex["session_url"].is_null());
        assert_eq!(codex["session_reason"], "Codex session opens only on mac", "remote codex: no link, reason");
        let mut remote = json!({"session": "claude:valid-thread mac", "session_url": url});
        task_session(Path::new("."), "win", &mut remote);
        assert_eq!((remote["local_session"].clone(), remote["session_url"].clone()), (json!(false), json!(url)), "remote claude: link");
        let mut forged = json!({"session": "claude:valid-thread mac", "session_url": "https://evil.example/session_1"});
        task_session(Path::new("."), "win", &mut forged);
        assert!(forged["session_url"].is_null(), "only a canonical claude.ai URL");
        let mut unpublished = json!({"session": "claude:valid-thread mac"});
        task_session(Path::new("."), "win", &mut unpublished);
        assert!(unpublished["session_url"].is_null() && unpublished["session_reason"].is_null());
        let mut invalid = json!({"session": "claude:bad/path win", "session_url": url});
        task_session(Path::new("."), "win", &mut invalid);
        assert!(invalid["session_url"].is_null());
        let mut local = json!({"session": "claude:valid-thread win"});
        task_session(Path::new("."), "win", &mut local);
        assert!(local["session_url"].is_null(), "a local UUID does not prove remote registration");
        let mut starting = json!({"session": "claude:- mac", "session_url": url});
        task_session(Path::new("."), "win", &mut starting);
        assert!(starting["session_url"].is_null(), "starting: no link");
        task_session(Path::new("."), "win", &mut Value::Null);
    }
}
