//! TaskQ's fault catalog; transport, budgets and dedup belong to coreq.
use coreq::incident::{Fault, Outcome, Transport};
use serde_json::{Value, json};
use std::cell::Cell;
use std::fs;
use std::path::Path;

pub const CATALOG: &[&str] = &[
    "invariant.double_attempt", "invariant.golden", "invariant.label_loop", "adapter.read_after_write",
    "adapter.bad_json", "dispatch.panic", "dispatch.start", "dispatch.reexec", "publication.readback",
    "publication.not_on_main", "update.smoke", "update.rollback",
];
const CODES: &[&str] = &[
    "read", "create", "update", "delete", "other", "direct", "pr", "ready", "waiting", "doing", "review",
    "ask", "later", "failed", "closed", "claim:rejected", "heartbeat:conflict", "passes:3", "spawn:missing",
    "http:2xx", "merge:unconfirmed", "ancestry:missing", "runtime:installed", "exit:101", "reexec:failed",
    "restart:stuck", "golden:mismatch", "smoke:failed", "starts:3", "fold.json", "observed", "defect",
];

thread_local!(static ACTIVE: Cell<bool> = const { Cell::new(false) });
#[cfg(test)]
thread_local!(static DETECTED: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) });
#[cfg(test)]
pub fn take_detected() -> Vec<String> { DETECTED.with(|v| std::mem::take(&mut *v.borrow_mut())) }

fn version(s: &str) -> bool {
    s.len() <= 64 && s.split_once('-').map_or(s, |p| p.0).split('.').count() == 3
        && s.split_once('-').map_or(s, |p| p.0).split('.').all(|n| !n.is_empty() && n.bytes().all(|c| c.is_ascii_digit()))
        && s.bytes().all(|c| c.is_ascii_alphanumeric() || b".-".contains(&c))
}

fn site(s: &str) -> bool {
    if s == "dep" { return true; }
    let Some((file, line)) = s.rsplit_once(':') else { return false };
    let Some(name) = file.strip_prefix("src/").and_then(|f| f.strip_suffix(".rs")) else { return false };
    ["main", "board", "core", "labels", "operator", "platform", "runtime", "service", "tray", "incident"].contains(&name)
        && line.parse::<u32>().is_ok_and(|n| n > 0)
}

fn safe(f: &Fault) -> bool {
    let value = |s: &&str| CODES.contains(s) || CATALOG.contains(s) || site(s) || s.strip_prefix("release:").is_some_and(version);
    CATALOG.contains(&f.code) && site(f.site) && matches!(f.board, "github" | "gitlab")
        && matches!(f.runtime, "none" | "claude" | "codex" | "codex-app")
        && f.fields.len() <= 8 && f.fields.iter().all(value)
        && [f.evidence, f.observed, f.inferred].into_iter().all(|a| a.len() <= 8 && a.iter().all(value))
        && f.revision.len() == 40 && f.revision.bytes().all(|c| c.is_ascii_hexdigit())
        && f.contract.len() == 12 && f.contract.bytes().all(|c| c.is_ascii_hexdigit())
}

fn unsafe_outcome(f: &Fault) -> Outcome {
    Outcome { state: "local", fault: CATALOG.contains(&f.code).then_some(f.code).unwrap_or("unknown").into(),
        at: now(), path: None, url: None, reason: Some("unsafe".into()) }
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn context(root: &Path) -> Option<(Value, Option<bool>)> {
    let v: Value = serde_json::from_slice(&fs::read(root.join("taskq.json")).ok()?).ok()?;
    let setting = match v.get("incident_reports") {
        None => None,
        Some(Value::Bool(b)) => Some(*b),
        _ => return None, // invalid configuration is never an incident or permission to send
    };
    Some((v, setting))
}

fn fault<'a>(code: &'a str, site: &'a str, runtime: &'a str, board: &'a str,
    fields: &'a [&'a str], evidence: &'a [&'a str], contract: &'a str) -> Fault<'a> {
    Fault { code, site, runtime, board, fields, evidence, revision: env!("TASKQ_REVISION"), contract,
        observed: evidence, inferred: CATALOG.iter().position(|c| *c == code).map(|i| &CATALOG[i..i+1]).unwrap_or(&[]) }
}

