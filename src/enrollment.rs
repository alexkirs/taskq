//! DashQ observation connection (docs/design/enrollment.md): coreq pairing and delivery of the board projection.
//! No command authority or task-state storage.
#![allow(dead_code)] // `coalesce` is the reader-side rule; no TaskQ reader yet.

use crate::{Res, core, snapshot::Snapshot};
use coreq::enroll::{self, Backoff, Delivery, Https};
use ring::rand::{SecureRandom, SystemRandom};
use serde::Serialize;
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs, io::{Read, Write}, path::{Path, PathBuf}, thread::JoinHandle};

pub const SCHEMA: &str = "taskq-board/1";
/// DashQ C4 metadata: at most 256 array items.
const MAX_TASKS: usize = 256;
/// An unchanged projection is resent after this many seconds, so DashQ's observation time stays fresh.
const RESEND: u64 = 300;

/// Protected coreq connection state; `platform::sweep` and data pruning keep it.
pub fn dir(install: &Path) -> PathBuf {
    install.join("connection")
}

/// DashQ serves machine routes under `/machine/`; its page prints the bare origin.
pub fn destination(url: &str) -> Res<String> {
    let u = url.trim_end_matches('/');
    if !u.starts_with("https://") || u.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(format!("not an https URL: {url}"));
    }
    Ok(if u.ends_with("/machine") { u.into() } else { format!("{u}/machine") })
}

/// The active connection, read without creating state (coreq's first read writes a new file).
pub fn active(install: &Path) -> Res<Option<Value>> {
    if state(install)?.is_none() { return Ok(None) }
    enroll::active(&dir(install))
}

/// Connection file for `connection status`; never printed whole (it holds the credential). A file that is not
/// coreq's state shape reads unknown and is kept, never as "no connection".
pub fn state(install: &Path) -> Res<Option<Value>> {
    let bad = || "connection: unknown (connection state unreadable; file kept)".to_string();
    let s: Value = match fs::read_to_string(dir(install).join("connection.json")) {
        Ok(t) => serde_json::from_str(&t).map_err(|_| bad())?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("connection: unknown (connection state: {e})")),
    };
    let text = |v: &Value, k: &str| v[k].as_str().is_some_and(|t| !t.is_empty());
    let conn = |v: &Value, keys: &[&str]| v.is_null() || (v.is_object() && keys.iter().all(|k| text(v, k)));
    let valid = s.is_object() && text(&s, "installation")
        && conn(&s["pending"], &["destination"]) && conn(&s["pending"]["staged"], &["destination", "credential"])
        && conn(&s["active"], &["destination", "credential"])
        && s["revoke"].as_array().is_some_and(|r| r.iter().all(|v| v.is_object() && text(v, "credential")));
    if valid { Ok(Some(s)) } else { Err(bad()) }
}

/// `taskq enroll`: browser or headless approval of this board's scope, then the first snapshot until DashQ
/// projects it. Capabilities are none: the grant can neither answer tasks nor run commands.
pub fn pair(install: &Path, url: &str, host: &str, board: &BoardScope, browser: bool) -> Res<Value> {
    let dest = destination(url)?;
    let id = installation_id(install)?;
    let value = board.value();
    let req = enroll::Request { installation: Some(&id), product: "taskq", host_label: host, schemas: &[SCHEMA], scope: Some((SCHEMA, &value)), capabilities: &[] };
    enroll::pair(&dir(install), &dest, &req, &mut Https { browser })
}

/// Sends `record` once and asks for its receipt until DashQ projects it (or `tries` run out).
pub fn first_receipt(install: &Path, record: &Value, tries: u32) -> Res<&'static str> {
    let d = dir(install);
    let mut w = Https { browser: false };
    match enroll::deliver(&d, std::slice::from_ref(record), &mut Backoff::default(), &mut w) {
        Ok(r) if r[0]["state"] == "accepted" => {}
        Ok(r) => return Err(format!("first snapshot rejected: {}", r[0]["reason"].as_str().unwrap_or("unknown"))),
        Err(Delivery::Retry(_)) => return Err("first snapshot not delivered; the service retries".into()),
        Err(Delivery::Stopped(e)) => return Err(format!("first snapshot not delivered: {e}")),
    }
    let mut state = "accepted";
    for i in 0..tries {
        state = enroll::receipt(&d, record, &mut w)?;
        if state == "projected" { break }
        if i + 1 < tries { std::thread::sleep(std::time::Duration::from_secs(2)) }
    }
    Ok(state)
}

