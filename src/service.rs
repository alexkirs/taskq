//! The host service (taskq.md § 3, § 4): one loop around `fold`. It owns this host's children and its heartbeat issue;
//! everything it knows about other hosts comes from the board.

use crate::board::{Board, Comment, Issue};
use crate::core::{self, State, Task};
use crate::runtime::{Launch, Proc};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use crate::admission::{Entry, Res, Want};
use std::path::PathBuf;

#[derive(Clone)]
pub struct Config {
    pub root: PathBuf,
    pub install: PathBuf,
    pub board: &'static crate::board::Kind,
    pub web_host: String,
    pub repo: String,
    /// Publication mode `direct|pr`, from the delivery preset (§ 7).
    pub publish: String,
    /// § 7 delivery preset and its policy revision.
    pub delivery: String,
    pub delivery_revision: u64,
    pub me: String,
    pub limits: BTreeMap<String, u32>,
    /// What this host offers for `needs-<x>` labels.
    pub has: Vec<String>,
    pub permission_mode: String,
    pub codex: Vec<String>,
    pub ttl: i64,
    pub poll: u64,
    pub update_every: u64,
    pub rollback_after: u64,
}

pub const HOST_LABEL: &str = "taskq-host";
/// The capacity reason of an empty focus (§ 4).
pub const FOCUS_OFF: &str = "focus off: nothing admitted; set `taskq service focus all|<label..>`";
pub const TASK_LABEL: &str = "taskq";

/// Only issues labelled `taskq` are tasks (§ 1): a user issue never starts by itself.
pub fn is_task(i: &Issue) -> bool {
    i.trusted && i.labels.iter().any(|l| l == TASK_LABEL)
}

/// A held task (§ 1): labelled `q-held` without `taskq`, so no version admits it; listed as `held` whatever its log.
pub fn is_held(i: &Issue) -> bool {
    i.trusted && !is_task(i) && i.labels.iter().any(|l| l == "q-held")
}

pub fn is_host(i: &Issue) -> bool {
    i.labels.iter().any(|l| l == HOST_LABEL) && i.title.starts_with("taskq host ")
}

fn label(i: &Issue, prefix: &str) -> Option<String> {
    i.labels.iter().find_map(|l| l.strip_prefix(prefix).map(String::from))
}

pub fn priority(i: &Issue) -> u8 {
    if crate::labels::matches(&i.labels, "p0") { 0 } else if crate::labels::matches(&i.labels, "p1") { 1 } else { 2 }
}

/// The host named in a `spawn` session (`<rt>:<session> <host>`).
pub fn host_of(t: &Task) -> Option<&str> {
    t.session.as_deref()?.rsplit(' ').next()
}

/// `rt` and the session id of a `spawn` session.
fn rt_of(t: &Task) -> Option<(&str, &str)> {
    t.session.as_deref()?.split(' ').next()?.split_once(':')
}

/// Set the `q-<state>` label from the fold; a closed task loses its `q-` labels and its issue is closed (§ 2).
pub fn project(board: &dyn Board, issue: &Issue, t: &Task) -> Result<(), String> {
    let want = board.state_label(t.state.name())?;
    let stale: Vec<String> = issue.labels.iter().filter(|l| l.starts_with("q-") && **l != want).cloned().collect();
    if t.state == State::Closed {
        board.labels(issue.number, &[], &stale)?;
        board.close(issue.number)
    } else if !stale.is_empty() || !issue.labels.contains(&want) {
        board.labels(issue.number, &[want], &stale)
    } else {
        Ok(())
    }
}

pub struct View {
    pub issue: Issue,
    pub task: Task,
    pub comments: Vec<Comment>,
    pub log: Vec<core::Entry>,
    pub open_deps: Vec<u64>,
}

/// Fold every open task (heartbeat issues and untrusted authors excluded).
/// Remove `.worktrees/taskq-<N>` of closed tasks and their merged local branch (§ 7, R11). Without `force` git refuses
/// dirty worktrees, so those stay; with it the uncommitted work goes to `.taskq/trash/taskq-<N>.patch` first. Commits
/// stay in the branch: `git branch -d` keeps an unmerged one. `ponytail:` a squash-merged branch stays until a manual
/// `git branch -D`.
pub fn cleanup(board: &dyn Board, root: &std::path::Path, mut skip: impl FnMut(u64) -> bool, force: bool) -> Vec<String> {
    let mut log = vec![];
    let Ok(dir) = std::fs::read_dir(root.join(".worktrees")) else { return log };
    let git = |dir: &std::path::Path, args: &[&str]| std::process::Command::new("git").arg("-C").arg(dir).args(["-c", "core.fsmonitor=false", "-c", "core.hooksPath=/dev/null"]).args(args).output().ok().filter(|o| o.status.success()).map(|o| o.stdout);
    for e in dir.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(n) = name.strip_prefix("taskq-").and_then(|n| n.parse::<u64>().ok()) else { continue };
        if skip(n) || !board.issue(n).is_ok_and(|i| !i.open) {
            continue; // a fresh read must show the issue closed (R12)
        }
        let wt = format!(".worktrees/{name}");
        let removed = if force {
            let trash = root.join(".taskq/trash");
            let saved = git(&e.path(), &["add", "-A"]).and_then(|_| git(&e.path(), &["diff", "--cached", "--binary"])).is_some_and(|patch| {
                patch.is_empty() || std::fs::create_dir_all(&trash).and_then(|_| std::fs::write(trash.join(format!("{name}.patch")), &patch)).is_ok()
            });
            saved && git(root, &["worktree", "remove", "--force", &wt]).is_some()
        } else {
            git(root, &["worktree", "remove", &wt]).is_some()
        };
        if removed {
            git(root, &["branch", "-d", &name]);
            log.push(format!("#{n} worktree removed"));
        } else {
            log.push(format!("#{n} worktree kept: {}", if force { "could not save its patch" } else { "uncommitted work, see `taskq cleanup --force`" }));
        }
    }
    log
}

pub fn views(board: &dyn Board, cache: &mut HashMap<u64, (i64, Vec<Comment>)>) -> Result<(Vec<View>, Vec<Issue>), String> {
    let issues = board.issues(None)?;
    let open: HashSet<u64> = issues.iter().map(|i| i.number).collect();
    let (hosts, tasks): (Vec<Issue>, Vec<Issue>) = issues.into_iter().filter(|i| i.trusted).partition(is_host);
    let mut out = vec![];
    for issue in tasks.into_iter().filter(|i| is_task(i) || is_held(i)) {
        let comments = match cache.get(&issue.number) {
            Some((u, c)) if *u == issue.updated => c.clone(),
            _ => board.comments(issue.number)?,
        };
        cache.insert(issue.number, (issue.updated, comments.clone()));
        let log: Vec<_> = comments.iter().filter(|c| c.trusted).map(|c| core::Entry { id: c.id, body: c.body.clone(), edited: c.edited }).collect();
        let open_deps: Vec<u64> = core::depends(&issue.body).into_iter().filter(|d| open.contains(d)).collect();
        let mut task = core::fold(issue.number, &log, !open_deps.is_empty());
        if is_held(&issue) {
            task.state = State::Held;
        }
        out.push(View { task, issue, comments, log, open_deps });
    }
    Ok((out, hosts))
}

pub struct Kid {
    pub task: u64,
    pub attempt: u64,
    pub rt: String,
    pub proc: Box<dyn Proc>,
}

pub struct Service<'a> {
    pub board: &'a dyn Board,
    pub cfg: &'a Config,
    pub version: String,
    pub kids: Vec<Kid>,
    /// `service <id>` in the heartbeat (§ 4): a hash of `<install>/installation-id`; empty writes no line.
    pub instance: String,
    /// Labels from `<root>/.taskq/focus`; empty: admit nothing, `all`: any task; `p0` passes any non-empty focus (§ 4).
    pub focus: Vec<String>,
    /// Why `focus` could not be read; it then admits nothing (§ 4 Data).
    pub focus_error: Option<String>,
    /// The last inbox id handed to each attempt's child.
    delivered: HashMap<u64, u64>,
    cache: HashMap<u64, (i64, Vec<Comment>)>,
    label_fixes: HashMap<u64, (String, Vec<String>, u8)>,
    /// Worktrees of closed tasks already tried once by `prune`.
    pruned: HashSet<u64>,
    start: Box<dyn FnMut(&Launch) -> Result<Box<dyn Proc>, crate::runtime::StartError> + 'a>,
    #[cfg(test)]
    agent_status: Value,
    pub log: Vec<String>,
    /// The read model of the last successful pass (for `status.json` and the tray).
    pub snapshot: Option<crate::snapshot::Snapshot>,
    pub tasks_read: Option<std::time::Instant>,
    /// This host's heartbeat issue, from the last board read: a lease write needs no second list (§ 4 budget).
    host_issue: Option<u64>,
    /// Owner comments of the last pass (§ 4): task, comment id, text, card.
    pub owner_comments: Vec<(u64, u64, String, String)>,
    /// State of each open task in the previous pass, for `events`.
    seen: HashMap<u64, State>,
    /// First time covered by `<root>/.taskq/events` (§ 4).
    pub events_since: Option<i64>,
    /// Capacity reloaded before each pass (§ 3); an error holds every launch.
    pub cap: Result<Capacity, String>,
    pub gate: Gate,
    /// Monotonic seconds for the gate.
    pub mono: u64,
    /// Won claims the gate holds before launch.
    held: Vec<Launch>,
    /// Attempts without a child that keep their lease: held claims and resumes (§ 3, § 4 heartbeat).
    pending: BTreeSet<(u64, u64)>,
    /// Wall seconds for the host authority, set by the loop.
    pub wall: i64,
    /// Auto reservations the host granted, not yet a child; refused requests of this pass and their reason (§ 3).
    reserved: Vec<Res>,
    requested: Vec<Res>,
    refused: Option<(&'static str, String)>,
    /// Held reservations of the whole host at the last host write; unknown when it failed.
    host_used: Option<u32>,
    /// The emergency hold read at the start of each pass (§ 3): refuses every launch.
    pub hold: Option<String>,
    /// Runtimes whose access policy refuses a launch, with the reason (§ 8 Access): no claim, resume or reviewer.
    pub refused_access: BTreeMap<String, String>,
    /// Children whose last liveness read failed, by attempt: kept and reported unknown, never `gone` (§ 4).
    pub unknown: HashMap<u64, String>,
}

const MONTH: i64 = 30 * 86400;

/// Load gate settings (§ 9).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct GateCfg {
    pub enabled: bool,
    pub cpu_max: u32,
    pub memory_min: u32,
    /// Auto: CPU is the 60 s mean; the latest 30 s interval at 100 % still vetoes; memory also holds below
    /// [`FLOOR_GIB`] available or at macOS pressure level [`PRESSURE_CRITICAL`] (D03).
    pub mean: bool,
}

pub const GATE_DEFAULT: GateCfg = GateCfg { enabled: false, cpu_max: 85, memory_min: 15, mean: false };
/// Auto admission defaults from the D03 calibration (docs/design/admission-calibration.md).
pub const GATE_AUTO: GateCfg = GateCfg { enabled: true, cpu_max: 90, memory_min: 10, mean: true };
pub const FLOOR_GIB: f64 = 2.5;
pub const PRESSURE_CRITICAL: u32 = 4;
pub const RUNTIMES: [&str; 3] = ["claude", "codex", "codex-app"];

/// Effective capacity of one host (§ 3, § 9).
#[derive(Clone, PartialEq, Debug)]
pub struct Capacity {
    pub limits: BTreeMap<String, u32>,
    pub gate: GateCfg,
    /// `admission` mode `auto`: the host authority admits; a runtime without a limit has no count ceiling.
    pub auto: bool,
    /// Auto: optional host-wide session ceiling.
    pub max: Option<u32>,
}

impl Capacity {
    pub fn fixed(limits: BTreeMap<String, u32>, gate: GateCfg) -> Capacity {
        Capacity { limits, gate, auto: false, max: None }
    }

    pub fn json(&self) -> Value {
        json!({"limits": self.limits, "load_gate": {"enabled": self.gate.enabled, "cpu_max": self.gate.cpu_max, "memory_min": self.gate.memory_min},
            "admission": {"mode": if self.auto { "auto" } else { "fixed" }, "max": self.max}})
    }
}