pub fn detect(code: &str, site: &str, runtime: &str, fields: &[&str], evidence: &[&str]) {
    #[cfg(test)]
    {
        let mut fingerprint_fields = vec![site];
        fingerprint_fields.extend_from_slice(fields);
        let f = fault(code, site, runtime, "github", &fingerprint_fields, evidence, "0123456789ab");
        assert!(safe(&f), "unsafe catalog detection {code}");
        assert!(coreq::incident::check(&coreq::incident::build(&crate::platform::tool(&crate::platform::default_install()), &f,
            "11111111-1111-4111-8111-111111111111")).is_ok());
        if let Some(root) = std::env::var_os("TASKQ_TEST_PANIC_REPORT") {
            let out = coreq::incident::report(&crate::platform::tool(&crate::platform::default_install()), Path::new(&root), Some(true), &f, None);
            assert_eq!(out.state, "unsupported-auth"); // fake transport: the subprocess never contacts a tracker
        }
        DETECTED.with(|v| v.borrow_mut().push(code.into()));
    }
    #[cfg(not(test))]
    {
        if ACTIVE.replace(true) { return; }
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let Ok(root) = crate::find_root() else { return };
            let Some((v, setting)) = context(&root) else { return };
            let tool = crate::platform::tool(&crate::platform::default_install());
            if coreq::incident::disabled(&tool, setting) { return; }
            let contract: String = ring::digest::digest(&ring::digest::SHA256, include_bytes!("../taskq.md"))
                .as_ref().iter().map(|b| format!("{b:02x}")).collect();
            let mut fingerprint_fields = vec![site];
            fingerprint_fields.extend_from_slice(fields);
            let f = fault(code, site, runtime, if v["board"] == "gitlab" { "gitlab" } else { "github" }, &fingerprint_fields, evidence, &contract[..12]);
            let mut out = if safe(&f) {
                coreq::incident::send(&tool, &root, setting, v["board"] != "gitlab" && v["host"].as_str().unwrap_or("github.com") == "github.com", &f)
            } else { unsafe_outcome(&f) };
            out.path = out.path.and_then(|p| p.strip_prefix(&root).ok().map(Path::to_path_buf));
            let _ = fs::create_dir_all(crate::dot(&root));
            let _ = crate::platform::write_atomic(&crate::dot(&root).join("incident.json"), &out.json().to_string());
        }));
        ACTIVE.set(false);
    }
    #[cfg(test)]
    let _ = (site, runtime, fields, evidence);
}

/// The shared launcher supplies only code values; remap its private implementation sites to TaskQ's adapter.
pub fn launcher(_tool: &coreq::Tool, f: &Fault) {
    match f.code {
        "update.rollback" => {
            let release = f.fields.first().and_then(|f| f.strip_prefix("failed:")).unwrap_or("");
            let field = format!("release:{release}");
            detect(f.code, concat!("src/platform.rs:", line!()), "none", &[&field], &["starts:3"]);
        }
        "dispatch.panic" => detect(f.code, concat!("src/platform.rs:", line!()), "none", &[], &["exit:101"]),
        _ => {},
    }
}

pub fn install_panic_hook() {
    // coreq initializes its guard on first report; an invalid probe does that without a file, send or panic-time hook change.
    let invalid = Fault { site: "/", ..fault("dispatch.panic", "dep", "none", "github", &[], &[], "0123456789ab") };
    let _ = coreq::incident::report(&crate::platform::tool(&crate::platform::default_install()), Path::new(""), Some(true), &invalid, None);
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if !ACTIVE.with(Cell::get) {
            let location = info.location().map(|l| {
                let file = l.file().replace('\\', "/");
                file.strip_prefix("src/").map(|f| format!("src/{f}:{}", l.line()))
                    .filter(|s| site(s)).unwrap_or_else(|| "dep".into())
            }).unwrap_or_else(|| "dep".into());
            detect("dispatch.panic", &location, "none", &[&location], &["exit:101"]);
            previous(info);
        }
    }));
}

pub fn status(root: &Path) -> Value {
    let Some((_, setting)) = context(root) else { return Value::Null };
    if coreq::incident::disabled(&crate::platform::tool(&crate::platform::default_install()), setting) {
        return json!({"state": "disabled"});
    }
    fs::read(crate::dot(root).join("incident.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(Value::Null)
}

pub fn notice(root: &Path) -> bool { context(root).is_some_and(|(_, s)| s.is_none()) }

pub fn disclosure(root: Option<&Path>) -> String {
    let tool = crate::platform::tool(&crate::platform::default_install());
    let mut text = coreq::incident::disclosure(&tool);
    #[cfg(not(test))]
    {
        let github = root.and_then(context).is_some_and(|(v, _)| v["board"] != "gitlab" && v["host"].as_str().unwrap_or("github.com") == "github.com");
        let account = coreq::incident::github_token(&tool, github).and_then(|token| {
            coreq::incident::GitHub::new(&tool, token).login().ok()
        });
        text.push_str(&format!("\nPosting account: {}", account.as_deref().unwrap_or("unavailable; local fallback")));
    }
    #[cfg(test)]
    { let _ = root; text.push_str("\nPosting account: test transport only"); }
    text
}

pub fn line(v: &Value) -> String {
    let state = v["state"].as_str().unwrap_or("none");
    if matches!(state, "none" | "disabled") { return format!("Incidents: {state}"); }
    if !matches!(state, "sent" | "local" | "unsupported-auth") { return "Incidents: unknown".into(); }
    let age = crate::age(now().saturating_sub(v["at"].as_u64().unwrap_or(now())) as i64);
    let reason = v["reason"].as_str().map(|r| format!(" ({r})")).unwrap_or_default();
    let dest = v["url"].as_str().or_else(|| v["path"].as_str()).map(|s| format!(" · {s}")).unwrap_or_default();
    format!("Incident: {state} {} {age}{reason}{dest}", v["fault"].as_str().unwrap_or("unknown"))
}

#[cfg(test)]
fn report(tool: &coreq::Tool, root: &Path, setting: Option<bool>, f: &Fault, net: Option<&mut dyn Transport>) -> Outcome {
    if coreq::incident::disabled(tool, setting) { return coreq::incident::report(tool, root, setting, f, None); }
    if !safe(f) { return unsafe_outcome(f); }
    coreq::incident::report(tool, root, setting, f, net)
}

#[cfg(test)]
#[path = "incident_tests.rs"]
mod tests;
