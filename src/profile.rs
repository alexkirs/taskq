//! §5 profiling: fixed numeric observations in the existing bounded taskq.log, never operation input/error text.
use serde::{Deserialize, Serialize};
use std::cell::Cell;
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, atomic::{AtomicU64, Ordering}};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

pub const OPS: [&str; 6] = ["service.pass", "dispatch", "spawn", "forge.call", "update", "tray.render"];
const PREFIX: &str = "taskq-profile ";
const ERRORS: [&str; 8] = ["none", "http4xx", "http5xx", "timeout", "permission", "invalid", "other", "panic"];
static LOG: OnceLock<PathBuf> = OnceLock::new();
static TRACE: OnceLock<bool> = OnceLock::new();
static RUN: OnceLock<Option<u64>> = OnceLock::new();
static PROJECT: OnceLock<String> = OnceLock::new();
static SPAN: AtomicU64 = AtomicU64::new(1);
thread_local!(static PARENT: Cell<u64> = const { Cell::new(0) });

#[cfg(test)]
thread_local!(static OBSERVED: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) });
#[cfg(test)]
thread_local!(static STAGED: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) });
#[cfg(test)]
thread_local!(static MOVED: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) });
/// Transition records of this thread and their report section.
#[cfg(test)]
pub fn take_moves() -> (String, String) {
    let text = MOVED.with(|v| std::mem::take(&mut *v.borrow_mut()));
    let report = render_system(&text).1;
    (text, report)
}
#[cfg(test)]
pub fn take_report() -> String {
    OBSERVED.with(|v| render(&std::mem::take(&mut *v.borrow_mut())).unwrap())
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    op: String,
    us: u64,
    error: String,
    at_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    trace: Option<Trace>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Trace { run: Option<u64>, process: u32, span: u64, parent: u64 }

pub fn init(install: &Path) {
    let _ = LOG.set(install.join("taskq.log"));
    let _ = TRACE.set(std::env::var_os("TASKQ_PROFILE_TRACE").is_some_and(|v| v == "1"));
}

/// This process's board `owner/name` for stage and transition records (#1174); an unsafe name is kept as `other`.
pub fn project(repo: &str) {
    let ok = repo.len() <= 128 && repo.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_./".contains(&b));
    let _ = PROJECT.set(if ok { repo } else { "other" }.into());
}

fn error(text: &str) -> &'static str {
    if text.contains("HTTP 4") { "http4xx" }
    else if text.contains("HTTP 5") { "http5xx" }
    else if text.contains("timed out") || text.contains("timeout") { "timeout" }
    else if text.contains("denied") || text.contains("refused") { "permission" }
    else if text.contains("invalid") || text.contains("bad JSON") { "invalid" }
    else { "other" }
}

pub fn measure<T, E: std::fmt::Display>(op: &'static str, f: impl FnOnce() -> Result<T, E>) -> Result<T, E> {
    measure_at(LOG.get().map(PathBuf::as_path), TRACE.get().copied().unwrap_or(false), op, f)
}

struct Observation<'a> { path: Option<&'a Path>, began: Instant, record: Record }
impl Drop for Observation<'_> {
    fn drop(&mut self) {
        if let Some(t) = &self.record.trace { PARENT.set(t.parent); }
        self.record.us = self.began.elapsed().as_micros().min(u64::MAX as u128) as u64;
        #[cfg(test)]
        OBSERVED.with(|v| { *v.borrow_mut() += &format!("{PREFIX}{}\n", serde_json::to_string(&self.record).unwrap()); });
        if let Some(path) = self.path {
            if append(path, &self.record).is_err() {
                let _ = writeln!(std::io::stderr().lock(), "Profiling: unknown (taskq.log full or append failed; service pruning required)");
            }
        }
    }
}

fn measure_at<T, E: std::fmt::Display>(path: Option<&Path>, detailed: bool, op: &'static str, f: impl FnOnce() -> Result<T, E>) -> Result<T, E> {
    let trace = detailed.then(|| {
        let span = SPAN.fetch_add(1, Ordering::Relaxed);
        Trace { run: *RUN.get_or_init(|| SystemTime::now().duration_since(UNIX_EPOCH).ok().and_then(|d| d.as_nanos().try_into().ok())),
            process: std::process::id(), span, parent: PARENT.replace(span) }
    });
    let mut observation = Observation { path, began: Instant::now(), record: Record { op: op.into(), us: 0, error: "panic".into(),
        at_ms: now_ms(), trace } };
    let result = f();
    observation.record.error = result.as_ref().err().map_or("none", |e| error(&e.to_string())).into();
    result
}

/// § 4 Profiling attempt stages; the behavioral model (§ 2) names where each starts and ends.
pub const STAGES: [&str; 7] = ["claim", "start", "build", "checks", "review", "ask", "close"];
const STAGE_PREFIX: &str = "taskq-stage ";
const OUTCOMES: [&str; 6] = ["ok", "ask", "retried", "failed", "cancelled", "unknown"];
/// Stage-only error classes: a child ended without hand-in; its issue left the board read or could not be read back.
const STAGE_ERRORS: [&str; 3] = ["exit", "left-board", "unreadable"];
const RUNTIMES: [&str; 4] = ["claude", "codex", "codex-app", "other"];
const UNKNOWN: [&str; 5] = ["no-model", "no-usage", "log-unreadable", "app-turn", "adopted"];

