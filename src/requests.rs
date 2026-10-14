//! Board request accounting and rate-limit pause (taskq.md § 4 Log and board budget): counts per operation of this
//! process and a pause, on the monotonic clock, while the board's shared request budget is exhausted.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Source records in `<install>/taskq.log` (§ 4 budget): one per CLI process at its exit, one per service hour.
const RECORD: &str = "taskq-requests ";
/// Requests of this process by operation, for its exit record.
static PROCESS: Mutex<BTreeMap<String, u64>> = Mutex::new(BTreeMap::new());

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Usage { at_ms: u64, source: String, ops: BTreeMap<String, u64> }

/// Longest pause: GitHub and GitLab budgets reset within the hour.
const MAX_PAUSE: i64 = 3600;
/// Operations named in the hourly log line; the rest add up to `other`.
const TOP: usize = 8;

#[derive(Clone, Debug, Default)]
pub struct Budget {
    /// Requests sent since the process started, by operation.
    pub ops: BTreeMap<String, u64>,
    /// Last `remaining`/`limit`/reset (board seconds) per budget (`x-ratelimit-resource`: GitHub `core`, `graphql`).
    buckets: BTreeMap<String, (u64, u64, Option<i64>)>,
    /// The budget with the smallest remaining share: its name, `remaining`, `limit` and reset.
    pub resource: Option<String>,
    pub remaining: Option<u64>,
    pub limit: Option<u64>,
    pub reset: Option<i64>,
    /// Paused until this instant; the reset in board seconds.
    paused: Option<(Instant, i64)>,
}

/// `<METHOD> <path>` of `path` (relative to the board API): numbers become `N`, other ids `*`, at most 3 segments.
/// No branch, SHA or text reaches a key.
pub fn op(method: &str, path: &str) -> String {
    let path = path.split(['?', '#']).next().unwrap_or_default();
    let segs: Vec<String> = path.split('/').filter(|s| !s.is_empty()).take(3).map(|s| {
        if s.bytes().all(|b| b.is_ascii_digit()) { "N".into() }
        else if s.len() <= 30 && s.bytes().all(|b| b.is_ascii_lowercase() || b == b'-' || b == b'_') { s.into() }
        else { "*".into() }
    }).collect();
    format!("{method} {}", segs.join("/"))
}

impl Budget {
    pub fn count(&mut self, op: String) {
        *self.ops.entry(op).or_default() += 1;
    }

    pub fn total(&self) -> u64 {
        self.ops.values().sum()
    }

    /// The reset (board seconds) while paused at `now`; a pause that ran out is cleared.
    pub fn paused(&mut self, now: Instant) -> Option<i64> {
        match self.paused {
            Some((until, reset)) if now < until => Some(reset),
            _ => { self.paused = None; None }
        }
    }

    pub fn paused_until(&self) -> Option<i64> {
        self.paused.filter(|(until, _)| Instant::now() < *until).map(|p| p.1)
    }

    /// Reads the budget headers of one reply. A 403 or 429 with `retry-after`, or with no requests remaining, pauses
    /// until the reset: `retry-after`, else the reset header against the reply's board time. Returns the reset when
    /// this reply started a pause.
    pub fn observe(&mut self, status: u16, header: impl Fn(&str) -> Option<String>, board_now: i64, now: Instant) -> Option<i64> {
        let num = |names: &[&str]| names.iter().find_map(|n| header(n)?.trim().parse::<i64>().ok());
        let remaining = num(&["x-ratelimit-remaining", "ratelimit-remaining"]);
        let limit = num(&["x-ratelimit-limit", "ratelimit-limit"]);
        if let Some(r) = remaining.and_then(|r| u64::try_from(r).ok()) {
            let name = header("x-ratelimit-resource").unwrap_or_default();
            let l = limit.and_then(|l| u64::try_from(l).ok()).or(self.buckets.get(&name).map(|b| b.1)).unwrap_or(0);
            self.buckets.insert(name, (r, l, num(&["x-ratelimit-reset", "ratelimit-reset"])));
            let low = self.buckets.iter().min_by_key(|(_, (r, l, _))| r * 10_000 / (*l).max(1)).map(|(k, v)| (k.clone(), *v));
            if let Some((name, (r, l, t))) = low {
                (self.resource, self.remaining, self.limit, self.reset) = ((!name.is_empty()).then_some(name), Some(r), (l > 0).then_some(l), t);
            }
        }
        if !matches!(status, 403 | 429) { return None; }
        let wait = match (num(&["retry-after"]), remaining) {
            (Some(s), _) => s,
            (None, Some(0)) => num(&["x-ratelimit-reset", "ratelimit-reset"]).map_or(60, |reset| reset - board_now),
            _ => return None,
        }.clamp(1, MAX_PAUSE);
        let reset = board_now + wait;
        self.paused = Some((now + Duration::from_secs(wait as u64), reset));
        Some(reset)
    }