/// `taskq disconnect`: cancels a pairing in progress (so `enroll` cannot resume it), stops sending at once and
/// reports what was revoked; a revoke DashQ has not confirmed stays queued for the service.
pub fn disconnect(install: &Path) -> Res<Option<String>> {
    disconnect_with(install, &mut Https { browser: false })
}

fn disconnect_with(install: &Path, w: &mut dyn enroll::Wire) -> Res<Option<String>> {
    let Some(s) = state(install)? else { return Ok(None) };
    let (active, pending) = (s["active"].is_object(), s["pending"].is_object());
    if !active && !pending { return Ok(None) }
    let queued = |e: &String| e.ends_with("revoke pending");
    if pending {
        // a staged credential DashQ already granted is queued for revoke by coreq
        enroll::cancel(&dir(install), w).or_else(|e| if queued(&e) { Ok(()) } else { Err(e) })?;
    }
    enroll::disconnect(&dir(install), w).or_else(|e| if queued(&e) { Ok(()) } else { Err(e) })?;
    let left = state(install)?.map_or(0, |s| s["revoke"].as_array().map_or(0, Vec::len));
    let mut out = vec![];
    if pending { out.push("pairing cancelled".to_string()) }
    if active { out.push("sending stopped".into()) }
    out.push(if left > 0 { format!("{left} revoke pending, the service retries it") }
        else if active || s["pending"]["staged"].is_object() || s["revoke"].as_array().is_some_and(|r| !r.is_empty()) { "revoke confirmed".into() }
        else { "nothing granted to revoke".into() });
    Ok(Some(format!("disconnected: {}", out.join("; "))))
}

/// The approved board of an active connection; `None` for a scope this version cannot read.
pub fn approved(active: &Value) -> Option<BoardScope> {
    let v = active["scope"]["value"].as_str().filter(|_| active["scope"]["name"] == SCHEMA)?;
    let (kind, rest) = v.split_once(':')?;
    let (host, repo) = rest.split_once('/')?;
    BoardScope::new(kind, host, repo).ok().filter(|b| b.value() == v)
}

/// The record of one snapshot: payload without `observed` names its content, so equal content dedupes by digest.
pub fn record(install: &Path, s: &Snapshot, active: &Value) -> Res<(String, Value)> {
    let board = approved(active).ok_or("connection has no readable board scope")?;
    let id = active["installation"].as_str().unwrap_or_default();
    let mut payload = serde_json::to_value(project(s, id, &[board])?).map_err(|e| e.to_string())?;
    let observed = payload.as_object_mut().unwrap().remove("observed");
    let digest = enroll::digest(&payload);
    payload["observed"] = observed.unwrap_or_default();
    let at = u64::try_from(s.observed).unwrap_or(0);
    let r = enroll::stamp(&dir(install), SCHEMA, &format!("{at}-{}", &digest[..16]), at, payload)?;
    Ok((digest, r))
}

type Job = JoinHandle<(Backoff, Result<String, Delivery>)>;

/// The service's sender: one delivery at a time on its own thread, never blocking a pass; each try sends the
/// latest snapshot (an older unsent one is dropped); a failure only waits (docs/design/enrollment.md).
#[derive(Default)]
pub struct Sender {
    /// Content digest (empty for a revoke retry), start time, thread.
    job: Option<(String, u64, Job)>,
    backoff: Backoff,
    wait_until: u64,
    sent: Option<(String, u64)>,
    /// Last outcome for `status.json` `connection`.
    pub outcome: Option<String>,
}

impl Sender {
    /// One step per service pass; returns a log line when the outcome changes.
    pub fn tick(&mut self, install: &Path, snap: Option<&Snapshot>, now: u64) -> Option<String> {
        let before = self.outcome.clone();
        if self.job.as_ref().is_some_and(|j| j.2.is_finished()) {
            let (digest, started, job) = self.job.take().unwrap();
            let r = job.join().map(|(b, r)| { self.backoff = b; r }).unwrap_or_else(|_| Err(Delivery::Retry(60)));
            self.outcome = Some(match r {
                Ok(state) => {
                    if digest.is_empty() { self.wait_until = now + RESEND } else { self.sent = Some((digest, started)) }
                    state
                }
                Err(Delivery::Retry(secs)) => { self.wait_until = now + secs; format!("retry in {secs} s") }
                Err(Delivery::Stopped(e)) => { self.wait_until = now + RESEND; format!("stopped: {e}") }
            });
        }
        if self.job.is_none() && now >= self.wait_until {
            if let Err(e) = self.start(install, snap, now) {
                self.outcome = Some(e);
                self.wait_until = now + RESEND;
            }
        }
        (self.outcome != before).then(|| format!("dashq connection: {}", self.outcome.as_deref().unwrap_or("none")))
    }

