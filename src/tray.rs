//! `taskq tray` (taskq.md § 4): the host's one menu-bar / notification-area icon over the `<root>/.taskq/status.json`
//! and `events` of every project in `<install>/projects`. A click opens `tray.html` in a wry webview; Rust pushes one
//! JSON state, the page sends actions back. No board access and no token: actions go through the same files and
//! commands as the CLI.

use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};
use tao::dpi::{LogicalSize, PhysicalPosition};
use tao::event::{Event, StartCause, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tao::window::{Window, WindowBuilder};
use tray_icon::{Icon, MouseButtonState, Rect, TrayIcon, TrayIconBuilder, TrayIconEvent};
use wry::{WebView, WebViewBuilder};

use crate::icon::{self, Rgba, compact};

const HTML: &str = include_str!("tray.html");
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
    Click(Rect),
    Shot,
    Shot2,
    /// Add project: the picked folder classified, or Null (cancel)
    Added(Value),
    Answer(Value),
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
}

/// Pinned window (menu.md M13): pinned or not, the last display and the last top-left per display, physical pixels.
#[derive(Clone, PartialEq, Default)]
struct Pin {
    pinned: bool,
    display: String,
    pos: std::collections::BTreeMap<String, (i32, i32)>,
}

impl Settings {
    fn load(install: &Path) -> Settings {
        // § 4 Data: a corrupt tray.json shows the defaults with its reason; the file stays until a save keeps a copy
        let v = crate::data::json(&install.join("tray.json")).unwrap_or_else(|e| {
            eprintln!("{e}; defaults shown");
            None
        }).unwrap_or_default();
        let pos = v["pos"].as_object().into_iter().flatten().filter_map(|(k, p)| Some((k.clone(), (i32::try_from(p[0].as_i64()?).ok()?, i32::try_from(p[1].as_i64()?).ok()?)))).collect();
        let pin = Pin { pinned: v["pinned"] == true, display: v["display"].as_str().unwrap_or("").into(), pos };
        let mut s = Settings { view: "icon".into(), font: 16, period: "1d".into(), start: 0, end: 0, tz: "local".into(), icon: Value::Null, pin };
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
        (now - PERIODS.iter().find(|p| p.0 == self.period).map_or(86400, |p| p.1), now)
    }