    /// The budget alarm (§ 4): the last `remaining` is at most 20 % of `limit`.
    pub fn low(&self) -> bool {
        matches!((self.remaining, self.limit), (Some(r), Some(l)) if l > 0 && r * 5 <= l)
    }

    /// The largest operation since the cumulative counts `start` (this process's current hour).
    pub fn top(&self, start: &BTreeMap<String, u64>) -> Option<(String, u64)> {
        diff(&self.ops, start).into_iter().max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)))
    }

    /// `status.json` `requests` (§ 4): `hour` the counts of the last full hour, `start` the cumulative counts at its end.
    pub fn json(&self, since: u64, hour: Option<&BTreeMap<String, u64>>, start: &BTreeMap<String, u64>) -> Value {
        json!({"since": since, "total": self.total(), "ops": self.ops, "last_hour": hour, "remaining": self.remaining, "limit": self.limit, "resource": self.resource,
            "reset": self.reset.map(crate::board::stamp), "top": self.top(start).map(|(op, n)| format!("{op} {n}")),
            "paused_until": self.paused_until().map(crate::board::stamp)})
    }
}

/// The refusal of a paused request: names the reset in UTC.
pub fn paused_text(reset: i64) -> String {
    format!("rate limit: board requests paused until {}", crate::board::stamp(reset))
}

/// Requests per operation between two cumulative counts.
pub fn diff(now: &BTreeMap<String, u64>, before: &BTreeMap<String, u64>) -> BTreeMap<String, u64> {
    now.iter().map(|(k, v)| (k.clone(), v.saturating_sub(before.get(k).copied().unwrap_or(0)))).filter(|(_, v)| *v > 0).collect()
}

/// `<total><unit>; <op> <n>, ..` with the largest operations first, the rest as `other`.
pub fn summary(ops: &BTreeMap<String, u64>, unit: &str) -> String {
    let mut v: Vec<(&String, &u64)> = ops.iter().collect();
    v.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    let mut parts: Vec<String> = v.iter().take(TOP).map(|(k, n)| format!("{k} {n}")).collect();
    let other: u64 = v.iter().skip(TOP).map(|x| *x.1).sum();
    if other > 0 { parts.push(format!("other {other}")); }
    format!("{}{unit}{}{}", ops.values().sum::<u64>(), if parts.is_empty() { "" } else { "; " }, parts.join(", "))
}

/// Owner lines of a `status.json` `requests` (§ 4, § 5 `service status`): the last full hour, else unknown, and a pause.
pub fn lines(status: &Value) -> Vec<String> {
    let r = &status["requests"];
    let mut out = vec![match r["last_hour"].as_object() {
        Some(h) => format!("Board requests: {}", summary(&h.iter().filter_map(|(k, v)| Some((k.clone(), v.as_u64()?))).collect(), "/h")),
        None if r.is_object() => "Board requests: unknown (no full hour yet)".into(),
        None => "Board requests: unknown (no request accounting in status)".into(),
    }];
    out.extend(warning(status));
    out
}

/// `Rate limit: paused until <time> (<host>)` while the service reports a pause, else the low-budget alarm (R6, § 4).
pub fn warning(status: &Value) -> Option<String> {
    let r = &status["requests"];
    let host = status["host"].as_str().unwrap_or("this host");
    if let Some(until) = r["paused_until"].as_str() {
        return Some(format!("Rate limit: board requests paused until {until} ({host})"));
    }
    let (remaining, limit) = (r["remaining"].as_u64()?, r["limit"].as_u64()?);
    let resource = r["resource"].as_str().map(|s| format!(" {s}")).unwrap_or_default();
    (limit > 0 && remaining * 5 <= limit).then(|| format!("Board budget low: {remaining}/{limit}{resource} until {} ({host}); top here: {}",
        r["reset"].as_str().unwrap_or("unknown"), r["top"].as_str().unwrap_or("none")))
}