    fn start(&mut self, install: &Path, snap: Option<&Snapshot>, now: u64) -> Result<(), String> {
        let d = dir(install);
        let Some(a) = active(install)? else {
            if self.sent.take().is_some() { self.outcome = Some("disconnected".into()) }
            // disconnected offline: queued revokes retry until DashQ confirms them
            if state(install).ok().flatten().is_some_and(|s| s["revoke"].as_array().is_some_and(|r| !r.is_empty())) {
                let mut backoff = std::mem::take(&mut self.backoff);
                self.job = Some((String::new(), now, std::thread::spawn(move || {
                    let r = enroll::retry_revokes(&d, &mut Https { browser: false });
                    let wait = backoff.next(None);
                    (backoff, r.map(|_| "revoke confirmed".into()).map_err(|_| Delivery::Retry(wait)))
                })));
            }
            return Ok(());
        };
        // another project's board, or no board read yet: nothing to send from here
        let Some(s) = snap.filter(|s| approved(&a).is_some_and(|b| BoardScope::snapshot(s).is_ok_and(|x| x == b))) else { return Ok(()) };
        let (digest, rec) = record(install, s, &a).map_err(|e| format!("not sent: {e}"))?;
        if self.sent.as_ref().is_some_and(|(d, at)| *d == digest && now < at + RESEND) { return Ok(()) }
        let mut backoff = std::mem::take(&mut self.backoff);
        self.job = Some((digest, now, std::thread::spawn(move || {
            let r = enroll::deliver(&d, &[rec], &mut backoff, &mut Https { browser: false }).map(|r| match r[0]["state"].as_str() {
                Some("accepted") => "accepted".to_string(),
                _ => format!("rejected: {}", r[0]["reason"].as_str().unwrap_or("unknown")),
            });
            (backoff, r)
        })));
        Ok(())
    }
}