/// Capacity of `host` from `taskq.json` (§ 9): its `capacity_hosts` limits override the top-level ones; its
/// `load_gate` replaces the top-level object; omitted gate fields take the defaults. Invalid values are an error.
pub fn capacity(v: &Value, host: &str) -> Result<Capacity, String> {
    let limits = |x: &Value, at: &str| -> Result<BTreeMap<String, u32>, String> {
        match x {
            Value::Null => Ok(BTreeMap::new()),
            Value::Object(m) => m.iter().map(|(k, n)| n.as_u64().and_then(|n| u32::try_from(n).ok()).map(|n| (k.clone(), n)).ok_or(format!("{at}.{k}: not an integer 0..{}", u32::MAX))).collect(),
            _ => Err(format!("{at}: not an object")),
        }
    };
    if !matches!(v["capacity_hosts"], Value::Null | Value::Object(_)) {
        return Err("capacity_hosts: not an object".into());
    }
    let h = &v["capacity_hosts"][host];
    let mut out = limits(&v["limits"], "limits")?;
    out.extend(limits(&h["limits"], &format!("capacity_hosts.{host}.limits"))?);
    let (a, aat) = if h["admission"].is_null() { (&v["admission"], "admission".to_string()) } else { (&h["admission"], format!("capacity_hosts.{host}.admission")) };
    if !matches!(a, Value::Null | Value::Object(_)) {
        return Err(format!("{aat}: not an object"));
    }
    let auto = match &a["mode"] {
        Value::Null => false,
        x if x == "auto" => true,
        x if x == "fixed" => false,
        _ => return Err(format!("{aat}.mode: auto or fixed")),
    };
    let max = match &a["max"] {
        Value::Null => None,
        x => Some(x.as_u64().and_then(|n| u32::try_from(n).ok()).ok_or(format!("{aat}.max: not an integer 0..{}", u32::MAX))?),
    };
    let d = if auto { GATE_AUTO } else { GATE_DEFAULT };
    let (g, at) = if h["load_gate"].is_null() { (&v["load_gate"], "load_gate".to_string()) } else { (&h["load_gate"], format!("capacity_hosts.{host}.load_gate")) };
    if !matches!(g, Value::Null | Value::Object(_)) {
        return Err(format!("{at}: not an object"));
    }
    let pct = |k: &str, d: u32, lo: u64, hi: u64| match &g[k] {
        Value::Null => Ok(d),
        x => x.as_u64().filter(|n| (lo..=hi).contains(n)).map(|n| n as u32).ok_or(format!("{at}.{k}: not an integer {lo}..{hi}")),
    };
    let enabled = match &g["enabled"] {
        Value::Null => d.enabled,
        x => x.as_bool().ok_or(format!("{at}.enabled: not true or false"))?,
    };
    if auto && !enabled {
        return Err(format!("{at}.enabled: Auto admission needs the load gate; use admission mode fixed"));
    }
    let gate = GateCfg { enabled, cpu_max: pct("cpu_max", d.cpu_max, 10, 100)?, memory_min: pct("memory_min", d.memory_min, 1, 95)?, mean: d.mean };
    Ok(Capacity { limits: out, gate, auto, max })
}

/// The load gate (§ 3): samples on a monotonic clock in seconds, decisions on unrounded percentages.
#[derive(Default)]
pub struct Gate {
    cfg: Option<GateCfg>,
    /// CPU (busy, total) ticks of the previous reading: the baseline of the next.
    base: Option<(u64, u64)>,
    /// The reading before `base`: with it CPU is the mean over two intervals (60 s, `GateCfg::mean`).
    prev: Option<(u64, u64)>,
    /// Latest sample: monotonic time, wall time, CPU busy %, available memory %.
    pub last: Option<(u64, i64, Option<f64>, Option<f64>)>,
    /// Auto: available GiB and pressure level read with the latest sample; `None` is a failed reading.
    pub host_mem: Option<(f64, u32)>,
    /// Consecutive calm samples toward reopening.
    good: u32,
    open: bool,
    /// Monotonic time of the last launch and whether a sample followed it.
    launched: Option<u64>,
    sampled: bool,
}

impl Gate {
    /// Enabling or changing thresholds starts held.
    pub fn configure(&mut self, c: GateCfg) {
        if self.cfg != Some(c) {
            (self.cfg, self.open, self.good) = (Some(c), false, 0);
        }
    }

    pub fn enabled(&self) -> bool {
        self.cfg.is_some_and(|c| c.enabled)
    }

    /// One sample: CPU ticks `(busy, total)` and available memory %; `None` is a failed reading.
    pub fn observe(&mut self, now: u64, wall: i64, ticks: Option<(u64, u64)>, memory: Option<f64>) {
        // decreasing counters or an empty interval are no reading; the baseline restarts either way
        let busy = |from: Option<(u64, u64)>| match (from, ticks) {
            (Some((b0, t0)), Some((b, t))) if b >= b0 && t > t0 && b - b0 <= t - t0 => Some((b - b0) as f64 * 100.0 / (t - t0) as f64),
            _ => None,
        };
        let raw = busy(self.base);
        let c = self.cfg.unwrap_or(GATE_DEFAULT);
        // the 60 s mean, unless the latest interval alone is at 100 % (veto) or no older reading exists
        let cpu = if c.mean && raw.is_some_and(|p| p < 100.0) { busy(self.prev).or(raw) } else { raw };
        let late = self.last.is_some_and(|l| now.saturating_sub(l.0) > 60);
        self.prev = if raw.is_some() { self.base } else { None };
        self.base = ticks;
        let memory = memory.filter(|m| (0.0..=100.0).contains(m));
        self.last = Some((now, wall, cpu, memory));
        self.sampled = true;
        let (max, min) = (c.cpu_max as f64, c.memory_min as f64);
        let floor = !c.mean || self.host_mem.is_some_and(|(g, l)| g >= FLOOR_GIB && l < PRESSURE_CRITICAL);
        match (cpu, memory) {
            (Some(p), Some(m)) if !late && floor && p < max && m > min => {
                if p <= max - 10.0 && m >= min + 5.0 {
                    self.good += 1;
                    self.open |= self.good >= 3;
                } else {
                    self.good = 0;
                }
            }
            _ => (self.open, self.good) = (false, 0),
        }
    }

    /// `off`, `open`, `held` or `unknown` with every blocker `(code, text)` (§ 4 `capacity`).
    pub fn state(&self, now: u64) -> (&'static str, Vec<(&'static str, String)>) {
        let Some(c) = self.cfg.filter(|c| c.enabled) else { return ("off", vec![]) };
        let mut r = vec![];
        match self.last {
            None => r.push(("sample", "no sample yet".to_string())),
            Some((at, ..)) if now.saturating_sub(at) > 60 => r.push(("sample", format!("last sample {} s old", now - at))),
            Some((_, _, cpu, mem)) => {
                match cpu {
                    None => r.push(("sample", "no CPU reading yet".into())),
                    Some(p) if p >= c.cpu_max as f64 => r.push(("cpu", format!("CPU {p:.0} % >= {} %", c.cpu_max))),
                    _ => {}
                }
                match mem {
                    None => r.push(("sample", "no memory reading".into())),
                    Some(m) if m <= c.memory_min as f64 => r.push(("memory", format!("available memory {m:.0} % <= {} %", c.memory_min))),
                    _ => {}
                }
                match self.host_mem.filter(|_| c.mean) {
                    None if c.mean => r.push(("sample", "no memory floor reading".into())),
                    Some((g, _)) if g < FLOOR_GIB => r.push(("memory", format!("available memory {g:.1} GiB < {FLOOR_GIB} GiB"))),
                    Some((_, l)) if l >= PRESSURE_CRITICAL => r.push(("memory", format!("memory pressure level {l} (critical)"))),
                    _ => {}
                }
            }
        }
        if r.is_empty() && !self.open {
            r.push(("cooldown", format!("{} of 3 calm samples", self.good)));
        }
        (if r.iter().any(|x| x.0 == "sample") { "unknown" } else if r.is_empty() { "open" } else { "held" }, r)
    }

    /// Off, or open with the previous launch at least 30 s ago and sampled since (§ 3).
    pub fn may_launch(&self, now: u64) -> bool {
        !self.enabled() || (self.state(now).0 == "open" && self.launched.is_none_or(|t| self.sampled && now.saturating_sub(t) >= 30))
    }

    pub fn launch(&mut self, now: u64) {
        (self.launched, self.sampled) = (Some(now), false);
    }
}

/// Append `(task, kind, attempt)` moves to the events file at board time `now` (§ 4): one line per attempt and kind,
/// lines older than 30 days dropped. Returns the first time covered.
pub fn record(path: &std::path::Path, now: i64, moves: &[(u64, &str, u64)]) -> Result<i64, String> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let mut lines = text.lines();
    let since = lines.next().and_then(|l| l.strip_prefix("since ")?.parse().ok()).unwrap_or(now).max(now - MONTH);
    let mut keep: Vec<String> = lines.filter(|l| l.split(' ').next().and_then(|t| t.parse::<i64>().ok()).is_some_and(|t| t >= since)).map(String::from).collect();
    for (n, kind, a) in moves {
        if !keep.iter().any(|l| l.ends_with(&format!(" {kind} {a}"))) {
            keep.push(format!("{now} #{n} {kind} {a}"));
        }
    }
    let out: String = std::iter::once(format!("since {since}")).chain(keep).map(|l| l + "\n").collect();
    if out != text {
        crate::platform::write_atomic(path, &out)?;
    }
    Ok(since)
}