/// Model use of one stage: service stages run no model; a runtime that reports none reads unknown, never zero;
/// `tokens` is `None` when any counted call lacked usage.
pub enum Usage { NoModel, Unknown(&'static str), Known { calls: Option<u64>, tokens: Option<u64> } }

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Stage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    project: Option<String>,
    stage: String,
    task: u64,
    attempt: u64,
    rt: String,
    /// `None`: the stage began in a replaced service image (adopted child), reason in `unknown`
    us: Option<u64>,
    outcome: String,
    error: String,
    at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    calls: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    unknown: Option<String>,
}

fn now_ms() -> Option<u64> {
    SystemTime::now().duration_since(UNIX_EPOCH).ok().map(|d| d.as_millis().min(u64::MAX as u128) as u64)
}

/// The error class of a stage that failed with `text`, never the text itself.
pub fn class(text: &str) -> &'static str { error(text) }

/// One `taskq-stage` line in this process's `taskq.log`; a failed write is reported unknown, never fails the stage.
pub fn stage(name: &'static str, task: u64, attempt: u64, rt: &str, took: Option<std::time::Duration>, outcome: &'static str, err: &'static str, usage: Usage) {
    #[cfg(test)]
    STAGED.with(|v| v.borrow_mut().push(format!("{name} {task} {attempt} {rt} {outcome} {err}")));
    stage_at(LOG.get().map(PathBuf::as_path), name, task, attempt, rt, took, outcome, err, usage)
}

#[allow(clippy::too_many_arguments)]
fn stage_at(path: Option<&Path>, name: &'static str, task: u64, attempt: u64, rt: &str, took: Option<std::time::Duration>, outcome: &'static str, err: &'static str, usage: Usage) {
    let (calls, tokens, unknown) = match usage {
        Usage::NoModel => (None, None, Some("no-model")),
        Usage::Unknown(r) => (None, None, Some(r)),
        Usage::Known { calls, tokens } => (calls, tokens, tokens.is_none().then_some("no-usage")),
    };
    let r = Stage { project: PROJECT.get().cloned(), stage: name.into(), task, attempt, rt: RUNTIMES.into_iter().find(|k| *k == rt).unwrap_or("other").into(),
        us: took.map(|d| d.as_micros().min(u64::MAX as u128) as u64), outcome: outcome.into(), error: err.into(), at_ms: now_ms(), calls, tokens, unknown: unknown.map(Into::into) };
    let Some(path) = path else { return };
    let line = serde_json::to_string(&r).map(|j| format!("{STAGE_PREFIX}{j}\n"));
    if line.map_err(std::io::Error::other).and_then(|l| append_line(path, &l)).is_err() {
        let _ = writeln!(std::io::stderr().lock(), "Profiling: unknown (taskq.log full or append failed; service pruning required)");
    }
}

/// Attempt stages ranked per runtime (§ 4 Profiling); a corrupt record makes the whole section unknown.
fn render_stages(text: &str) -> Result<String, &'static str> {
    let mut rows: Vec<((usize, &str), Vec<Stage>)> = vec![];
    for line in text.lines().filter_map(|l| l.strip_prefix(STAGE_PREFIX)) {
        let r: Stage = serde_json::from_str(line).map_err(|_| "corrupt stage record; log kept")?;
        let i = STAGES.iter().position(|s| *s == r.stage).ok_or("unknown stage; log kept")?;
        let rt = RUNTIMES.into_iter().find(|k| *k == r.rt).ok_or("unknown stage runtime; log kept")?;
        if !OUTCOMES.contains(&r.outcome.as_str()) || !(ERRORS.contains(&r.error.as_str()) || STAGE_ERRORS.contains(&r.error.as_str()))
            || r.unknown.as_deref().is_some_and(|u| !UNKNOWN.contains(&u)) { return Err("unknown stage code; log kept"); }
        match rows.iter_mut().find(|(k, _)| *k == (i, rt)) {
            Some((_, v)) => v.push(r),
            None => rows.push(((i, rt), vec![r])),
        }
    }
    if rows.is_empty() { return Ok("Attempt stages: unknown (no observations)\n".into()); }
    rows.sort_by_key(|(k, _)| *k);
    for (_, v) in &mut rows { v.sort_unstable_by_key(|r| r.us); }
    let share = |v: &[Stage], o: &str| v.iter().filter(|r| r.outcome == o).count() as f64 / v.len() as f64;
    // percentiles over timed records only (sorted, untimed first); untimed rows rank last
    let p = |v: &[Stage], p: usize| { let t: Vec<u64> = v.iter().filter_map(|r| r.us).collect(); (!t.is_empty()).then(|| t[(t.len() * p).div_ceil(100) - 1]) };
    let rank = |key: &dyn Fn(&[Stage]) -> f64| {
        let mut r: Vec<_> = rows.iter().collect();
        r.sort_by(|a, b| key(&b.1).total_cmp(&key(&a.1)).then(a.0.cmp(&b.0)));
        r.iter().map(|((i, rt), _)| format!("{}/{rt}", STAGES[*i])).collect::<Vec<_>>().join(" > ")
    };
    let mut out = format!("Attempt stages: retained observations; all budgets unknown\nSlow stages (p95): {}\nFailing stages (rate): {}\nRetried stages (rate): {}\n\n| Stage | Runtime | Count | p50/p95/max ms | Outcomes | Model calls | Context tokens | Budget |\n|---|---|---:|---|---|---|---|---|\n",
        rank(&|v| p(v, 95).map_or(-1., |u| u as f64)), rank(&|v| share(v, "failed")), rank(&|v| share(v, "retried")));
    for ((i, rt), v) in &rows {
        let mut counts = std::collections::BTreeMap::new();
        for r in v.iter() {
            *counts.entry(if r.error == "none" { r.outcome.clone() } else { format!("{} {}", r.outcome, r.error) }).or_insert(0) += 1;
        }
        let outcomes = counts.iter().map(|(k, n)| format!("{k}:{n}")).collect::<Vec<_>>().join(", ");
        let unknown = |field: fn(&Stage) -> Option<u64>| {
            let known: Vec<u64> = v.iter().filter_map(field).collect();
            let mut reasons: Vec<&str> = v.iter().filter(|r| field(r).is_none()).map(|r| r.unknown.as_deref().unwrap_or("not-reported")).collect();
            reasons.sort_unstable();
            reasons.dedup();
            match known.len() {
                0 if reasons == ["no-model"] => "none (no model)".to_string(),
                0 => format!("unknown ({})", reasons.join(", ")),
                k if k == v.len() => known.iter().sum::<u64>().to_string(),
                k => format!("{} ({k} of {} known; {})", known.iter().sum::<u64>(), v.len(), reasons.join(", ")),
            }
        };
        let ms = |u: u64| u as f64 / 1000.;
        let timed = v.iter().filter(|r| r.us.is_some()).count();
        let took = match (p(v, 50), p(v, 95), v.last().and_then(|r| r.us)) {
            (Some(a), Some(b), Some(c)) if timed == v.len() => format!("{:.3}/{:.3}/{:.3}", ms(a), ms(b), ms(c)),
            (Some(a), Some(b), Some(c)) => format!("{:.3}/{:.3}/{:.3} ({timed} of {} timed; adopted)", ms(a), ms(b), ms(c), v.len()),
            _ => "unknown (adopted)".into(),
        };
        out += &format!("| {} | {rt} | {} | {took} | {outcomes} | {} | {} | unknown |\n", STAGES[*i], v.len(), unknown(|r| r.calls), unknown(|r| r.tokens));
    }
    Ok(out)
}

/// § 4 Profiling board operation kinds, one per board adapter call.
pub const BOARD_OPS: [&str; 9] = ["list", "inbox", "read", "claim", "comment", "label", "close", "write", "publish"];
const BOARD_PREFIX: &str = "taskq-board ";
const TRANSITION_PREFIX: &str = "taskq-transition ";
const STATES: [&str; 9] = ["ready", "waiting", "doing", "review", "ask", "later", "failed", "closed", "held"];

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BoardOp { op: String, us: u64, outcome: String, error: String, remaining: Option<u64>, at_ms: Option<u64> }

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Transition { #[serde(default, skip_serializing_if = "Option::is_none")] project: Option<String>, task: u64, entry: u64, from: String, to: String, prev_s: Option<u64>, rt: Option<String>, host: Option<String>, at_ms: Option<u64> }

/// Appends one record line; a failed write is reported unknown and never fails the observed operation.
fn record(prefix: &str, r: &impl Serialize) {
    let Some(path) = LOG.get() else { return };
    let line = serde_json::to_string(r).map(|j| format!("{prefix}{j}\n"));
    if line.map_err(std::io::Error::other).and_then(|l| append_line(path, &l)).is_err() {
        let _ = writeln!(std::io::stderr().lock(), "Profiling: unknown (taskq.log full or append failed; service pruning required)");
    }
}

/// One board operation of `kind`; `failed` is its error text (only its class is kept).
pub fn board(kind: &'static str, took: std::time::Duration, failed: Option<&str>, remaining: Option<u64>) {
    record(BOARD_PREFIX, &BoardOp { op: kind.into(), us: took.as_micros().min(u64::MAX as u128) as u64,
        outcome: if failed.is_some() { "failed" } else { "ok" }.into(), error: failed.map_or("none", error).into(), remaining, at_ms: now_ms() });
}

/// One queue transition of `task` caused by log `entry`; `rt`/`host` of the attempt on either side.
pub fn transition(task: u64, entry: u64, from: &'static str, to: &'static str, prev_s: Option<u64>, rt: Option<&str>, host: Option<&str>) {
    // a host name is the operator's own short label; anything else is not kept
    let host = host.map(|h| if h.len() <= 64 && h.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b)) { h } else { "other" });
    let r = Transition { project: PROJECT.get().cloned(), task, entry, from: from.into(), to: to.into(), prev_s,
        rt: rt.map(|r| RUNTIMES.into_iter().find(|k| *k == r).unwrap_or("other").into()), host: host.map(Into::into), at_ms: now_ms() };
    #[cfg(test)]
    MOVED.with(|v| { *v.borrow_mut() += &format!("{TRANSITION_PREFIX}{}\n", serde_json::to_string(&r).unwrap()); });
    record(TRANSITION_PREFIX, &r);
}