    fn save(&self, install: &Path) -> bool {
        let v = json!({"view": self.view, "font": self.font, "period": self.period, "start": self.start, "end": self.end, "tz": self.tz, "icon": self.icon,
            "pinned": self.pin.pinned, "display": self.pin.display, "pos": self.pin.pos});
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
            crate::poke(root);
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
                crate::poke(Path::new(r));
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
        let out = crate::platform::quiet(owner_command(root)?.args(["status", "--json"])).output().map_err(|e| e.to_string())?;
        let snapshot = serde_json::from_slice(&out.stdout).map_err(|_| String::from_utf8_lossy(&out.stderr).into_owned())?;
        answer_snapshot(&snapshot, m)?;
        Ok((root, args))
    };
    let (root, args) = match prepare() {
        Ok(c) => c,
        Err(e) => return response("rejected", &e, "Refresh the question or open it on the board."),
    };
    let r = answer_dispatch(m, &args, |args| {
        let out = owner_command(root).and_then(|mut c| crate::platform::quiet(c.args(args)).output().map_err(|e| e.to_string()));
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

/// The project's capacity as one group of `taskq.resources/1` projection targets (host, project, runtimes) for the
/// shared resource controls (docs/design/shared-resource-controls.md): `cap` is `status.json` `capacity` (null:
/// unknown), `saved` the host's capacity from `taskq.json`. The only writes the tray offers are `service capacity`
/// fields: legacy limits and gate, and Auto/Fixed with its maximum when the service reports admission.
fn resource_group(cap: &Value, saved: &Value, observed_at: Option<i64>, installation: Option<String>, project: &str) -> Value {
    let ok = cap.is_object();
    let saved = if saved["error"].is_null() { saved } else { &Value::Null };
    let auto = if ok { cap["admission"]["mode"] == "auto" } else { saved["admission"]["mode"] == "auto" };
    let target = |p: bool, rt: Option<&str>| json!({"installation_id": installation, "generation": 1, "project_id": if p { json!(project) } else { Value::Null }, "runtime_id": rt});
    let legacy = |field: &str, scope: &str| json!({"policy_schema": "taskq.capacity/1", "modes": ["fixed"], "fields": [field], "scopes": [scope]});
    let none = json!({"sample": null, "runtime_usage": [], "blockers": [], "completeness": if ok { "complete" } else { "unknown" }});
    let entry = |t: Value, supported: Value, current: Value, effective: Value, rest: &Value| {
        let rev = |p: &Value| { let mut p = p.clone(); p.as_object_mut().map(|o| o.remove("sources")); revision(&p) };
        let mut e = json!({"target": t, "service_version": null, "supported": supported, "current_revision": if saved.is_null() { Value::Null } else { rev(&current) },
            "effective_revision": if ok { rev(&effective) } else { Value::Null }, "current_policy": current, "effective_policy": effective});
        for (k, v) in rest.as_object().unwrap() { e[k] = v.clone(); }
        e
    };
    let mut host_supported = vec![];
    let (mut cur, mut eff) = (json!({"sources": {}}), json!({"sources": {}}));
    if ok && cap["admission"].is_object() {
        // writable through `service capacity --mode/--max` (§ 5); advertised only when the service reports admission
        host_supported.push(json!({"policy_schema": "taskq.admission/1", "modes": ["auto", "fixed"], "fields": ["mode", "maximum"], "scopes": ["host"]}));
        let pol = |a: &Value| json!({"mode": a["mode"], "maximum": a["max"], "sources": {"mode": "unknown", "maximum": "unknown"}});
        (cur, eff) = (pol(&saved["admission"]), pol(&cap["admission"]));
    }
    let mut targets = vec![entry(target(false, None), json!(host_supported), cur, eff, &none)];
    let mut rts: Vec<String> = ["claude", "codex"].map(String::from).to_vec();
    for m in [&saved["limits"], &cap["limits"], &cap["used"]] {
        for k in m.as_object().into_iter().flatten().map(|(k, _)| k) { if !rts.contains(k) { rts.push(k.clone()); } }
    }
    let limits = if ok { &cap["limits"] } else { &saved["limits"] };
    let usage: Vec<Value> = rts.iter().map(|rt| {
        let l = limits[rt].as_u64();
        let kind = if !ok && saved.is_null() { "unknown" } else { match l { Some(0) => "disabled", Some(_) => "ceiling", None if auto => "unbounded", None => "disabled" } };
        json!({"runtime_id": rt, "used": if ok { json!(cap["used"][rt].as_u64().unwrap_or(0)) } else { Value::Null }, "limit": l, "limit_kind": kind})
    }).collect();
    let sample = if cap["sample_at"].is_i64() {
        json!({"source": format!("taskq-admission-{}", if cfg!(windows) { "windows" } else if cfg!(target_os = "macos") { "macos" } else { "linux" }),
            "sampled_at": cap["sample_at"], "age_seconds": cap["sample_age_seconds"], "cadence_seconds": 30, "window_seconds": if auto { 60 } else { 30 },
            "cpu_busy_percent": cap["cpu_percent"], "memory_available_percent": cap["memory_available_percent"], "memory_available_gib": null, "pressure": null})
    } else { Value::Null };
    const CODES: [&str; 18] = ["cpu", "memory", "pressure", "sample", "config", "cooldown", "spacing", "max", "version", "host", "runtime_disabled", "pause", "focus", "dependency", "routing", "access", "provider", "unknown"];
    let blockers: Vec<Value> = cap["reasons"].as_array().into_iter().flatten().map(|r| {
        let code = match r["code"].as_str().unwrap_or("") { "hold" => "host", c if CODES.contains(&c) => c, _ => "unknown" };
        let host_scope = ["host", "max", "spacing", "version"].contains(&code);
        json!({"code": code, "scope": target(!host_scope, None), "knowledge": if code == "sample" || code == "unknown" || (code == "host" && cap["gate"] == "unknown") { "unknown" } else { "known" }})
    }).collect();
    let gate = |g: &Value| json!({"load_gate": g, "sources": {"load_gate": "unknown"}});
    targets.push(entry(target(true, None), json!([legacy("load_gate", "project")]), gate(&saved["load_gate"]), gate(&cap["load_gate"]),
        &json!({"sample": sample, "runtime_usage": usage, "blockers": blockers, "completeness": if ok { "complete" } else { "unknown" }})));
    for rt in &rts {
        let lim = |m: &Value| json!({"runtime_limit": m[rt], "sources": {"runtime_limit": "unknown"}});
        targets.push(entry(target(true, Some(rt)), json!([legacy("runtime_limit", "runtime")]), lim(&saved["limits"]), lim(&cap["limits"]), &none));
    }
    json!({"schema": "taskq.resources/1", "observed_at": observed_at, "targets": targets})
}

/// One project for the page: its `status.json`, saved focus, labels and `events` lines `[time, kind, attempt]`.
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
        Some(json!([w.first()?.parse::<i64>().ok()?, w.get(2)?, w.get(3)?]))
    }).collect();
    // the local host's saved capacity, for `Edit limits` and its readback (§ 4 Capacity)
    let (cap_host, cap_saved, board_kind) = match crate::load(r) {
        Ok((c, v)) => (json!(c.me), crate::service::capacity(&v, &c.me).map(|x| x.json()).unwrap_or_else(|e| json!({"error": e})), json!(c.board.name)),
        Err(e) => (Value::Null, json!({"error": e}), Value::Null),
    };
    let mut tasks = s["tasks"].as_array().cloned().unwrap_or_default();
    for task in &mut tasks { task_session(r, cap_host.as_str().unwrap_or(""), task); }
    let installation = fs::read_to_string(install.join("installation-id")).ok().filter(|i| i.len() == 64 && i.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)));
    let id: String = crate::project_name(r).chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).take(64).collect();
    let resources = resource_group(&s["capacity"], &cap_saved, age.map(|a| now() - a as i64), installation, &id);
    json!({"root": root, "name": s["project"].as_str().map(String::from).unwrap_or_else(|| crate::project_name(r)),
        "issues": s["issues"].as_str().unwrap_or(""), "board_kind": board_kind, "closed": Value::Null,
        "log": s["log"].is_string(), "alive": age.is_some_and(|a| a < 300), "age": age,
        "complete": crate::snapshot::complete(&s) && age.is_some_and(|a| a < 300),
        "error": bad.map(Value::from).or_else(|| [&s["error"], &s["focus_error"]].into_iter().find(|e| e.is_string()).cloned()).unwrap_or(Value::Null), "focus": s["focus"].as_array().cloned().unwrap_or_default(), "saved": saved, "labels": crate::status_labels(r),
        "request": s["request"], "since": since, "events": events, "tasks": tasks, "host": s["host"], "version": s["version"], "update": s["update"],
        "resources": resources, "cap_host": cap_host, "cap_saved": cap_saved, "agents": s["agents"],
        "incident": s["incident"], "incident_line": s.get("incident").map(crate::incident::line), "incident_notice": s["incident_notice"],
        "incident_repo": crate::platform::RELEASE_REPO})
}