/// Counts one request of this process for its exit record.
pub fn tally(op: &str) {
    if let Ok(mut m) = PROCESS.lock() { *m.entry(op.into()).or_default() += 1; }
}

/// The source of a CLI process: `<role> <command>` (`service` adds its subcommand). Role `worker` or `reviewer`
/// from `TASKQ_ROLE`, else `cli` (owner, PM, operator). None for `service run`: the service records each hour.
pub fn source(args: &[String], role: Option<&str>) -> Option<String> {
    let word = |s: Option<&String>| s.filter(|s| !s.is_empty() && s.len() <= 30 && s.bytes().all(|b| b.is_ascii_lowercase() || b == b'-'))
        .map_or("*".to_string(), Clone::clone);
    let mut cmd = word(args.first());
    if cmd == "service" {
        if args.get(1).is_some_and(|s| s == "run") { return None; }
        cmd = format!("service {}", word(args.get(1)));
    }
    let role = role.filter(|r| matches!(*r, "worker" | "reviewer")).unwrap_or("cli");
    Some(format!("{role} {cmd}"))
}

/// Appends one source record of `ops` to `<install>/taskq.log`; nothing when no request was sent.
pub fn record(source: &str, ops: &BTreeMap<String, u64>) {
    let Some(path) = crate::profile::log_path() else { return };
    if ops.is_empty() { return; }
    let at_ms = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis().min(u64::MAX as u128) as u64);
    let usage = Usage { at_ms, source: source.into(), ops: ops.clone() };
    let line = format!("{RECORD}{}\n", serde_json::to_string(&usage).unwrap_or_default());
    if crate::profile::append_line(path, &line).is_err() {
        eprintln!("Board request record: unknown (taskq.log full or append failed; service pruning required)");
    }
}

/// The exit record of this CLI process.
pub fn flush(source: &str) {
    let ops = PROCESS.lock().map(|mut m| std::mem::take(&mut *m)).unwrap_or_default();
    record(source, &ops);
}