/// p50/p95/max of sorted `v`, each divided by `div`.
fn spread(v: &[u64], div: f64) -> String {
    let p = |p: usize| v[(v.len() * p).div_ceil(100) - 1] as f64 / div;
    format!("{:.3}/{:.3}/{:.3}", p(50), p(95), *v.last().unwrap() as f64 / div)
}

/// Seconds in the previous state of sorted `s` plus `unknown` moves without an entry time.
fn in_state(s: &[u64], unknown: usize) -> String {
    match (s.is_empty(), unknown) {
        (true, _) => "unknown (no entry time)".into(),
        (false, 0) => spread(s, 1.),
        (false, u) => format!("{} ({} of {} known; no entry time)", spread(s, 1.), s.len(), s.len() + u),
    }
}

/// Board operations by p95 and queue transitions (§ 4 Profiling); a corrupt record makes its section unknown.
fn render_system(text: &str) -> (String, String) {
    let board = || -> Result<String, &'static str> {
        let mut rows: Vec<(&str, Vec<u64>, BTreeMap<String, usize>, Option<u64>)> = vec![];
        for line in text.lines().filter_map(|l| l.strip_prefix(BOARD_PREFIX)) {
            let r: BoardOp = serde_json::from_str(line).map_err(|_| "corrupt board record; log kept")?;
            let op = BOARD_OPS.into_iter().find(|k| *k == r.op).ok_or("unknown board operation; log kept")?;
            if !ERRORS.contains(&r.error.as_str()) || !["ok", "failed"].contains(&r.outcome.as_str()) { return Err("unknown board code; log kept"); }
            let i = rows.iter().position(|x| x.0 == op).unwrap_or_else(|| { rows.push((op, vec![], Default::default(), None)); rows.len() - 1 });
            let row = &mut rows[i];
            row.1.push(r.us);
            if r.outcome == "failed" { *row.2.entry(r.error).or_default() += 1; }
            if let Some(m) = r.remaining { row.3 = Some(row.3.map_or(m, |x| x.min(m))); }
        }
        if rows.is_empty() { return Ok("Board operations: unknown (no observations)\n".into()); }
        for r in &mut rows { r.1.sort_unstable(); }
        rows.sort_by_key(|r| (std::cmp::Reverse(r.1[(r.1.len() * 95).div_ceil(100) - 1]), r.0));
        let mut out = "Board operations: retained observations by p95; all budgets unknown\n\n| Operation | Count | p50/p95/max ms | Failed | Errors | Lowest remaining | Budget |\n|---|---:|---|---:|---|---|---|\n".to_string();
        for (op, us, errors, low) in rows {
            let e = errors.iter().map(|(k, n)| format!("{k}:{n}")).collect::<Vec<_>>().join(", ");
            out += &format!("| {op} | {} | {} | {} | {} | {} | unknown |\n", us.len(), spread(&us, 1000.), errors.values().sum::<usize>(),
                if e.is_empty() { "none" } else { &e }, low.map_or("unknown (not reported)".into(), |l| l.to_string()));
        }
        Ok(out)
    };
    let moves = || -> Result<String, &'static str> {
        let mut rows: BTreeMap<(usize, usize, String), (Vec<u64>, usize)> = BTreeMap::new();
        for line in text.lines().filter_map(|l| l.strip_prefix(TRANSITION_PREFIX)) {
            let r: Transition = serde_json::from_str(line).map_err(|_| "corrupt transition record; log kept")?;
            let state = |s: &str| STATES.iter().position(|k| *k == s).ok_or("unknown transition state; log kept");
            if r.rt.as_deref().is_some_and(|rt| !RUNTIMES.contains(&rt)) { return Err("unknown transition runtime; log kept"); }
            let row = rows.entry((state(&r.from)?, state(&r.to)?, r.rt.unwrap_or_else(|| "none".into()))).or_default();
            match r.prev_s { Some(s) => row.0.push(s), None => row.1 += 1 }
        }
        if rows.is_empty() { return Ok("Queue transitions: unknown (no observations)\n".into()); }
        let mut out = "Queue transitions: retained observations; moves while no service ran unknown\n\n| From | To | Runtime | Count | p50/p95/max s in previous state |\n|---|---|---|---:|---|\n".to_string();
        for ((f, t, rt), (mut s, unknown)) in rows {
            s.sort_unstable();
            out += &format!("| {} | {} | {rt} | {} | {} |\n", STATES[f], STATES[t], s.len() + unknown, in_state(&s, unknown));
        }
        Ok(out)
    };
    (board().unwrap_or_else(|e| format!("Board operations: unknown ({e})\n")), moves().unwrap_or_else(|e| format!("Queue transitions: unknown ({e})\n")))
}

pub struct LogLock(fs::File);
impl Drop for LogLock {
    fn drop(&mut self) {
        // A child can inherit the descriptor on Unix; close alone can leave its lock held.
        let _ = self.0.unlock();
    }
}

pub fn log_lock(path: &Path) -> std::io::Result<LogLock> {
    let lock = OpenOptions::new().create(true).truncate(false).read(true).write(true).open(path.parent().unwrap().join("taskq-profile.lock"))?;
    lock.try_lock().map_err(std::io::Error::other)?;
    Ok(LogLock(lock))
}

fn append(path: &Path, r: &Record) -> std::io::Result<()> {
    append_line(path, &format!("{PREFIX}{}\n", serde_json::to_string(r)?))
}

/// One record line under the profile lock and retention bound; other record kinds use their own prefix (§ 4 budget).
pub fn append_line(path: &Path, line: &str) -> std::io::Result<()> {
    let full = || fs::metadata(path).is_ok_and(|m| m.len() > crate::data::LOG_MAX - 512);
    // A full log is refused before the lock too: appenders then never take the lock from the pruner's nonblocking try.
    if full() {
        return Err(std::io::Error::other("log full"));
    }
    fs::create_dir_all(path.parent().unwrap())?;
    let _lock = log_lock(path)?;
    if full() {
        return Err(std::io::Error::other("log full"));
    }
    OpenOptions::new().create(true).append(true).open(path)?.write_all(line.as_bytes())
}

/// `<install>/taskq.log` of this process, once `init` ran.
pub fn log_path() -> Option<&'static Path> {
    LOG.get().map(PathBuf::as_path)
}

