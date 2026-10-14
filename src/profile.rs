//! §5 profiling: fixed numeric observations in the existing bounded taskq.log, never operation input/error text.
use serde::{Deserialize, Serialize};
use std::cell::Cell;
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
static SPAN: AtomicU64 = AtomicU64::new(1);
thread_local!(static PARENT: Cell<u64> = const { Cell::new(0) });

#[cfg(test)]
thread_local!(static OBSERVED: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) });
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
const OUTCOMES: [&str; 5] = ["ok", "ask", "retried", "failed", "cancelled"];
const RUNTIMES: [&str; 4] = ["claude", "codex", "codex-app", "other"];
const UNKNOWN: [&str; 5] = ["no-model", "no-usage", "log-unreadable", "app-turn", "adopted"];

/// Model use of one stage: service stages run no model; a runtime that reports none reads unknown, never zero;
/// `tokens` is `None` when any counted call lacked usage.
pub enum Usage { NoModel, Unknown(&'static str), Known { calls: Option<u64>, tokens: Option<u64> } }

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Stage {
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
    stage_at(LOG.get().map(PathBuf::as_path), name, task, attempt, rt, took, outcome, err, usage)
}

#[allow(clippy::too_many_arguments)]
fn stage_at(path: Option<&Path>, name: &'static str, task: u64, attempt: u64, rt: &str, took: Option<std::time::Duration>, outcome: &'static str, err: &'static str, usage: Usage) {
    let (calls, tokens, unknown) = match usage {
        Usage::NoModel => (None, None, Some("no-model")),
        Usage::Unknown(r) => (None, None, Some(r)),
        Usage::Known { calls, tokens } => (calls, tokens, tokens.is_none().then_some("no-usage")),
    };
    let r = Stage { stage: name.into(), task, attempt, rt: RUNTIMES.into_iter().find(|k| *k == rt).unwrap_or("other").into(),
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
        if !OUTCOMES.contains(&r.outcome.as_str()) || !(ERRORS.contains(&r.error.as_str()) || r.error == "exit")
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

pub fn report(install: &Path) -> String {
    let read = || -> Result<String, String> {
        let mut text = String::new();
        match fs::File::open(install.join("taskq.log")) {
            Ok(f) => { f.take(crate::data::LOG_MAX + 1).read_to_string(&mut text).map_err(|_| "taskq.log unreadable or invalid UTF-8")?; }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err("taskq.log unreadable".into()),
        }
        if text.len() as u64 > crate::data::LOG_MAX - 512 { return Err("taskq.log at retention bound; collection stopped until service pruning".into()); }
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis().min(u64::MAX as u128) as u64);
        let profile = render(&text).unwrap_or_else(|e| format!("Profile: unknown ({e}); all budgets unknown\n"));
        let stages = render_stages(&text).unwrap_or_else(|e| format!("Attempt stages: unknown ({e})\n"));
        Ok(format!("{profile}\n{stages}\n{}", crate::requests::sources(&text, now)))
    };
    read().unwrap_or_else(|e| format!("Profile: unknown ({e}); all budgets unknown\n\nAttempt stages: unknown ({e})\n\nBoard requests by source: unknown ({e})\n"))
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
        let text = report(&dir);
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
        let log = fs::read_to_string(&path).unwrap();
        assert!(!log.contains("secret") && !log.contains("/host/path") && !log.contains("private-rt"));
        let text = report(&dir);
        assert!(text.contains("Slow stages (p95): build/claude > build/codex > review/claude > claim/other > review/codex-app"), "{text}");
        assert!(text.contains("Failing stages (rate): claim/other > build/codex"), "{text}");
        assert!(text.contains("| build | claude | 1 | 9.000/9.000/9.000 | ok:1 | 2 | 230 | unknown |"), "{text}");
        assert!(text.contains("| build | codex | 2 | 1.000/5.000/5.000 | ask:1, failed exit:1 | unknown (log-unreadable, not-reported) | 1400 (1 of 2 known; log-unreadable) |"), "{text}");
        assert!(text.contains("| review | codex-app | 1 | 2.000/2.000/2.000 | ok:1 | unknown (app-turn) | unknown (app-turn) |"), "{text}");
        assert!(text.contains("| review | claude | 2 | 4.000/4.000/4.000 (1 of 2 timed; adopted) | ok:2 | 1 (1 of 2 known; adopted) | unknown (adopted, no-usage) |"), "{text}");
        assert!(text.contains("| claim | other | 1 | 3.000/3.000/3.000 | failed http5xx:1 | none (no model) | none (no model) |"), "{text}");
        fs::write(&path, format!("{log}{STAGE_PREFIX}{{\"stage\":\"lunch\"}}\n")).unwrap();
        assert!(report(&dir).contains("Attempt stages: unknown (corrupt stage record; log kept)"));
        fs::remove_dir_all(dir).unwrap();
    }
}