/// `taskq profile` section: board requests of the hour before `now_ms` by source, from the retained records.
pub fn sources(text: &str, now_ms: u64) -> String {
    let start = now_ms.saturating_sub(3_600_000);
    let (mut rows, mut first) = (BTreeMap::<String, (u64, BTreeMap<String, u64>)>::new(), None::<u64>);
    for line in text.lines().filter_map(|l| l.strip_prefix(RECORD)) {
        let Ok(u) = serde_json::from_str::<Usage>(line) else {
            return "Board requests by source: unknown (corrupt request record; log kept)\n".into();
        };
        first = Some(first.map_or(u.at_ms, |f| f.min(u.at_ms)));
        if u.at_ms <= start || u.at_ms > now_ms { continue; }
        let row = rows.entry(u.source).or_default();
        row.0 += 1;
        for (op, n) in u.ops { *row.1.entry(op).or_default() += n; }
    }
    if rows.is_empty() {
        return "Board requests by source: unknown (no request records in the last hour)\n".into();
    }
    let coverage = match first {
        Some(f) if f <= start => "full hour".to_string(),
        _ => format!("partial: records start {} Unix ms; earlier requests unknown", first.unwrap_or(start)),
    };
    let mut rows: Vec<_> = rows.into_iter().map(|(s, (n, ops))| (s, n, ops.values().sum::<u64>(), ops)).collect();
    rows.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.cmp(&b.0)));
    let total: u64 = rows.iter().map(|r| r.2).sum();
    let mut out = format!("Board requests by source: {total} in {start}..{now_ms} Unix ms ({coverage}); this host only; \
        a CLI process counts at its exit, a service each hour; agent `gh`, PM chat tools and DashQ not counted\n\n\
        | Source | Records | Requests | Top operations |\n|---|---:|---:|---|\n");
    for (source, n, sum, ops) in rows {
        let mut ops: Vec<_> = ops.into_iter().collect();
        ops.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        let top = ops.iter().take(3).map(|(op, n)| format!("{op} {n}")).collect::<Vec<_>>().join(", ");
        out += &format!("| {source} | {n} | {sum} | {top} |\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // Chain: #1084 A1 CLI process -> source record -> `taskq profile` hour table. Risk: a source is merged with
    // another, a request outside the hour is counted, a missing or corrupt record reads as zero, or free text
    // from arguments reaches the log.
    #[test]
    fn requests_by_source_for_the_last_hour() {
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(source(&args(&["result", "1084"]), Some("worker")).unwrap(), "worker result");
        assert_eq!(source(&args(&["status"]), Some("operator")).unwrap(), "cli status");
        assert_eq!(source(&args(&["service", "status"]), None).unwrap(), "cli service status");
        assert_eq!(source(&args(&["service", "run"]), None), None, "the service records its own hours");
        assert_eq!(source(&args(&["C:\\secret path"]), None).unwrap(), "cli *");
        assert_eq!(source(&[], None).unwrap(), "cli *");
        let line = |at: u64, src: &str, ops: &[(&str, u64)]| format!("{RECORD}{}\n", serde_json::to_string(&Usage { at_ms: at,
            source: src.into(), ops: ops.iter().map(|(k, v)| (k.to_string(), *v)).collect() }).unwrap());
        let now = 10_000_000;
        assert!(sources("", now).contains("unknown (no request records"));
        let text = [line(now - 3_700_000, "cli status", &[("GET issues", 50)]), "taskq-profile {}\nother line\n".into(),
            line(now - 60_000, "worker result", &[("POST issues/N/comments", 1), ("GET issues/N/comments", 2)]),
            line(now - 50_000, "worker result", &[("GET issues/N/comments", 2)]),
            line(now - 40_000, "service", &[("POST graphql", 60), ("PATCH issues/N", 30)])].concat();
        let report = sources(&text, now);
        assert!(report.starts_with("Board requests by source: 95 in 6400000..10000000 Unix ms (full hour)"), "{report}");
        assert!(report.contains("| service | 1 | 90 | POST graphql 60, PATCH issues/N 30 |\n| worker result | 2 | 5 | GET issues/N/comments 4, POST issues/N/comments 1 |"), "{report}");
        assert!(!report.contains("cli status"), "a record before the hour is not counted");
        assert!(sources(&text[line(now - 3_700_000, "cli status", &[("GET issues", 50)]).len()..], now).contains("(partial: records start 9940000"));
        assert!(sources(&format!("{text}{RECORD}{{\"at_ms\":1}}\n"), now).starts_with("Board requests by source: unknown (corrupt"));
        let dir = std::env::temp_dir().join(format!("taskq-sources-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let wall = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64;
        std::fs::write(dir.join("taskq.log"), line(wall - 60_000, "worker result", &[("GET issues/N/comments", 5)]).repeat(2)).unwrap();
        assert!(crate::profile::report(&dir).contains("| worker result | 2 | 10 |"), "`taskq profile` prints the table");
        std::fs::remove_dir_all(dir).unwrap();
    }

    // Chain: § 4 board budget. Risk: an exhausted budget keeps sending requests, or a key leaks a branch or SHA.
    #[test]
    fn exhausted_budget_pauses_until_reset_and_resumes() {
        let t0 = Instant::now();
        let mut b = Budget::default();
        let h = |pairs: &'static [(&'static str, &'static str)]| move |n: &str| pairs.iter().find(|(k, _)| *k == n).map(|(_, v)| v.to_string());
        assert_eq!(b.observe(200, h(&[("x-ratelimit-remaining", "12"), ("x-ratelimit-limit", "5000")]), 1000, t0), None);
        assert_eq!((b.remaining, b.limit, b.paused(t0)), (Some(12), Some(5000), None));
        // a 403 without budget headers is an ordinary refusal
        assert_eq!(b.observe(403, h(&[]), 1000, t0), None);
        // primary limit: wait for the reset header against board time, not the local clock
        assert_eq!(b.observe(403, h(&[("x-ratelimit-remaining", "0"), ("x-ratelimit-reset", "1840")]), 1000, t0), Some(1840));
        assert_eq!(b.paused(t0 + Duration::from_secs(839)), Some(1840));
        assert_eq!(b.paused(t0 + Duration::from_secs(840)), None, "resumes at the reset without a restart");
        // secondary limit: retry-after wins; a far reset is capped at one hour
        assert_eq!(b.observe(429, h(&[("retry-after", "30"), ("x-ratelimit-remaining", "0")]), 1000, t0), Some(1030));
        assert_eq!(b.observe(403, h(&[("ratelimit-remaining", "0"), ("ratelimit-reset", "999999")]), 1000, t0), Some(4600));
        assert_eq!(op("PATCH", "/issues/77"), "PATCH issues/N");
        assert_eq!(op("GET", "/issues/5/comments?per_page=100"), "GET issues/N/comments");
        assert_eq!(op("GET", "/git/ref/heads/taskq-1009"), "GET git/ref/heads");
        assert_eq!(op("GET", &format!("/commits/{}/check-runs", "a1".repeat(20))), "GET commits/*/check-runs");
        let mut later = BTreeMap::from([("GET issues".to_string(), 7), ("PATCH issues/N".to_string(), 60)]);
        let hour = diff(&later, &BTreeMap::from([("GET issues".to_string(), 7)]));
        assert_eq!(summary(&hour, ""), "60; PATCH issues/N 60");
        later.extend((0..9).map(|i| (format!("GET x{i}"), 1)));
        assert!(summary(&later, "").ends_with(", GET x5 1, other 3"));
    }

    // Chain: § 4 board budget to `service status` and R6. Risk: missing accounting reads as zero requests.
    #[test]
    fn request_lines_keep_unknown_and_name_the_pause() {
        assert_eq!(lines(&json!({})), ["Board requests: unknown (no request accounting in status)"]);
        let s = json!({"host": "mac", "requests": {"total": 3, "last_hour": null, "paused_until": "2026-10-14T04:41:00Z"}});
        assert_eq!(lines(&s), ["Board requests: unknown (no full hour yet)", "Rate limit: board requests paused until 2026-10-14T04:41:00Z (mac)"]);
        let s = json!({"requests": {"last_hour": {"PATCH issues/N": 60}, "paused_until": null}});
        assert_eq!(lines(&s), ["Board requests: 60/h; PATCH issues/N 60"]);
    }

    // Chain: § 4 budget alarm to `service status` and R6. Risk: a draining budget stays silent, or names no consumer.
    #[test]
    fn low_budget_alarm_names_the_top_consumer() {
        let h = |r: &'static str| move |n: &str| match n { "x-ratelimit-remaining" => Some(r.to_string()), "x-ratelimit-limit" => Some("5000".into()),
            "x-ratelimit-reset" => Some("1791964800".into()), _ => None };
        let mut b = Budget::default();
        b.observe(200, h("1001"), 0, Instant::now());
        assert!(!b.low(), "above 20 % is quiet");
        b.observe(200, h("1000"), 0, Instant::now());
        assert!(b.low());
        let start = BTreeMap::from([("PATCH issues/N".to_string(), 50)]);
        b.ops = BTreeMap::from([("PATCH issues/N".to_string(), 80), ("GET issues".to_string(), 20)]);
        assert_eq!(b.top(&start), Some(("PATCH issues/N".into(), 30)));
        let s = json!({"host": "mac", "requests": b.json(0, None, &start)});
        assert_eq!(warning(&s).unwrap(), "Board budget low: 1000/5000 until 2026-10-14T08:00:00Z (mac); top here: PATCH issues/N 30");
    }

    // Chain: § 4 per-resource budget. Risk: a drained GraphQL bucket hides behind a full REST one, or the reverse.
    #[test]
    fn budget_follows_the_lowest_resource() {
        let h = |res: &'static str, r: &'static str| move |n: &str| match n { "x-ratelimit-resource" => Some(res.to_string()),
            "x-ratelimit-remaining" => Some(r.to_string()), "x-ratelimit-limit" => Some("5000".into()), _ => None };
        let mut b = Budget::default();
        b.observe(200, h("core", "900"), 0, Instant::now());
        b.observe(200, h("graphql", "4000"), 0, Instant::now());
        assert_eq!((b.resource.as_deref(), b.remaining), (Some("core"), Some(900)), "a full bucket never hides a low one");
        b.observe(200, h("graphql", "500"), 0, Instant::now());
        b.observe(200, h("core", "4990"), 0, Instant::now());
        assert_eq!((b.resource.as_deref(), b.remaining, b.low()), (Some("graphql"), Some(500), true));
        let s = json!({"host": "mac", "requests": b.json(0, None, &BTreeMap::new())});
        assert_eq!(warning(&s).unwrap(), "Board budget low: 500/5000 graphql until unknown (mac); top here: none");
    }
}