struct Stats { op: &'static str, times: Vec<u64>, errors: std::collections::BTreeMap<String, usize> }
impl Stats {
    fn percentile(&self, p: usize) -> u64 { self.times[(self.times.len() * p).div_ceil(100) - 1] }
    fn failures(&self) -> usize { self.errors.values().sum() }
    fn rate(&self) -> f64 { self.failures() as f64 / self.times.len() as f64 }
    fn variance(&self) -> f64 {
        let mean = self.times.iter().map(|t| *t as f64).sum::<f64>() / self.times.len() as f64;
        self.times.iter().map(|t| (*t as f64 - mean).powi(2)).sum::<f64>() / self.times.len() as f64
    }
}

fn render(text: &str) -> Result<String, &'static str> {
    let mut stats: Vec<_> = OPS.into_iter().map(|op| Stats { op, times: vec![], errors: Default::default() }).collect();
    let (mut first, mut last, mut traces) = (None::<u64>, None::<u64>, 0);
    for line in text.lines().filter_map(|l| l.strip_prefix(PREFIX)) {
        let r: Record = serde_json::from_str(line).map_err(|_| "corrupt profile record; log kept")?;
        let s = stats.iter_mut().find(|s| s.op == r.op).ok_or("unknown profile operation; log kept")?;
        if !ERRORS.contains(&r.error.as_str()) { return Err("unknown profile error code; log kept"); }
        if let Some(at) = r.at_ms {
            first = Some(first.map_or(at, |f| f.min(at)));
            last = Some(last.map_or(at, |l| l.max(at)));
        }
        traces += usize::from(r.trace.is_some());
        s.times.push(r.us);
        if r.error != "none" { *s.errors.entry(r.error).or_default() += 1; }
    }
    for s in &mut stats { s.times.sort_unstable(); }
    let rank = |key: fn(&Stats) -> f64| {
        let mut rows: Vec<_> = stats.iter().filter(|s| !s.times.is_empty()).collect();
        rows.sort_by(|a, b| key(b).total_cmp(&key(a)).then(a.op.cmp(b.op)));
        rows.iter().map(|s| s.op).collect::<Vec<_>>().join(" > ")
    };
    let ranking = |s: String| if s.is_empty() { "unknown (no observations)".into() } else { s };
    let window = first.zip(last).map_or("unknown (no timestamps)".into(), |(f, l)| format!("{f}..{l} Unix ms; earlier/pruned history unknown"));
    let mut out = format!("Profile: retained observations; all budgets unknown\nWindow: {window}\nCollection completeness: unknown (best-effort writes; check CLI/service diagnostics)\nDetailed traces: {traces} retained; switch {} for this run\nRetries/cancellations: unknown (not exposed by adapters)\nSlow (p95): {}\nFailing (rate): {}\nUnstable (variance): {}\n\n| Operation | Count | p50/p95/max ms | Success/failure | Failure % | Variance ms² | Errors | Budget |\n|---|---:|---|---|---:|---:|---|---|\n",
        if TRACE.get().copied().unwrap_or(false) { "on" } else { "off" },
        ranking(rank(|s| s.percentile(95) as f64)), ranking(rank(Stats::rate)), ranking(rank(Stats::variance)));
    for s in stats {
        if s.times.is_empty() {
            out += &format!("| {} | unknown (no observations) | unknown | unknown | unknown | unknown | unknown | unknown |\n", s.op);
        } else {
            let errors = s.errors.iter().map(|(k, n)| format!("{k}:{n}")).collect::<Vec<_>>().join(", ");
            out += &format!("| {} | {} | {:.3}/{:.3}/{:.3} | {}/{} | {:.2} | {:.6} | {} | unknown |\n", s.op, s.times.len(),
                s.percentile(50) as f64 / 1000., s.percentile(95) as f64 / 1000., *s.times.last().unwrap() as f64 / 1000.,
                s.times.len() - s.failures(), s.failures(), s.rate() * 100., s.variance() / 1_000_000., if errors.is_empty() { "none" } else { &errors });
        }
    }
    Ok(out)
}

/// The retained `<install>/taskq.log`; a missing log is empty.
fn read_log(install: &Path) -> Result<String, String> {
    let mut text = String::new();
    match fs::File::open(install.join("taskq.log")) {
        Ok(f) => { f.take(crate::data::LOG_MAX + 1).read_to_string(&mut text).map_err(|_| "taskq.log unreadable or invalid UTF-8")?; }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err("taskq.log unreadable".into()),
    }
    if text.len() as u64 > crate::data::LOG_MAX - 512 { return Err("taskq.log at retention bound; collection stopped until service pruning".into()); }
    Ok(text)
}

pub fn report(install: &Path) -> String {
    let read = || -> Result<String, String> {
        let text = read_log(install)?;
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis().min(u64::MAX as u128) as u64);
        let profile = render(&text).unwrap_or_else(|e| format!("Profile: unknown ({e}); all budgets unknown\n"));
        let stages = render_stages(&text).unwrap_or_else(|e| format!("Attempt stages: unknown ({e})\n"));
        let (board, moves) = render_system(&text);
        Ok(format!("{profile}\n{board}\n{moves}\n{stages}\n{}", crate::requests::sources(&text, now)))
    };
    read().unwrap_or_else(|e| format!("Profile: unknown ({e}); all budgets unknown\n\nBoard operations: unknown ({e})\n\nQueue transitions: unknown ({e})\n\nAttempt stages: unknown ({e})\n\nBoard requests by source: unknown ({e})\n"))
}

/// § 5 `taskq profile export` columns, in order; every row has each of them, null where its kind has none.
const COLUMNS: [&str; 19] = ["kind", "at_ms", "project", "task", "attempt", "entry", "name", "from", "to", "runtime", "host", "us", "prev_s",
    "outcome", "error", "remaining", "calls", "tokens", "unknown"];

#[derive(Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Row { kind: String, at_ms: Option<u64>, project: Option<String>, task: Option<u64>, attempt: Option<u64>, entry: Option<u64>,
    name: Option<String>, from: Option<String>, to: Option<String>, runtime: Option<String>, host: Option<String>, us: Option<u64>,
    prev_s: Option<u64>, outcome: Option<String>, error: Option<String>, remaining: Option<u64>, calls: Option<u64>, tokens: Option<u64>, unknown: Option<String> }

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Header { schema: u64, columns: Vec<String>, since_ms: Option<u64>, window_ms: Option<[u64; 2]>, samples: usize, corrupt: usize }

/// The export row of one log line: `None` for other lines, `Some(None)` for a corrupt record or an unknown code.
fn row(line: &str) -> Option<Option<Row>> {
    let s = |v: &str| Some(v.to_string());
    Some(if let Some(j) = line.strip_prefix(PREFIX) {
        serde_json::from_str::<Record>(j).ok().filter(|r| OPS.contains(&r.op.as_str()) && ERRORS.contains(&r.error.as_str())).map(|r| Row {
            kind: "operation".into(), at_ms: r.at_ms, name: s(&r.op), us: Some(r.us), outcome: s(if r.error == "none" { "ok" } else { "failed" }), error: s(&r.error), ..Default::default() })
    } else if let Some(j) = line.strip_prefix(STAGE_PREFIX) {
        serde_json::from_str::<Stage>(j).ok().filter(|r| STAGES.contains(&r.stage.as_str()) && RUNTIMES.contains(&r.rt.as_str()) && OUTCOMES.contains(&r.outcome.as_str())
            && (ERRORS.contains(&r.error.as_str()) || STAGE_ERRORS.contains(&r.error.as_str())) && r.unknown.as_deref().is_none_or(|u| UNKNOWN.contains(&u))).map(|r| Row {
            kind: "stage".into(), at_ms: r.at_ms, project: r.project, task: Some(r.task), attempt: Some(r.attempt), name: Some(r.stage), runtime: Some(r.rt),
            us: r.us, outcome: Some(r.outcome), error: Some(r.error), calls: r.calls, tokens: r.tokens, unknown: r.unknown, ..Default::default() })
    } else if let Some(j) = line.strip_prefix(BOARD_PREFIX) {
        serde_json::from_str::<BoardOp>(j).ok().filter(|r| BOARD_OPS.contains(&r.op.as_str()) && ERRORS.contains(&r.error.as_str()) && ["ok", "failed"].contains(&r.outcome.as_str())).map(|r| Row {
            kind: "board".into(), at_ms: r.at_ms, name: Some(r.op), us: Some(r.us), outcome: Some(r.outcome), error: Some(r.error), remaining: r.remaining, ..Default::default() })
    } else if let Some(j) = line.strip_prefix(TRANSITION_PREFIX) {
        serde_json::from_str::<Transition>(j).ok().filter(|r| STATES.contains(&r.from.as_str()) && STATES.contains(&r.to.as_str()) && r.rt.as_deref().is_none_or(|rt| RUNTIMES.contains(&rt))).map(|r| Row {
            kind: "transition".into(), at_ms: r.at_ms, project: r.project, task: Some(r.task), entry: Some(r.entry), from: Some(r.from), to: Some(r.to),
            runtime: r.rt, host: r.host, prev_s: r.prev_s, ..Default::default() })
    } else {
        return None;
    })
}