fn now() -> i64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Native adapter: the owning host may open its session; remote observations never invent a local link (M3).
fn task_session(root: &Path, local: &str, row: &mut Value) {
    let Some(object) = row.as_object_mut() else { return };
    object.remove("session_url");
    object.insert("local_session".into(), json!(false));
    let session = row["session"].as_str().unwrap_or("").to_string();
    let words: Vec<&str> = session.split_whitespace().collect();
    let Some((rt, id)) = words.first().and_then(|s| s.split_once(':')) else { return };
    let host = words.get(1).copied().unwrap_or("");
    row["runtime"] = json!(rt);
    row["host"] = json!(host);
    row["local_session"] = json!(!local.is_empty() && host == local);
    if local.is_empty() || host != local { return }
    let id = if (id == "-" && rt == "codex") || rt == "claude" {
        let l = crate::runtime::Launch { task: row["n"].as_u64().unwrap_or(0), title: String::new(), attempt: row["attempt"].as_u64().unwrap_or(0),
            role: row["role"].as_str().unwrap_or("").into(), rt: rt.into(), session: id.into(), url: String::new(), resume: None };
        crate::runtime::codex_thread(&crate::runtime::log_path(root, &l)).unwrap_or_else(|| id.into())
    } else { id.into() };
    if id == "-" || id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') { return }
    row["session_url"] = match rt {
        "codex" | "codex-app" => json!(format!("codex://threads/{id}")),
        "claude" => json!(crate::runtime::claude_remote(&id)),
        _ => Value::Null,
    };
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

pub fn run(install: PathBuf, snapshot: Option<PathBuf>) -> Result<(), String> {
    if snapshot.is_none() && !alone(&install) {
        return Err("taskq tray already runs on this host".into());
    }
    #[allow(unused_mut)]
    let mut event_loop = EventLoopBuilder::<Ev>::with_user_event().build();
    #[cfg(target_os = "macos")]
    {
        // menu bar only: no Dock icon, no app menu
        use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};
        event_loop.set_activation_policy(ActivationPolicy::Accessory);
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
    let mut settings = Settings::load(&install);
    let mut ui: Option<(Window, WebView)> = None;
    let mut tray: Option<TrayIcon> = None;
    let (mut shown, mut icon_sig, mut dark) = (String::new(), String::new(), true);
    let mut hidden_at = Instant::now() - Duration::from_secs(1);
    let mut anchor: Option<Rect> = None;
    let mut step = 0usize;
    let mut adding = Value::Null;
    let mut moved = false;
    event_loop.run(move |event, target, flow| {
        *flow = ControlFlow::Wait;
        let push = |ui: &Option<(Window, WebView)>, st: &Value| {
            if let Some((_, wv)) = ui {
                let _ = wv.evaluate_script(&format!("taskq.receive({st})"));
            }
        };
        match event {
            Event::NewEvents(StartCause::Init) => {
                let b = WindowBuilder::new().with_title("taskq").with_decorations(false).with_resizable(false).with_always_on_top(true)
                    .with_visible(snapshot.is_some()).with_inner_size(LogicalSize::new(320.0, 400.0)).with_position(tao::dpi::LogicalPosition::new(40.0, 40.0));
                #[cfg(windows)]
                let b = tao::platform::windows::WindowBuilderExtWindows::with_skip_taskbar(b, true);
                let p = proxy.clone();
                let built = b.build(target).map_err(|e| e.to_string()).and_then(|w| {
                    // only our page: a link goes to the browser through the bridge
                    WebViewBuilder::new().with_html(HTML.replace("{{ANSWER_CARD_CSS}}", include_str!("answer-card.css")).replace("{{ANSWER_CARD_JS}}", include_str!("answer-card.js"))
                        .replace("{{PROJECT_BLOCKS_CSS}}", include_str!("project-blocks.css")).replace("{{PROJECT_BLOCKS_JS}}", include_str!("project-blocks.js"))
                        .replace("{{RESOURCE_CONTROLS_CSS}}", include_str!("resource-controls.css")).replace("{{RESOURCE_CONTROLS_JS}}", include_str!("resource-controls.js"))).with_navigation_handler(|u| !u.starts_with("http"))
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
                refresh_icon(&tray, &st, &settings, dark, &mut icon_sig);
                let sig = st.to_string().replace(&format!("\"now\":{}", st["now"]), "");
                if sig != shown {
                    shown = sig;
                    push(&ui, &st);
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
            }
            Event::UserEvent(Ev::Answer(receipt)) => push(&ui, &receipt),
            Event::UserEvent(Ev::Ipc(m)) => {
                let Ok(m) = serde_json::from_str::<Value>(&m) else { return };
                let Some((w, _)) = &ui else { return };
                match m["a"].as_str().unwrap_or("") {
                    "answer" if snapshot.is_none() => {
                        let (p, install) = (proxy.clone(), install.clone());
                        std::thread::spawn(move || { let _ = p.send_event(Ev::Answer(answer(&install, &m))); });
                    }
                    "ready" => {
                        if snapshot.is_some() {
                            let _ = proxy.send_event(Ev::Shot);
                            return;
                        }
                        push(&ui, &state(&install, &settings, dark, None, &adding));
                    }
                    "size" => {
                        let max = w.current_monitor().map_or(800.0, |m| m.size().to_logical::<f64>(m.scale_factor()).height) * if snapshot.is_some() { 0.95 } else { 0.8 };
                        w.set_inner_size(LogicalSize::new(m["w"].as_f64().unwrap_or(320.0), m["h"].as_f64().unwrap_or(400.0).min(max)));
                        // pinned: the window stays where it was put; a move then keeps it in the work area
                        if let (Some(r), false) = (anchor, settings.pin.pinned) {
                            place(w, r);
                        }
                    }
                    "hide" => w.set_visible(false),
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
                        push(&ui, &state(&install, &settings, dark, None, &adding));
                    }
                    "drag" if settings.pin.pinned => {
                        let _ = w.drag_window();
                    }
                    "resources" => {
                        std::thread::spawn(resources);
                    }
                    "open" => {
                        if let Some(u) = m["url"].as_str().filter(|u| u.starts_with("https://") || u.starts_with("http://")) {
                            open(u);
                        }
                    }
                    "set" => {
                        settings.set(&m);
                        settings.save(&install);
                        let st = state(&install, &settings, dark, None, &adding);
                        refresh_icon(&tray, &st, &settings, dark, &mut icon_sig);
                        push(&ui, &st);
                    }
                    // `Edit limits` after the page's confirm: the same write as `service capacity` (§ 5), this host only
                    "capacity" => {
                        let root = m["root"].as_str().unwrap_or("");
                        if roots(&install).iter().any(|r| r == root) {
                            let msg = capacity(Path::new(root), &m);
                            push(&ui, &json!({"capmsg": {"root": root, "id": m["id"], "text": msg}}));
                        }
                    }
                    // Add project (§ 4): the folder comes only from the OS picker, never from the page
                    "add" => {
                        adding = json!({"state": "picking"});
                        push(&ui, &state(&install, &settings, dark, None, &adding));
                        let p = proxy.clone();
                        std::thread::spawn(move || {
                            let v = pick().map_or(Value::Null, |d| classify(&d, selftest));
                            let _ = p.send_event(Ev::Added(v));
                        });
                    }
                    "add-cancel" => {
                        adding = Value::Null;
                        push(&ui, &state(&install, &settings, dark, None, &adding));
                    }
                    "add-install" if adding["kind"] == "taskq" && adding["state"] == "picked" => {
                        adding["state"] = json!("installing");
                        push(&ui, &state(&install, &settings, dark, None, &adding));
                        let (p, a, inst) = (proxy.clone(), adding.clone(), install.clone());
                        std::thread::spawn(move || {
                            let _ = p.send_event(Ev::Added(install_service(&inst, a)));
                        });
                    }
                    "add-setup" if matches!(adding["kind"].as_str(), Some("new" | "plain")) => {
                        let text = format!("Set up {} as a taskq project", adding["name"].as_str().unwrap_or("this folder"));
                        ai(&install, "ai-send", &json!({"text": text, "onboard": adding["path"]}));
                        adding = Value::Null;
                        push(&ui, &state(&install, &settings, dark, None, &adding));
                    }
                    a if a.starts_with("ai-") => {
                        let mut m = m.clone();
                        m["onboard"] = Value::Null;
                        ai(&install, a, &m);
                        push(&ui, &state(&install, &settings, dark, None, &adding));
                    }
                    a => {
                        // only a registered project (§ 4): the page never names a path of its own
                        let root = m["root"].as_str().unwrap_or("");
                        if roots(&install).iter().any(|r| r == root) {
                            act(a, Path::new(root), m["label"].as_str());
                        }
                    }
                }
            }
            Event::UserEvent(Ev::Added(v)) => {
                adding = v;
                push(&ui, &state(&install, &settings, dark, None, &adding));
                if let (Some((w, _)), false) = (&ui, adding.is_null()) {
                    w.set_visible(true);
                    w.set_focus();
                }
            }
            // snapshot mode: per step push a state, wait for layout, capture, next
            Event::UserEvent(Ev::Shot) => {
                let dir = snapshot.clone().unwrap_or_default();
                let steps = shots(&install);
                if step == 0 {
                    let _ = fs::create_dir_all(&dir);
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
                    println!("snapshots in {}", dir.display());
                    *flow = ControlFlow::Exit;
                    return;
                };
                let mut s = settings.clone();
                s.font = font;
                push(&ui, &state(&install, &s, theme == "dark", Some((theme, &page)), &Value::Null));
                if let (Some((_, wv)), false) = (&ui, script.is_empty()) {
                    let _ = wv.evaluate_script(script);
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
                let name = format!("popover-{}.png", shot_name(&install, step));
                step += 1;
                capture(wv, dir.join(name), move || {
                    let _ = p.send_event(Ev::Shot);
                });
            }
            _ => {}
        }
    })
}

/// The `actions` snapshot: a refused custom range (menu.md M6), without applying a setting.
const ACTIONS: &str = "(()=>{const q=s=>document.querySelector(s),put=(s,v)=>{const i=q(s);i.value=v;i.dispatchEvent(new Event('input',{bubbles:true}));};
q('[data-route=cadence]').click();q('[data-v=custom]').click();put('#hs','2026-10-13T10:00');put('#he','2026-10-13T09:00');q('[data-a=range]').click();})()";

/// Snapshot steps: theme, page (`home` or a project root), text size, a script run on the page.
fn shots(install: &Path) -> Vec<(&'static str, String, u64, &'static str)> {
    let first = roots(install).into_iter().next();
    ["dark", "light"].iter().flat_map(|t| {
        let mut v = vec![(*t, "home".to_string(), 16, ""), (*t, "home".to_string(), 20, ""), (*t, "home".to_string(), 16, ACTIONS)];
        v.extend(first.clone().map(|r| (*t, r, 16, "")));
        for route in ["projects", "attention", "settings", "appearance", "cadence", "agents", "services", "about"] {
            for font in [16, 20] { v.push((*t, route.into(), font, "")); }
        }
        v
    }).collect()
}

fn shot_name(install: &Path, step: usize) -> String {
    shots(install).get(step).map_or(step.to_string(), |(t, p, f, js)| format!("{t}-{}{}-{f}", if Path::new(p).is_absolute() { "project" } else { p }, if js.is_empty() { "" } else { "-actions" }))
}

fn state(install: &Path, s: &Settings, dark: bool, snap: Option<(&str, &str)>, add: &Value) -> Value {
    let ps: Vec<Value> = roots(install).iter().map(|r| project(install, r)).collect();
    let (start, end) = s.range(now());
    let sum = summary(&ps, (start, end));
    let url = |rgba: &Rgba| format!("data:image/png;base64,{}", base64(&rgba.png()));
    let mut st = json!({"platform": if cfg!(windows) { "windows" } else { "macos" }, "version": crate::platform::VERSION,
        "host": ps.iter().find_map(|p| p["host"].as_str()).unwrap_or("?"), "view": s.view, "font": s.font, "period": s.period, "pinned": s.pin.pinned,
        "now": now(), "projects": ps, "agents": agents(&ps), "ai": {"target": target(install), "request": newest(install), "draft": requests_file(install)["draft"], "history": requests_file(install)["history"]}, "add": add, "history": {"start": start, "end": end, "tz": s.tz, "bins": sum.bins, "since": sum.since},
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

/// Host-level `Agents` (§ 4): the rows of the freshest probe in any `status.json`, as menu lines; null when none has one.
fn agents(ps: &[Value]) -> Value {
    let Some(a) = ps.iter().map(|p| &p["agents"]).filter(|a| a["at"].is_u64()).max_by_key(|a| a["at"].as_u64()) else { return Value::Null };
    json!({"at": a["at"], "rows": a["rows"].as_array().into_iter().flatten().map(crate::runtime::agent_text).collect::<Vec<_>>()})
}

/// The last 24 h, the period of the icon histogram (icon-summary.config.json).
fn day(now: i64) -> (i64, i64) {
    (now - 86400, now)
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

#[cfg(not(target_os = "macos"))]
fn work_area(m: &tao::monitor::MonitorHandle) -> (i32, i32, i32, i32) {
    (m.position().x, m.position().y, m.size().width as i32, m.size().height as i32)
}

fn act(action: &str, root: &Path, label: Option<&str>) {
    let d = crate::dot(root);
    match action {
        "pause" => { let _ = crate::pause(root); }
        "resume" => {
            let _ = crate::resume(root);
        }
        // a label of the project's open tasks, never `all` (§ 4)
        "focus" => {
            if let Some(l) = label.filter(|l| crate::status_labels(root).iter().any(|x| x == l)) {
                let _ = crate::set_focus(root, l, None);
            }
        }
        // `Update now` (menu.md M9): the service checks at once (§ 4 step 7); the page waits for the version in status.json
        "update" => {
            let _ = fs::write(d.join("update"), "");
            crate::poke(root);
        }
        "restart" => {
            let _ = fs::write(d.join("restart"), "");
            crate::poke(root);
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
            let Ok(me) = std::env::current_exe() else {
                return;
            };
            let mut c = std::process::Command::new(me);
            c.args(["service", action])
                .current_dir(root)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
            #[cfg(unix)]
            std::os::unix::process::CommandExt::process_group(&mut c, 0);
            let _ = crate::platform::quiet(&mut c).spawn();
        }
        _ => {}
    }
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
    let o = crate::platform::quiet(std::process::Command::new(me).arg("selftest").current_dir(dir).env("TASKQ_ROOT", dir)).output().map_err(|e| e.to_string())?;
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
        let o = crate::platform::quiet(&mut c).output().map_err(|e| e.to_string())?;
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
fn capacity(root: &Path, m: &Value) -> String {
    let set = &m["set"];
    let mut e = crate::CapEdit { gate: set["enabled"].as_bool(), ..Default::default() };
    let num = |k: &str| set[k].as_u64().map(|n| u32::try_from(n).map_err(|_| format!("{k}: not an integer 0..{}", u32::MAX))).transpose();
    let r = (|| {
        e.cpu_max = num("cpu_max")?;
        e.memory_min = num("memory_min")?;
        e.mode = match &set["mode"] {
            Value::Null => None,
            m if m == "auto" || m == "fixed" => Some(m == "auto"),
            m => return Err(format!("mode: auto or fixed, not {m}")),
        };
        e.max = match set.get("maximum") {
            None => None,
            Some(Value::Null) => Some(None),
            Some(n) => Some(Some(n.as_u64().and_then(|n| u32::try_from(n).ok()).ok_or(format!("maximum: not an integer 0..{}", u32::MAX))?)),
        };
        for (rt, n) in set["limits"].as_object().into_iter().flatten() {
            e.limits.insert(rt.clone(), n.as_u64().and_then(|n| u32::try_from(n).ok()).ok_or(format!("{rt}: not an integer 0..{}", u32::MAX))?);
        }
        let me = crate::load(root)?.0.me;
        crate::write_capacity(root, &me, &e, Some(&m["expect"]))
    })();
    match r {
        Ok((old, new)) => format!("Saved: {}", crate::cap_changes(&old, &new)),
        Err(e) => format!("Error: {e}"),
    }
}

fn open(arg: &str) {
    let opener = if cfg!(windows) { "explorer" } else { "open" };
    let _ = crate::platform::quiet(std::process::Command::new(opener).arg(arg)).spawn();
}

/// The menu bar's theme: macOS follows the app appearance; the Windows taskbar has its own setting.
fn is_dark(_w: &Window) -> bool {
    #[cfg(windows)]
    {
        let mut c = std::process::Command::new("reg");
        c.args(["query", r"HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize", "/v", "SystemUsesLightTheme"]);
        let out = crate::platform::quiet(&mut c).output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
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

    // Chain: status.json capacity -> tray projection -> shared resource controls. Risk: absent reads as zero, a legacy
    // guard reads as Auto, saved reads as applied, or an unknown producer reason disappears.
    #[test]
    fn capacity_projects_to_resource_targets() {
        let saved = json!({"limits": {"claude": 0}, "load_gate": {"enabled": true, "cpu_max": 85, "memory_min": 10}, "admission": {"mode": "auto", "max": null}});
        let cap = json!({"limits": {"claude": 0}, "used": {"claude": 1}, "load_gate": {"enabled": true, "cpu_max": 90, "memory_min": 10},
            "sample_at": 95, "sample_age_seconds": 3, "cpu_percent": 12.5, "memory_available_percent": 40.0, "reasons": [{"code": "hold"}, {"code": "future"}, {"code": "cpu"}]});
        let g = resource_group(&cap, &saved, Some(100), None, "taskq");
        let t = g["targets"].as_array().unwrap();
        assert_eq!(t.len(), 4);
        assert!(t[0]["target"]["project_id"].is_null() && t[0]["supported"] == json!([]), "no admission in status: legacy guard");
        let u = &t[1]["runtime_usage"];
        assert_eq!(u[0], json!({"runtime_id": "claude", "used": 1, "limit": 0, "limit_kind": "disabled"}));
        assert_eq!(u[1], json!({"runtime_id": "codex", "used": 0, "limit": null, "limit_kind": "disabled"}), "Fixed: absent admits none");
        assert_ne!(t[1]["current_revision"], t[1]["effective_revision"], "saved gate 85 is not applied 90");
        assert_eq!(t[2]["current_revision"], t[2]["effective_revision"]);
        let codes: Vec<_> = t[1]["blockers"].as_array().unwrap().iter().map(|b| b["code"].as_str().unwrap()).collect();
        assert_eq!(codes, ["host", "unknown", "cpu"]);
        assert!(t[1]["blockers"][0]["scope"]["project_id"].is_null());
        assert_eq!(t[1]["sample"]["age_seconds"], 3);
        let mut auto = cap.clone();
        auto["admission"] = json!({"mode": "auto", "max": null});
        let g = resource_group(&auto, &saved, Some(100), None, "taskq");
        assert_eq!(g["targets"][0]["supported"][0]["policy_schema"], "taskq.admission/1");
        assert_eq!(g["targets"][0]["supported"][0]["fields"], json!(["mode", "maximum"]), "Work mode writes Auto/maximum");
        assert_eq!(g["targets"][1]["runtime_usage"][1]["limit_kind"], "unbounded", "Auto: absent has no count cap");
        let unknown = resource_group(&Value::Null, &json!({"error": "not configured"}), None, None, "taskq");
        assert_eq!(unknown["targets"][1]["runtime_usage"][0], json!({"runtime_id": "claude", "used": null, "limit": null, "limit_kind": "unknown"}));
        assert!(unknown["targets"][1]["effective_revision"].is_null() && unknown["targets"][1]["current_revision"].is_null());
    }

    // Chain: Work mode -> tray bridge -> `service capacity --mode/--max`. Risk: an unknown mode reads as Fixed or a
    // malformed maximum removes the ceiling.
    #[test]
    fn work_mode_bridge_refuses_malformed_mode_and_maximum() {
        let root = Path::new("/nonexistent-taskq-954");
        assert!(capacity(root, &json!({"set": {"mode": "fast"}})).starts_with("Error: mode: auto or fixed"));
        assert!(capacity(root, &json!({"set": {"maximum": "4"}})).starts_with("Error: maximum: not an integer"));
        assert!(capacity(root, &json!({"set": {"maximum": -1}})).starts_with("Error: maximum: not an integer"));
        assert!(!capacity(root, &json!({"set": {"mode": "auto", "maximum": null}})).contains("mode:"), "valid fields reach the config read");
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
        assert!(include_str!("tray.html").contains("Settings unknown: '+esc(S.settings_error)"));
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

    // Chain: § 4 tray settings input. Risk: webview input stores unknown values.
    #[test]
    fn settings_keep_only_known_values() {
        let mut s = Settings { view: "icon".into(), font: 16, period: "1d".into(), start: 0, end: 0, tz: "local".into(), icon: Value::Null, pin: Pin::default() };
        s.set(&json!({"view": "full", "font": 18, "period": "1w"}));
        s.set(&json!({"view": "x", "font": 15, "period": "2y"}));
        s.set(&json!({"period": "custom", "start": 50, "end": 50, "tz": "UTC"}));
        assert!(s == Settings { view: "full".into(), font: 18, period: "1w".into(), start: 0, end: 0, tz: "local".into(), icon: Value::Null, pin: Pin::default() });
        assert_eq!(s.range(1000), (1000 - 604800, 1000));
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
        assert!(!s.pin.pinned, "unpinned by default");
        s.pin = Pin { pinned: true, display: "B".into(), pos: [("A".into(), (10, 20)), ("B".into(), (-1500, 300))].into() };
        assert!(s.save(&install));
        let r = Settings::load(&install);
        assert!(r.pin == s.pin && r.font == 18);
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
    // Chain: canonical session -> native row -> shared task link. Risk: a remote/starting observation opens locally.
    #[test]
    fn project_row_links_only_owned_valid_sessions() {
        let mut row = json!({"n": 938, "session": "codex:valid-thread win"});
        task_session(Path::new("."), "win", &mut row);
        assert_eq!(row["session_url"], "codex://threads/valid-thread");
        assert_eq!(row["runtime"], "codex");
        let mut remote = json!({"session": "claude:valid-thread mac"});
        task_session(Path::new("."), "win", &mut remote);
        assert_eq!(remote["local_session"], false);
        assert!(remote["session_url"].is_null());
        let mut invalid = json!({"session": "claude:bad/path win"});
        task_session(Path::new("."), "win", &mut invalid);
        assert!(invalid["session_url"].is_null());
        let mut local = json!({"session": "claude:valid-thread win"});
        task_session(Path::new("."), "win", &mut local);
        assert!(local["session_url"].is_null(), "a local UUID does not prove remote registration");
        let mut starting = json!({"session": "claude:- win", "session_url": "https://unverified.example"});
        task_session(Path::new("."), "win", &mut starting);
        assert!(starting["session_url"].is_null());
        task_session(Path::new("."), "win", &mut Value::Null);
    }
}