/// Create once, outside release folders; a corrupt identity never silently becomes a new installation.
pub fn installation_id(install: &Path) -> Res<String> {
    let path = install.join("installation-id");
    let read = || -> Res<Option<String>> {
        let bytes = fs::File::open(&path).and_then(|f| {
            let mut bytes = Vec::new();
            f.take(65).read_to_end(&mut bytes)?;
            Ok(bytes)
        });
        match bytes {
            Ok(b) if b.len() == 64 && b.iter().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(c)) =>
                Ok(Some(String::from_utf8(b).unwrap())),
            Ok(_) => Err("installation-id: corrupt; file kept".into()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("installation-id: unreadable ({e}); file kept")),
        }
    };
    if let Some(id) = read()? { return Ok(id) }
    fs::create_dir_all(install).map_err(|e| e.to_string())?;
    let mut bytes = [0; 32];
    SystemRandom::new().fill(&mut bytes).map_err(|_| "installation-id: randomness unavailable")?;
    let id: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    let temp = install.join(format!("installation-id-{id}.tmp"));
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)] {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let publish = || -> Res<()> {
        let mut file = opts.open(&temp).map_err(|e| e.to_string())?;
        file.write_all(id.as_bytes()).and_then(|_| file.sync_all()).map_err(|e| e.to_string())?;
        drop(file);
        // A hard link publishes complete bytes without overwriting another project's winner.
        match fs::hard_link(&temp, &path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
            Err(e) => Err(format!("installation-id: publish failed ({e})")),
        }
    };
    let result = publish();
    let _ = fs::remove_file(&temp);
    result?;
    read()?.ok_or("installation-id: missing after publication".into())
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct BoardScope {
    kind: String,
    host: String,
    repo: String,
}

impl BoardScope {
    /// `<kind>:<host>/<repo>`: the scope DashQ's approval page shows and its grant stores.
    pub fn value(&self) -> String {
        format!("{}:{}/{}", self.kind, self.host, self.repo)
    }

    pub fn new(kind: &str, host: &str, repo: &str) -> Res<Self> {
        let safe = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b".-_".contains(&b)) && s != "." && s != "..";
        if !matches!(kind, "github" | "gitlab") || !safe(host) || repo.split('/').count() < 2 || !repo.split('/').all(safe) {
            return Err("invalid board scope".into());
        }
        Ok(Self { kind: kind.into(), host: host.to_ascii_lowercase(), repo: if kind == "github" { repo.to_ascii_lowercase() } else { repo.into() } })
    }

    fn snapshot(s: &Snapshot) -> Res<Self> {
        let field = |k: &str| s.board[k].as_str().ok_or("missing board identity".to_string());
        Self::new(field("kind")?, field("host")?, field("repo")?)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct Choice { number: usize, label: String, recommended: bool }

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct TaskObservation {
    n: u64,
    state: String,
    revision: u64,
    choices: Vec<Choice>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Observation {
    schema: u64,
    installation: String,
    board: BoardScope,
    observed: i64,
    complete: bool,
    reason: Option<&'static str>,
    totals: Option<BTreeMap<&'static str, usize>>,
    tasks: Vec<TaskObservation>,
}

/// The scope is the approved board list, never inferred from host names or projects on disk.
pub fn project(s: &Snapshot, installation: &str, approved: &[BoardScope]) -> Res<Observation> {
    let board = BoardScope::snapshot(s)?;
    if !approved.contains(&board) { return Err("board outside approved scope".into()) }
    if installation.len() != 64 || !installation.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)) {
        return Err("invalid installation identity".into());
    }
    let over = s.tasks.len() > MAX_TASKS;
    Ok(Observation {
        schema: 1, installation: installation.into(), board, observed: s.observed,
        complete: s.reason.is_none() && !over,
        reason: if s.reason.is_some() { Some("source incomplete") } else if over { Some("task limit") } else { None },
        totals: s.reason.is_none().then(|| s.totals()),
        tasks: s.tasks.iter().take(MAX_TASKS).map(|r| TaskObservation {
            n: r.n, state: r.task.state.name().into(), revision: r.revision,
            choices: core::options(r.task.card.as_deref().unwrap_or_default()).into_iter()
                .map(|(number, label, recommended)| Choice { number, label, recommended }).collect(),
        }).collect(),
    })
}

/// One row per approved board, never the sum of host snapshots. Delivery time cannot refresh data.
pub fn coalesce(observations: &[Observation], approved: &[BoardScope], now: i64, max_age: u64) -> Vec<Value> {
    let mut boards = BTreeMap::new();
    for board in approved {
        let latest = observations.iter().filter(|o| &o.board == board).max_by_key(|o| o.observed);
        let row = match latest {
            None => json!({"board": board, "freshness": "missing", "complete": false, "reason": "no observation", "totals": null, "observation": null}),
            Some(o) => {
                // Same-time reports differing only in installation are the same board observation.
                let conflict = observations.iter().filter(|v| &v.board == board && v.observed == o.observed)
                    .any(|v| v.complete != o.complete || v.totals != o.totals || v.tasks != o.tasks);
                let age = now.checked_sub(o.observed);
                let freshness = if conflict || o.observed <= 0 || age.is_none_or(|a| a < 0) { "unknown" }
                    else if age.unwrap() as u64 > max_age { "stale" } else { "fresh" };
                json!({"board": board, "freshness": freshness, "complete": o.complete && !conflict,
                    "reason": if conflict { Some("conflicting observations") } else { o.reason },
                    "totals": if freshness == "fresh" && !conflict { json!(o.totals) } else { Value::Null },
                    "observation": if conflict { Value::Null } else { json!(o) }})
            }
        };
        boards.insert(board.clone(), row);
    }
    boards.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Chain: identity creation -> concurrent project startup -> update/data pruning -> reread.
    #[test]
    fn identity_survives_pruning_and_concurrent_creation() {
        let root = std::env::temp_dir().join(format!("taskq-enrollment-{}", std::process::id()));
        let install = root.join("install");
        let _ = fs::remove_dir_all(&root);
        let ids = std::thread::scope(|s| {
            let threads: Vec<_> = (0..8).map(|_| s.spawn(|| installation_id(&install).unwrap())).collect();
            threads.into_iter().map(|t| t.join().unwrap()).collect::<Vec<_>>()
        });
        assert!(ids.iter().all(|id| id == &ids[0]));
        crate::platform::sweep(&install, &root.join("absent-shim"));
        crate::data::prune(&root, &install, "p", &[]);
        assert_eq!(installation_id(&install).unwrap(), ids[0]);
        fs::write(install.join("installation-id"), "broken").unwrap();
        assert!(installation_id(&install).unwrap_err().contains("corrupt"));
        assert_eq!(fs::read_to_string(install.join("installation-id")).unwrap(), "broken");
        // connection state survives the sweep and pruning too
        fs::create_dir_all(dir(&install)).unwrap();
        fs::write(dir(&install).join("connection.json"), "{}").unwrap();
        crate::platform::sweep(&install, &root.join("absent-shim"));
        assert!(dir(&install).join("connection.json").exists());
        fs::remove_file(install.join("installation-id")).unwrap();
        assert_ne!(installation_id(&install).unwrap(), ids[0]);
        fs::remove_dir_all(root).unwrap();
    }

    /// DashQ stand-in: answers revokes (or nothing, offline) and records the credentials it was asked to revoke.
    struct Dash { online: bool, revoked: Vec<String> }
    impl enroll::Wire for Dash {
        fn post(&mut self, _: &str, route: &str, auth: Option<&str>, _: &Value) -> Result<(u16, Value), String> {
            if !self.online { return Err("offline".into()) }
            assert_eq!(route, "/connection/revoke", "disconnect only revokes");
            self.revoked.push(auth.unwrap_or_default().into());
            Ok((200, json!({"protocol": 1})))
        }
        fn now(&self) -> u64 { 0 }
        fn sleep(&mut self, _: u64) {}
        fn show(&mut self, _: &str, _: &str) {}
    }

    // Chain: enroll interrupted mid-pairing -> taskq disconnect -> enroll rerun. Risk: disconnect reporting the
    // revoke confirmed while a pending pairing (or its staged, already granted credential) survives and resumes.
    #[test]
    fn disconnect_cancels_pending_pairing() {
        let install = std::env::temp_dir().join(format!("taskq-disconnect-{}", std::process::id()));
        let _ = fs::remove_dir_all(&install);
        let write = |pending: Value, active: Value, revoke: Value| {
            fs::create_dir_all(dir(&install)).unwrap();
            fs::write(dir(&install).join("connection.json"), json!({"installation": "i", "pending": pending, "active": active, "revoke": revoke}).to_string()).unwrap();
        };
        let conn = |c: &str| json!({"destination": "https://d/machine", "credential": c});
        let mut dash = Dash { online: true, revoked: vec![] };
        assert_eq!(disconnect_with(&install, &mut dash).unwrap(), None, "no state: none");
        // code shown, not approved yet: nothing granted
        write(json!({"destination": "https://d/machine", "device_code": "x", "staged": null}), Value::Null, json!([]));
        assert_eq!(disconnect_with(&install, &mut dash).unwrap().as_deref(), Some("disconnected: pairing cancelled; nothing granted to revoke"));
        assert!(state(&install).unwrap().unwrap()["pending"].is_null(), "enroll cannot resume the pairing");
        assert_eq!(disconnect_with(&install, &mut dash).unwrap(), None, "second disconnect says none");
        // approved and staged, ack not sent, beside an active connection: both credentials revoked
        write(json!({"destination": "https://d/machine", "device_code": "x", "staged": conn("staged")}), conn("active"), json!([]));
        assert_eq!(disconnect_with(&install, &mut dash).unwrap().as_deref(), Some("disconnected: pairing cancelled; sending stopped; revoke confirmed"));
        assert_eq!(dash.revoked, ["staged", "active"]);
        // offline: the revoke stays queued for the service, reported as pending
        write(json!({"destination": "https://d/machine", "device_code": "x", "staged": conn("s2")}), Value::Null, json!([]));
        let mut off = Dash { online: false, revoked: vec![] };
        assert_eq!(disconnect_with(&install, &mut off).unwrap().as_deref(), Some("disconnected: pairing cancelled; 1 revoke pending, the service retries it"));
        let s = state(&install).unwrap().unwrap();
        assert!(s["pending"].is_null() && s["revoke"][0]["credential"] == "s2");
        fs::remove_dir_all(install).unwrap();
    }

    // Chain: damaged connection.json -> connection status, disconnect, service pass. Risk: valid JSON of the wrong
    // shape reading as "connection: none" (R12) or being overwritten, losing a credential DashQ still honours.
    #[test]
    fn malformed_connection_state_reads_unknown_and_is_kept() {
        let install = std::env::temp_dir().join(format!("taskq-malformed-{}", std::process::id()));
        let _ = fs::remove_dir_all(&install);
        fs::create_dir_all(dir(&install)).unwrap();
        let ok = json!({"installation": "i", "pending": null, "active": {"destination": "https://d/machine", "credential": "c"}, "revoke": []});
        let mut bad = vec![json!({}), json!([]), json!(null), json!("x")];
        for (k, v) in [("installation", json!(1)), ("pending", json!("x")), ("pending", json!({"staged": null})),
            ("active", json!({"destination": "https://d/machine"})), ("active", json!([])), ("revoke", json!(null)), ("revoke", json!([1]))] {
            let mut s = ok.clone();
            s[k] = v;
            bad.push(s);
        }
        let path = dir(&install).join("connection.json");
        fs::write(&path, ok.to_string()).unwrap();
        assert!(state(&install).is_ok() && active(&install).unwrap().is_some(), "coreq's own shape reads");
        for b in bad {
            fs::write(&path, b.to_string()).unwrap();
            let unknown = |r: Res<String>| assert!(r.unwrap_err().starts_with("connection: unknown"), "{b}");
            unknown(state(&install).map(|_| String::new()));
            unknown(active(&install).map(|_| String::new()));
            unknown(disconnect_with(&install, &mut Dash { online: true, revoked: vec![] }).map(|_| String::new()));
            let mut s = Sender::default();
            assert!(s.tick(&install, Some(&snap("o/a")), 1).unwrap().contains("unknown"), "{b}");
            assert!(s.job.is_none());
            assert_eq!(fs::read_to_string(&path).unwrap(), b.to_string(), "file kept");
        }
        fs::remove_dir_all(install).unwrap();
    }

    fn snap(repo: &str) -> Snapshot {
        Snapshot { source: "host", observed: 1_700_000_000, reason: None, board: json!({"kind": "github", "host": "github.com", "repo": repo}), tasks: vec![], hosts: vec![] }
    }

    // Chain: enrolled board scope -> service pass of each project -> delivery thread. Risk: another project's board
    // leaking into the grant, or an unreachable DashQ stalling the pass that launches workers.
    #[test]
    fn sender_sends_only_the_approved_board_and_never_blocks() {
        let install = std::env::temp_dir().join(format!("taskq-sender-{}", std::process::id()));
        let _ = fs::remove_dir_all(&install);
        assert_eq!(Sender::default().tick(&install, Some(&snap("o/a")), 1), None, "no connection: nothing sent");
        assert!(!dir(&install).exists(), "a status read creates no connection state");
        let board = BoardScope::new("github", "GitHub.com", "O/A").unwrap();
        assert_eq!(board.value(), "github:github.com/o/a");
        let id = installation_id(&install).unwrap();
        let conn = json!({"destination": "https://127.0.0.1:9/machine", "credential": "c", "generation": 1, "installation": id, "scope": {"name": SCHEMA, "value": board.value()}});
        fs::create_dir_all(dir(&install)).unwrap();
        fs::write(dir(&install).join("connection.json"), json!({"installation": id, "pending": null, "active": conn, "revoke": []}).to_string()).unwrap();
        assert_eq!(approved(&conn), Some(board));
        assert_eq!(approved(&json!({"scope": {"name": SCHEMA, "value": "github:github.com/O/A"}})), None, "non-canonical scope");
        let mut s = Sender::default();
        assert_eq!(s.tick(&install, Some(&snap("o/b")), 1), None, "another board: nothing sent");
        assert!(s.job.is_none());
        let (_, rec) = record(&install, &snap("o/a"), &conn).unwrap();
        assert_eq!((rec["schema"].as_str(), rec["payload"]["board"]["repo"].as_str()), (Some(SCHEMA), Some("o/a")));
        let t = std::time::Instant::now();
        assert_eq!(s.tick(&install, Some(&snap("o/a")), 1), None);
        assert!(s.job.is_some() && t.elapsed().as_secs() < 2, "delivery runs on its own thread");
        while !s.job.as_ref().unwrap().2.is_finished() { std::thread::sleep(std::time::Duration::from_millis(50)) }
        assert_eq!(s.tick(&install, Some(&snap("o/a")), 2).as_deref(), Some("dashq connection: retry in 1 s"), "outage only waits");
        fs::remove_dir_all(install).unwrap();
    }
}