/// § 5 `taskq profile export`: a header naming window and sample count, then one JSON line per retained observation at or after `since_ms`.
fn export(text: &str, since_ms: Option<u64>) -> String {
    let (mut rows, mut corrupt) = (vec![], 0);
    for r in text.lines().filter_map(row) {
        // ponytail: an untimed record falls outside every `--since` window
        match r { Some(r) if since_ms.is_none_or(|s| r.at_ms.is_some_and(|a| a >= s)) => rows.push(r), Some(_) => {}, None => corrupt += 1 }
    }
    let at = || rows.iter().filter_map(|r| r.at_ms);
    let window_ms = at().min().zip(at().max()).map(|(a, b)| [a, b]);
    let h = Header { schema: 1, columns: COLUMNS.map(String::from).to_vec(), since_ms, window_ms, samples: rows.len(), corrupt };
    let mut out = serde_json::to_string(&h).unwrap() + "\n";
    for r in rows { out += &(serde_json::to_string(&r).unwrap() + "\n"); }
    out
}

fn read_export(export: &str) -> Result<(Header, Vec<Row>), &'static str> {
    let mut lines = export.lines();
    let h: Header = lines.next().and_then(|l| serde_json::from_str(l).ok()).filter(|h: &Header| h.schema == 1 && h.columns == COLUMNS).ok_or("export header unreadable")?;
    let rows = lines.map(|l| serde_json::from_str(l).map_err(|_| "export row unreadable")).collect::<Result<Vec<Row>, _>>()?;
    Ok((h, rows))
}

fn window(h: &Header) -> String {
    let since = h.since_ms.map_or("all retained".into(), |s| format!("since {s}"));
    let w = h.window_ms.map_or("unknown (no timestamps)".into(), |[a, b]| format!("{a}..{b} Unix ms"));
    format!("Window: {w} ({since}); {} samples, {} corrupt skipped; earlier/pruned history unknown\n", h.samples, h.corrupt)
}

/// § 5 `taskq profile tasks`: time in state per state, runtime and host, then the tasks with the most time per state.
fn render_tasks(export: &str) -> Result<String, &'static str> {
    let (h, rows) = read_export(export)?;
    let none = |v: &Option<String>| v.clone().unwrap_or_else(|| "none".into());
    let mut states: BTreeMap<(String, String, String), (Vec<u64>, usize)> = BTreeMap::new();
    let mut tasks: BTreeMap<(String, u64, String, String, String), (Vec<u64>, usize)> = BTreeMap::new();
    for r in rows.iter().filter(|r| r.kind == "transition") {
        let (from, rt, host) = (none(&r.from), none(&r.runtime), none(&r.host));
        let project = r.project.clone().unwrap_or_else(|| "unknown (not recorded)".into());
        for g in [states.entry((from.clone(), rt.clone(), host.clone())).or_default(), tasks.entry((project, r.task.unwrap_or(0), from, rt, host)).or_default()] {
            match r.prev_s { Some(s) => g.0.push(s), None => g.1 += 1 }
        }
    }
    let mut out = format!("Time in state: retained queue transitions; moves while no service ran unknown\n{}", window(&h));
    if states.is_empty() { return Ok(out + "States by time: unknown (no transitions)\n"); }
    // unknown-only groups rank after every known one, never as zero
    let p95 = |s: &[u64]| (s.is_empty(), std::cmp::Reverse(s.get((s.len() * 95).div_ceil(100).max(1) - 1).copied()));
    let mut states: Vec<_> = states.into_iter().map(|(k, (mut s, u))| { s.sort_unstable(); (k, s, u) }).collect();
    states.sort_by(|a, b| p95(&a.1).cmp(&p95(&b.1)).then(a.0.cmp(&b.0)));
    out += "\nStates by p95\n\n| State | Runtime | Host | Count | p50/p95/max s |\n|---|---|---|---:|---|\n";
    for ((state, rt, host), s, u) in &states { out += &format!("| {state} | {rt} | {host} | {} | {} |\n", s.len() + u, in_state(s, *u)); }
    let mut tasks: Vec<_> = tasks.into_iter().map(|(k, (mut s, u))| { s.sort_unstable(); (k, s, u) }).collect();
    let sum = |s: &[u64]| (s.is_empty(), std::cmp::Reverse(s.iter().sum::<u64>()));
    tasks.sort_by(|a, b| sum(&a.1).cmp(&sum(&b.1)).then(a.0.cmp(&b.0)));
    out += "\nTasks by total time in state\n\n| Project | Task | State | Runtime | Host | Count | Total s | p50/p95/max s |\n|---|---|---|---|---|---:|---:|---|\n";
    for ((project, task, state, rt, host), s, u) in tasks.iter().take(20) {
        let total = match (s.is_empty(), *u) {
            (true, _) => "unknown (no entry time)".into(),
            (false, 0) => s.iter().sum::<u64>().to_string(),
            (false, u) => format!("{} partial ({} of {} known)", s.iter().sum::<u64>(), s.len(), s.len() + u),
        };
        out += &format!("| {project} | #{task} | {state} | {rt} | {host} | {} | {total} | {} |\n", s.len() + u, in_state(s, *u));
    }
    if tasks.len() > 20 { out += &format!("{} more task states\n", tasks.len() - 20); }
    Ok(out)
}

/// § 5 `taskq profile board`: board operations ranked by p95, failure rate and lowest rate-limit `remaining`.
fn render_board(export: &str) -> Result<String, &'static str> {
    let (h, rows) = read_export(export)?;
    let mut ops: BTreeMap<String, (Vec<u64>, BTreeMap<String, usize>, Option<u64>)> = BTreeMap::new();
    for r in rows.iter().filter(|r| r.kind == "board") {
        let o = ops.entry(r.name.clone().unwrap_or_default()).or_default();
        o.0.push(r.us.ok_or("export board row without duration")?);
        if r.outcome.as_deref() == Some("failed") { *o.1.entry(r.error.clone().unwrap_or_default()).or_default() += 1; }
        if let Some(m) = r.remaining { o.2 = Some(o.2.map_or(m, |x| x.min(m))); }
    }
    let mut out = format!("Board operations: retained observations; all budgets unknown\n{}", window(&h));
    if ops.is_empty() { return Ok(out + "Board operations: unknown (no observations)\n"); }
    for o in ops.values_mut() { o.0.sort_unstable(); }
    let failed = |o: &(Vec<u64>, BTreeMap<String, usize>, Option<u64>)| o.1.values().sum::<usize>() as f64 / o.0.len() as f64;
    let rank = |key: &dyn Fn(&(Vec<u64>, BTreeMap<String, usize>, Option<u64>)) -> Option<f64>| {
        let mut r: Vec<_> = ops.iter().filter_map(|(k, o)| key(o).map(|v| (k, v))).collect();
        r.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(b.0)));
        if r.is_empty() { "unknown (not reported)".into() } else { r.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>().join(" > ") }
    };
    out += &format!("Slow (p95): {}\nFailing (rate): {}\nRate-limit use (lowest remaining): {}\n\n| Operation | Count | p50/p95/max ms | Failed % | Errors | Lowest remaining |\n|---|---:|---|---:|---|---|\n",
        rank(&|o| Some(o.0[(o.0.len() * 95).div_ceil(100) - 1] as f64)), rank(&|o| Some(failed(o))), rank(&|o| o.2.map(|m| -(m as f64))));
    for (op, o) in &ops {
        let e = o.1.iter().map(|(k, n)| format!("{k}:{n}")).collect::<Vec<_>>().join(", ");
        out += &format!("| {op} | {} | {} | {:.2} | {} | {} |\n", o.0.len(), spread(&o.0, 1000.), failed(o) * 100., if e.is_empty() { "none" } else { &e },
            o.2.map_or("unknown (not reported)".into(), |m| m.to_string()));
    }
    Ok(out)
}

/// `--since 30m|24h|7d` as Unix ms before `now`.
fn since_ms(s: &str, now: u64) -> Result<u64, String> {
    let bad = || format!("--since {s}: use a number with m, h or d, e.g. 24h");
    let (n, unit) = s.split_at(s.len().saturating_sub(1));
    let mins: u64 = match unit { "m" => 1, "h" => 60, "d" => 1440, _ => return Err(bad()) };
    Ok(now.saturating_sub(n.parse::<u64>().map_err(|_| bad())?.saturating_mul(mins * 60_000)))
}