impl<'a> Service<'a> {
    pub fn new(board: &'a dyn Board, cfg: &'a Config, version: &str, start: Box<dyn FnMut(&Launch) -> Result<Box<dyn Proc>, crate::runtime::StartError> + 'a>) -> Self {
        let cap = Ok(Capacity::fixed(cfg.limits.clone(), GATE_DEFAULT));
        Service { wall: 0, reserved: vec![], requested: vec![], refused: None, host_used: None, board, cfg, version: version.into(), kids: vec![], instance: String::new(), focus: vec![], focus_error: None, delivered: HashMap::new(), cache: HashMap::new(), label_fixes: HashMap::new(), pruned: HashSet::new(), start, #[cfg(test)] agent_status: json!([]), log: vec![], snapshot: None, tasks_read: None, host_issue: None, owner_comments: vec![], seen: HashMap::new(), events_since: None, cap, gate: Gate::default(), mono: 0, held: vec![], pending: BTreeSet::new(), hold: None, refused_access: BTreeMap::new(), unknown: HashMap::new() }
    }

    /// New capacity config (§ 3): a gate change starts held.
    pub fn reload(&mut self, cap: Result<Capacity, String>) {
        if let Ok(c) = &cap {
            self.gate.configure(c.gate);
        }
        self.cap = cap;
    }

    fn may_launch(&self) -> bool {
        self.hold.is_none() && self.cap.is_ok() && self.gate.may_launch(self.mono)
    }

    fn auto(&self) -> bool {
        self.cap.as_ref().is_ok_and(|c| c.auto)
    }

    /// This service's entry in the host authority: children and granted reservations are held.
    fn entry(&self) -> Entry {
        let held = self.kids.iter().map(|k| (k.task, k.attempt, k.rt.clone())).chain(self.reserved.iter().cloned()).collect();
        Entry { version: self.version.clone(), at: 0, held, requested: self.requested.clone() }
    }

    /// Admission of one launch (§ 3): the load gate, then under Auto a reservation from the host authority, which
    /// refuses with a reason; the request stays `requested`. Fixed admits as before.
    fn reserve(&mut self, task: u64, attempt: u64, rt: &str) -> bool {
        if !self.may_launch() {
            return false;
        }
        if !self.auto() {
            return true;
        }
        let old = crate::admission::old_services(&self.cfg.install);
        let res = (task, attempt, rt.to_string());
        let want = Want { max: self.cap.as_ref().ok().and_then(|c| c.max), sample_at: self.gate.last.map(|l| l.1), old: &old, res: res.clone() };
        let (r, used) = crate::admission::sync(&self.cfg.install, &self.cfg.root.display().to_string(), self.entry(), Some(want), self.wall);
        self.host_used = used;
        match r {
            Ok(()) => {
                self.reserved.push(res);
                true
            }
            Err(e) => {
                self.requested.push(res);
                self.refused = Some(e);
                false
            }
        }
    }

    /// Rewrite this service's host entry: released reservations leave it (§ 3). Fixed services write too, so Auto
    /// counts their children; a failed write only matters to Auto, whose next request refuses.
    pub fn sync_host(&mut self) {
        let (r, used) = crate::admission::sync(&self.cfg.install, &self.cfg.root.display().to_string(), self.entry(), None, self.wall);
        self.host_used = used;
        if let (Err((_, e)), true) = (r, self.auto()) {
            self.refused = Some(("host", e));
        }
    }

    /// The `capacity` object of `status.json` (§ 4).
    pub fn capacity_json(&self) -> Value {
        let (mut gate, mut reasons) = match &self.cap {
            Err(e) => ("unknown", vec![("config", e.clone())]),
            Ok(_) => self.gate.state(self.mono),
        };
        if let Some(h) = &self.hold {
            (gate, reasons) = ("held", [vec![("hold", h.clone())], reasons].concat());
        }
        if let Some((code, text)) = self.refused.clone().filter(|_| self.auto()) {
            gate = if code == "host" { "unknown" } else if gate == "open" { "held" } else { gate };
            reasons.push((code, text));
        }
        // § 4 Focus: an empty focus admits nothing, so it is a reason like any refusal (#974)
        if self.focus.is_empty() {
            reasons.push(("focus", self.focus_error.clone().unwrap_or_else(|| FOCUS_OFF.into())));
        }
        reasons.extend(self.refused_access.iter().map(|(rt, r)| ("access", format!("{rt}: {r}"))));
        let res = |r: &Res, state: &str| json!({"task": r.0, "attempt": r.1, "rt": r.2, "state": state});
        let reservations: Vec<Value> = self.kids.iter().map(|k| res(&(k.task, k.attempt, k.rt.clone()), "held"))
            .chain(self.reserved.iter().map(|r| res(r, "held"))).chain(self.requested.iter().map(|r| res(r, "requested"))).collect();
        let limits = self.cap.as_ref().map(|c| c.limits.clone()).unwrap_or_default();
        let mut used: BTreeMap<String, u32> = limits.keys().map(|k| (k.clone(), 0)).collect();
        for k in &self.kids {
            *used.entry(k.rt.clone()).or_default() += 1;
        }
        let last = self.gate.last.filter(|_| self.gate.enabled());
        json!({"limits": limits, "used": used, "load_gate": self.cap.as_ref().map(|c| c.json()["load_gate"].clone()).unwrap_or(Value::Null),
            "admission": self.cap.as_ref().map(|c| c.json()["admission"].clone()).unwrap_or(Value::Null), "host_used": self.host_used,
            "min_service_version": crate::admission::MIN_SERVICE, "reservations": reservations,
            "gate": gate, "reasons": reasons.iter().map(|(c, t)| json!({"code": c, "text": t})).collect::<Vec<_>>(),
            "cpu_percent": last.and_then(|l| l.2), "memory_available_percent": last.and_then(|l| l.3),
            "sample_at": last.map(|l| l.1), "sample_age_seconds": last.map(|l| self.mono.saturating_sub(l.0))})
    }

    /// Children of the previous process image after an update re-exec; their inbox counts as delivered.
    pub fn adopt(&mut self, kid: Kid, delivered: u64) {
        self.delivered.insert(kid.attempt, delivered);
        self.kids.push(kid);
    }

    pub fn delivered(&self, attempt: u64) -> u64 {
        self.delivered.get(&attempt).copied().unwrap_or(0)
    }

    fn post(&mut self, n: u64, body: String) -> Result<u64, String> {
        self.log.push(format!("#{n} {}", body.lines().next().unwrap_or_default()));
        self.cache.remove(&n);
        self.board.comment(n, &body)
    }

    fn heartbeat_body(&self) -> String {
        let mut lines = vec![format!("version {}", self.version)];
        if !self.instance.is_empty() {
            lines.push(format!("service {}", self.instance));
        }
        if self.focus.is_empty() {
            lines.push(format!("focus {}", if self.focus_error.is_some() { "unknown" } else { "off" }));
        }
        let kids: BTreeSet<_> = self.kids.iter().map(|k| (k.task, k.attempt)).chain(self.pending.iter().copied()).collect();
        lines.extend(kids.iter().map(|(t, a)| format!("#{t} {a}")));
        lines.join("\n")
    }

    /// One pass of § 4. Tasks written to in a step are left alone by later steps until the next pass re-reads them.
    pub fn pass(&mut self) -> Result<(), String> {
        crate::profile::measure("service.pass", || self.pass_inner())
    }

    fn pass_inner(&mut self) -> Result<(), String> {
        let previous_fixes = std::mem::take(&mut self.label_fixes);
        let me = self.cfg.me.clone();
        let ttl = self.cfg.ttl;
        let (views, hosts) = views(self.board, &mut self.cache)?;
        let now = self.board.now()?;
        self.snapshot = Some(crate::snapshot::from_views("host", self.board, self.cfg, now, &views, &hosts));
        self.tasks_read = Some(std::time::Instant::now());
        // § 4 one service per host name: another install's fresh heartbeat under this name holds every write (#933)
        let title = format!("taskq host {me}");
        let other = hosts.iter().filter(|h| h.title == title && now - h.updated < ttl).flat_map(|h| h.body.lines()).find_map(|l| l.strip_prefix("service ")).filter(|id| *id != self.instance);
        if let Some(id) = other {
            return Err(format!("another service {id} runs as host {me}; stop it or set TASKQ_HOST"));
        }
        self.owner_comments = views.iter().flat_map(|v| {
            core::owner_comments(v.issue.number, &v.log).into_iter().filter_map(|id| Some((v.issue.number, id, v.log.iter().find(|e| e.id == id)?.body.trim().to_string(), v.task.card.clone().unwrap_or_default())))
        }).collect();
        self.events(&views, now)?;
        let by_n: HashMap<u64, &View> = views.iter().map(|v| (v.issue.number, v)).collect();
        let mut touched = HashSet::new();

        self.hold = crate::admission::hold(&self.cfg.install);
        // 1. reap; a failed read keeps the child, unknown with its reason
        let mut exited = vec![];
        let unknown = &mut self.unknown;
        self.kids.retain_mut(|k| match k.proc.exited() {
            Ok(Some(code)) => {
                exited.push((k.task, k.attempt, code));
                unknown.remove(&k.attempt);
                false
            }
            Ok(None) => {
                unknown.remove(&k.attempt);
                true
            }
            Err(e) => {
                unknown.insert(k.attempt, e);
                true
            }
        });
        for (n, a, code) in exited {
            if let Some(v) = by_n.get(&n)
                && v.task.attempt == Some(a)
                && matches!(v.task.state, State::Doing | State::Review)
                && !core::handed_in(n, &v.log, a, a.max(self.delivered(a)))
            {
                self.post(n, core::entry("gone", &[], Some(a), &format!("exit {code}")))?;
                touched.insert(n);
            }
        }

        // A replaced, once-accepted attempt is ordinary cancellation, not a double claim.
        for k in &self.kids {
            if let Some(v) = by_n.get(&k.task) {
                let history: Vec<_> = v.log.iter().filter(|e| e.id <= k.attempt).cloned().collect();
                if core::fold(k.task, &history, false).attempt != Some(k.attempt)
                {
                    crate::incident::detect("invariant.double_attempt", concat!("src/service.rs:", line!()), &k.rt, &[], &["claim:rejected"]);
                }
            }
        }

        // 2. stop children of replaced attempts (R11)
        self.kids.retain_mut(|k| {
            let current = by_n.get(&k.task).is_some_and(|v| v.task.attempt == Some(k.attempt));
            if !current {
                k.proc.kill();
            }
            current
        });
        self.held.retain(|l| by_n.get(&l.task).is_some_and(|v| v.task.attempt == Some(l.attempt)));
        let kids: Vec<u64> = self.kids.iter().map(|k| k.attempt).collect();
        self.unknown.retain(|a, _| kids.contains(a));
        self.prune(&by_n);

        // 3. heartbeat
        let body = self.heartbeat_body();
        self.host_issue = hosts.iter().find(|h| h.title == title).map(|h| h.number);
        match hosts.iter().find(|h| h.title == title) {
            Some(h) if h.body.lines().filter(|l| !l.starts_with("at ")).eq(body.lines()) && now - h.updated < ttl / 3 => {}
            // a same-body edit leaves `updated_at` as is (GitHub), so the stamp keeps the heartbeat fresh
            Some(h) => self.board.edit(h.number, &format!("{body}\nat {now}"))?,
            None => {
                self.host_issue = Some(self.board.create(&title, &body, &[HOST_LABEL.to_string()])?);
            }
        }

        // 4. gone for attempts of dead hosts (§ 3)
        let mut leases: HashMap<u64, Vec<(u64, u64)>> = HashMap::new();
        for hb in hosts.iter().filter(|h| now - h.updated <= ttl) {
            for line in hb.body.lines() {
                let Some((n, a)) = line.strip_prefix('#').and_then(|l| l.split_once(' ')) else { continue };
                let (Ok(n), Ok(a)) = (n.parse::<u64>(), a.parse::<u64>()) else { continue };
                let prior = leases.entry(n).or_default();
                if prior.iter().any(|(host, attempt)| *host != hb.number && *attempt != a) {
                    crate::incident::detect("invariant.double_attempt", concat!("src/service.rs:", line!()), "none", &[], &["heartbeat:conflict"]);
                }
                prior.push((hb.number, a));
            }
        }
        for v in &views {
            let (n, t) = (v.issue.number, &v.task);
            let (Some(a), Some(h)) = (t.attempt, host_of(t)) else { continue };
            if h == me || !matches!(t.state, State::Doing | State::Review) || touched.contains(&n) {
                continue;
            }
            let young = v.comments.iter().filter(|c| c.id == a || t.inbox.contains(&c.id)).any(|c| now - c.created <= ttl);
            let dead = match hosts.iter().find(|i| i.title == format!("taskq host {h}")) {
                None => !young,
                Some(hb) => now - hb.updated > ttl || (!young && !hb.body.lines().any(|l| l == format!("#{n} {a}"))),
            };
            if dead {
                self.post(n, core::entry("gone", &[], Some(a), &format!("host {h} lease expired")))?;
                touched.insert(n);
            }
        }

        // 5. projected labels
        for v in &views {
            let n = v.issue.number;
            if touched.contains(&n) {
                continue;
            }
            let want = self.board.state_label(v.task.state.name())?;
            let mut stale: Vec<_> = v.issue.labels.iter().filter(|l| l.starts_with("q-") && **l != want).cloned().collect();
            stale.sort();
            let fix = !stale.is_empty() || !v.issue.labels.contains(&want);
            project(self.board, &v.issue, &v.task)?;
            if fix && v.task.state != State::Closed {
                // Adapter success includes assignment readback; one hand-moved label is expected.
                let count = previous_fixes.get(&n).filter(|(w, old, _)| w == &want && old == &stale)
                    .map_or(1, |(_, _, count)| count.saturating_add(1));
                self.label_fixes.insert(n, (want, stale, count));
                if count >= 3 {
                    crate::incident::detect("invariant.label_loop", concat!("src/service.rs:", line!()), "none", &[v.task.state.name()], &["passes:3"]);
                }
            }
            if v.task.state == State::Closed {
                touched.insert(n);
            }
        }

        // 6. route to own attempts, then admit within capacity; the gate holds launches, the lease stays (§ 3)
        let mut pending = BTreeSet::new();
        // a reservation lives within one pass: it becomes a child or a held claim, or is released here
        (self.reserved, self.requested, self.refused) = (vec![], vec![], None);
        for v in &views {
            let (n, t) = (v.issue.number, &v.task);
            let (Some(a), Some(h)) = (t.attempt, host_of(t)) else { continue };
            if h != me || !matches!(t.state, State::Doing | State::Review) || touched.contains(&n) || self.kids.iter().any(|k| k.attempt == a) {
                continue;
            }
            if let Some(i) = self.held.iter().position(|l| l.attempt == a) {
                let rt = self.held[i].rt.clone();
                if self.reserve(n, a, &rt) {
                    let l = self.held.remove(i);
                    self.launch(l)?;
                } else {
                    pending.insert((n, a));
                }
                continue;
            }
            let seen = self.delivered.get(&a).copied().unwrap_or(0);
            let new: Vec<&Comment> = v.comments.iter().filter(|c| t.inbox.contains(&c.id) && c.id > seen).collect();
            let Some(last) = new.last().map(|c| c.id) else {
                self.post(n, core::entry("gone", &[], Some(a), "no child on its host"))?;
                touched.insert(n);
                continue;
            };
            let (rt, session) = rt_of(t).unwrap_or(("", ""));
            let mut session = session.to_string();
            let mut l = Launch { task: n, title: v.issue.title.clone(), attempt: a, role: t.role.clone().unwrap_or_default(), rt: rt.into(), session: session.clone(), url: self.board.url(n), resume: None };
            if session == "-" || rt == "claude" {
                session = crate::runtime::codex_thread(&crate::runtime::log_path(&self.cfg.root, &l)).unwrap_or_else(|| if session == "-" { String::new() } else { session });
            }
            let text: Vec<String> = new.iter().map(|c| core::delivery(n, &v.log, c.id)).collect();
            l.session = session;
            l.resume = Some(text.join("\n\n"));
            if l.session.is_empty() {
                self.post(n, core::entry("gone", &[], Some(a), "session unknown"))?;
                touched.insert(n);
                continue;
            }
            if self.refused_access.contains_key(&l.rt) || !self.reserve(n, a, &l.rt) {
                pending.insert((n, a)); // the inbox stays undelivered
                continue;
            }
            self.delivered.insert(a, last);
            self.launch(l)?;
        }
        pending.extend(self.held.iter().map(|l| (l.task, l.attempt)));
        if pending != self.pending {
            self.pending = pending;
            self.beat()?;
        }
        if self.focus.is_empty() {
            return Ok(());
        }
        let user = self.board.me().unwrap_or_default();
        let mut queue: Vec<&View> = views
            .iter()
            .filter(|v| !touched.contains(&v.issue.number))
            .filter(|v| v.task.state == State::Ready || (v.task.state == State::Review && v.task.attempt.is_none()))
            .filter(|v| {
                if let Err(e) = crate::labels::validate(&v.issue.labels) { log!("#{}: {e}; not admitted", v.issue.number); return false; }
                true
            })
            .filter(|v| label(&v.issue, "host-").is_none_or(|h| h == me))
            .filter(|v| !v.issue.labels.iter().any(|l| l == "assignee-only") || v.issue.assignees.contains(&user))
            // `needs-gate`: every host of this version has gates; N-1 lacks them and leaves the task alone (§ 8)
            .filter(|v| v.issue.labels.iter().filter_map(|l| l.strip_prefix("needs-")).all(|x| x == "gate" || self.cfg.has.iter().any(|h| h == x)))
            .filter(|v| priority(&v.issue) == 0 || self.focus.iter().any(|f| f == "all" || crate::labels::matches(&v.issue.labels, f)))
            .collect();
        queue.sort_by_key(|v| (priority(&v.issue), v.issue.number));
        for v in queue {
            if !self.may_launch() {
                break;
            }
            let role = if v.task.state == State::Review { "reviewer" } else { "worker" };
            let rt = if role == "reviewer" { self.reviewer_free(v) } else { self.free(label(&v.issue, "run-").as_deref()) };
            let Some(rt) = rt else { continue };
            if !self.reserve(v.issue.number, 0, &rt) {
                break;
            }
            self.claim(v, role, &rt)?;
        }
        Ok(())
    }

    /// Moves to closed, `failed`, `ask` or `waiting` since the previous pass go to `<root>/.taskq/events` (§ 4); a task
    /// new since then counts from its first state. A task gone from the open list counts only when a fresh read shows it
    /// closed (R12).
    fn events(&mut self, views: &[View], now: i64) -> Result<(), String> {
        // the attempt current at the move; a `close` or `failed` that ended it leaves the last spawn
        let attempt = |t: &Task, c: &[Comment]| t.attempt.or_else(|| c.iter().rev().find(|c| c.trusted && c.body.starts_with("taskq: spawn ")).map(|c| c.id));
        let now_seen: HashMap<u64, State> = views.iter().map(|v| (v.issue.number, v.task.state)).collect();
        let first = self.seen.is_empty() && self.events_since.is_none();
        let mut ns: Vec<u64> = self.seen.keys().copied().collect();
        if !first {
            ns.extend(views.iter().map(|v| v.issue.number).filter(|n| !self.seen.contains_key(n)));
        }
        let mut moves = vec![];
        for n in ns {
            let (to, a) = match views.iter().find(|v| v.issue.number == n) {
                // `ask` and `waiting` without an attempt: the last comment, else the issue number
                Some(v) if matches!(v.task.state, State::Ask | State::Waiting) => (v.task.state, attempt(&v.task, &v.comments).or(v.comments.last().map(|c| c.id)).or(Some(n))),
                Some(v) => (v.task.state, attempt(&v.task, &v.comments)),
                None if self.board.issue(n).is_ok_and(|i| !i.open) => (State::Closed, attempt(&Task::default(), &self.board.comments(n)?)),
                None => continue,
            };
            let kind = match to {
                State::Closed => "close",
                State::Failed => "failed",
                State::Ask => "ask",
                State::Waiting => "blocked",
                _ => continue,
            };
            if let Some(a) = a.filter(|_| self.seen.get(&n) != Some(&to)) {
                moves.push((n, kind, a));
            }
        }
        moves.sort();
        self.seen = now_seen;
        if first || !moves.is_empty() || self.events_since.is_none_or(|s| s < now - MONTH) {
            let path = crate::dot(&self.cfg.root).join("events");
            match record(&path, now, &moves) {
                Ok(s) => self.events_since = Some(s),
                Err(e) => self.log.push(format!("events: {e}")),
            }
        }
        for (n, kind, a) in moves {
            self.log.push(format!("#{n} event {kind} {a}"));
        }
        Ok(())
    }

    fn prune(&mut self, open: &HashMap<u64, &View>) {
        let tried = &mut self.pruned;
        let kids: Vec<u64> = self.kids.iter().map(|k| k.task).collect();
        let skip = |n: u64| open.contains_key(&n) || kids.contains(&n) || !tried.insert(n);
        self.log.extend(cleanup(self.board, &self.cfg.root, skip, false));
    }

    fn reviewer_free(&self, v: &View) -> Option<String> {
        let mut log = v.log.clone();
        log.sort_by_key(|e| e.id);
        // Replay candidate worker spawns so losing, edited and stale entries cannot choose the reviewer.
        let worker = log.iter().enumerate().rev()
            .filter(|(_, e)| e.body.split_whitespace().take(3).eq(["taskq:", "spawn", "worker"]))
            .find_map(|(i, e)| {
                let t = core::fold(v.issue.number, &log[..=i], false);
                if t.attempt == Some(e.id) { rt_of(&t).map(|(rt, _)| rt.to_string()) } else { None }
            })?;
        let other = match worker.as_str() {
            "claude" => "codex",
            "codex" | "codex-app" => "claude",
            _ => return None,
        };
        self.free(Some(other)).or_else(|| self.free(Some(&worker))
            .filter(|rt| rt != "codex-app" || label(&v.issue, "run-").as_deref() == Some("codex-app")))
    }

    /// A runtime with free capacity: `limits.<rt>` minus live children of it (§ 3).
    /// `codex-app` only for `run-codex-app` tasks on a host whose `has` lists it (§ 9 opt-in).
    fn free(&self, want: Option<&str>) -> Option<String> {
        let app = |rt: &str| rt != "codex-app" || (cfg!(target_os = "macos") && want == Some(rt) && self.cfg.has.iter().any(|h| h == rt));
        let c = self.cap.as_ref().ok()?;
        // Auto: a runtime without a limit has no count ceiling (§ 9); Fixed: only listed runtimes
        let limits: Vec<(String, u32)> = if c.auto { RUNTIMES.iter().map(|r| (r.to_string(), c.limits.get(*r).copied().unwrap_or(u32::MAX))).collect() } else { c.limits.clone().into_iter().collect() };
        limits.into_iter().filter(|(rt, _)| want.is_none_or(|w| w == rt.as_str()) && app(rt) && !self.refused_access.contains_key(rt)).find(|(rt, n)| self.kids.iter().filter(|k| &k.rt == rt).count() < *n as usize).map(|(rt, _)| rt)
    }

    /// Post `spawn`, re-read, start the child only if `fold` accepted this spawn (§ 3 claim).
    fn claim(&mut self, v: &View, role: &str, rt: &str) -> Result<(), String> {
        match crate::profile::measure("dispatch", || self.claim_inner(v, role, rt))? {
            Some(l) => self.launch(l),
            None => Ok(()),
        }
    }

    fn claim_inner(&mut self, v: &View, role: &str, rt: &str) -> Result<Option<Launch>, String> {
        let n = v.issue.number;
        let session = crate::runtime::new_session(rt);
        let id = self.post(n, core::entry("spawn", &[role, &format!("{rt}:{session}"), &self.cfg.me], None, ""))?;
        let comments = self.board.comments(n)?;
        if !comments.iter().any(|c| c.id == id) {
            crate::incident::detect("adapter.read_after_write", concat!("src/service.rs:", line!()), rt, &[], &["spawn:missing"]);
        }
        let log: Vec<_> = comments.into_iter().filter(|c| c.trusted).map(|c| core::Entry { id: c.id, body: c.body, edited: c.edited }).collect();
        let t = core::fold(n, &log, false); // only ready or review tasks are claimed
        if t.attempt != Some(id) {
            self.reserved.retain(|r| r.0 != n); // released
            self.log.push(format!("#{n} spawn {id} lost the claim"));
            return Ok(None);
        }
        let l = Launch { task: n, title: v.issue.title.clone(), attempt: id, role: role.into(), rt: rt.into(), session, url: self.board.url(n), resume: None };
        self.delivered.insert(id, t.inbox.last().copied().unwrap_or(0));
        if !self.may_launch() {
            self.pending.insert((n, id));
            self.held.push(l);
            self.beat()?;
            return Ok(None);
        }
        Ok(Some(l))
    }

    /// Write the heartbeat now: the lease of a new child or held launch is listed before the next pass judges it.
    fn beat(&self) -> Result<(), String> {
        match self.host_issue {
            Some(n) => self.board.edit(n, &self.heartbeat_body()),
            None => Ok(()),
        }
    }

    fn launch(&mut self, l: Launch) -> Result<(), String> {
        // the reservation becomes the child, or is released when the start fails
        self.reserved.retain(|r| r.0 != l.task);
        match (self.start)(&l) {
            Ok(proc) => {
                self.kids.push(Kid { task: l.task, attempt: l.attempt, rt: l.rt.clone(), proc });
                self.pending.remove(&(l.task, l.attempt));
                self.gate.launch(self.mono);
                self.beat()
            }
            Err(e) => {
                if e.reportable {
                    #[cfg(not(test))]
                    let agents = crate::runtime::agents();
                    #[cfg(test)]
                    let agents = self.agent_status.clone();
                    if agents.as_array().is_some_and(|rows| rows.iter().any(|r| r["runtime"] == l.rt && r["path"].is_string() && r["login"] == "logged in")) {
                        crate::incident::detect("dispatch.start", concat!("src/service.rs:", line!()), &l.rt, &[], &["runtime:installed"]);
                    }
                }
                self.post(l.task, core::entry("gone", &[], Some(l.attempt), &format!("start failed: {e}"))).map(|_| ())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::{Board, Fake, entries};
    use std::cell::RefCell;
    use std::rc::Rc;

    struct FakeProc(Rc<RefCell<Option<i32>>>);
    impl Proc for FakeProc {
        fn exited(&mut self) -> Result<Option<i32>, String> {
            // -999: a failed read
            match *self.0.borrow() {
                Some(-999) => Err("read failed".into()),
                c => Ok(c),
            }
        }
        fn kill(&mut self) {
            *self.0.borrow_mut() = Some(-9);
        }
        fn id(&self) -> u32 {
            0
        }
    }

    fn cfg(me: &str) -> Config {
        Config {
            root: PathBuf::from("/nonexistent"),
            install: PathBuf::from("/nonexistent"),
            board: &crate::board::github::KIND,
            web_host: String::new(),
            repo: "o/r".into(),
            publish: "pr".into(),
            delivery: "auto-merge".into(),
            delivery_revision: 0,
            me: me.into(),
            limits: BTreeMap::from([("claude".to_string(), 1)]),
            has: vec![],
            permission_mode: "dontAsk".into(),
            codex: vec![],
            ttl: 300,
            poll: 60,
            update_every: 900,
            rollback_after: 600,
        }
    }

    type Exits = Rc<RefCell<Vec<(u64, Rc<RefCell<Option<i32>>>)>>>;

    fn service<'a>(b: &'a Fake, c: &'a Config, exits: Exits) -> Service<'a> {
        let mut s = Service::new(b, c, "4.0.0", Box::new(move |l: &Launch| {
            let flag = Rc::new(RefCell::new(None));
            exits.borrow_mut().push((l.task, flag.clone()));
            Ok(Box::new(FakeProc(flag)) as Box<dyn Proc>)
        }));
        s.focus = vec!["all".into()];
        s
    }

    fn task(b: &Fake, n: u64) -> Task {
        core::fold(n, &entries(b, n).unwrap(), false)
    }

    // Chain: § 3 cross-host grace. Risk: ignored entries extend another host's lease.
    #[test]
    fn cross_host_grace_uses_only_accepted_inbox_entries() {
        for (entry, trusted, edited, grace) in [
            ("taskq: answer 1", true, false, true),
            ("taskq: steer\ncontinue", true, false, true),
            ("taskq: steer attempt 999999\ncontinue", true, false, false),
            ("taskq: steer\ncontinue", false, false, false),
            ("taskq: steer\ncontinue", true, true, false),
            ("unrelated comment", true, false, false),
        ] {
            let b = Fake::default();
            let n = b.create("T", "", &["taskq".into()]).unwrap();
            let a = b.comment(n, &core::entry("spawn", &["worker", "claude:session", "win"], None, "")).unwrap();
            if entry.starts_with("taskq: answer") {
                b.comment(n, &core::entry("ask", &[], Some(a), "Which?\n1. yes\n2. no")).unwrap();
            }
            b.advance(400);
            b.raw(n, entry, trusted, edited);
            let hb = b.create("taskq host win", "version 4.0.0", &[HOST_LABEL.into()]).unwrap();
            let c = cfg("mac");
            let mut s = service(&b, &c, Default::default());
            s.focus.clear();
            s.pass().unwrap();
            assert_eq!(task(&b, n).attempt, grace.then_some(a), "{entry}, trusted={trusted}, edited={edited}");
            if grace {
                b.advance(301);
                b.edit(hb, "version 4.0.0\nat fresh").unwrap();
                s.pass().unwrap();
                assert_eq!(task(&b, n).attempt, None, "inbox grace expires despite a fresh unlisted heartbeat");
            }
        }
    }

    // Chain: § 3 answered attempt across hosts. Risk: another host steals an answered attempt.
    #[test]
    fn answered_attempt_survives_other_host_until_owner_resumes() {
        let b = Fake::default();
        let n = b.create("T", "", &["taskq".into()]).unwrap();
        let (win, mac) = (cfg("win"), cfg("mac"));
        let exits: Exits = Default::default();
        let mut owner = service(&b, &win, exits.clone());
        let mut other = service(&b, &mac, Default::default());
        owner.pass().unwrap();
        let a = task(&b, n).attempt.unwrap();
        b.comment(n, &core::entry("ask", &[], Some(a), "Which?\n1. yes\n2. no")).unwrap();
        *exits.borrow()[0].1.borrow_mut() = Some(0);
        b.advance(400);
        owner.pass().unwrap();
        b.comment(n, &core::entry("answer", &["1"], None, "yes")).unwrap();
        other.pass().unwrap();
        assert_eq!(task(&b, n).attempt, Some(a));
        owner.pass().unwrap();
        assert_eq!((task(&b, n).attempt, exits.borrow().len()), (Some(a), 2));
        b.advance(301);
        other.pass().unwrap();
        assert_eq!(task(&b, n).attempt, None, "an accepted inbox entry does not protect a stale host");
    }

    // Chain: incident worker start. Risk: missing runtime or login reported as a fault.
    #[test]
    fn incident_start_requires_installed_logged_in_runtime_and_non_environment_failure() {
        use std::io::{Error, ErrorKind};
        for (path, login, kind, reports) in [(Some("fake"), "logged in", ErrorKind::Other, true),
            (None, "not installed", ErrorKind::Other, false), (Some("fake"), "not logged in", ErrorKind::Other, false),
            (Some("fake"), "unknown", ErrorKind::Other, false), (Some("fake"), "logged in", ErrorKind::OutOfMemory, false),
            (Some("fake"), "logged in", ErrorKind::PermissionDenied, false)] {
            crate::incident::take_detected();
            let b = Fake::default();
            let n = b.create("task", "", &[TASK_LABEL.into()]).unwrap();
            let c = cfg("local");
            let mut s = Service::new(&b, &c, "4.0.0", Box::new(move |_| Err(crate::runtime::StartError::io(Error::new(kind, "private failure text"), None))));
            s.agent_status = json!([{"runtime": "claude", "path": path, "login": login}]);
            let v = views(&b, &mut HashMap::new()).unwrap().0.remove(0);
            s.claim(&v, "worker", "claude").unwrap();
            assert_eq!(task(&b, n).state, State::Ready);
            assert!(b.comments(n).unwrap().last().unwrap().body.contains("start failed: private failure text"));
            assert_eq!(crate::incident::take_detected(), if reports { vec!["dispatch.start"] } else { vec![] });
        }
    }

    // Chain: incident lease heartbeat. Risk: one slow attempt reported as a fault.
    #[test]
    fn incident_heartbeats_require_fresh_distinct_attempts() {
        for (other_attempt, age, expected) in [(10, 0, false), (11, 0, true), (11, 301, false)] {
            crate::incident::take_detected();
            let b = Fake::default();
            let old = b.create("taskq host other", &format!("#99 {other_attempt}"), &[HOST_LABEL.into()]).unwrap();
            b.advance(age);
            b.create("taskq host first", "#99 10", &[HOST_LABEL.into()]).unwrap();
            let c = cfg("local");
            let mut s = service(&b, &c, Rc::new(RefCell::new(vec![])));
            s.pass().unwrap();
            assert_eq!(crate::incident::take_detected(), if expected { vec!["invariant.double_attempt"] } else { vec![] }, "heartbeat {old}");
        }
    }

    // Chain: incident rejected child. Risk: a replaced child reported as rejected.
    #[test]
    fn incident_rejected_child_excludes_replaced_accepted_child() {
        for rejected in [false, true] {
            crate::incident::take_detected();
            let b = Fake::default();
            let n = b.create("task", "", &[TASK_LABEL.into()]).unwrap();
            let accepted = b.comment(n, "taskq: spawn worker claude:first local").unwrap();
            let lost = b.comment(n, "taskq: spawn worker claude:second local").unwrap();
            b.comment(n, "taskq: requeue").unwrap();
            let c = cfg("local");
            let mut s = service(&b, &c, Rc::new(RefCell::new(vec![])));
            s.focus.clear();
            let flag = Rc::new(RefCell::new(None));
            s.kids.push(Kid { task: n, attempt: if rejected { lost } else { accepted }, rt: "claude".into(), proc: Box::new(FakeProc(flag.clone())) });
            s.pass().unwrap();
            assert_eq!(*flag.borrow(), Some(-9));
            assert_eq!(crate::incident::take_detected(), if rejected { vec!["invariant.double_attempt"] } else { vec![] });
        }
    }

    // Chain: incident claim readback. Risk: normal contention reported as a fault.
    #[test]
    fn incident_claim_missing_readback_excludes_normal_contention() {
        for missing in [false, true] {
            crate::incident::take_detected();
            let b = Fake::default();
            let n = b.create("task", "", &[TASK_LABEL.into()]).unwrap();
            let v = views(&b, &mut HashMap::new()).unwrap().0.remove(0);
            if !missing { b.comment(n, "taskq: spawn worker claude:first other").unwrap(); }
            b.state.borrow_mut().hide_latest_comment = missing;
            let c = cfg("local");
            let mut s = service(&b, &c, Rc::new(RefCell::new(vec![])));
            s.claim(&v, "worker", "claude").unwrap();
            assert!(s.kids.is_empty());
            assert_eq!(crate::incident::take_detected(), if missing { vec!["adapter.read_after_write"] } else { vec![] });
        }
    }

    // Chain: incident label loop. Risk: one relabel reported as a loop.
    #[test]
    fn incident_label_loop_requires_consecutive_matching_successful_fixes() {
        crate::incident::take_detected();
        let b = Fake::default();
        let n = b.create("task", "", &[TASK_LABEL.into(), "q-doing".into()]).unwrap();
        let c = cfg("local");
        let mut s = service(&b, &c, Rc::new(RefCell::new(vec![])));
        s.focus.clear();
        for pass in 1..=3 {
            b.labels(n, &["q-doing".into()], &["q-ready".into()]).unwrap();
            s.pass().unwrap();
            assert!(b.issue(n).unwrap().labels.contains(&"q-ready".into()));
            assert_eq!(crate::incident::take_detected(), if pass == 3 { vec!["invariant.label_loop"] } else { vec![] });
        }
        s.pass().unwrap(); // no fix breaks the streak
        b.labels(n, &["q-doing".into()], &["q-ready".into()]).unwrap();
        s.pass().unwrap();
        assert!(crate::incident::take_detected().is_empty());
        b.labels(n, &["q-ask".into()], &["q-ready".into()]).unwrap();
        s.pass().unwrap(); // different fix also breaks the streak
        assert!(crate::incident::take_detected().is_empty());
    }

    // Chain: § 4 one service per host name (#933). Risk: a second install under the same host name posts `gone` for
    // the first one's live child and respawns, so two workers run one task.
    #[test]
    fn a_second_service_under_one_host_name_writes_nothing_and_the_live_child_stays() {
        let b = Fake::default();
        let n = b.create("T", "## Goal\ng", &["taskq".into()]).unwrap();
        let c = cfg("win");
        let (ea, eb): (Exits, Exits) = Default::default();
        let mut a = service(&b, &c, ea.clone());
        let mut w = service(&b, &c, eb.clone());
        (a.instance, w.instance) = ("native".into(), "wsl".into());
        a.pass().unwrap();
        let comments = b.comments(n).unwrap().len();
        for _ in 0..3 {
            b.advance(70);
            assert!(w.pass().unwrap_err().contains("another service native runs as host win"));
            a.pass().unwrap();
        }
        let t = task(&b, n);
        assert_eq!((t.state, t.retries, a.kids.len(), w.kids.len(), eb.borrow().len()), (State::Doing, 0, 1, 0, 0));
        assert_eq!(b.comments(n).unwrap().len(), comments, "no gone, no second spawn");
        // a restart of the same install reads its own id and carries on; the first one stopped, the heartbeat goes
        // stale and the other install takes over the host name
        let mut again = service(&b, &c, Default::default());
        again.instance = "native".into();
        again.pass().unwrap();
        b.advance(301);
        assert!(w.pass().is_ok());
    }

    // Chain: § 4 board budget, heartbeat. Risk: a PATCH every pass burns the shared budget, or a sparse heartbeat
    // lets another host expire a live lease.
    #[test]
    fn idle_heartbeat_writes_every_third_of_ttl_and_stays_fresh() {
        let b = Fake::default();
        let c = cfg("mac");
        let mut s = service(&b, &c, Default::default());
        let beat = |b: &Fake| b.issues(None).unwrap().into_iter().find(|i| i.title == "taskq host mac").map(|i| i.updated).unwrap();
        s.pass().unwrap();
        let (mut writes, mut last) = (0, beat(&b));
        for _ in 0..60 {
            b.advance(60); // default poll
            assert!(b.now().unwrap() - beat(&b) < c.ttl, "lease stays fresh between passes");
            s.pass().unwrap();
            if beat(&b) != last { writes += 1; last = beat(&b); }
        }
        assert_eq!(writes, 30, "one write per 2 passes at poll 60, ttl 300; was 60 at ttl / 5");
    }

    // Chain: R2 § 3 two hosts one attempt. Risk: two attempts run, or a dead host blocks the task.
    #[test]
    fn two_hosts_admit_one_attempt_and_the_survivor_recovers_a_dead_host() {
        let b = Fake::default();
        let n = b.create("T", "## Goal\ng", &["taskq".into()]).unwrap();
        let (ca, cb) = (cfg("mac"), cfg("win"));
        let (ea, eb): (Exits, Exits) = Default::default();
        let mut a = service(&b, &ca, ea.clone());
        let mut w = service(&b, &cb, eb.clone());
        a.pass().unwrap();
        w.pass().unwrap();
        let t = task(&b, n);
        assert_eq!((t.state, t.role.as_deref()), (State::Doing, Some("worker")));
        assert_eq!((a.kids.len(), w.kids.len()), (1, 0));
        assert!(b.issue(n).unwrap().labels.contains(&"q-doing".to_string()));

        // a live mac outlasts the ttl: its unchanged heartbeat stays fresh, win leaves the task alone
        for _ in 0..3 {
            b.advance(200);
            a.pass().unwrap();
            w.pass().unwrap();
        }
        assert_eq!(host_of(&task(&b, n)), Some("mac"));
        // the mac host dies: win waits out the ttl, then posts gone and takes the task
        b.advance(301);
        w.pass().unwrap();
        assert_eq!(task(&b, n).state, State::Ready);
        w.pass().unwrap();
        let t = task(&b, n);
        assert_eq!((t.state, host_of(&t)), (State::Doing, Some("win")));
        assert_eq!(t.retries, 1);
    }

    // Chain: § 7 worker exit to review. Risk: a silent exit is not gone; a result gets no reviewer.
    #[test]
    fn exit_without_result_is_gone_and_result_brings_a_reviewer() {
        let b = Fake::default();
        let n = b.create("T", "", &["taskq".into()]).unwrap();
        let c = cfg("mac");
        let e: Exits = Default::default();
        let mut s = service(&b, &c, e.clone());
        s.pass().unwrap();
        let a = task(&b, n).attempt.unwrap();
        b.comment(n, &core::entry("result", &["0123456789abcdef0123456789abcdef01234567"], Some(a), "ok")).unwrap();
        *e.borrow()[0].1.borrow_mut() = Some(0);
        s.pass().unwrap();
        let t = task(&b, n);
        assert_eq!((t.state, t.role.as_deref(), t.retries), (State::Review, Some("reviewer"), 0));
        let r = t.attempt.unwrap();
        *e.borrow()[1].1.borrow_mut() = Some(1);
        s.pass().unwrap(); // reviewer exits without a verdict: gone keeps the result
        let t = task(&b, n);
        assert_eq!((t.state, t.retries, t.result.is_some()), (State::Review, 1, true));
        assert_ne!(t.attempt, Some(r));
    }

    // Chain: § 4 worker result to reviewer claim. Risk: run-* pins reviewers or capacity fallback stalls review.
    #[test]
    fn reviewer_prefers_other_runtime_and_falls_back_when_unavailable() {
        for (worker, other, capacity, expected) in [
            ("codex", "claude", 1, "claude"),
            ("codex", "claude", 0, "codex"),
            ("claude", "codex", 1, "codex"),
            ("claude", "codex", 0, "claude"),
            ("codex-app", "claude", 1, "claude"),
            ("codex-app", "claude", 0, "codex-app"),
        ] {
            let b = Fake::default();
            let n = b.create("T", "", &["taskq".into(), format!("run-{worker}")]).unwrap();
            let mut c = cfg("mac");
            c.limits = BTreeMap::from([(worker.into(), 1), (other.into(), capacity)]);
            c.has = vec!["codex-app".into()];
            let e: Exits = Default::default();
            let mut s = service(&b, &c, e.clone());
            s.pass().unwrap();
            let t = task(&b, n);
            if worker == "codex-app" && !cfg!(target_os = "macos") {
                assert_eq!(t.state, State::Ready, "§ 9: app workers are macOS-only");
                assert!(t.attempt.is_none() && e.borrow().is_empty(), "no app worker claimed or started");
                continue;
            }
            assert_eq!(rt_of(&t).unwrap().0, worker);
            // A losing spawn must not change which runtime actually did the work.
            b.comment(n, &core::entry("spawn", &["worker", &format!("{other}:ignored"), "mac"], None, "")).unwrap();
            b.comment(n, &core::entry("result", &["0123456789abcdef0123456789abcdef01234567"], t.attempt, "ok")).unwrap();
            *e.borrow()[0].1.borrow_mut() = Some(0);
            for retry in 0..2 {
                s.pass().unwrap();
                let t = task(&b, n);
                assert_eq!(t.role.as_deref(), Some("reviewer"));
                assert_eq!(rt_of(&t).unwrap().0, expected, "{worker}, capacity {capacity}, retry {retry}");
                if retry == 0 {
                    *e.borrow().last().unwrap().1.borrow_mut() = Some(1);
                    s.pass().unwrap(); // gone ends the reviewer; the next pass claims its replacement
                    assert_eq!(task(&b, n).attempt, None);
                }
            }
        }
    }

    // Chain: § 2 answer and requeue on a live child. Risk: a new session on answer; requeue leaves the child running.
    #[test]
    fn answer_resumes_the_same_session_and_requeue_stops_the_child() {
        let b = Fake::default();
        let n = b.create("T", "", &["taskq".into()]).unwrap();
        let mut c = cfg("mac");
        c.root = std::env::temp_dir().join(format!("taskq-session-{}", crate::runtime::uuid()));
        std::fs::create_dir_all(c.root.join(".taskq")).unwrap();
        let e: Exits = Default::default();
        let sessions = Rc::new(RefCell::new(Vec::new()));
        let seen = sessions.clone();
        let exits = e.clone();
        let mut s = Service::new(&b, &c, "4.0.0", Box::new(move |l| {
            seen.borrow_mut().push(l.session.clone());
            let flag = Rc::new(RefCell::new(None));
            exits.borrow_mut().push((l.task, flag.clone()));
            Ok(Box::new(FakeProc(flag)))
        }));
        s.focus = vec!["all".into()];
        s.pass().unwrap();
        let a = task(&b, n).attempt.unwrap();
        std::fs::write(c.root.join(format!(".taskq/T{n}-{a}.log")), "{\"type\":\"thread.started\",\"thread_id\":\"native-session\"}\n").unwrap();
        b.comment(n, &core::entry("ask", &[], Some(a), "Which?\n1. x\n2. y ★")).unwrap();
        *e.borrow()[0].1.borrow_mut() = Some(0);
        s.pass().unwrap();
        assert_eq!(task(&b, n).state, State::Ask);
        // no PM: the service alone finds the owner's free text; an untrusted author's comment is not one
        let id = b.raw(n, "prefer z", true, false);
        b.raw(n, "do x", false, false);
        s.pass().unwrap();
        assert_eq!(s.owner_comments, [(n, id, "prefer z".to_string(), "Which?\n1. x\n2. y ★".to_string())]);
        assert_eq!((task(&b, n).state, e.borrow().len()), (State::Ask, 1), "a free comment moves nothing");
        b.comment(n, &core::entry("answer", &["2"], None, "y")).unwrap();
        s.pass().unwrap();
        assert!(s.owner_comments.is_empty(), "answered: no owner comment left");
        assert_eq!(e.borrow().len(), 2, "resumed once");
        assert_eq!(sessions.borrow()[1], "native-session", "resume resolves the pre-launch alias");
        let t = task(&b, n);
        assert_eq!((t.state, t.attempt), (State::Doing, Some(a)));
        s.pass().unwrap();
        assert_eq!(e.borrow().len(), 2, "an answer is delivered once");
        b.comment(n, &core::entry("requeue", &[], None, "redo")).unwrap();
        s.focus = vec![];
        s.pass().unwrap();
        assert_eq!(*e.borrow()[1].1.borrow(), Some(-9));
        assert!(s.kids.is_empty());
        std::fs::remove_dir_all(&c.root).unwrap();
    }

    // Chain: § 8 Access refusal. Risk: a refused runtime is claimed (phantom attempt, retries) or resumed, or its
    // reason is missing from status.
    #[test]
    fn refused_access_claims_and_resumes_nothing() {
        let b = Fake::default();
        let mut c = cfg("mac");
        c.limits = BTreeMap::from([("claude".to_string(), 2)]);
        let n1 = b.create("A", "", &["taskq".into()]).unwrap();
        let e: Exits = Default::default();
        let mut s = service(&b, &c, e.clone());
        s.pass().unwrap();
        let a1 = task(&b, n1).attempt.unwrap();
        b.comment(n1, &core::entry("ask", &[], Some(a1), "Which?\n1. x\n2. y ★")).unwrap();
        *e.borrow()[0].1.borrow_mut() = Some(0);
        s.pass().unwrap();
        s.refused_access.insert("claude".into(), "workspace enforcement unknown for claude on windows".into());
        b.comment(n1, &core::entry("answer", &["1"], None, "x")).unwrap();
        let n2 = b.create("B", "", &["taskq".into()]).unwrap();
        s.pass().unwrap();
        assert_eq!((task(&b, n2).state, task(&b, n2).attempt, task(&b, n1).state, e.borrow().len()), (State::Ready, None, State::Doing, 1));
        assert!(!b.comments(n2).unwrap().iter().any(|c| c.body.starts_with("taskq: spawn")), "no phantom attempt");
        let cap = s.capacity_json();
        assert_eq!((cap["reasons"][0]["code"].as_str(), cap["reasons"][0]["text"].as_str()), (Some("access"), Some("claude: workspace enforcement unknown for claude on windows")));
        s.refused_access.clear();
        s.pass().unwrap();
        assert_eq!((task(&b, n2).state, e.borrow().len()), (State::Doing, 3), "granted: resume and claim");
    }

    // Chain: § 3 emergency hold, § 4 reconciliation and recovery. Risk: a held host launches, an unknown read is
    // marked gone, or a recovery op is applied twice or over a newer revision.
    #[test]
    fn emergency_hold_refuses_launches_and_recovery_fences_attempts() {
        use crate::dispatch::{proposal, recover};
        let b = Fake::default();
        let dir = std::env::temp_dir().join(format!("taskq-hold-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut c = cfg("mac");
        (c.install, c.root, c.limits) = (dir.clone(), dir.clone(), BTreeMap::from([("claude".to_string(), 3)]));
        let (n1, n2) = (b.create("A", "", &["taskq".into()]).unwrap(), b.create("B", "", &["taskq".into()]).unwrap());
        let e: Exits = Default::default();
        let mut s = service(&b, &c, e.clone());
        s.pass().unwrap();
        let (a1, a2) = (task(&b, n1).attempt.unwrap(), task(&b, n2).attempt.unwrap());
        // hold: a new task is not claimed, an answer is not resumed; children keep running
        std::fs::write(dir.join("hold"), "drill\n").unwrap();
        let n3 = b.create("C", "", &["taskq".into()]).unwrap();
        *e.borrow()[1].1.borrow_mut() = Some(0);
        b.comment(n2, &core::entry("ask", &[], Some(a2), "Which?\n1. x\n2. y ★")).unwrap();
        s.pass().unwrap();
        b.comment(n2, &core::entry("answer", &["1"], None, "x")).unwrap();
        *e.borrow()[0].1.borrow_mut() = Some(-999); // n1's liveness read fails
        s.pass().unwrap();
        assert_eq!((task(&b, n3).state, task(&b, n2).state, e.borrow().len()), (State::Ready, State::Doing, 2), "held: nothing launched");
        let cap = s.capacity_json();
        assert_eq!((cap["gate"].as_str(), cap["reasons"][0]["code"].as_str(), cap["reasons"][0]["text"].as_str()), (Some("held"), Some("hold"), Some("drill")));
        // reconciliation: an unknown read keeps the child and the attempt; no gone, no op
        assert_eq!((task(&b, n1).state, task(&b, n1).attempt, s.kids.len()), (State::Doing, Some(a1), 1));
        let kids = |s: &Service| Ok(s.kids.iter().map(|k| json!({"attempt": k.attempt, "pid": 1, "live": if s.unknown.contains_key(&k.attempt) { "unknown" } else { "running" }, "reason": s.unknown.get(&k.attempt)})).collect());
        let rows = proposal(&b, "mac", &kids(&s)).unwrap();
        assert_eq!(rows.iter().map(|r| (r.task, r.live.as_str(), r.revision)).collect::<Vec<_>>(), [(n1, "unknown (read failed)", None), (n2, "no child", Some(core::revision(n2, &entries(&b, n2).unwrap())))]);
        assert!(proposal(&b, "mac", &Err("status.json 90 s old".into())).unwrap().iter().all(|r| r.revision.is_none()), "stale status: all unknown");
        // stale recovery: the worker writes after the proposal; the op is refused and the board is unchanged
        let before = b.comments(n2).unwrap().len();
        b.comment(n2, &core::entry("ask", &[], Some(a2), "Again?\n1. x\n2. y ★")).unwrap();
        let r = recover(&b, &dir, &rows);
        assert_eq!((r.len(), r[0].outcome, b.comments(n2).unwrap().len()), (1, "stale", before + 1));
        // a fresh proposal once n1 reads running: one receipt per op, a repeated apply writes nothing more
        *e.borrow()[0].1.borrow_mut() = None;
        s.pass().unwrap();
        let rows = proposal(&b, "mac", &kids(&s)).unwrap();
        let r = recover(&b, &dir, &rows);
        assert_eq!(r.iter().map(|r| (r.task, r.outcome, r.state)).collect::<Vec<_>>(), [(n1, "accepted", Some("ready"))], "n2 asks: not interrupted");
        let count = b.comments(n1).unwrap().len();
        assert!(recover(&b, &dir, &rows).iter().all(|r| r.ok()) && b.comments(n1).unwrap().len() == count, "retried op ids");
        // the fenced child is stopped (R11); still held, nothing relaunches
        s.pass().unwrap();
        assert_eq!((*e.borrow()[0].1.borrow(), s.kids.len(), e.borrow().len()), (Some(-9), 0, 2));
        // release: admission resumes
        std::fs::remove_file(dir.join("hold")).unwrap();
        s.pass().unwrap();
        assert!(task(&b, n1).attempt.is_some_and(|a| a != a1) && task(&b, n3).state == State::Doing);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // Chain: § 2 early answer. Risk: an early answer marks the attempt gone.
    #[test]
    fn answer_before_the_asking_child_exits_resumes_without_gone() {
        let b = Fake::default();
        let n = b.create("T", "", &["taskq".into()]).unwrap();
        let c = cfg("mac");
        let e: Exits = Default::default();
        let mut s = service(&b, &c, e.clone());
        s.pass().unwrap();
        let a = task(&b, n).attempt.unwrap();
        b.comment(n, &core::entry("ask", &[], Some(a), "Which?\n1. x\n2. y ★")).unwrap();
        b.comment(n, &core::entry("answer", &["2"], None, "y")).unwrap();
        *e.borrow()[0].1.borrow_mut() = Some(0);
        s.pass().unwrap();
        let t = task(&b, n);
        assert_eq!((t.state, t.attempt, t.retries, e.borrow().len()), (State::Doing, Some(a), 0, 2), "resumed, no gone");
        // the resumed child exits without a new hand-in: that exit is gone
        *e.borrow()[1].1.borrow_mut() = Some(0);
        s.pass().unwrap();
        assert_eq!(task(&b, n).retries, 1);
    }

    // Chain: § 3 admission filters. Risk: a waiting or filtered task starts.
    #[test]
    fn waiting_and_filters_hold_admission() {
        let b = Fake::default();
        let dep = b.create("dep", "", &["later".into()]).unwrap();
        b.comment(dep, &core::entry("later", &[], None, "")).unwrap();
        let n = b.create("T", &format!("Depends on #{dep}"), &["taskq".into()]).unwrap();
        let other = b.create("O", "", &["taskq".into(), "host-win".into()]).unwrap();
        let user = b.create("U", "", &[]).unwrap();
        let c = cfg("mac");
        let mut s = service(&b, &c, Default::default());
        s.pass().unwrap();
        assert_eq!(task(&b, other).state, State::Ready);
        assert!(b.issue(n).unwrap().labels.contains(&"q-blocked".to_string()));
        b.close(dep).unwrap();
        s.pass().unwrap();
        assert_eq!(task(&b, n).state, State::Doing);
        assert!(b.comments(user).unwrap().is_empty() && b.issue(user).unwrap().labels.is_empty(), "no taskq label: never a task");
    }

    // Chain: § 3 priority order. Risk: legacy priority labels order or filter wrong.
    #[test]
    fn canonical_and_legacy_priorities_filters_and_conflicts() {
        let b = Fake::default();
        let old = b.create("old", "", &["taskq".into(), "priority-1".into()]).unwrap();
        let new = b.create("new", "", &["taskq".into(), "p1".into()]).unwrap();
        let conflict = b.create("bad", "", &["taskq".into(), "p1".into(), "p2".into()]).unwrap();
        assert_eq!(priority(&b.issue(old).unwrap()), priority(&b.issue(new).unwrap()));
        let mut c = cfg("mac"); c.limits.insert("claude".into(), 3);
        let mut s = service(&b, &c, Default::default()); s.focus = vec!["p1".into()];
        s.pass().unwrap();
        assert_eq!(task(&b, old).state, State::Doing);
        assert_eq!(task(&b, new).state, State::Doing);
        assert_eq!(task(&b, conflict).state, State::Ready);
        assert!(b.comments(conflict).unwrap().is_empty(), "conflicting restriction never claimed");
    }

    // Chain: § 3 p0 admission. Risk: p0 blocked by focus or passes pause.
    #[test]
    fn p0_passes_any_focus_but_not_pause_or_limits() {
        let b = Fake::default();
        let p1 = b.create("P1", "", &["taskq".into(), "p1".into(), "area-maps".into()]).unwrap();
        let p0 = b.create("P0", "", &["taskq".into(), "p0".into(), "area-ui".into()]).unwrap();
        let other = b.create("O", "", &["taskq".into(), "area-ui".into()]).unwrap();
        assert!(priority(&b.issue(p0).unwrap()) < priority(&b.issue(p1).unwrap()));
        let c = cfg("mac");
        let mut s = service(&b, &c, Default::default());
        s.focus = vec![];
        s.pass().unwrap();
        assert_eq!(task(&b, p0).state, State::Ready, "pause: p0 waits too");
        s.focus = vec!["area-maps".into()];
        s.pass().unwrap();
        assert_eq!((task(&b, p0).state, task(&b, p1).state, task(&b, other).state), (State::Doing, State::Ready, State::Ready), "p0 first; limit 1 holds");
    }

    // Chain: § 3 focus and needs. Risk: a task outside focus or needs starts.
    #[test]
    fn focus_and_needs_choose_tasks() {
        let b = Fake::default();
        let win = b.create("W", "", &["taskq".into(), "needs-windows".into()]).unwrap();
        let maps = b.create("M", "", &["taskq".into(), "area-maps".into()]).unwrap();
        let other = b.create("O", "", &["taskq".into()]).unwrap();
        let gated = b.create("G", "", &["taskq".into(), "needs-gate".into()]).unwrap();
        let c = cfg("mac");
        let mut s = service(&b, &c, Default::default());
        s.focus = vec![];
        s.pass().unwrap();
        assert_eq!(task(&b, other).state, State::Ready, "focus off: nothing starts");
        s.focus = vec!["area-maps".into()];
        s.pass().unwrap();
        assert_eq!((task(&b, maps).state, task(&b, other).state), (State::Doing, State::Ready));
        let mut c = cfg("mac");
        c.limits.insert("claude".into(), 9);
        let mut s = service(&b, &c, Default::default());
        s.pass().unwrap();
        assert_eq!((task(&b, win).state, task(&b, other).state), (State::Ready, State::Doing), "mac has no windows");
        assert_eq!(task(&b, gated).state, State::Doing, "every host of this version has gates (§ 8)");
        let w = Config { has: vec!["windows".into()], ..c.clone() };
        let mut s = service(&b, &w, Default::default());
        s.pass().unwrap();
        assert_eq!(task(&b, win).state, State::Doing);
    }

    // Chain: § 4 empty focus (#974). Risk: a host with free slots admits nothing and says nowhere why.
    #[test]
    fn empty_focus_is_a_capacity_reason_and_shows_in_hosts() {
        let b = Fake::default();
        let t = b.create("T", "", &["taskq".into()]).unwrap();
        let c = cfg("mac");
        let mut s = service(&b, &c, Default::default());
        s.focus.clear();
        s.pass().unwrap();
        s.pass().unwrap();
        assert_eq!(task(&b, t).state, State::Ready);
        let focus = |s: &Service| s.capacity_json()["reasons"].as_array().unwrap().iter().find(|r| r["code"] == "focus").map(|r| r["text"].as_str().unwrap().to_string());
        assert_eq!(focus(&s).as_deref(), Some(FOCUS_OFF));
        let text = s.snapshot.as_ref().unwrap().text(&c, &Value::Null).unwrap();
        assert!(text.contains("Hosts: mac v4.0.0 ") && text.ends_with(" focus off"), "{text}");
        s.focus_error = Some("focus: not UTF-8 text; file kept".into());
        assert_eq!(focus(&s).as_deref(), s.focus_error.as_deref(), "an unreadable focus names its own reason");
        s.focus_error = None;
        s.focus = vec!["all".into()];
        s.pass().unwrap();
        s.pass().unwrap();
        assert_eq!((focus(&s), task(&b, t).state), (None, State::Doing));
        assert!(!s.snapshot.as_ref().unwrap().text(&c, &Value::Null).unwrap().contains("focus"));
    }

    // Chain: § 8 codex-app admission. Risk: codex-app runs an unlabelled task.
    #[test]
    fn codex_app_runs_only_labelled_tasks_on_opted_in_hosts() {
        let b = Fake::default();
        let app = b.create("A", "", &["taskq".into(), "run-codex-app".into()]).unwrap();
        let other = b.create("O", "", &["taskq".into()]).unwrap();
        let mut c = cfg("win");
        c.limits = BTreeMap::from([("codex-app".to_string(), 1)]);
        service(&b, &c, Default::default()).pass().unwrap();
        assert_eq!((task(&b, app).state, task(&b, other).state), (State::Ready, State::Ready), "no opt-in: unchanged");
        let m = Config { has: vec!["codex-app".into()], me: "mac".into(), ..c.clone() };
        service(&b, &m, Default::default()).pass().unwrap();
        assert_eq!((task(&b, app).state, task(&b, other).state), (if cfg!(target_os = "macos") { State::Doing } else { State::Ready }, State::Ready), "only macOS admits app turns; unlabelled tasks never get the app");
        if cfg!(target_os = "macos") { assert!(task(&b, app).session.unwrap().starts_with("codex-app:- mac")); }
    }

    // Chain: § 3 capacity config. Risk: host override ignored or invalid limits accepted.
    #[test]
    fn capacity_merges_host_overrides_and_validates() {
        let v = json!({"limits": {"claude": 4, "codex": 4}, "load_gate": {"cpu_max": 90},
            "capacity_hosts": {"win": {"limits": {"codex": 2}, "load_gate": {"enabled": true}}}});
        let win = capacity(&v, "win").unwrap();
        assert_eq!(win.limits, BTreeMap::from([("claude".into(), 4), ("codex".into(), 2)]));
        assert_eq!(win.gate, GateCfg { enabled: true, cpu_max: 85, memory_min: 15, mean: false }, "a host gate replaces the default object");
        assert_eq!(capacity(&v, "mac").unwrap().gate, GateCfg { enabled: false, cpu_max: 90, memory_min: 15, mean: false });
        assert_eq!(capacity(&json!({"limits": {"claude": 1}}), "mac").unwrap(), Capacity::fixed(BTreeMap::from([("claude".into(), 1)]), GATE_DEFAULT), "absent keys: today");
        for bad in [json!({"limits": {"claude": -1}}), json!({"limits": {"claude": 5_000_000_000u64}}), json!({"load_gate": {"cpu_max": 9}}),
            json!({"load_gate": {"memory_min": 96}}), json!({"load_gate": {"enabled": "yes"}}), json!({"capacity_hosts": {"mac": {"limits": {"codex": "2"}}}})] {
            assert!(capacity(&bad, "mac").is_err(), "{bad}");
        }
    }

    fn on() -> GateCfg {
        GateCfg { enabled: true, ..GATE_DEFAULT }
    }

    /// Feed CPU busy percentages at 30 s steps from `t`; ticks advance by 100 per sample.
    fn feed(g: &mut Gate, t: &mut u64, ticks: &mut (u64, u64), samples: &[(f64, f64)]) {
        for (cpu, mem) in samples {
            *ticks = (ticks.0 + *cpu as u64, ticks.1 + 100);
            *t += 30;
            g.host_mem = Some((8.0, 1));
            g.observe(*t, *t as i64, Some(*ticks), Some(*mem));
        }
    }

    // Chain: § 3 load gate. Risk: starts under high load or never reopens.
    #[test]
    fn gate_holds_over_thresholds_and_reopens_after_90_s_calm() {
        let mut g = Gate::default();
        g.configure(GATE_DEFAULT);
        assert!(g.may_launch(0) && g.state(0).0 == "off", "off by default");
        g.configure(on());
        assert_eq!(g.state(0).0, "unknown", "enabled: held until sampled");
        let (mut t, mut k) = (0, (0, 0));
        g.observe(0, 0, Some(k), Some(50.0));
        assert_eq!(g.state(0), ("unknown", vec![("sample", "no CPU reading yet".into())]), "first CPU reading is a baseline");
        feed(&mut g, &mut t, &mut k, &[(50.0, 50.0), (50.0, 50.0)]);
        assert!(!g.may_launch(t) && g.state(t).0 == "held");
        feed(&mut g, &mut t, &mut k, &[(50.0, 50.0)]);
        assert!(g.may_launch(t), "three calm samples: 90 s");
        // exact thresholds hold; both reasons are kept
        feed(&mut g, &mut t, &mut k, &[(85.0, 15.0)]);
        assert_eq!(g.state(t).1.iter().map(|r| r.0).collect::<Vec<_>>(), ["cpu", "memory"]);
        // mid-band (CPU 80 > 85 - 10) resets recovery
        feed(&mut g, &mut t, &mut k, &[(50.0, 50.0), (50.0, 50.0), (80.0, 50.0), (50.0, 50.0), (50.0, 50.0)]);
        assert_eq!(g.state(t).0, "held");
        feed(&mut g, &mut t, &mut k, &[(50.0, 50.0)]);
        assert_eq!(g.state(t).0, "open");
        // a stale sample holds; a failed reading holds and resets
        assert_eq!(g.state(t + 61).0, "unknown");
        g.observe(t + 30, 0, Some(k), None);
        assert!(!g.may_launch(t + 30));
        // a threshold change starts held
        let mut g2 = Gate::default();
        g2.configure(on());
        let (mut t2, mut k2) = (0, (0, 0));
        g2.observe(0, 0, Some(k2), Some(50.0));
        feed(&mut g2, &mut t2, &mut k2, &[(10.0, 50.0); 3]);
        assert!(g2.may_launch(t2));
        g2.configure(GateCfg { cpu_max: 90, ..on() });
        assert!(!g2.may_launch(t2));
        // Auto: the 60 s mean smooths one busy interval; a 100 % interval still vetoes
        let mut g3 = Gate::default();
        g3.configure(GATE_AUTO);
        let (mut t3, mut k3) = (0, (0, 0));
        g3.observe(0, 0, Some(k3), Some(50.0));
        feed(&mut g3, &mut t3, &mut k3, &[(70.0, 50.0), (95.0, 50.0)]);
        assert_eq!(g3.last.unwrap().2, Some(82.5));
        feed(&mut g3, &mut t3, &mut k3, &[(100.0, 50.0)]);
        assert_eq!(g3.state(t3).1[0].0, "cpu");
        // Auto memory floor: max(memory_min %, 2.5 GiB) and macOS pressure level 4 hold; no reading is unknown
        feed(&mut g3, &mut t3, &mut k3, &[(10.0, 50.0); 3]);
        assert_eq!(g3.state(t3).0, "open");
        for (m, code) in [(Some((2.4, 1)), "memory"), (Some((8.0, PRESSURE_CRITICAL)), "memory"), (None, "sample")] {
            g3.host_mem = m;
            g3.observe(t3, t3 as i64, Some(k3), Some(50.0));
            assert_eq!(g3.state(t3).1.last().unwrap().0, code, "{m:?}");
            assert!(!g3.may_launch(t3));
        }
        g2.host_mem = None;
        assert_eq!(g2.state(t2).0, "held", "Fixed ignores the floor reading");
    }

    // Chain: § 3 load gate spacing. Risk: launch burst or bad counters accepted.
    #[test]
    fn gate_spaces_launches_and_rejects_bad_counters() {
        let mut g = Gate::default();
        g.configure(on());
        let (mut t, mut k) = (0, (0, 0));
        g.observe(0, 0, Some(k), Some(50.0));
        feed(&mut g, &mut t, &mut k, &[(10.0, 50.0); 3]);
        g.launch(t);
        assert!(!g.may_launch(t + 40), "no sample after the launch");
        feed(&mut g, &mut t, &mut k, &[(10.0, 50.0)]);
        assert!(g.may_launch(t), "post-launch sample, 30 s later");
        g.observe(t + 30, 0, Some((0, 0)), Some(50.0)); // counters went backwards
        assert_eq!(g.state(t + 30).0, "unknown");
    }

    // Chain: § 3 load gate with live attempts. Risk: the gate drops leases or blocks resumes.
    #[test]
    fn load_gate_holds_claims_and_resumes_keeps_leases_and_recovers() {
        let b = Fake::default();
        let n = b.create("T", "", &["taskq".into()]).unwrap();
        let mut c = cfg("mac");
        c.limits.insert("claude".into(), 2);
        let e: Exits = Default::default();
        let mut s = service(&b, &c, e.clone());
        s.pass().unwrap();
        let a = task(&b, n).attempt.unwrap();
        let m = b.create("M", "", &["taskq".into(), "host-mac".into()]).unwrap();
        // gate on: held until three calm samples, the running child continues
        s.reload(Ok(Capacity::fixed(c.limits.clone(), on())));
        let (mut t, mut k) = (0, (0, 0));
        s.gate.observe(0, 0, Some(k), Some(50.0));
        feed(&mut s.gate, &mut t, &mut k, &[(90.0, 50.0)]);
        s.mono = t;
        b.comment(n, &core::entry("ask", &[], Some(a), "Which?\n1. x\n2. y ★")).unwrap();
        *e.borrow()[0].1.borrow_mut() = Some(0);
        s.pass().unwrap();
        b.comment(n, &core::entry("answer", &["1"], None, "x")).unwrap();
        s.pass().unwrap();
        assert_eq!((e.borrow().len(), task(&b, m).state), (1, State::Ready), "no resume, no claim while held");
        assert!(b.comments(m).unwrap().is_empty(), "no spawn written while held");
        let hb = b.issues(None).unwrap().into_iter().find(is_host).unwrap();
        assert!(hb.body.lines().any(|l| l == format!("#{n} {a}")), "held resume keeps its lease: {}", hb.body);
        assert_eq!(s.capacity_json()["gate"], "held");
        // other hosts leave a listed lease alone
        let w = cfg("win");
        let mut other = service(&b, &w, Default::default());
        b.advance(400);
        s.pass().unwrap();
        other.pass().unwrap();
        assert_eq!(task(&b, n).attempt, Some(a));
        // recovery: the resume goes first, the claim waits for a post-launch sample and 30 s
        feed(&mut s.gate, &mut t, &mut k, &[(10.0, 50.0); 3]);
        s.mono = t;
        s.pass().unwrap();
        assert_eq!((e.borrow().len(), task(&b, m).state), (2, State::Ready), "one launch per 30 s");
        feed(&mut s.gate, &mut t, &mut k, &[(10.0, 50.0)]);
        s.mono = t;
        s.pass().unwrap();
        assert_eq!(task(&b, m).state, State::Doing);
        assert_eq!(s.capacity_json()["used"]["claude"], 2);
        // invalid config holds and reports, the children continue
        s.reload(Err("limits.claude: not an integer".into()));
        assert_eq!(s.capacity_json()["reasons"][0]["code"], "config");
        assert_eq!(s.kids.len(), 2);
    }

    // Chain: § 9 Auto admission config. Risk: Auto silently off, or a cap or invalid mode accepted wrong.
    #[test]
    fn auto_admission_takes_d03_defaults_and_caps() {
        let a = capacity(&json!({"admission": {"mode": "auto", "max": 6}, "limits": {"codex": 2}}), "mac").unwrap();
        assert_eq!((a.auto, a.max, a.gate), (true, Some(6), GATE_AUTO));
        let tuned = capacity(&json!({"capacity_hosts": {"mac": {"admission": {"mode": "auto"}, "load_gate": {"cpu_max": 80}}}}), "mac").unwrap();
        assert_eq!((tuned.gate.cpu_max, tuned.gate.memory_min, tuned.gate.enabled), (80, 10, true), "a host gate field overrides one default");
        assert!(!capacity(&json!({}), "mac").unwrap().auto, "absent: Fixed, today's behavior");
        for bad in [json!({"admission": "auto"}), json!({"admission": {"mode": "fast"}}), json!({"admission": {"max": -1}}),
            json!({"admission": {"mode": "auto"}, "load_gate": {"enabled": false}})] {
            assert!(capacity(&bad, "mac").is_err(), "{bad}");
        }
    }

    // Chain: § 3 host authority under Auto. Risk: a launch without a reservation, a cap overrun or admission on unknown data.
    #[test]
    fn auto_service_reserves_holds_at_cap_releases_and_refuses_unknown() {
        let install = std::env::temp_dir().join(format!("taskq-auto-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&install);
        std::fs::create_dir_all(&install).unwrap();
        let b = Fake::default();
        let ns: Vec<u64> = (0..3).map(|i| b.create(&format!("T{i}"), "", &["taskq".into()]).unwrap()).collect();
        let mut c = cfg("mac");
        c.install = install.clone();
        let e: Exits = Default::default();
        let mut s = service(&b, &c, e.clone());
        s.reload(capacity(&json!({"admission": {"mode": "auto", "max": 1}}), "mac"));
        // unknown: no sample yet, nothing claimed
        s.pass().unwrap();
        assert_eq!((s.capacity_json()["gate"].as_str(), e.borrow().len()), (Some("unknown"), 0));
        let (mut t, mut k) = (0, (0, 0));
        s.gate.observe(0, 0, Some(k), Some(50.0));
        feed(&mut s.gate, &mut t, &mut k, &[(10.0, 50.0); 3]);
        (s.mono, s.wall) = (t, t as i64);
        // admit: one reservation, held as a child
        s.pass().unwrap();
        assert_eq!((e.borrow().len(), task(&b, ns[0]).state), (1, State::Doing));
        let cap = s.capacity_json();
        assert_eq!((cap["host_used"].as_u64(), cap["reservations"][0]["state"].as_str()), (Some(1), Some("held")));
        // hold at the host cap of 1: requested with a reason, no spawn written
        feed(&mut s.gate, &mut t, &mut k, &[(10.0, 50.0); 2]);
        (s.mono, s.wall) = (t, t as i64);
        s.pass().unwrap();
        assert_eq!((e.borrow().len(), task(&b, ns[1]).state), (1, State::Ready));
        let cap = s.capacity_json();
        assert_eq!((cap["gate"].as_str(), cap["reasons"][0]["code"].as_str()), (Some("held"), Some("max")));
        assert_eq!(cap["reservations"][1]["state"], "requested");
        // release: the child exits, the next pass admits the next task
        b.comment(ns[0], &core::entry("result", &["0123456789abcdef0123456789abcdef01234567"], task(&b, ns[0]).attempt, "ok")).unwrap();
        *e.borrow()[0].1.borrow_mut() = Some(0);
        s.pass().unwrap();
        s.sync_host();
        feed(&mut s.gate, &mut t, &mut k, &[(10.0, 50.0)]);
        (s.mono, s.wall) = (t, t as i64);
        s.pass().unwrap();
        assert_eq!(e.borrow().len(), 2, "released slot admits one more (reviewer or next task)");
        // unknown host data: an unreadable host file refuses and is rebuilt
        std::fs::write(install.join("admission.json"), "{").unwrap();
        *e.borrow()[1].1.borrow_mut() = Some(0);
        feed(&mut s.gate, &mut t, &mut k, &[(10.0, 50.0)]);
        (s.mono, s.wall) = (t, t as i64);
        let before = e.borrow().len();
        s.pass().unwrap();
        assert_eq!(e.borrow().len(), before, "refused on unknown");
        assert_eq!(s.capacity_json()["gate"], "unknown");
        let _ = std::fs::remove_dir_all(&install);
    }

    // Chain: § 4 history events file. Risk: duplicate events or unbounded growth.
    #[test]
    fn events_append_dedup_and_trim() {
        let p = std::env::temp_dir().join(format!("taskq-events-{}", std::process::id()));
        let _ = std::fs::remove_file(&p);
        assert_eq!(record(&p, 1000, &[(1, "close", 7)]).unwrap(), 1000);
        record(&p, 2000, &[(1, "close", 7), (2, "failed", 7), (3, "close", 8)]).unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "since 1000\n1000 #1 close 7\n2000 #2 failed 7\n2000 #3 close 8\n", "same attempt and kind once");
        let late = 1500 + MONTH;
        assert_eq!(record(&p, late, &[]).unwrap(), 1500);
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "since 1500\n2000 #2 failed 7\n2000 #3 close 8\n", "older than 30 days dropped");
        let _ = std::fs::remove_file(&p);
    }

    // Chain: § 4 history across restart. Risk: a restart loses or doubles events.
    #[test]
    fn a_pass_records_close_and_failed_and_a_restart_keeps_them() {
        let root = std::env::temp_dir().join(format!("taskq-ev-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join(".taskq")).unwrap();
        let b = Fake::default();
        let (x, y) = (b.create("X", "", &["taskq".into()]).unwrap(), b.create("Y", "", &["taskq".into()]).unwrap());
        let mut c = cfg("mac");
        c.root = root.clone();
        c.limits.insert("claude".into(), 2);
        let e: Exits = Default::default();
        let mut s = service(&b, &c, e.clone());
        s.pass().unwrap();
        let (ax, ay) = (task(&b, x).attempt.unwrap(), task(&b, y).attempt.unwrap());
        b.comment(x, &core::entry("result", &["0123456789abcdef0123456789abcdef01234567"], Some(ax), "ok")).unwrap();
        *e.borrow()[0].1.borrow_mut() = Some(0);
        s.pass().unwrap();
        let r = task(&b, x).attempt.unwrap();
        b.comment(x, &core::entry("close", &[], Some(r), "ok")).unwrap();
        b.close(x).unwrap();
        b.comment(y, &core::entry("failed", &[], None, "no way")).unwrap();
        s.pass().unwrap();
        let ev = std::fs::read_to_string(root.join(".taskq/events")).unwrap();
        let lines: Vec<&str> = ev.lines().skip(1).map(|l| l.split_once(' ').unwrap().1).collect();
        assert_eq!(lines, [format!("#{x} close {r}"), format!("#{y} failed {ay}")], "{ev}");
        let mut again = service(&b, &c, Default::default());
        again.pass().unwrap();
        again.pass().unwrap();
        assert_eq!(std::fs::read_to_string(root.join(".taskq/events")).unwrap(), ev, "a restart keeps history");
        assert!(again.events_since.is_some());
        // a new task enters waiting (no comment: the issue number), a worker asks (its attempt)
        let z = b.create("Z", &format!("Depends on #{y}"), &["taskq".into()]).unwrap();
        let w = b.create("W", "", &["taskq".into()]).unwrap();
        again.pass().unwrap();
        let aw = task(&b, w).attempt.unwrap();
        b.comment(w, &core::entry("ask", &[], Some(aw), "Which?\n1. x\n2. y ★")).unwrap();
        again.pass().unwrap();
        again.pass().unwrap();
        let ev = std::fs::read_to_string(root.join(".taskq/events")).unwrap();
        let lines: Vec<&str> = ev.lines().skip(3).map(|l| l.split_once(' ').unwrap().1).collect();
        assert_eq!(lines, [format!("#{z} blocked {z}"), format!("#{w} ask {aw}")], "{ev}");
        let _ = std::fs::remove_dir_all(&root);
    }

    // Chain: § 7 worktree cleanup. Risk: a dirty worktree is deleted.
    #[test]
    fn closed_tasks_lose_clean_worktrees_only() {
        let root = std::env::temp_dir().join(format!("taskq-prune-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let git = |args: &[&str]| assert!(std::process::Command::new("git").arg("-C").arg(&root).args(["-c", "user.name=t", "-c", "user.email=t@t"]).args(args).output().unwrap().status.success(), "{args:?}");
        git(&["init", "-q"]);
        git(&["commit", "-q", "--allow-empty", "-m", "init"]);
        for n in ["1", "2", "3"] {
            git(&["worktree", "add", "-q", "-b", &format!("taskq-{n}"), &format!(".worktrees/taskq-{n}")]);
        }
        std::fs::write(root.join(".worktrees/taskq-2/wip"), "x").unwrap();
        let b = Fake::default();
        for _ in 0..3 {
            b.create("T", "", &["taskq".into()]).unwrap();
        }
        b.close(1).unwrap();
        b.close(2).unwrap();
        let mut c = cfg("mac");
        c.root = root.clone();
        let mut s = service(&b, &c, Default::default());
        s.pass().unwrap();
        let wt = |n: &str| root.join(format!(".worktrees/taskq-{n}")).exists();
        assert_eq!((wt("1"), wt("2"), wt("3")), (false, true, true), "clean closed removed; dirty and open stay");
        cleanup(&b, &root, |_| false, true);
        assert_eq!((wt("2"), wt("3")), (false, true), "force removes dirty closed; open stays");
        assert!(std::fs::read_to_string(root.join(".taskq/trash/taskq-2.patch")).unwrap().contains("wip"), "dirty work saved");
        let _ = std::fs::remove_dir_all(&root);
    }
}
