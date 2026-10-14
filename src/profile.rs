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
        at_ms: SystemTime::now().duration_since(UNIX_EPOCH).ok().map(|d| d.as_millis().min(u64::MAX as u128) as u64), trace } };
    let result = f();
    observation.record.error = result.as_ref().err().map_or("none", |e| error(&e.to_string())).into();
    result
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
    fs::create_dir_all(path.parent().unwrap())?;
    let _lock = log_lock(path)?;
    if fs::metadata(path).is_ok_and(|m| m.len() > crate::data::LOG_MAX - 512) {
        return Err(std::io::Error::other("log full"));
    }
    let line = format!("{PREFIX}{}\n", serde_json::to_string(r)?);
    OpenOptions::new().create(true).append(true).open(path)?.write_all(line.as_bytes())
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
        let file = match fs::File::open(install.join("taskq.log")) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return render("").map_err(String::from),
            Err(_) => return Err("taskq.log unreadable".into()),
        };
        let mut text = String::new();
        file.take(crate::data::LOG_MAX + 1).read_to_string(&mut text).map_err(|_| "taskq.log unreadable or invalid UTF-8")?;
        if text.len() as u64 > crate::data::LOG_MAX - 512 { return Err("taskq.log at retention bound; collection stopped until service pruning".into()); }
        render(&text).map_err(String::from)
    };
    read().unwrap_or_else(|e| format!("Profile: unknown ({e}); all budgets unknown\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    // Chain: A2/A3 operation -> bounded log -> ranked report. Risk: lost failures, wrong ranks/quantiles,
    // unsafe input persisted, corrupt/missing data called healthy, or telemetry outgrows existing retention.
    #[test]
    fn retained_profile_report() {
        let dir = std::env::temp_dir().join(format!("taskq-profile-{}", std::process::id()));
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
            // the trimmer retries until it wins the lock once: a run where it never did kept the older record (#1072 CI)
            scope.spawn(|| {
                while !crate::data::trim_tail(&path, crate::data::LOG_MAX) { std::thread::yield_now(); }
                for _ in 0..49 { crate::data::trim_tail(&path, crate::data::LOG_MAX); }
            });
        });
        let records = fs::read_to_string(&path).unwrap().lines().filter(|l| l.starts_with(PREFIX)).count();
        assert_eq!(records, successful.load(Ordering::Relaxed));
        assert!(!report(&dir).starts_with("Profile: unknown"));
        fs::remove_dir_all(dir).unwrap();
    }
}