/// § 5 `taskq profile export|tasks|board [--since D]` over this host's retained log; no board access.
pub fn view(install: &Path, name: &str, since: Option<&str>) -> Result<String, String> {
    let now = now_ms().ok_or("system clock before 1970")?;
    let since = since.map(|s| since_ms(s, now)).transpose()?;
    let text = export(&read_log(install)?, since);
    match name {
        "export" => Ok(text),
        "tasks" => render_tasks(&text).map_err(String::from),
        "board" => render_board(&text).map_err(String::from),
        _ => Err(format!("unknown profile view: {name} (export, tasks or board)")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // Chain: A2/A3 operation -> bounded log -> ranked report. Risk: lost failures, wrong ranks/quantiles,
    // unsafe input persisted, corrupt/missing data called healthy, or telemetry outgrows existing retention.
    #[test]
    fn retained_profile_report() {
        let dir = std::env::temp_dir().join(format!("taskq-profile-{}", std::process::id()));
        // A failed run leaves its directory; a reused process id must not inherit that log.
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("taskq.log");
        assert!(report(&dir).contains("unknown (no observations)"));
        fs::write(&path, "normal taskq diagnostic\n").unwrap();
        for (op, times, err) in [("spawn", vec![1000, 3000], "none"), ("forge.call", vec![2000, 2000], "http5xx")] {
            for us in times { append(&path, &Record { op: op.into(), us, error: err.into(), at_ms: Some(us), trace: None }).unwrap(); }
        }
        // board operations (#1173): ranked by p95; failures keep only their class; no remaining reads unknown
        for (op, us, err, remaining) in [("list", 4000, None, Some(4900)), ("list", 1000, Some("HTTP 502 secret /host/path"), Some(4800)), ("claim", 2000, None, None)] {
            append_line(&path, &format!("{BOARD_PREFIX}{}\n", serde_json::to_string(&BoardOp { op: op.into(), us, outcome: if err.is_some() { "failed" } else { "ok" }.into(),
                error: err.map_or("none", error).into(), remaining, at_ms: Some(1) }).unwrap())).unwrap();
        }
        let text = report(&dir);
        assert!(text.contains("| list | 2 | 1.000/4.000/4.000 | 1 | http5xx:1 | 4800 | unknown |\n| claim | 1 | 2.000/2.000/2.000 | 0 | none | unknown (not reported) |"), "{text}");
        assert!(text.contains("Slow (p95): spawn > forge.call"));
        assert!(text.contains("Failing (rate): forge.call > spawn"));
        assert!(text.contains("Unstable (variance): spawn > forge.call"));
        assert!(text.contains("1.000/3.000/3.000 | 2/0 | 0.00 | 1.000000"));
        assert!(text.contains("http5xx:2"));
        assert!(text.contains("1000..3000 Unix ms") && text.contains("switch off for this run"));
        assert_eq!(error("HTTP 500 private token /host/path"), "http5xx");
        assert_eq!(measure_at(Some(&path), true, "spawn", || Err::<(), _>("private token /host/path")), Err("private token /host/path"));
        let log = fs::read_to_string(&path).unwrap();
        assert!(!log.contains("private") && !log.contains("/host/path"));
        let r: Record = serde_json::from_str(log.lines().last().unwrap().strip_prefix(PREFIX).unwrap()).unwrap();
        assert_eq!(r.error, "other");
        assert_eq!(r.trace.unwrap().parent, 0);
        measure_at(Some(&path), true, "dispatch", || measure_at(Some(&path), true, "spawn", || Ok::<_, String>(()))).unwrap();
        let log = fs::read_to_string(&path).unwrap();
        let r: Vec<Record> = log.lines().filter_map(|l| l.strip_prefix(PREFIX)).map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(r[r.len()-2].trace.as_ref().unwrap().parent, r.last().unwrap().trace.as_ref().unwrap().span);
        assert!(std::panic::catch_unwind(|| measure_at(Some(&path), true, "spawn", || -> Result<(), String> { panic!("private panic") })).is_err());
        assert!(report(&dir).contains("panic:1"));
        assert_eq!(PARENT.get(), 0);
        assert_eq!(measure_at(Some(&path.join("unwritable")), false, "spawn", || Ok::<_, String>(7)).unwrap(), 7);
        for corrupt in ["{", "{\"op\":\"spawn\",\"us\":2,\"error\":\"secret\"}"] {
            let bad = format!("{PREFIX}{corrupt}\n");
            fs::write(&path, &bad).unwrap();
            assert!(report(&dir).starts_with("Profile: unknown"));
            assert_eq!(fs::read_to_string(&path).unwrap(), bad);
        }
        fs::write(&path, "normal log\n".repeat((crate::data::LOG_MAX as usize - 256) / 11)).unwrap();
        assert!(fs::metadata(&path).unwrap().len() <= crate::data::LOG_MAX);
        assert!(report(&dir).contains("collection stopped"));
        assert!(crate::data::trim_tail(&path, crate::data::LOG_MAX));
        append(&path, &Record { op: "spawn".into(), us: 1, error: "none".into(), at_ms: Some(1), trace: None }).unwrap();
        assert!(fs::metadata(&path).unwrap().len() <= crate::data::LOG_MAX);
        assert!(report(&dir).contains("| spawn | 1 |"));
        let held = log_lock(&path).unwrap();
        assert!(log_lock(&path).is_err());
        assert!(!crate::data::trim_tail(&path, crate::data::LOG_MAX));
        assert_eq!(measure_at(Some(&path), false, "spawn", || Ok::<_, String>(7)).unwrap(), 7);
        drop(held);
        // Force real truncation while concurrent appenders use the same nonblocking lock.
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all("normal log\n".repeat(crate::data::LOG_MAX as usize / 11).as_bytes()).unwrap();
        let successful = std::sync::atomic::AtomicUsize::new(0);
        std::thread::scope(|scope| {
            for _ in 0..4 { scope.spawn(|| {
                for _ in 0..50 {
                    if append(&path, &Record { op: "spawn".into(), us: 1, error: "none".into(), at_ms: Some(1), trace: None }).is_ok() {
                        successful.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }); }
            // Rule (§ 4 Profiling, R12): one prune wins the nonblocking lock while appends race, so the pre-filled
            // record is truncated and every retained record is a counted successful append. 50 losses happened in CI.
            scope.spawn(|| {
                let deadline = Instant::now() + std::time::Duration::from_secs(10);
                while !crate::data::trim_tail(&path, crate::data::LOG_MAX) {
                    assert!(Instant::now() < deadline, "no prune won the profile log lock");
                    std::thread::yield_now();
                }
                for _ in 0..49 { crate::data::trim_tail(&path, crate::data::LOG_MAX); }
            });
        });
        let records = fs::read_to_string(&path).unwrap().lines().filter(|l| l.starts_with(PREFIX)).count();
        assert_eq!(records, successful.load(Ordering::Relaxed));
        assert!(!report(&dir).starts_with("Profile: unknown"));
        fs::remove_dir_all(dir).unwrap();
    }

    // Chain: #1147 attempt stage -> runtime usage from the attempt log -> bounded log -> per-runtime report.
    // Risk: tokens double-counted or invented, failure text or paths persisted, unknown usage shown as zero.
    #[test]
    fn attempt_stage_trace() {
        let dir = std::env::temp_dir().join(format!("taskq-stage-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let (path, attempt) = (dir.join("taskq.log"), dir.join("attempt.log"));
        assert!(report(&dir).contains("Attempt stages: unknown (no observations)"));
        let claude = r#"{"type":"assistant","message":{"id":"m1","usage":{"input_tokens":10,"cache_read_input_tokens":100,"cache_creation_input_tokens":5}}}"#;
        let codex = r#"{"type":"turn.completed","usage":{"input_tokens":700,"cached_input_tokens":600,"output_tokens":9}}"#;
        // one Claude message spans several lines with one id; a second id is a second call; lines before launch are skipped
        let before = format!("{}\n", claude.replace("m1", "m0"));
        fs::write(&attempt, format!("{before}{claude}\n{claude}\n{}\n", claude.replace("m1", "m2"))).unwrap();
        let ms = |m| Some(std::time::Duration::from_millis(m));
        stage_at(Some(&path), "build", 7, 1, "claude", ms(9), "ok", "none", crate::runtime::usage(&attempt, before.len() as u64, "claude"));
        fs::write(&attempt, format!("{codex}\n{codex}\n")).unwrap();
        stage_at(Some(&path), "build", 8, 2, "codex", ms(5), "ask", "none", crate::runtime::usage(&attempt, 0, "codex"));
        stage_at(Some(&path), "build", 9, 3, "codex", ms(1), "failed", "exit", crate::runtime::usage(&dir.join("missing"), 0, "codex"));
        stage_at(Some(&path), "review", 7, 4, "codex-app", ms(2), "ok", "none", crate::runtime::usage(&attempt, 0, "codex-app"));
        stage_at(Some(&path), "claim", 7, 1, "private-rt", ms(3), "failed", class("HTTP 502 secret /host/path"), Usage::NoModel);
        // a Claude call with empty usage keeps the call count and makes tokens unknown, never 0; an adopted child is untimed
        fs::write(&attempt, format!("{}\n", claude.replace(r#""usage":{"input_tokens":10,"cache_read_input_tokens":100,"cache_creation_input_tokens":5}"#, r#""usage":{}"#))).unwrap();
        stage_at(Some(&path), "review", 7, 5, "claude", ms(4), "ok", "none", crate::runtime::usage(&attempt, 0, "claude"));
        stage_at(Some(&path), "review", 7, 6, "claude", None, "ok", "none", Usage::Unknown("adopted"));
        // a child stopped after its result or requeue keeps that outcome; a live child exiting without one failed;
        // a stop without hand-in (owner requeue) is a cancellation; a hand-in `fold` ignored (invalid SHA, a repeated
        // op id, a result in `review`) is none
        let e = |id, body: &str| crate::core::Entry { id, body: body.into(), edited: false };
        let spawn = e(10, "taskq: spawn worker claude:s1 mac");
        let sha = "a".repeat(40);
        let hand = |kind: &str, arg: &str| vec![spawn.clone(), e(11, &format!("taskq: {kind} {arg}attempt 10
Changed: x"))];
        let (result, requeue, owner) = (hand("result", &format!("{sha} ")), hand("requeue", ""), vec![spawn.clone(), e(11, "taskq: requeue
Reason: owner")]);
        let invalid = hand("result", "abc ");
        let op = |id, body: &str| e(id, &format!("{body}
op-id: x"));
        let repeated = vec![spawn.clone(), op(11, "taskq: ask attempt 10
Done: x"), e(12, "taskq: answer 1"), op(13, &format!("taskq: result {sha} attempt 10
Changed: x"))];
        let review = vec![e(9, "taskq: spawn worker claude:s0 mac"), e(10, &format!("taskq: result {sha} attempt 9
Changed: x")),
            e(11, "taskq: spawn reviewer codex:- win"), e(12, &format!("taskq: result {sha} attempt 11
Changed: x"))];
        assert_eq!(crate::core::stage_outcome(7, Ok((review.as_slice(), Some(true))), 11, 11), ("failed", "exit"));
        // an issue that left the board read is read back: the reviewer's accepted close is ok; a label removal or an
        // owner close (no reviewer hand-in) and an unreadable read-back are unknown with reason, never ok
        let closed = |by: &str| [&review[..3], &[e(13, &format!("taskq: close {by}\nlooks fine"))]].concat();
        let (own, other, removed) = (closed("attempt 11"), closed(""), review[..3].to_vec());
        for (log, want) in [(&own, ("ok", "none")), (&other, ("unknown", "left-board")), (&removed, ("unknown", "left-board"))] {
            assert_eq!(crate::core::stage_outcome(7, Ok((log.as_slice(), None)), 11, 11), want, "{:?}", log.last().map(|e| &e.body));
        }
        assert_eq!(crate::core::stage_outcome(7, Err("unreadable"), 11, 11), ("unknown", "unreadable"));
        for (log, live, want) in [(&result, false, ("ok", "none")), (&requeue, false, ("retried", "none")), (&owner, false, ("cancelled", "none")),
            (&owner, true, ("failed", "exit")), (&invalid, true, ("failed", "exit")), (&repeated, true, ("ask", "none"))] {
            assert_eq!(crate::core::stage_outcome(7, Ok((log.as_slice(), Some(live))), 10, 10), want, "{:?}", log.last().map(|e| &e.body));
        }
        stage_at(Some(&path), "review", 7, 11, "codex", ms(6), "unknown", "left-board", Usage::Unknown("log-unreadable"));
        // `close` times the whole command from entry: an unreadable task, a gate refusal, a stale revision, a CLI verdict refusal, a non-publication close
        // and a publication each record one `close` stage (the publication also `checks`)
        use crate::board::Board;
        let staged = || STAGED.with(|v| std::mem::take(&mut *v.borrow_mut())).join("; ");
        let (b, c) = (crate::board::Fake::default(), crate::tests::cfg());
        let gated = crate::Config { delivery: "review-pr".into(), ..crate::tests::cfg() };
        let t = b.create("T", "## Goal\ng", &["taskq".into()]).unwrap();
        let close = |c: &crate::Config, op: &crate::dispatch::Op| crate::dispatch::close_one(&b, c, t, "ok", op).ok();
        assert!(!close(&c, &Default::default()));
        b.comment(t, &crate::core::entry("spawn", &["worker", "claude:u", "mac"], None, "")).unwrap();
        let a = crate::core::fold(t, &crate::board::entries(&b, t).unwrap(), false).attempt.unwrap();
        b.comment(t, &crate::core::entry("result", &[&sha], Some(a), "")).unwrap();
        b.comment(t, &crate::core::entry("spawn", &["reviewer", "codex:-", "mac"], None, "")).unwrap();
        let a = crate::core::fold(t, &crate::board::entries(&b, t).unwrap(), false).attempt.unwrap();
        assert!(!close(&gated, &Default::default()));
        assert!(!close(&c, &crate::dispatch::Op { revision: Some(1), ..Default::default() }));
        // the CLI's verdict refusal and unreadable task are inside the stage too
        let cli = |n, text: &str| crate::dispatch::close_checked(&b, &c, n, text, &Default::default(), &|i, t| crate::close_check(text, i, t)).ok();
        assert!(!cli(t, "no verdict"));
        assert!(!cli(999, ""));
        assert!(close(&c, &Default::default()));
        assert!(!crate::dispatch::close_one(&b, &c, 999, "ok", &Default::default()).ok());
        assert_eq!(staged(), format!("close {t} 0 other failed other; close {t} {a} codex failed other; close {t} {a} codex failed other; \
            close {t} {a} codex failed other; close 999 0 other failed other; \
            checks {t} {a} codex ok none; close {t} {a} codex ok none; close 999 0 other failed other"));
        let log = fs::read_to_string(&path).unwrap();
        assert!(!log.contains("secret") && !log.contains("/host/path") && !log.contains("private-rt"));
        let text = report(&dir);
        assert!(text.contains("Slow stages (p95): build/claude > review/codex > build/codex > review/claude > claim/other > review/codex-app"), "{text}");
        assert!(text.contains("Failing stages (rate): claim/other > build/codex"), "{text}");
        assert!(text.contains("| build | claude | 1 | 9.000/9.000/9.000 | ok:1 | 2 | 230 | unknown |"), "{text}");
        assert!(text.contains("| build | codex | 2 | 1.000/5.000/5.000 | ask:1, failed exit:1 | unknown (log-unreadable, not-reported) | 1400 (1 of 2 known; log-unreadable) |"), "{text}");
        assert!(text.contains("| review | codex-app | 1 | 2.000/2.000/2.000 | ok:1 | unknown (app-turn) | unknown (app-turn) |"), "{text}");
        assert!(text.contains("| review | claude | 2 | 4.000/4.000/4.000 (1 of 2 timed; adopted) | ok:2 | 1 (1 of 2 known; adopted) | unknown (adopted, no-usage) |"), "{text}");
        assert!(text.contains("| claim | other | 1 | 3.000/3.000/3.000 | failed http5xx:1 | none (no model) | none (no model) |"), "{text}");
        assert!(text.contains("| review | codex | 1 | 6.000/6.000/6.000 | unknown left-board:1 |"), "{text}");
        fs::write(&path, format!("{log}{STAGE_PREFIX}{{\"stage\":\"lunch\"}}\n")).unwrap();
        assert!(report(&dir).contains("Attempt stages: unknown (corrupt stage record; log kept)"));
        fs::remove_dir_all(dir).unwrap();
    }
    // Chain: #1174 retained records -> `profile export` (header, one row per observation) -> `profile tasks` and `profile board`.
    // Risk: a record lost or invented, columns drift for DashQ, one task number merged across projects, unknown shown as a number.
    #[test]
    fn export_then_reports() {
        let t = |p: Option<&str>, task, from: &str, to: &str, prev_s, rt: Option<&str>, host: Option<&str>, at| format!("{TRANSITION_PREFIX}{}\n", serde_json::to_string(&Transition {
            project: p.map(Into::into), task, entry: at, from: from.into(), to: to.into(), prev_s, rt: rt.map(Into::into), host: host.map(Into::into), at_ms: Some(at) }).unwrap());
        let b = |op: &str, us, err: Option<&str>, remaining, at| format!("{BOARD_PREFIX}{}\n", serde_json::to_string(&BoardOp { op: op.into(), us,
            outcome: if err.is_some() { "failed" } else { "ok" }.into(), error: err.unwrap_or("none").into(), remaining, at_ms: Some(at) }).unwrap());
        let log = [
            "normal taskq diagnostic\n".to_string(),
            t(Some("o/a"), 5, "ready", "doing", Some(10), Some("claude"), Some("mac"), 100),
            t(Some("o/a"), 5, "doing", "review", Some(100), Some("claude"), Some("mac"), 110),
            t(Some("o/b"), 5, "doing", "review", Some(300), Some("codex"), Some("win"), 120),
            t(None, 6, "doing", "ask", None, Some("claude"), Some("mac"), 130),
            t(Some("o/a"), 7, "ready", "doing", Some(999), None, None, 1),
            t(Some("o/a"), 8, "doing", "review", Some(7), Some("claude"), Some("lin"), 131),
            t(Some("o/a"), 8, "doing", "review", None, Some("claude"), Some("lin"), 132),
            t(Some("o/b"), 9, "ask", "ready", None, Some("codex"), Some("win"), 133),
            b("list", 4000, None, Some(4900), 100), b("list", 2000, Some("http5xx"), Some(4100), 101), b("claim", 1000, None, None, 102),
            format!("{BOARD_PREFIX}{{\"op\":\"lunch\"}}\n"),
            format!("{PREFIX}{}\n", serde_json::to_string(&Record { op: "spawn".into(), us: 7, error: "none".into(), at_ms: Some(140), trace: None }).unwrap()),
            format!("{STAGE_PREFIX}{}\n", serde_json::to_string(&Stage { project: Some("o/a".into()), stage: "build".into(), task: 5, attempt: 9, rt: "codex".into(),
                us: Some(5), outcome: "ok".into(), error: "none".into(), at_ms: Some(150), calls: None, tokens: Some(3), unknown: None }).unwrap()),
        ].concat();
        let text = export(&log, Some(100));
        let mut lines = text.lines();
        assert_eq!(lines.next().unwrap(), format!(r#"{{"schema":1,"columns":{},"since_ms":100,"window_ms":[100,150],"samples":12,"corrupt":1}}"#, serde_json::to_string(&COLUMNS).unwrap()));
        let first: serde_json::Map<String, serde_json::Value> = serde_json::from_str(lines.next().unwrap()).unwrap();
        assert_eq!(first.keys().collect::<Vec<_>>(), COLUMNS.iter().collect::<Vec<_>>());
        assert_eq!(serde_json::Value::Object(first), serde_json::json!({"kind":"transition","at_ms":100,"project":"o/a","task":5,"attempt":null,"entry":100,"name":null,
            "from":"ready","to":"doing","runtime":"claude","host":"mac","us":null,"prev_s":10,"outcome":null,"error":null,"remaining":null,"calls":null,"tokens":null,"unknown":null}));
        assert!(text.contains(r#""kind":"stage","at_ms":150,"project":"o/a","task":5,"attempt":9,"entry":null,"name":"build""#), "{text}");
        assert!(!text.contains("999"), "a record before the window is not exported");
        let tasks = render_tasks(&text).unwrap();
        assert!(tasks.contains("Window: 100..150 Unix ms (since 100); 12 samples, 1 corrupt skipped"), "{tasks}");
        assert!(tasks.contains("| doing | codex | win | 1 | 300.000/300.000/300.000 |\n| doing | claude | mac | 2 | 100.000/100.000/100.000 (1 of 2 known; no entry time) |\n| ready | claude | mac | 1 | 10.000/10.000/10.000 |"), "{tasks}");
        assert!(tasks.contains("| o/b | #5 | doing | codex | win | 1 | 300 | 300.000/300.000/300.000 |\n| o/a | #5 | doing | claude | mac | 1 | 100 |"), "{tasks}");
        assert!(tasks.contains("| o/a | #5 | ready | claude | mac | 1 | 10 | 10.000/10.000/10.000 |
| o/a | #8 | doing | claude | lin | 2 | 7 partial (1 of 2 known) | 7.000/7.000/7.000 (1 of 2 known; no entry time) |
| o/b | #9 | ask | codex | win | 1 | unknown (no entry time) | unknown (no entry time) |
| unknown (not recorded) | #6 | doing | claude | mac | 1 | unknown (no entry time) | unknown (no entry time) |"), "{tasks}");
        assert!(tasks.contains("| ready | claude | mac | 1 | 10.000/10.000/10.000 |
| doing | claude | lin | 2 | 7.000/7.000/7.000 (1 of 2 known; no entry time) |
| ask | codex | win | 1 | unknown (no entry time) |"), "unknown-only state ranks last: {tasks}");
        assert!(!tasks.contains("| 0 |"), "unknown never reads zero: {tasks}");
        let board = render_board(&text).unwrap();
        assert!(board.contains("Slow (p95): list > claim\nFailing (rate): list > claim\nRate-limit use (lowest remaining): list\n"), "{board}");
        assert!(board.contains("| claim | 1 | 1.000/1.000/1.000 | 0.00 | none | unknown (not reported) |\n| list | 2 | 2.000/4.000/4.000 | 50.00 | http5xx:1 | 4100 |"), "{board}");
        let empty = export("", None);
        assert!(render_tasks(&empty).unwrap().contains("Window: unknown (no timestamps) (all retained); 0 samples") && render_board(&empty).unwrap().contains("unknown (no observations)"));
        assert_eq!(since_ms("2h", 10_000_000), Ok(2_800_000));
        assert!(since_ms("2w", 1).is_err() && since_ms("h", 1).is_err());
        assert!(view(Path::new("/nonexistent-taskq"), "lunch", None).unwrap_err().contains("unknown profile view"));
    }

}
