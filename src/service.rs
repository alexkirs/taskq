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
const NATIVE_PASS_BUDGET:std::time::Duration=std::time::Duration::from_secs(20);

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

/// `who` is an assignee; an issue listed without assignees (github.com, § 4 budget) costs one `issue(n)` read.
pub fn assigned(board: &dyn Board, i: &Issue, who: &str) -> Result<bool, String> {
    Ok(i.assignees.iter().any(|a| a == who) || i.assignees.is_empty() && board.issue(i.number)?.assignees.iter().any(|a| a == who))
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
    /// `Depends on` issues that keep a ready task waiting (§ 1).
    pub blockers: Vec<Blocker>,
}

/// A `Depends on` issue that is not done (§ 1): `reason` is `open` or `unknown: <why>`.
#[derive(Clone, Debug, PartialEq)]
pub struct Blocker {
    pub dep: core::Dep,
    pub url: String,
    pub reason: String,
}

/// Closed dependencies already read, `None` when done (§ 4 budget): an issue of this board until the open-issue list
/// shows it again; of another repository only when done. `ponytail:` a done foreign issue reopened later is not seen.
pub type DepCache = HashMap<core::Dep, Option<String>>;

/// The `Depends on` issues of `body` that are not done (§ 1): open, closed other than completed, or unreadable.
/// `open` is this board's open-issue list when the caller read it.
pub fn blockers(b: &dyn Board, body: &str, open: Option<&HashSet<u64>>, done: &mut DepCache) -> Vec<Blocker> {
    core::depends(body).into_iter().filter_map(|dep| {
        let listed = dep.repo.is_none() && open.is_some_and(|o| o.contains(&dep.n));
        if listed {
            done.remove(&dep);
        }
        let reason = match done.get(&dep) {
            _ if listed => Some("open".to_string()),
            Some(r) => r.clone(),
            None => match dep.repo.as_deref().map_or_else(|| b.issue(dep.n), |r| b.foreign(r, dep.n)) {
                Err(e) => Some(format!("unknown: {e}")),
                Ok(i) if i.open => Some("open".into()),
                Ok(i) => {
                    let r = i.reason.filter(|r| r != "completed").map(|r| format!("unknown: closed {}", r.replace('_', " ")));
                    if dep.repo.is_none() || r.is_none() {
                        done.insert(dep.clone(), r.clone());
                    }
                    r
                }
            },
        };
        let url = dep.repo.as_deref().map_or_else(|| b.url(dep.n), |r| b.foreign_url(r, dep.n));
        reason.map(|reason| Blocker { dep, url, reason })
    }).collect()
}

/// Fold every open task (heartbeat issues and untrusted authors excluded).
/// Remove `.worktrees/taskq-<N>` of closed tasks and their merged local branch (§ 7, R11). Without `force` git refuses
/// dirty worktrees, so those stay; with it the uncommitted work goes to `.taskq/trash/taskq-<N>.patch` first. Commits
/// stay in the branch: `git branch -d` keeps an unmerged one. `ponytail:` a squash-merged branch stays until a manual
/// `git branch -D`.
pub fn cleanup(board: &dyn Board, root: &std::path::Path, mut skip: impl FnMut(u64) -> bool, force: bool) -> Vec<String> {
    let mut log = vec![];
    let Ok(dir) = std::fs::read_dir(root.join(".worktrees")) else { return log };
    let git = |dir: &std::path::Path, args: &[&str]| coreq::process::output("cleanup.git", std::process::Command::new("git").arg("-C").arg(dir).args(["-c", "core.fsmonitor=false", "-c", "core.hooksPath=/dev/null"]).args(args), std::time::Duration::from_secs(30)).ok().map(|o| o.stdout);
    for e in dir.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(n) = name.strip_prefix("taskq-").and_then(|n| n.parse::<u64>().ok()) else { continue };
        if skip(n) || !board.issue(n).is_ok_and(|i| !i.open) {
            continue; // a fresh read must show the issue closed (R12)
        }
        if !crate::board::entries(board, n).is_ok_and(|entries| core::executions(n, &entries).iter().all(|x| x.release.is_some())) {
            log.push(format!("#{n} worktree kept: execution termination unconfirmed"));
            continue;
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
            log.push(format!("#{n} worktree removal unconfirmed: {}", if force { "patch save or Git command failed; inspect the worktree" } else { "Git refused or failed; inspect uncommitted work before `taskq cleanup --force`" }));
        }
    }
    log
}

pub fn views(board: &dyn Board, cache: &mut HashMap<u64, (i64, Vec<Comment>)>, deps: &mut DepCache) -> Result<(Vec<View>, Vec<Issue>), String> {
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
        // only a task that would be ready reads its dependencies (§ 4 budget)
        let ready = !is_held(&issue) && core::fold(issue.number, &log, false).state == State::Ready;
        let blockers = if ready { blockers(board, &issue.body, Some(&open), deps) } else { vec![] };
        let mut task = core::fold(issue.number, &log, !blockers.is_empty());
        if is_held(&issue) {
            task.state = State::Held;
        }
        out.push(View { task, issue, comments, log, blockers });
    }
    Ok((out, hosts))
}

/// Release only a known submission, with idempotent write and authoritative readback.
fn release_execution(board: &dyn Board, task: u64, attempt: u64, execution: u64, proof: &str) -> Result<(), String> {
    let expected = match proof { "exit" => core::Release::Exit, "no-start" => core::Release::NoStart, _ => return Err("invalid release proof".into()) };
    let released = |log: &[core::Entry]| core::executions(task, log).into_iter()
        .find(|x| x.id == execution && x.attempt == attempt).map(|x| x.release);
    match released(&crate::board::entries(board, task)?) {
        Some(Some(found)) if found == expected => return Ok(()),
        Some(Some(_)) => return Err(format!("execution {execution} has contradictory release proof")),
        Some(None) => {},
        None => return Err(format!("execution {execution} ownership unknown; release refused")),
    }
    let op = format!("release-{execution}");
    let body = core::with_fields(core::entry("released", &[&execution.to_string(), proof], Some(attempt), ""), Some(&op), None);
    let written = board.comment(task, &body);
    if released(&crate::board::entries(board, task)?) == Some(Some(expected)) { Ok(()) }
    else { Err(format!("execution {execution} release unconfirmed: {}", written.err().unwrap_or_else(|| "readback missing".into()))) }
}

fn accept_execution(board: &dyn Board, task: u64, attempt: u64, execution: u64) -> Result<u64, String> {
    let accepted = |log: &[core::Entry]| core::executions(task, log).into_iter()
        .find(|x| x.id == execution && x.attempt == attempt && x.accepted).map(|x| x.through);
    let log = crate::board::entries(board, task)?;
    let known = core::executions(task,&log).into_iter().find(|x| x.id == execution && x.attempt == attempt)
        .ok_or_else(|| format!("execution {execution} ownership unknown; acceptance refused"))?;
    if known.release == Some(core::Release::NoStart) { return Err(format!("execution {execution} confirmed no-start; acceptance refused")); }
    if known.accepted { return Ok(known.through); }
    let op = format!("accept-{execution}");
    let body = core::with_fields(core::entry("accepted", &[&execution.to_string()], Some(attempt), ""), Some(&op), None);
    let written = board.comment(task, &body);
    accepted(&crate::board::entries(board, task)?).ok_or_else(|| format!("execution {execution} acceptance unconfirmed: {}",
        written.err().unwrap_or_else(|| "readback missing or contradictory".into())))
}

pub struct Kid {
    pub execution: u64,
    pub task: u64,
    pub attempt: u64,
    pub rt: String,
    pub proc: Box<dyn Proc>,
}

pub struct Service<'a> {
    pub host: Option<crate::host::Shared>,
    /// A verified legacy native owner drains its own attempts without claiming a new worker/reviewer.
    pub legacy_handoff:bool,
    pub legacy_prepared:bool,
    pub board: &'a dyn Board,
    pub cfg: Config,
    pub version: String,
    pub kids: Vec<Kid>,
    /// `service <id>` in the heartbeat (§ 4): a hash of `<install>/installation-id`; empty writes no line.
    pub instance: String,
    /// `machine <hostname>[ wsl]` in the heartbeat (§ 4): the agentq `host.name` DashQ joins hosts by; empty writes no line.
    pub machine: String,
    /// Labels from `<root>/.taskq/focus`; empty: admit nothing, `all`: any task; `p0` passes any non-empty focus (§ 4).
    pub focus: Vec<String>,
    /// Why `focus` could not be read; it then admits nothing (§ 4 Data).
    pub focus_error: Option<String>,
    /// The last inbox id handed to each attempt's child.
    delivered: HashMap<u64, u64>,
    cache: HashMap<u64, (i64, Vec<Comment>)>,
    deps: DepCache,
    label_fixes: HashMap<u64, (String, Vec<String>, u8)>,
    /// Worktrees of closed tasks already tried once by `prune`.
    pruned: HashSet<u64>,
    start: Box<dyn FnMut(&Launch) -> Result<Box<dyn Proc>, crate::runtime::StartError> + 'a>,
    pub recover: Option<Box<dyn FnMut(u64, &core::Execution) -> Result<Box<dyn Proc>, String> + 'a>>,
    #[cfg(test)]
    agent_status: Value,
    pub log: Vec<String>,
    /// The read model of the last successful pass (for `status.json` and the tray).
    pub snapshot: Option<crate::snapshot::Snapshot>,
    pub tasks_read: Option<std::time::Instant>,
    /// A complete board ownership read is required before drain or handover.
    ownership_known: bool,
    /// This host's heartbeat issue, from the last board read: a lease write needs no second list (§ 4 budget).
    host_issue: Option<u64>,
    /// Rebuilt from board ownership; heartbeat projection only, never a launch/release authority.
    observations: BTreeMap<(u64,u64),core::Execution>,
    /// Owner comments of the last pass (§ 4): task, comment id, text, card.
    pub owner_comments: Vec<(u64, u64, String, String)>,
    /// State of each open task in the previous pass, for `events`.
    seen: HashMap<u64, State>,
    /// The newest comment id an events pass saw; `gone` events come only from newer comments (§ 4)
    gone_after: Option<u64>,
    /// Newest comment id of the previous pass, for queue transitions; None before the first (§ 4 Profiling).
    moved_after: Option<u64>,
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
    pub execution_waits: BTreeMap<u64, String>,
    pending_releases: BTreeSet<(u64, u64, u64)>,
    stopping: BTreeSet<u64>,
    /// Children by attempt: role, runtime, and start with attempt log offset; `None` when adopted across a re-exec
    /// (§ 4 Profiling stages).
    begun: HashMap<u64, (String, String, Option<(std::time::Instant, u64)>)>,
    /// This host's tasks seen moving into `ask`: since when, attempt, runtime (§ 4 Profiling stages).
    asked: HashMap<u64, (std::time::Instant, u64, String)>,
    /// Remote Control URL of each live Claude child by attempt, read once from its transcript (§ 4 heartbeat `session`).
    sessions: HashMap<u64, String>,
    reduced_at: Option<u64>,
    recovery_after: Option<u64>,
}

const MONTH: i64 = 30 * 86400;

/// Host readback stays separate from a project's effective narrowing limits.
pub fn host_capacity(capacity:&Value)->Option<Value> {
    if !capacity["host_policy"].is_object() {return None;}
    let mut host=capacity.clone();
    for field in ["limits","load_gate","admission"] {host[field]=capacity["host_policy"][field].clone();}
    host["used"]=capacity["host_runtime_used"].clone();
    Some(host)
}

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
#[derive(Default, Clone)]
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
        Service { recovery_after: None, reduced_at: None, host: None, legacy_handoff:false, legacy_prepared:false, wall: 0, reserved: vec![], requested: vec![], refused: None, host_used: None, board, cfg: cfg.clone(), version: version.into(), kids: vec![], instance: String::new(), machine: String::new(), focus: vec![], focus_error: None, delivered: HashMap::new(), cache: HashMap::new(), deps: HashMap::new(), label_fixes: HashMap::new(), pruned: HashSet::new(), start, recover: None, #[cfg(test)] agent_status: json!([]), log: vec![], snapshot: None, tasks_read: None, ownership_known: false, host_issue: None, observations: BTreeMap::new(), owner_comments: vec![], seen: HashMap::new(), gone_after: None, moved_after: None, events_since: None, cap, gate: Gate::default(), mono: 0, held: vec![], pending: BTreeSet::new(), hold: None, refused_access: BTreeMap::new(), unknown: HashMap::new(), execution_waits: BTreeMap::new(), pending_releases: BTreeSet::new(), stopping: BTreeSet::new(), begun: HashMap::new(), asked: HashMap::new(), sessions: HashMap::new() }
    }

    /// Rebuild board-derived state after unwind; live handles, pending launches and delivery cursors stay owned.
    pub fn rebuild(&mut self) {
        self.unready();
        self.cache.clear();
        self.deps.clear();
        self.label_fixes.clear();
        self.snapshot = None;
        self.tasks_read = None;
    }

    /// Quarantine/backoff: only verified reaping, leases and status; no claims, resumes or recovery writes.
    pub fn reduced(&mut self) -> Result<(), String> {
        self.unready();
        let before = self.kids.len();
        self.reap_owned();
        self.reconcile_ownership()?;
        self.sync_host();
        if before != self.kids.len() || self.reduced_at.is_none_or(|t| self.mono.saturating_sub(t) >= (self.cfg.ttl / 5).max(1) as u64) {
            self.reduced_at = Some(self.mono);
            if let Some(n) = self.host_issue {
                let hb = self.board.issue(n)?;
                let now = self.board.now()?;
                let body = self.heartbeat_body();
                if now - hb.updated >= self.cfg.ttl / 5 || !hb.body.lines().filter(|l| !l.starts_with("at ")).eq(body.lines()) {
                    self.board.edit(n, &format!("{body}\nat {now}"))?;
                }
            }
        }
        Ok(())
    }

    pub fn unready(&self) {
        if let Some(host) = &self.host { if let Ok(mut h) = host.lock() { h.ready(&self.cfg.root, false); } }
    }

    /// Liveness monitoring never frees ownership before the exact execution release is read back.
    pub fn reap_owned(&mut self) {self.reap_owned_within(NATIVE_PASS_BUDGET);}

    fn reap_owned_within(&mut self,budget:std::time::Duration) {
        let until=std::time::Instant::now()+budget;
        let mut deferred=None;
        let mut released=BTreeSet::new();
        self.kids.retain_mut(|k| {
            if std::time::Instant::now()>=until {
                deferred.get_or_insert(k.execution);
                self.unknown.insert(k.attempt,"native observation deferred; reduced pass budget exhausted".into());return true;
            }
            let status = crate::runtime::observe_until(until,||k.proc.exited());
            let mut acceptance_unknown = false;
            match crate::runtime::observe_until(until,||k.proc.accepted()) {
                Ok(true) => match accept_execution(self.board,k.task,k.attempt,k.execution) {
                    Ok(through) => { self.delivered.entry(k.attempt).and_modify(|v| *v = (*v).max(through)).or_insert(through); },
                    Err(e) => { self.unknown.insert(k.attempt,e); return true; }
                },
                Ok(false) => {},
                Err(e) => { acceptance_unknown = true; self.unknown.insert(k.attempt,e); }
            }
            match status {
            Ok(Some(_)) => match release_execution(self.board,k.task,k.attempt,k.execution,"exit") {
                Ok(())=>{
                    if matches!(k.rt.as_str(),"codex"|"claude") { if let Err(e)=crate::retire_kid(&self.cfg,k.task,k.attempt,k.execution) { self.log.push(format!("native recovery cleanup: {e}")); } }
                    released.insert((k.task,k.attempt));
                    self.observations.remove(&(k.task,k.execution));self.cache.remove(&k.task);self.unknown.remove(&k.attempt);self.stopping.remove(&k.attempt);false
                },
                Err(e)=>{self.unknown.insert(k.attempt,e);true}
            },
            Ok(None) => { if !self.stopping.contains(&k.attempt) && !acceptance_unknown { self.unknown.remove(&k.attempt); } true }
            Err(e) => { self.unknown.insert(k.attempt, e); true }
            }
        });
        self.retire_reservations(released);
        if let Some(first)=deferred.and_then(|id|self.kids.iter().position(|k|k.execution==id)) {self.kids.rotate_left(first);}
    }

    fn retire_reservations(&mut self, released:BTreeSet<(u64,u64)>) {
        for (task,attempt) in released {
            if self.kids.iter().any(|k|k.task==task && k.attempt==attempt)
                || self.observations.iter().any(|((n,_),x)|*n==task && x.attempt==attempt)
                || self.held.iter().any(|l|l.task==task && l.attempt==attempt) {continue;}
            self.reserved.retain(|(n,a,_)|*n!=task || *a!=attempt);
            self.pending.remove(&(task,attempt));
        }
    }

    /// New capacity config (§ 3): a gate change starts held.
    pub fn reload(&mut self, cap: Result<Capacity, String>) {
        if let Ok(c) = &cap {
            self.gate.configure(c.gate);
        }
        self.cap = cap;
    }

    #[tracing::instrument(name="service.Service.may_launch",skip_all)]
    fn may_launch(&self) -> bool {
        let refusal = if self.hold.is_some() { Some("hold") }
            else if self.cap.is_err() { Some("config") }
            else if !self.gate.may_launch(self.mono) {
                Some(self.gate.state(self.mono).1.first().map_or("launch-interval", |r| r.0))
            } else { None };
        if let Some(code) = refusal { tracing::info!(coreq_refusal = code); }
        refusal.is_none()
    }

    fn auto(&self) -> bool {
        self.cap.as_ref().is_ok_and(|c| c.auto)
    }

    /// This service's entry in the host authority: children and granted reservations are held.
    fn entry(&self) -> Entry {
        let held = self.kids.iter().map(|k| (k.task, k.attempt, k.rt.clone())).chain(self.reserved.iter().cloned())
            .collect::<BTreeSet<_>>().into_iter().collect();
        Entry { version: self.version.clone(), at: 0, held, requested: self.requested.clone() }
    }

    /// Lost native handles still hold reservations and must retain their recovery inputs.
    pub fn retained_attempts(&self)->Vec<u64> {
        self.entry().held.into_iter().map(|(_,attempt,_)|attempt).collect()
    }

    fn observe_ownership(&mut self, views:&[View]) -> Result<(),String> {
        let mut released=BTreeSet::new();
        for task in self.observations.keys().map(|(task,_)|*task).collect::<BTreeSet<_>>() {
            let read;
            let log=if let Some(v)=views.iter().find(|v|v.issue.number==task) {&v.log}
                else {read=crate::board::entries(self.board,task)?;&read};
            let current=core::executions(task,log);
            // Open-list absence, label removal and missing history never release known ownership.
            self.observations.retain(|(n,id),old| {
                let terminal=*n==task && !self.kids.iter().any(|k|k.task==task && k.execution==*id)
                    && current.iter().any(|x|x.id==*id && x.attempt==old.attempt && x.host==old.host && x.runtime==old.runtime && x.release.is_some());
                if terminal {released.insert((task,old.attempt));}
                !terminal
            });
        }
        self.observations.extend(views.iter().flat_map(|v|core::executions(v.issue.number,&v.log).into_iter()
            .filter(|x|x.host==self.cfg.me && x.release.is_none()).map(|x|((v.issue.number,x.id),x))));
        self.retire_reservations(released);
        for ((task,_),x) in &self.observations {
            let reservation=(*task,x.attempt,x.runtime.clone());
            if !self.kids.iter().any(|k|k.task==*task && k.execution==x.id) && !self.reserved.contains(&reservation) {self.reserved.push(reservation);}
        }
        self.ownership_known=true;
        Ok(())
    }

    /// Recovery monitoring reads ownership only; it cannot resume ordinary task effects.
    pub fn reconcile_ownership(&mut self) -> Result<(),String> {
        if self.ownership_known && (self.handoff_waits().is_empty()
            || self.tasks_read.is_some_and(|at|at.elapsed().as_secs()<self.cfg.poll.max(1))) {return Ok(());}
        self.ownership_known=false;
        let (views,hosts)=views(self.board,&mut self.cache,&mut self.deps)?;
        let now=self.board.now()?;
        self.observe_ownership(&views)?;
        self.snapshot=Some(crate::snapshot::from_views("host",self.board,&self.cfg,now,&views,&hosts));
        self.tasks_read=Some(std::time::Instant::now());
        Ok(())
    }

    /// Handles may cross a qualified image switch; childless uncertainty cannot.
    pub fn handoff_waits(&self) -> Vec<String> {
        let owns=|task,attempt|self.kids.iter().any(|k|k.task==task && k.attempt==attempt);
        let mut waits=BTreeSet::new();
        if !self.ownership_known {waits.insert("ownership unknown".into());}
        for (task,attempt) in self.reserved.iter().map(|(n,a,_)|(*n,*a))
            .chain(self.pending.iter().copied()).chain(self.held.iter().map(|l|(l.task,l.attempt))) {
            if !owns(task,attempt) {waits.insert(format!("#{task} unresolved ownership"));}
        }
        for ((task,id),_) in &self.observations {
            if !self.kids.iter().any(|k|k.task==*task && k.execution==*id) {waits.insert(format!("#{task} unresolved execution {id}"));}
        }
        for (task,_,_) in &self.pending_releases {waits.insert(format!("#{task} release unconfirmed"));}
        for attempt in self.unknown.keys().chain(self.stopping.iter()) {
            if !self.kids.iter().any(|k|k.attempt==*attempt) {waits.insert(format!("attempt {attempt} ownership unknown"));}
        }
        waits.into_iter().collect()
    }

    pub fn idle(&self) -> bool {self.kids.is_empty() && self.handoff_waits().is_empty()}

    /// Admission of one launch (§ 3): the load gate, then under Auto a reservation from the host authority, which
    /// refuses with a reason; the request stays `requested`. Fixed admits as before.
    fn reserve(&mut self, task: u64, attempt: u64, rt: &str) -> bool {
        if self.legacy_prepared||self.legacy_handoff&&attempt==0 {
            tracing::info!(coreq_refusal="handoff");
            self.refused=Some(("handoff","legacy service draining; no new attempt claims".into()));
            return false;
        }
        if !self.may_launch() {
            return false;
        }
        if let Some(host) = self.host.clone() {
            let r = host.lock().map_err(|_| "host admission poisoned; launches held".to_string()).and_then(|mut h| {
                let now = h.elapsed();
                h.reserve(&self.cfg.root, task, attempt, rt, self.cap.as_ref().unwrap(), attempt == 0, now)
            });
            return match r {
                Ok(used) => {
                    let res = (task, attempt, rt.to_string());
                    if !self.reserved.contains(&res) { self.reserved.push(res); }
                    self.host_used = Some(used as u32);
                    true
                }
                Err(e) => { self.refused = Some(("host", e)); false }
            };
        }
        if !self.auto() { return true; }
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
        if let Some(host) = &self.host {
            let recovered=self.handoff_waits().is_empty();
            let held=self.entry().held;
            match host.lock() {
                Ok(mut h) => {
                    if recovered {h.recovered.insert(self.cfg.root.clone());}else {h.recovered.remove(&self.cfg.root);}
                    if h.rebuilding&&self.ownership_known {h.rebuild_slots(&self.cfg.root,held);}else {h.sync(&self.cfg.root,held);}
                    self.host_used=(!h.rebuilding).then(||h.used().values().sum::<usize>() as u32);
                }
                Err(e) => {
                    self.host_used = None;
                    self.refused = Some(("host", "host admission poisoned; rebuilding from owned handles".into()));
                    let mut h = e.into_inner();
                    h.reload(crate::host::Policy::read(&self.cfg.install));
                    if h.rebuild_slots(&self.cfg.root, self.entry().held) { host.clear_poison(); }
                }
            }
            return;
        }
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
        if self.legacy_handoff {
            if matches!(gate,"open"|"off") {gate="held";}
            reasons.insert(0,("handoff","legacy service draining; owned attempts remain supervised".into()));
        }
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
        for (_, _, rt) in self.entry().held {
            *used.entry(rt).or_default() += 1;
        }
        let last = self.gate.last.filter(|_| self.gate.enabled());
        let (host_policy,host_used,host_command)=self.host.as_ref().and_then(|h|h.lock().ok().map(|h| {
            if h.rebuilding {gate="unknown";reasons.push(("host","host occupancy recovery pending".into()));}
            if let Some(reason)=&h.held {gate="unknown";reasons.push(("host",reason.clone()));}
            (h.policy.as_ref().map(|p|p.cap.json()).unwrap_or(Value::Null),if h.rebuilding {Value::Null}else{json!(h.used())},json!(h.capacity_command))
        })).unwrap_or((Value::Null,Value::Null,Value::Null));
        json!({"host_policy":host_policy,"host_runtime_used":host_used,"host_command":host_command,"project_limits":self.cap.as_ref().ok().map(|_| &self.cfg.limits),"limits": limits, "used": used, "load_gate": self.cap.as_ref().map(|c| c.json()["load_gate"].clone()).unwrap_or(Value::Null),
            "admission": self.cap.as_ref().map(|c| c.json()["admission"].clone()).unwrap_or(Value::Null), "host_used": self.host_used,
            "min_service_version": crate::admission::MIN_SERVICE, "reservations": reservations,
            "gate": gate, "reasons": reasons.iter().map(|(c, t)| json!({"code": c, "text": t})).collect::<Vec<_>>(),
            "cpu_percent": last.and_then(|l| l.2), "memory_available_percent": last.and_then(|l| l.3),
            "sample_at": last.map(|l| l.1), "sample_age_seconds": last.map(|l| self.mono.saturating_sub(l.0))})
    }

    /// Handover carries execution identity; only board acceptance establishes delivery.
    pub fn adopt(&mut self, kid: Kid) -> Result<(),String> {
        let log = crate::board::entries(self.board,kid.task)?;
        let x=core::executions(kid.task,&log).into_iter().find(|x|x.id==kid.execution && x.attempt==kid.attempt && x.host==self.cfg.me && x.runtime==kid.rt && x.release.is_none())
            .ok_or_else(||format!("execution {} handover does not match unresolved board ownership",kid.execution))?;
        if self.kids.iter().any(|k|k.task==kid.task || k.execution==kid.execution) { return Err("duplicate native handover".into()); }
        if let Some(through) = core::delivered(kid.task, &log, kid.attempt) { self.delivered.insert(kid.attempt, through); }
        // the role is the attempt log that exists; its start and offset stayed in the replaced image
        let role = if crate::runtime::attempt_log(&self.cfg.root, "reviewer", kid.task, kid.attempt).exists() { "reviewer" } else { "worker" };
        self.begun.insert(kid.attempt, (role.into(), kid.rt.clone(), None));
        self.observations.insert((kid.task,kid.execution),x);
        self.kids.push(kid);
        Ok(())
    }

    pub fn delivered(&self, attempt: u64) -> u64 {
        self.delivered.get(&attempt).copied().unwrap_or(0)
    }

    /// The `build` or `review` stage of a child that exited or was stopped (§ 4 Profiling); `v` is its task's view.
    fn ended(&mut self, n: u64, a: u64, v: Option<&View>, failure: Option<&'static str>) {
        let Some((role, rt, begun)) = self.begun.remove(&a) else { return };
        // an issue that left the board read (closed, label removed) is read back once for its hand-in
        let back = if v.is_none() { crate::board::entries(self.board, n).map_err(|_| "unreadable") } else { Ok(vec![]) };
        let log = match v {
            Some(v) => Ok((v.log.as_slice(), Some(v.task.attempt == Some(a)))),
            None => back.as_deref().map(|l| (l, None)).map_err(|e| *e),
        };
        let (outcome, err) = core::stage_outcome(n, log, a, a.max(self.delivered(a)));
        let err = if outcome == "failed" { failure.unwrap_or(err) } else { err };
        let log = crate::runtime::attempt_log(&self.cfg.root, &role, n, a);
        let stage = if role == "reviewer" { "review" } else { "build" };
        let (took, usage) = match begun {
            Some((began, pos)) => (Some(began.elapsed()), crate::runtime::usage(&log, pos, &rt)),
            None => (None, crate::profile::Usage::Unknown("adopted")),
        };
        crate::profile::stage(stage, n, a, &rt, took, outcome, err, usage);
    }

    /// The worker's last message in the attempt log as a public `gone` reason line of at most 240 characters
    /// (§ 4 step 1); empty when unreadable or blank.
    fn last_message(&self, n: u64, a: u64, t: &Task) -> String {
        const LABEL: &str = "Last message: ";
        let log = crate::runtime::attempt_log(&self.cfg.root, t.role.as_deref().unwrap_or_default(), n, a);
        let line = crate::runtime::last_message(&log).map(|m| crate::runtime::public_line(&m, &self.cfg.root, 240 - LABEL.len()));
        line.filter(|m| !m.is_empty()).map_or_else(String::new, |m| format!("\n{LABEL}{m}"))
    }

    /// `gone attempt a` on `t`; at the retry limit it carries the § 2 limit card (#1123).
    fn gone(&mut self, n: u64, t: &Task, a: u64, text: &str) -> Result<u64, String> {
        let id = self.post(n, core::entry("gone", &[], Some(a), &core::limit_text(n, t, "gone", text)))?;
        if let Ok(log) = crate::board::entries(self.board, n) {
            let through: Vec<_> = log.into_iter().filter(|e| e.id <= id).collect();
            let before: Vec<_> = through.iter().filter(|e| e.id < id).cloned().collect();
            if core::fold(n, &before, false).attempt == Some(a) && core::fold(n, &through, false).attempt != Some(a) {
                crate::profile::repair(&self.cfg.install, &self.cfg.repo, host_of(t).unwrap_or(&self.cfg.me), n, a,
                    rt_of(t).map_or("other", |r| r.0), t.role.as_deref() == Some("reviewer"));
            }
        } else {
            eprintln!("Profiling: unknown (gone readback failed; terminal record not repaired)");
        }
        Ok(id)
    }

    fn post(&mut self, n: u64, body: String) -> Result<u64, String> {
        self.log.push(format!("#{n} {}", body.lines().next().unwrap_or_default()));
        self.cache.remove(&n);
        self.board.comment(n, &body)
    }

    fn execution_statuses(&self)->Vec<crate::snapshot::ExecutionStatus> {
        self.observations.iter().map(|((task,_),x)| {
            let child=self.kids.iter().any(|k|k.execution==x.id && k.task==*task);
            let (state,reason)=if let Some(e)=self.unknown.get(&x.attempt) {("unknown",e.clone())}
                else if self.stopping.contains(&x.attempt) {("waiting","cancellation pending; awaiting native termination".into())}
                else if self.held.iter().any(|l|l.task==*task && l.attempt==x.attempt) {("waiting","launch held by admission gate".into())}
                else if !child {("unknown",self.execution_waits.get(task).cloned().unwrap_or("native observation unavailable; restore owning runtime".into()))}
                else if self.delivered(x.attempt)<x.through {("waiting","native input acceptance pending".into())}
                else if self.snapshot.as_ref().is_some_and(|s|s.tasks.iter().any(|r|r.n==*task && r.task.state==State::Ask)) {("waiting","owner answer required".into())}
                else {("running","native execution observed; termination pending".into())};
            crate::snapshot::ExecutionStatus {task:*task,attempt:x.attempt,id:x.id,through:x.through,state:state.into(),
                reason:reason.chars().map(|c|if c.is_control() {' '} else {c}).take(240).collect()}
        }).collect()
    }

    fn heartbeat_body(&self) -> String {
        let mut lines = vec![format!("version {}", self.version)];
        if !self.instance.is_empty() {
            lines.push(format!("service {}", self.instance));
        }
        if !self.machine.is_empty() {
            lines.push(format!("machine {}", self.machine));
        }
        if self.focus.is_empty() {
            lines.push(format!("focus {}", if self.focus_error.is_some() { "unknown" } else { "off" }));
        }
        let kids: BTreeSet<_> = self.kids.iter().map(|k| (k.task, k.attempt)).chain(self.pending.iter().copied()).collect();
        lines.extend(kids.iter().map(|(t, a)| format!("#{t} {a}")));
        lines.extend(kids.iter().filter_map(|(t, a)| self.sessions.get(a).map(|u| format!("session #{t} {a} {u}"))));
        lines.extend(self.execution_statuses().iter().map(|s|format!("execution {}",serde_json::to_string(s).unwrap())));
        lines.join("\n")
    }

    /// One pass of § 4. Tasks written to in a step are left alone by later steps until the next pass re-reads them.
    #[tracing::instrument(name="service.Service.pass",skip_all)]
    pub fn pass(&mut self) -> Result<(), String> {
        self.ownership_known = false;
        if self.host.is_none() {
            if let Err(e) = crate::profile::prune(&self.cfg.install) { eprintln!("taskq: telemetry unknown: {e}"); }
        }
        let r = crate::profile::measure("service.pass", || self.pass_inner(NATIVE_PASS_BUDGET));
        if r.is_err() { self.unready(); }
        let observations=self.execution_statuses();
        if let Some(snapshot) = &mut self.snapshot {
            for row in &mut snapshot.tasks {
                row.executions.retain(|value|value["host"].as_str()!=Some(&self.cfg.me)
                    || observations.iter().any(|s|s.task==row.n && Some(s.id)==value["id"].as_u64()));
                for value in &mut row.executions {
                    if let Some(status)=observations.iter().find(|s|s.task==row.n && Some(s.id)==value["id"].as_u64()
                        && Some(s.through)==value["through"].as_u64()) {
                        *value=serde_json::to_value(status).unwrap();
                        value["host"]=json!(self.cfg.me);value["observed"]=json!(snapshot.observed);
                    }
                }
                if row.executions.iter().any(|x|x["host"].as_str()==Some(&self.cfg.me)) {
                    row.execution_wait=row.executions.iter().find(|x|x["state"]!="running" || x["attempt"].as_u64()!=row.task.attempt)
                        .map(|x|format!("execution {} on {}: {}: {}",x["id"],self.cfg.me,
                            x["state"].as_str().unwrap_or("unknown"),x["reason"].as_str().unwrap_or("observation unavailable")));
                }
                if !self.kids.iter().any(|k| k.task == row.n && row.task.attempt == Some(k.attempt)) {
                    if let Some(reason)=self.execution_waits.get(&row.n) {row.execution_wait=Some(reason.clone());}
                }
            }
        }

        r
    }

    fn pass_inner(&mut self, native_budget:std::time::Duration) -> Result<(), String> {
        for (task, attempt, execution) in self.pending_releases.clone() {
            release_execution(self.board, task, attempt, execution, "no-start")?;
            self.pending_releases.remove(&(task, attempt, execution));
            self.cache.remove(&task);
        }
        let previous_fixes = std::mem::take(&mut self.label_fixes);
        let me = self.cfg.me.clone();
        let ttl = self.cfg.ttl;
        let (views, hosts) = views(self.board, &mut self.cache, &mut self.deps)?;
        let now = self.board.now()?;
        self.observe_ownership(&views)?;
        self.snapshot = Some(crate::snapshot::from_views("host", self.board, &self.cfg, now, &views, &hosts));
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
        let mut released = HashSet::new();
        self.execution_waits.clear();
        let native_until=std::time::Instant::now()+native_budget;
        // Cold scans leave time for already owned work and control.
        let recovery_until=std::time::Instant::now()+native_budget/4;
        let pivot=self.recovery_after.and_then(|n|views.iter().position(|v|v.issue.number==n)).map_or(0,|i|i+1);
        for v in views[pivot..].iter().chain(&views[..pivot]) {
            let task = v.issue.number;
            let unresolved: Vec<_> = core::executions(task, &v.log).into_iter().filter(|x| x.release.is_none()).collect();
            if self.kids.iter().any(|k| k.task == task) || self.held.iter().any(|l| l.task == task) { continue; }
            let Some(x) = unresolved.iter().find(|x| x.host == me && matches!(x.runtime.as_str(), "claude" | "codex-app")) else { continue };
            if unresolved.len() != 1 {
                self.execution_waits.insert(task, "multiple unresolved executions; verify native ownership before recovery".into());
                continue;
            }
            let Some(recover) = &mut self.recover else { continue };
            if std::time::Instant::now()>=recovery_until {
                self.execution_waits.insert(task,format!("execution {}: native observation deferred; recovery budget exhausted",x.id));
                continue;
            }
            self.recovery_after=Some(task);
            let result = crate::runtime::observe_until(recovery_until,||recover(task,x))
                .and_then(|proc| self.adopt(Kid { task, attempt: x.attempt, execution: x.id, rt: x.runtime.clone(), proc }));
            if let Err(e) = result {
                self.execution_waits.insert(task, format!("execution {} on {}: recovery unknown: {e}", x.id, x.host));
            }
        }

        self.hold = crate::admission::hold(&self.cfg.install);
        let mut pending_acceptance = HashSet::new();
        // 1. Observe native state once; retry lost evidence writes without resubmitting input.
        let mut exited = vec![];
        let unknown = &mut self.unknown;
        let stopping = &mut self.stopping;
        let needs_stop=self.kids.iter().any(|k|!by_n.get(&k.task).is_some_and(|v|v.task.attempt==Some(k.attempt)));
        let observation_until=if needs_stop {std::time::Instant::now()+native_until.saturating_duration_since(std::time::Instant::now())/2} else {native_until};
        let mut deferred=None;
        self.kids.retain_mut(|k| {
            if std::time::Instant::now()>=observation_until {
                deferred.get_or_insert(k.execution);
                unknown.insert(k.attempt,"native observation deferred; pass budget exhausted".into());return true;
            }
            let status = crate::runtime::observe_until(observation_until,||k.proc.exited());
            let confirmed = by_n.get(&k.task).and_then(|v| core::executions(k.task, &v.log).into_iter().find(|x| x.id == k.execution && x.accepted));
            let accepted = if let Some(x) = confirmed { Ok(Some(x.through)) } else {
                crate::runtime::observe_until(observation_until,||k.proc.accepted()).and_then(|yes| if yes {
                    accept_execution(self.board, k.task, k.attempt, k.execution).map(Some).inspect_err(|_| { pending_acceptance.insert(k.attempt); })
                } else { Ok(None) })
            };
            let acceptance_unknown = accepted.is_err();
            match accepted {
                Ok(Some(through)) => { self.delivered.entry(k.attempt).and_modify(|v| *v = (*v).max(through)).or_insert(through); self.cache.remove(&k.task); },
                Ok(None) => {},
                Err(e) => { unknown.insert(k.attempt, e); },
            }
            match status {
            Ok(Some(code)) => {
                if pending_acceptance.contains(&k.attempt) { return true; }
                if let Err(e)=release_execution(self.board,k.task,k.attempt,k.execution,"exit") {unknown.insert(k.attempt,e);return true;}
                if matches!(k.rt.as_str(),"codex"|"claude") { if let Err(e)=crate::retire_kid(&self.cfg,k.task,k.attempt,k.execution) { self.log.push(format!("native recovery cleanup: {e}")); } }
                self.observations.remove(&(k.task,k.execution));released.insert(k.execution);self.cache.remove(&k.task);
                exited.push((k.task, k.attempt, code, k.proc.exit_error()));
                unknown.remove(&k.attempt);
                stopping.remove(&k.attempt);
                false
            }
            Ok(None) => {
                if !stopping.contains(&k.attempt) && !acceptance_unknown { unknown.remove(&k.attempt); }
                true
            }
            Err(e) => {
                unknown.insert(k.attempt, e);
                true
            }
            }
        });
        for (n, a, code, failure) in exited {
            self.ended(n, a, by_n.get(&n).copied(), failure);
            if let Some(v) = by_n.get(&n)
                && v.task.attempt == Some(a)
                && matches!(v.task.state, State::Doing | State::Review)
                && !core::handed_in(n, &v.log, a, a.max(self.delivered(a)))
            {
                let text = format!("exit {code}{}", self.last_message(n, a, &v.task));
                self.gone(n, &v.task, a, &text)?;
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
        let mut stopped = vec![];
        self.kids.retain_mut(|k| {
            let current = by_n.get(&k.task).is_some_and(|v| v.task.attempt == Some(k.attempt));
            if !current {
                self.stopping.insert(k.attempt);
                if std::time::Instant::now()>=native_until {
                    deferred.get_or_insert(k.execution);
                    self.unknown.insert(k.attempt,"native observation deferred; cancellation pending".into());return true;
                }
                match crate::runtime::observe_until(native_until,||crate::runtime::stop(k.proc.as_mut())) {
                    Ok(_) if pending_acceptance.contains(&k.attempt) => return true,
                    Ok(_) => match release_execution(self.board,k.task,k.attempt,k.execution,"exit") {
                        Ok(())=>{
                            if matches!(k.rt.as_str(),"codex"|"claude") { if let Err(e)=crate::retire_kid(&self.cfg,k.task,k.attempt,k.execution) { self.log.push(format!("native recovery cleanup: {e}")); } }
                            self.observations.remove(&(k.task,k.execution));released.insert(k.execution);self.cache.remove(&k.task);stopped.push((k.task,k.attempt));self.stopping.remove(&k.attempt);
                        },
                        Err(e)=>{self.unknown.insert(k.attempt,e);return true;}
                    },
                    Err(e) => { self.unknown.insert(k.attempt, e); return true; }
                }
            }
            current
        });
        for (n, a) in stopped {
            self.ended(n, a, by_n.get(&n).copied(), None);
        }
        self.held.retain(|l| by_n.get(&l.task).is_some_and(|v| v.task.attempt == Some(l.attempt)));
        let kids: Vec<u64> = self.kids.iter().map(|k| k.attempt).collect();
        self.unknown.retain(|a, _| kids.contains(a));
        // A Claude Remote Control URL is account-scoped: the heartbeat publishes it so every host's tray links it (#1204).
        // shortcut: bounded transcript scans restart until a URL appears; retain a cursor if these become costly.
        self.sessions.retain(|a, _| kids.contains(a));
        let found: Vec<(u64, String)> = self.kids.iter().filter(|k| std::time::Instant::now()<native_until && k.rt == "claude" && !self.sessions.contains_key(&k.attempt))
            .filter_map(|k| by_n.get(&k.task).filter(|v| v.task.attempt == Some(k.attempt))?.task.session.as_deref()?.split(' ').next()?.strip_prefix("claude:")
                .and_then(|id|crate::runtime::observe_until(native_until,||crate::runtime::claude_remote(id))).map(|u| (k.attempt, u)))
            .collect();
        self.sessions.extend(found);
        if let Some(first)=deferred.and_then(|id|self.kids.iter().position(|k|k.execution==id)) {self.kids.rotate_left(first);}

        // 4. Expired heartbeat is not evidence that an external executor stopped.
        // A lease newer than the accepted attempt is a spawn that lost the claim and still runs. An older one is a
        // replaced attempt its host lists until its next pass: normal lag, not a second live attempt (#1140).
        for hb in hosts.iter().filter(|h| now - h.updated <= ttl) {
            for line in hb.body.lines() {
                let Some((n, a)) = line.strip_prefix('#').and_then(|l| l.split_once(' ')) else { continue };
                let (Ok(n), Ok(a)) = (n.parse::<u64>(), a.parse::<u64>()) else { continue };
                if by_n.get(&n).and_then(|v| v.task.attempt).is_some_and(|c| a > c) {
                    crate::incident::detect("invariant.double_attempt", concat!("src/service.rs:", line!()), "none", &[], &["heartbeat:conflict"]);
                }
            }
        }
        let mut unresolved_local = BTreeSet::new();
        for ((task,_),x) in &self.observations {
            let reservation=(*task,x.attempt,x.runtime.clone());
            unresolved_local.insert(reservation.clone());
            if !self.kids.iter().any(|k|k.task==*task && k.execution==x.id) && !self.reserved.contains(&reservation) {self.reserved.push(reservation);}
        }
        for v in &views {
            let (n,t) = (v.issue.number,&v.task);
            if let Some(a) = t.attempt.filter(|_| host_of(t) != Some(&me) && matches!(t.state,State::Doing|State::Review)) {
                let delivered = core::delivered(n,&v.log,a).unwrap_or(0);
                if t.inbox.iter().all(|id| *id <= delivered) {
                    let executions = core::executions(n,&v.log);
                    if let Some(x) = executions.iter().rev().find(|x| x.attempt == a).filter(|_| executions.iter().all(|x| x.release.is_some())) {
                        if !core::handed_in(n,&v.log,a,x.through) {
                            self.gone(n,t,a,&format!("execution {} confirmed {:?} without task outcome",x.id,x.release.unwrap()))?;
                            touched.insert(n);
                        }
                    }
                }
            }
            let unresolved: Vec<_> = core::executions(v.issue.number, &v.log).into_iter()
                .filter(|x| x.release.is_none() && !released.contains(&x.id)).collect();
            if let Some(x) = unresolved.first().filter(|x| !self.held.iter().any(|l| l.attempt == x.id)) {
                self.execution_waits.entry(v.issue.number).or_insert_with(|| format!("execution {} on {}: termination unconfirmed", x.id, x.host));
            } else if let Some(x) = core::executions(v.issue.number, &v.log).into_iter()
                .find(|x| Some(x.attempt) == v.task.attempt && !v.task.inbox.is_empty() && x.release == Some(core::Release::Exit) && !x.accepted) {
                self.execution_waits.insert(v.issue.number, format!("execution {} on {}: input acceptance unknown; reconcile native input", x.id, x.host));
            } else if let (Some(a),Some(owner)) = (v.task.attempt,host_of(&v.task)) {
                if owner != me && matches!(v.task.state,State::Doing|State::Review) && v.task.inbox.iter().any(|id| *id > core::delivered(v.issue.number,&v.log,a).unwrap_or(0)) {
                    self.execution_waits.insert(v.issue.number,format!("input pending on {owner}; await or restore the owning service"));
                }
            }
        }

        // 3. heartbeat
        let body = self.heartbeat_body();
        self.host_issue = hosts.iter().find(|h| h.title == title).map(|h| h.number);
        match hosts.iter().find(|h| h.title == title) {
            Some(h) if h.body.lines().filter(|l| !l.starts_with("at ")).eq(body.lines()) && now - h.updated < 60.min(ttl / 3) => {}
            // a same-body edit leaves `updated_at` as is (GitHub), so the stamp keeps the heartbeat fresh
            Some(h) => self.board.edit(h.number, &format!("{body}\nat {now}"))?,
            None => {
                self.host_issue = Some(self.board.create(&title, &body, &[HOST_LABEL.to_string()])?);
            }
        }

        crate::profile::repair_ended(&self.cfg.install, &self.cfg.root, &self.cfg.repo, &me, self.board);
        self.prune(&by_n);

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
        self.reserved.retain(|r| self.held.iter().any(|l| l.task == r.0 && l.attempt == r.1) || unresolved_local.contains(r));
        (self.requested, self.refused) = (vec![], None);
        if self.host.is_some() { self.sync_host(); }
        for v in &views {
            let (n, t) = (v.issue.number, &v.task);
            let (Some(a), Some(h)) = (t.attempt, host_of(t)) else { continue };
            if h != me || !matches!(t.state, State::Doing | State::Review) || touched.contains(&n) || self.kids.iter().any(|k| k.task == n) {
                continue;
            }
            if let Some(i) = self.held.iter().position(|l| l.attempt == a) {
                let rt = self.held[i].rt.clone();
                if self.reserve(n, a, &rt) {
                    let l = self.held.remove(i);
                    self.launch(l, a)?;
                } else {
                    pending.insert((n, a));
                }
                continue;
            }
            if self.execution_waits.contains_key(&n) { pending.insert((n, a)); continue; }
            let seen = core::delivered(n, &v.log, a).unwrap_or(0);
            let new: Vec<&Comment> = v.comments.iter().filter(|c| t.inbox.contains(&c.id) && c.id > seen).collect();
            let Some(last) = new.last().map(|c| c.id) else {
                let last = core::executions(n, &v.log).into_iter().rev().find(|x| x.attempt == a);
                if let Some(x) = last.filter(|x| x.release.is_some()) {
                    let text = format!("execution {} confirmed {:?} without task outcome{}", x.id, x.release.unwrap(), self.last_message(n, a, t));
                    self.gone(n, t, a, &text)?;
                    touched.insert(n);
                } else {
                    self.execution_waits.insert(n,"native execution evidence missing; reconcile the owning runtime".into());
                }
                continue;
            };
            let (rt, session) = rt_of(t).unwrap_or(("", ""));
            let mut session = session.to_string();
            let mut l = Launch { execution: None, task: n, title: v.issue.title.clone(), attempt: a, role: t.role.clone().unwrap_or_default(), rt: rt.into(), session: session.clone(), url: self.board.url(n), resume: None };
            if session == "-" || rt == "claude" {
                session = crate::runtime::codex_thread(&crate::runtime::log_path(&self.cfg.root, &l)).unwrap_or_else(|| if session == "-" { String::new() } else { session });
            }
            let text: Vec<String> = new.iter().map(|c| core::delivery(n, &v.log, c.id)).collect();
            l.session = session;
            l.resume = Some(text.join("\n\n"));
            if l.session.is_empty() {
                self.execution_waits.insert(n,"session identity unknown; recover the original native session".into());
                pending.insert((n,a));
                continue;
            }
            if self.refused_access.contains_key(&l.rt) || !self.reserve(n, a, &l.rt) {
                pending.insert((n, a)); // the inbox stays undelivered
                continue;
            }
            self.launch(l, last)?;
        }
        pending.extend(self.held.iter().map(|l| (l.task, l.attempt)));
        if pending != self.pending {
            self.pending = pending;
            self.beat()?;
        }
        if self.legacy_handoff {
            self.unready();
            tracing::info!(coreq_refusal="handoff");
            return Ok(());
        }
        if self.focus.is_empty() {
            tracing::info!(coreq_refusal = "focus");
            if let Some(host) = &self.host { if let Ok(mut h) = host.lock() { h.ready(&self.cfg.root, false); } }
            return Ok(());
        }
        let user = self.board.me().unwrap_or_default();
        let mut queue: Vec<&View> = views
            .iter()
            .filter(|v| !touched.contains(&v.issue.number) && !self.execution_waits.contains_key(&v.issue.number))
            .filter(|v| !self.kids.iter().any(|k| k.task == v.issue.number))
            .filter(|v| v.task.state == State::Ready || (v.task.state == State::Review && v.task.attempt.is_none()))
            .filter(|v| {
                if let Err(e) = crate::labels::validate(&v.issue.labels) { log!("#{}: {e}; not admitted", v.issue.number); return false; }
                true
            })
            .filter(|v| label(&v.issue, "host-").is_none_or(|h| h == me))
            // `needs-gate`: every host of this version has gates; N-1 lacks them and leaves the task alone (§ 8)
            .filter(|v| v.issue.labels.iter().filter_map(|l| l.strip_prefix("needs-")).all(|x| x == "gate" || self.cfg.has.iter().any(|h| h == x)))
            .filter(|v| priority(&v.issue) == 0 || self.focus.iter().any(|f| f == "all" || crate::labels::matches(&v.issue.labels, f)))
            // last: the github.com list carries no assignees (§ 4 budget), so only a candidate reads them
            .filter(|v| !v.issue.labels.iter().any(|l| l == "assignee-only") || assigned(self.board, &v.issue, &user).unwrap_or(false))
            .collect();
        queue.sort_by_key(|v| (priority(&v.issue), v.issue.number));
        let ready = queue.iter().any(|v| self.free(label(&v.issue, "run-").as_deref()).is_some());
        if let Some(host) = &self.host {
            if let Ok(mut h) = host.lock() { h.ready(&self.cfg.root, ready); }
        }
        for v in queue {
            if !self.may_launch() {
                break;
            }
            let role = if v.task.state == State::Review { "reviewer" } else { "worker" };
            let rt = if role == "reviewer" { self.reviewer_free(v) } else { self.worker_free(v) };
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
        let (mut moves, mut closed) = (vec![], vec![]);
        for n in ns {
            let (to, a) = match views.iter().find(|v| v.issue.number == n) {
                // `ask` and `waiting` without an attempt: the last comment, else the issue number
                Some(v) if matches!(v.task.state, State::Ask | State::Waiting) => (v.task.state, attempt(&v.task, &v.comments).or(v.comments.last().map(|c| c.id)).or(Some(n))),
                Some(v) => (v.task.state, attempt(&v.task, &v.comments)),
                None if self.board.issue(n).is_ok_and(|i| !i.open) => {
                    let c = self.board.comments(n)?;
                    let a = attempt(&Task::default(), &c);
                    closed.push((n, c));
                    (State::Closed, a)
                }
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
        // `gone`: each trusted `gone attempt` entry newer than the previous pass; the first pass adds none
        let newest = views.iter().flat_map(|v| v.comments.iter().map(|c| c.id)).max();
        if let Some(after) = self.gone_after {
            for v in views {
                for c in v.comments.iter().filter(|c| c.trusted && c.id > after) {
                    if let Some(a) = c.body.lines().next().and_then(|l| l.strip_prefix("taskq: gone attempt ")).and_then(|a| a.parse().ok()) {
                        moves.push((v.issue.number, "gone", a));
                    }
                }
            }
        }
        // queue transitions (§ 4 Profiling): accepted entries newer than the previous pass that changed the fold
        if let Some(after) = self.moved_after {
            let logs = views.iter().map(|v| (v.issue.number, &v.comments)).chain(closed.iter().map(|(n, c)| (*n, c)));
            for (n, comments) in logs.filter(|(_, c)| c.iter().any(|c| c.trusted && c.id > after)) {
                let log: Vec<_> = comments.iter().filter(|c| c.trusted).map(|c| core::Entry { id: c.id, body: c.body.clone(), edited: c.edited }).collect();
                let at = |id: u64| comments.iter().find(|c| c.id == id).map(|c| c.created);
                let mut since = None;
                for (id, from, to) in core::changes(n, &log) {
                    let entered = std::mem::replace(&mut since, at(id));
                    // `held` is outside the queue model (§ Executable behavioral model): `hold` and `start` add none
                    if id <= after || from.state == State::Held || to.state == State::Held { continue; }
                    let side = if to.session.is_some() { &to } else { &from };
                    let host = host_of(side);
                    if host.is_none_or(|h| h == self.cfg.me) {
                        let prev = entered.zip(at(id)).and_then(|(a, b)| u64::try_from(b - a).ok());
                        crate::profile::transition(n, id, from.state.name(), to.state.name(), prev, rt_of(side).map(|r| r.0), host);
                    }
                }
            }
        }
        self.gone_after = self.gone_after.max(newest);
        self.moved_after = Some(self.gone_after.unwrap_or(0));
        moves.sort();
        // `ask` stage (§ 4 Profiling): from the move into `ask` of this host's attempt to the move out
        for v in views.iter().filter(|v| v.task.state == State::Ask && self.seen.get(&v.issue.number).is_some_and(|s| *s != State::Ask)) {
            if let (Some(a), Some((rt, _)), true) = (v.task.attempt, rt_of(&v.task), host_of(&v.task) == Some(self.cfg.me.as_str())) {
                self.asked.insert(v.issue.number, (std::time::Instant::now(), a, rt.to_string()));
            }
        }
        let left: Vec<u64> = self.asked.keys().filter(|n| now_seen.get(n) != Some(&State::Ask)).copied().collect();
        for n in left {
            let (began, a, rt) = self.asked.remove(&n).unwrap();
            let outcome = if matches!(now_seen.get(&n), Some(State::Doing | State::Review | State::Ready)) { "ok" } else { "cancelled" };
            crate::profile::stage("ask", n, a, &rt, Some(began.elapsed()), outcome, "none", crate::profile::Usage::NoModel);
        }
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

    /// `run-*` is hard; else a `prefer-*` runtime with a free slot; else the most free slots (§ 4 Filter).
    fn worker_free(&mut self, v: &View) -> Option<String> {
        if let Some(rt) = label(&v.issue, "run-") {
            return self.free(Some(&rt));
        }
        let prefer: Vec<&str> = v.issue.labels.iter().filter_map(|l| l.strip_prefix("prefer-")).collect();
        if prefer.len() > 1 {
            self.log.push(format!("#{}: conflicting prefer-* labels; preference ignored", v.issue.number));
        }
        prefer.first().filter(|_| prefer.len() == 1).and_then(|p| self.free(Some(p))).or_else(|| self.free(None))
    }

    /// A runtime with free capacity: `limits.<rt>` minus live children of it (§ 3).
    /// `codex-app` only for `run-codex-app` tasks on a host whose `has` lists it (§ 9 opt-in).
    fn free(&self, want: Option<&str>) -> Option<String> {
        let app = |rt: &str| rt != "codex-app" || (cfg!(any(target_os = "macos", windows)) && want == Some(rt) && self.cfg.has.iter().any(|h| h == rt));
        let c = self.cap.as_ref().ok()?;
        // Auto: a runtime without a limit has no count ceiling (§ 9); Fixed: only listed runtimes
        let limits: Vec<(String, u32)> = if c.auto { RUNTIMES.iter().map(|r| (r.to_string(), c.limits.get(*r).copied().unwrap_or(u32::MAX))).collect() } else { c.limits.clone().into_iter().collect() };
        // the most free slots wins, so any-runtime tasks alternate; a tie keeps the listed order (§ 4 Filter)
        let mut best: Option<(String, usize)> = None;
        for (rt, n) in limits.into_iter().filter(|(rt, _)| want.is_none_or(|w| w == rt.as_str()) && app(rt) && !self.refused_access.contains_key(rt)) {
            let mut left = (n as usize).saturating_sub(self.entry().held.iter().filter(|r| r.2 == rt).count());
            if let Some(host) = &self.host {
                let h = host.lock().ok()?;
                left = left.min(h.remaining(&self.cfg.root, &rt, c));
            }
            if left > 0 && best.as_ref().is_none_or(|b| left > b.1) {
                best = Some((rt, left));
            }
        }
        best.map(|(rt, _)| rt)
    }

    /// Post `spawn`, re-read, start the child only if `fold` accepted this spawn (§ 3 claim).
    fn claim(&mut self, v: &View, role: &str, rt: &str) -> Result<(), String> {
        let (n, began) = (v.issue.number, std::time::Instant::now());
        let r = crate::profile::measure("dispatch", || self.claim_inner(v, role, rt));
        let held = self.held.iter().find(|l| l.task == n).map(|l| l.attempt);
        let (a, outcome, err) = match &r {
            Ok(Some(l)) => (l.attempt, "ok", "none"),
            Ok(None) => held.map_or((0, "cancelled", "none"), |a| (a, "ok", "none")),
            Err(e) => (0, "failed", crate::profile::class(e)),
        };
        crate::profile::stage("claim", n, a, rt, Some(began.elapsed()), outcome, err, crate::profile::Usage::NoModel);
        match r? {
            Some(l) => { let through = l.attempt; self.launch(l, through) },
            None => Ok(()),
        }
    }

    fn claim_inner(&mut self, v: &View, role: &str, rt: &str) -> Result<Option<Launch>, String> {
        let n = v.issue.number;
        let before = crate::board::entries(self.board, n)?;
        if core::executions(n, &before).iter().any(|x| x.release.is_none()) { return Ok(None); }
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
            self.sync_host();
            self.log.push(format!("#{n} spawn {id} lost the claim"));
            return Ok(None);
        }
        for r in self.reserved.iter_mut().filter(|r| r.0 == n && r.1 == 0) { r.1 = id; }
        if let Some(host) = &self.host { if let Ok(mut h) = host.lock() { h.claimed(&self.cfg.root, n, id); } }
        let l = Launch { execution: None, task: n, title: v.issue.title.clone(), attempt: id, role: role.into(), rt: rt.into(), session, url: self.board.url(n), resume: None };
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

    #[tracing::instrument(name="service.Service.launch",skip_all)]
    fn launch(&mut self, mut l: Launch, through: u64) -> Result<(), String> {
        let log = crate::board::entries(self.board, l.task)?;
        let executions = core::executions(l.task, &log);
        let t = core::fold(l.task, &log, false);
        if t.attempt != Some(l.attempt) || !matches!(t.state, State::Doing | State::Review) {
            if l.resume.is_none() { release_execution(self.board, l.task, l.attempt, l.attempt, "no-start")?; }
            return Ok(());
        }
        let execution = if l.resume.is_some() {
            if executions.iter().any(|x| x.release.is_none()) { return Err(format!("#{} prior execution termination unconfirmed", l.task)); }
            let previous = executions.last().ok_or("execution ownership missing")?.id;
            let op = format!("execute-{}-{through}-{previous}", l.attempt);
            // A previously submitted intent is uncertain, never permission to repeat the external call.
            if log.iter().any(|e| core::op_id(l.task, &e.body) == Some(&op)) { return Err("execution intent already submitted; reconcile native ownership".into()); }
            let id = self.post(l.task, core::with_fields(core::entry("executing", &[&through.to_string()], Some(l.attempt), ""), Some(&op), None))?;
            let fresh = crate::board::entries(self.board, l.task)?;
            if !core::executions(l.task, &fresh).iter().any(|x| x.id == id && x.release.is_none()) { return Err("execution intent readback missing".into()); }
            id
        } else {
            if !executions.iter().any(|x| x.id == l.attempt && x.release.is_none()) { return Err("initial execution ownership missing".into()); }
            if executions.iter().any(|x| x.release.is_none() && x.id != l.attempt) {
                release_execution(self.board, l.task, l.attempt, l.attempt, "no-start")?;
                return Err("previous execution termination unconfirmed".into());
            }
            l.attempt
        };
        // The reservation becomes the child; uncertain handoff keeps its execution fence.
        l.execution=Some(execution);
        self.observations.insert((l.task,execution),core::Execution {id:execution,attempt:l.attempt,through,
            host:self.cfg.me.clone(),runtime:l.rt.clone(),accepted:false,release:None});
        let pos = std::fs::metadata(crate::runtime::attempt_log(&self.cfg.root, &l.role, l.task, l.attempt)).map_or(0, |m| m.len());
        let began = std::time::Instant::now();
        let started = (self.start)(&l).map(|proc| {
            self.kids.push(Kid { execution, task: l.task, attempt: l.attempt, rt: l.rt.clone(), proc });
            if matches!(l.rt.as_str(),"codex"|"claude") {
                // A failed checkpoint retains the handle; polling retries persistence, never launch.
                if let Err(e)=self.kids.last_mut().unwrap().proc.checkpoint(&self.cfg,&l,execution) { self.unknown.insert(l.attempt,e); }
            }
        });
        let err = started.as_ref().err().map_or("none", |e| crate::profile::class(&e.to_string()));
        crate::profile::stage("start", l.task, l.attempt, &l.rt, Some(began.elapsed()), if started.is_ok() { "ok" } else { "failed" }, err, crate::profile::Usage::NoModel);
        match started {
            Ok(()) => {
                self.reserved.retain(|r| r.0 != l.task || r.1 != l.attempt);
                self.sync_host();
                crate::profile::open(&self.cfg.install, &self.cfg.repo, &self.cfg.me, l.task, l.attempt, &l.rt, l.role == "reviewer");
                self.begun.insert(l.attempt, (l.role.clone(), l.rt.clone(), Some((std::time::Instant::now(), pos))));
                self.pending.remove(&(l.task, l.attempt));
                self.gate.launch(self.mono);
                // Keep the owned handle even if acceptance/readback fails.
                let k = self.kids.last_mut().unwrap();
                match k.proc.accepted().and_then(|yes| if yes { accept_execution(self.board, l.task, l.attempt, execution).map(Some) } else { Ok(None) }) {
                    Ok(Some(cursor)) => { self.delivered.insert(l.attempt, cursor); self.cache.remove(&l.task); },
                    Ok(None) => {},
                    Err(e) => { self.unknown.insert(l.attempt, e); },
                }
                self.beat()
            }
            Err(e) => {
                if !e.no_start {
                    let reason = format!("execution {execution} launch outcome unknown: {e}");
                    self.execution_waits.insert(l.task, reason.clone());
                    let reservation = (l.task, l.attempt, l.rt.clone());
                    if !self.reserved.contains(&reservation) { self.reserved.push(reservation); }
                    self.pending.insert((l.task, l.attempt));
                    self.sync_host();
                    self.beat()?;
                    return Err(reason);
                }
                self.pending_releases.insert((l.task, l.attempt, execution));
                release_execution(self.board, l.task, l.attempt, execution, "no-start")?;
                self.pending_releases.remove(&(l.task, l.attempt, execution));
                self.observations.remove(&(l.task,execution));
                self.cache.remove(&l.task);
                self.reserved.retain(|r| r.0 != l.task || r.1 != l.attempt);
                self.sync_host();
                if e.reportable {
                    #[cfg(not(test))]
                    let agents = crate::runtime::agents();
                    #[cfg(test)]
                    let agents = self.agent_status.clone();
                    if agents.as_array().is_some_and(|rows| rows.iter().any(|r| r["runtime"] == l.rt && r["path"].is_string() && r["login"] == "logged in")) {
                        crate::incident::detect("dispatch.start", concat!("src/service.rs:", line!()), &l.rt, &[], &["runtime:installed"]);
                    }
                }
                let text = format!("start failed: {e}");
                // without a fresh fold a gone could park the task on a card without options: post nothing, report the
                // read error; the next pass reconciles the no-start proof and posts the limit card (#1123).
                let log = crate::board::entries(self.board, l.task).map_err(|r| format!("#{} {text}; gone not posted: {r}", l.task))?;
                if l.resume.is_some() {
                    let failures = core::executions(l.task, &log).iter().filter(|x| x.attempt == l.attempt && x.through == through && x.release == Some(core::Release::NoStart)).count();
                    if failures >= core::GONE_MAX as usize {
                        let op = format!("input-retry-{execution}");
                        let card = format!("{text}\nProblem: input launch failed {failures} times; input retained.\n1. Retry undelivered input in this session ★\n2. later: park the task");
                        self.post(l.task, core::with_fields(core::entry("ask", &[], Some(l.attempt), &card), Some(&op), None))?;
                        return Ok(());
                    }
                    return Err(format!("#{} {text}; input retained for retry", l.task));
                }
                self.gone(l.task, &core::fold(l.task, &log, false), l.attempt, &text).map(|_| ())
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
        fn accepted(&mut self) -> Result<bool, String> { Ok(true) }
        fn exited(&mut self) -> Result<Option<i32>, String> {
            // -999: a failed read
            match *self.0.borrow() {
                Some(-999) => Err("read failed".into()),
                c => Ok(c),
            }
        }
        fn kill(&mut self) -> Result<(), String> {
            *self.0.borrow_mut() = Some(-9); Ok(())
        }
        fn id(&self) -> u32 {
            0
        }
    }

    // Chain: cold service -> board ownership -> observer -> terminal receipt. Risk: recovery replays an uncertain submit.
    #[test]
    fn cold_native_observers_never_use_the_launch_path() {
        for rt in ["claude","codex-app"] {
            for case in ["live","unknown","missing","foreign","released","ambiguous"] {
                let b=Fake::default();let c=cfg("mac");
                let n=b.create("T","",&[TASK_LABEL.into()]).unwrap();
                let host=if case=="foreign" {"other"} else {"mac"};
                let a=b.comment(n,&format!("taskq: spawn worker {rt}:session {host}")).unwrap();
                if case=="released" {release_execution(&b,n,a,a,"exit").unwrap();}
                if case=="ambiguous" {
                    b.comment(n,&core::entry("gone",&[],Some(a),"legacy gone without native exit")).unwrap();
                    b.comment(n,&format!("taskq: spawn worker {rt}:second mac")).unwrap();
                    assert_eq!(core::executions(n,&entries(&b,n).unwrap()).iter().filter(|x|x.release.is_none()).count(),2);
                }
                let seen=Rc::new(RefCell::new(Vec::new()));let calls=seen.clone();
                let exit=Rc::new(RefCell::new(if case=="unknown" {Some(-999)} else {None}));let flag=exit.clone();
                let mut s=Service::new(&b,&c,"fixture",Box::new(|_|panic!("recovery must never launch")));
                s.recover=Some(Box::new(move |task,x| {
                    calls.borrow_mut().push((task,x.attempt,x.id,x.runtime.clone()));
                    if case=="missing" {Err("original input unavailable; inspect native submission".into())}
                    else {Ok(Box::new(FakeProc(flag.clone())))}
                }));
                s.pass().unwrap();s.pass().unwrap();
                if matches!(case,"foreign"|"released"|"ambiguous") {
                    assert!(seen.borrow().is_empty(),"{rt}: {case}");
                    if case=="ambiguous" {assert!(s.execution_waits[&n].contains("multiple unresolved"));}
                } else if case=="missing" {
                    assert_eq!(seen.borrow().len(),2,"only observation is retried");
                    assert!(s.kids.is_empty());
                    assert!(s.reserved.iter().any(|r|r.0==n && r.1==a));
                    assert!(s.execution_waits[&n].contains("original input unavailable"));
                    assert!(s.snapshot.as_ref().unwrap().tasks.iter().find(|r|r.n==n).unwrap().execution_wait.as_ref().unwrap().contains("original input unavailable"));
                } else {
                    assert_eq!(*seen.borrow(),vec![(n,a,a,rt.to_owned())]);
                    assert_eq!(s.kids.len(),1);
                    assert!(core::executions(n,&entries(&b,n).unwrap())[0].release.is_none());
                    if case=="unknown" {assert_eq!(s.unknown[&a],"read failed");}
                    *exit.borrow_mut()=Some(0);
                    s.pass().unwrap();
                    assert!(s.kids.is_empty());
                    assert_eq!(core::executions(n,&entries(&b,n).unwrap())[0].release,Some(core::Release::Exit));
                }
            }
        }
    }

    // Chain: re-exec during pending acceptance -> exact execution. Risk: the old delivered cursor adopts execution zero.
    #[test]
    fn handover_uses_execution_identity_before_input_acceptance() {
        let b=Fake::default(); let n=b.create("T","",&[TASK_LABEL.into()]).unwrap(); let c=cfg("mac");
        let a=b.comment(n,"taskq: spawn worker codex:session mac").unwrap();
        accept_execution(&b,n,a,a).unwrap(); release_execution(&b,n,a,a,"exit").unwrap();
        let input=b.comment(n,"taskq: steer\ncontinue").unwrap();
        let execution=b.comment(n,&core::entry("executing",&[&input.to_string()],Some(a),"")).unwrap();
        let mut s=service(&b,&c,Default::default());
        s.adopt(Kid {execution,task:n,attempt:a,rt:"codex".into(),proc:Box::new(FakeProc(Rc::new(RefCell::new(None))))}).unwrap();
        assert_eq!(s.kids[0].execution,execution,"pending acceptance does not change native ownership");
        assert_eq!(s.delivered(a),a,"handover does not invent input acceptance");
    }

    // Chain: undelivered continuation -> no-start -> restart -> retry. Risk: an intent cursor loses input or retries without a bound.
    #[test]
    fn continuation_no_start_retries_retained_input_across_restart() {
        for refused in [1, 2] {
            let b = Fake::default();
            let n = b.create("T", "", &[TASK_LABEL.into()]).unwrap();
            let a = b.comment(n, "taskq: spawn worker codex:session mac").unwrap();
            accept_execution(&b,n,a,a).unwrap(); release_execution(&b,n,a,a,"exit").unwrap();
            let cursor = b.comment(n,"taskq: steer\nretained user input").unwrap();
            let c = cfg("mac");
            let calls = Rc::new(RefCell::new(Vec::new()));
            for count in 0..refused {
                let seen = calls.clone();
                let mut s = Service::new(&b,&c,"fixture",Box::new(move |l| {
                    assert!(l.execution.is_some_and(|id|id>l.attempt));
                    seen.borrow_mut().push((l.resume.clone().unwrap(),l.execution.unwrap()));
                    Err(crate::runtime::StartError::io(std::io::Error::other("pre-submit refusal"),None))
                }));
                let result = s.pass();
                if count + 1 < core::GONE_MAX { assert!(result.unwrap_err().contains("input retained")); } else { result.unwrap(); }
                assert_eq!(core::delivered(n,&entries(&b,n).unwrap(),a),Some(a));
                assert_eq!(task(&b,n).attempt,Some(a));
            }
            if refused == 2 {
                assert_eq!(task(&b,n).state,State::Ask);
                let mut parked = service(&b,&c,Default::default()); parked.pass().unwrap();
                assert!(parked.kids.is_empty());
                b.comment(n,"taskq: answer 1").unwrap();
            }
            let seen = calls.clone();
            let mut recovered = Service::new(&b,&c,"fixture",Box::new(move |l| {
                assert!(l.execution.is_some_and(|id|id>l.attempt));
                seen.borrow_mut().push((l.resume.clone().unwrap(),l.execution.unwrap()));
                Ok(Box::new(FakeProc(Rc::new(RefCell::new(None)))))
            }));
            recovered.pass().unwrap();
            assert_eq!(calls.borrow().len(),refused as usize + 1);
            assert!(calls.borrow().iter().all(|(input,_)|input.contains("retained user input")));
            assert!(core::delivered(n,&entries(&b,n).unwrap(),a).unwrap() >= cursor);
            let execution = recovered.kids[0].execution;
            assert_eq!(calls.borrow().last().unwrap().1,execution,"adapter receives the exact continuation intent");
            recovered.pass().unwrap();
            assert_eq!(recovered.kids[0].execution,execution);
            drop(recovered);
            let mut restarted = service(&b,&c,Default::default()); restarted.pass().unwrap();
            assert!(restarted.kids.is_empty());
            assert!(restarted.execution_waits[&n].contains("termination unconfirmed"));
            assert!(restarted.retained_attempts().contains(&a),"lost handle retains its input evidence after restart");
        }
    }

    // Chain: native acceptance/exit -> failed or lost board response -> durable readback. Risk: evidence retry replays work or drops its handle.
    #[test]
    fn acceptance_readback_retries_evidence_and_keeps_the_native_handle() {
        struct Deferred { accepted: Rc<std::cell::Cell<bool>>, exit: Rc<std::cell::Cell<bool>> }
        impl Proc for Deferred {
            fn accepted(&mut self) -> Result<bool,String> { Ok(self.accepted.get()) }
            fn exited(&mut self) -> Result<Option<i32>,String> { Ok(self.exit.get().then_some(0)) }
            fn kill(&mut self) -> Result<(),String> { self.exit.set(true); Ok(()) }
            fn id(&self) -> u32 { 0 }
        }
        let b=Fake::default(); let n=b.create("T","",&[TASK_LABEL.into()]).unwrap(); let c=cfg("mac");
        let accepted=Rc::new(std::cell::Cell::new(false)); let exit=Rc::new(std::cell::Cell::new(false));
        let starts=Rc::new(std::cell::Cell::new(0)); let (ac,ex,calls)=(accepted.clone(),exit.clone(),starts.clone());
        let mut s=Service::new(&b,&c,"fixture",Box::new(move |_| {
            calls.set(calls.get()+1); Ok(Box::new(Deferred { accepted:ac.clone(),exit:ex.clone() }))
        }));
        s.focus=vec!["all".into()]; s.pass().unwrap(); let a=task(&b,n).attempt.unwrap();
        assert_eq!(s.delivered(a),0,"a created handle is not acceptance");
        assert_eq!(core::delivered(n,&entries(&b,n).unwrap(),a),None);
        b.comment(n,&core::entry("ask",&[],Some(a),"Which?\n1. Continue ★")).unwrap();
        accepted.set(true); exit.set(true); b.state.borrow_mut().fail_comments=1;
        s.pass().unwrap();
        assert_eq!(s.kids.len(),1,"retain native proof until its receipt is durable");
        assert_eq!(s.delivered(a),0); assert_eq!(starts.get(),1);
        assert!(s.unknown[&a].contains("acceptance unconfirmed"));
        b.state.borrow_mut().lose_comment_response=true;
        s.pass().unwrap();
        assert_eq!(s.delivered(a),a); assert!(s.kids.is_empty()); assert_eq!(starts.get(),1);
        let log=entries(&b,n).unwrap();
        assert_eq!(log.iter().filter(|e|e.body.starts_with("taskq: accepted ")).count(),1);
        assert_eq!(core::executions(n,&log)[0].release,Some(core::Release::Exit));
        assert!(release_execution(&b,n,a,a,"no-start").unwrap_err().contains("contradictory"));
        let mut restarted=service(&b,&c,Default::default()); restarted.pass().unwrap();
        assert!(restarted.kids.is_empty()); assert_eq!(task(&b,n).state,State::Ask);
    }

    // Chain: terminal old turn -> pending owner input -> unavailable owner/session. Risk: recovery drops input by fabricating gone.
    #[test]
    fn unavailable_owner_or_session_preserves_pending_input() {
        for (runtime, host) in [("codex:session","other"),("codex:-","mac")] {
            let b=Fake::default(); let n=b.create("T","",&[TASK_LABEL.into()]).unwrap(); let c=cfg("mac");
            let a=b.comment(n,&core::entry("spawn",&["worker",runtime,host],None,"")).unwrap();
            accept_execution(&b,n,a,a).unwrap(); release_execution(&b,n,a,a,"exit").unwrap();
            let input=b.comment(n,"taskq: steer\nkeep this input").unwrap();
            b.advance(3600);
            let mut s=service(&b,&c,Default::default()); s.pass().unwrap();
            assert_eq!(task(&b,n).attempt,Some(a)); assert_eq!(task(&b,n).inbox,[input]);
            assert!(s.kids.is_empty());
            assert!(!b.comments(n).unwrap().iter().any(|c|c.body.starts_with("taskq: gone")));
            if host == "mac" {
                assert!(s.execution_waits[&n].contains("session identity unknown"));
                assert!(s.snapshot.as_ref().unwrap().tasks.iter().find(|r|r.n==n).unwrap().execution_wait.as_ref().unwrap().contains("session identity unknown"));
            } else {
                assert!(s.execution_waits[&n].contains("input pending on other"));
            }
        }
    }

    // Chain: uncertain native submit -> service restart -> result/requeue -> another host.
    // Risk: task state or a lost process handle permits duplicate external execution.
    #[test]
    fn uncertain_submission_stays_fenced_through_restart_and_task_moves() {
        for result in [true, false] {
            let b = Fake::default();
            let n = b.create("T", "", &[TASK_LABEL.into()]).unwrap();
            let (ca, cb) = (cfg("mac"), cfg("win"));
            let starts = Rc::new(std::cell::Cell::new(0)); let called = starts.clone();
            let mut a = Service::new(&b, &ca, "fixture", Box::new(move |_| {
                called.set(called.get() + 1);
                let mut error = crate::runtime::StartError::io(std::io::Error::other("response lost after submission"), None);
                error.no_start = false;
                Err(error)
            }));
            a.focus = vec!["all".into()];
            assert!(a.pass().unwrap_err().contains("launch outcome unknown"));
            let attempt = task(&b, n).attempt.unwrap();
            assert_eq!(a.entry().held.len(), 1, "unknown submission retains capacity");
            drop(a);
            let exits: Exits = Default::default();
            let mut restored = service(&b, &ca, exits.clone());
            restored.pass().unwrap();
            assert_eq!(restored.entry().held.len(), 1, "restart rebuilds unresolved ownership");
            assert!(exits.borrow().is_empty());
            b.comment(n, &if result { core::entry("result", &[&"a".repeat(40)], Some(attempt), "ready for review") }
                else { core::entry("requeue", &[], Some(attempt), "retry") }).unwrap();
            b.advance(900);
            let mut other = service(&b, &cb, exits.clone());
            other.pass().unwrap(); restored.pass().unwrap();
            assert_eq!(starts.get(), 1); assert!(exits.borrow().is_empty());
            assert!(other.execution_waits.contains_key(&n));
            release_execution(&b, n, attempt, attempt, "exit").unwrap();
            other.pass().unwrap();
            assert_eq!(exits.borrow().len(), 1, "fresh native exit evidence permits the next role");
            restored.pass().unwrap();
            assert!(restored.entry().held.is_empty(), "another host's new attempt cannot retain the old reservation");
            assert_eq!(task(&b, n).role.as_deref(), Some(if result { "reviewer" } else { "worker" }));
        }
    }

    // Chain: requeue -> stop request -> uncertain exit -> occupied slot -> confirmed exit -> replacement.
    #[test]
    fn cancellation_retains_an_unconfirmed_child_and_blocks_replacement() {
        struct SlowStop(Rc<std::cell::Cell<u8>>);
        impl Proc for SlowStop {
            fn exited(&mut self) -> Result<Option<i32>, String> {
                if self.0.get() == 2 { Err("invented read failure".into()) } else { Ok((self.0.get() == 3).then_some(-9)) }
            }
            fn kill(&mut self) -> Result<(), String> {
                if self.0.get() == 1 { Err("invented stop refusal".into()) } else { Ok(()) }
            }
            fn id(&self) -> u32 { 0 }
        }
        let b = Fake::default();
        let n = b.create("T", "", &[TASK_LABEL.into()]).unwrap();
        let mut c = cfg("mac"); c.limits = BTreeMap::from([("claude".into(), 3)]);
        let exited = Rc::new(std::cell::Cell::new(0));
        let starts = Rc::new(std::cell::Cell::new(0));
        let mut s = Service::new(&b, &c, "test", Box::new(|_| {
            starts.set(starts.get() + 1); Ok(Box::new(SlowStop(exited.clone())))
        }));
        s.focus = vec!["all".into()]; s.pass().unwrap();
        let attempt = s.kids[0].attempt;
        b.comment(n, "taskq: requeue").unwrap();
        for mode in [1, 0, 2, 0] {
            exited.set(mode);
            s.pass().unwrap();
            assert_eq!(starts.get(), 1, "an unconfirmed old process cannot overlap its replacement even with spare capacity");
            assert_eq!(s.kids[0].attempt, attempt);
            assert!(s.unknown.contains_key(&attempt));
            assert_eq!(task(&b, n).state, State::Ready);
            s.reap_owned();
            assert!(s.unknown.contains_key(&attempt), "reduced monitoring must retain the pending-stop reason");
        }
        exited.set(3); s.pass().unwrap();
        assert_eq!(starts.get(), 2);
        assert!(s.kids.iter().all(|k| k.attempt != attempt));
    }

    // Chain: §4 log retention -> service pass -> collection/selftest. Risk: the hourly timer leaves a full log stuck.
    #[test]
    fn service_pass_restores_bounded_log_collection() {
        let root = std::env::temp_dir().join(format!("taskq-service-retention-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("taskq.log");
        let c = Config { install: root.clone(), ..cfg("win") };
        let b = Fake::default();
        let mut s = Service::new(&b, &c, "test", Box::new(|_| panic!("no tasks")));
        for size in [crate::data::LOG_MAX - 256, crate::data::LOG_MAX + 1024] {
            let mut text = "old diagnostic\n".repeat(size as usize / 15);
            let newest = "taskq-profile {\"op\":\"spawn\",\"us\":1,\"error\":\"none\",\"at_ms\":1}\n";
            text.push_str(newest);
            let terminal = format!("taskq-stage {}\n", serde_json::json!({"project":"o/r", "host":"win",
                "stage":"build", "task":7, "attempt":41, "rt":"codex", "us":null,
                "outcome":"cancelled", "error":"none", "at_ms":1, "unknown":"adopted"}));
            text.push_str(&terminal);
            std::fs::write(&path, &text).unwrap();
            let mut writer = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
            {
                let _lock=crate::profile::log_lock(&path).unwrap();
                let now=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
                crate::telemetry::read(&root,now).unwrap();
            }
            assert!(crate::profile::selftest(&root, "o/r", &vec![], 300).is_ok(), "legacy log size no longer stops collection");
            let lock = crate::profile::log_lock(&path).unwrap();
            s.pass().unwrap();
            assert!(std::fs::read_to_string(&path).unwrap()==text, "busy lock preserves evidence");
            drop(lock);
            let (_capture,_guard)=crate::profile::tests::collecting(&root);
            s.pass().unwrap();
            let retained = std::fs::read_to_string(&path).unwrap();
            assert!(retained.len() as u64 <= crate::data::LOG_MAX / 2);
            assert!(retained.starts_with("old diagnostic\n") && retained.ends_with(&terminal));
            use std::io::Write;
            writer.write_all(b"still open diagnostic\n").unwrap();
            crate::profile::append_line(&path, newest).unwrap();
            assert!(crate::profile::selftest(&root, "o/r", &vec![], 300).unwrap().contains("no overdue"));
            assert!(!crate::profile::report(&root).contains("collection stopped"));
            drop(writer);
            drop(_guard);drop(_capture);std::fs::remove_dir_all(&root).unwrap();
            std::fs::create_dir_all(&root).unwrap();
        }
        std::fs::remove_dir_all(root).unwrap();
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

    // Chain: Windows native reviewer launch -> service pass -> gone, on a disposable board only.
    #[cfg(windows)]
    #[test]
    #[ignore = "live Claude reviewer; TASKQ_CLAUDE_ROOT selects the worktree"]
    fn live_claude_reviewer_exit_writes_gone() {
        use crate::runtime::{ClaudeTurn, Env, access, log_path};
        let mut c = cfg("win");
        c.root = PathBuf::from(std::env::var("TASKQ_CLAUDE_ROOT").expect("TASKQ_CLAUDE_ROOT"));
        c.install = std::env::temp_dir().join(format!("taskq-claude-{}", std::process::id()));
        std::fs::create_dir_all(&c.install).unwrap();
        crate::policy::grant(&c.install, "claude", "full", None, "op", 1).unwrap();
        let env = Env { root: &c.root, exe: std::path::Path::new("taskq"), install: &c.install, permission_mode: "dontAsk", codex: &[], publish: "pr", board_env: &[] };
        let b = Fake::default();
        let n = b.create("Exit qualification", "", &["taskq".into()]).unwrap();
        let a = b.comment(n, "taskq: spawn worker claude:invented win").unwrap();
        b.comment(n, &core::entry("result", &["0123456789abcdef0123456789abcdef01234567"], Some(a), "invented candidate")).unwrap();
        release_execution(&b, n, a, a, "exit").unwrap();
        let mut s = Service::new(&b, &c, "4.4.0", Box::new(|l| {
            assert_eq!(l.role, "reviewer");
            let text = format!("{}\nReply EXIT_CHECK and do nothing else.", "Qualification input only.\n".repeat(1800));
            ClaudeTurn::start(&env, l, &log_path(env.root, l), &text, &access(&env, "claude").unwrap())
                .map(|t| Box::new(t) as Box<dyn Proc>)
        }));
        s.focus = vec!["all".into()];
        s.pass().unwrap();
        let r = task(&b, n).attempt.unwrap();
        assert_ne!(r, a);
        eprintln!("win reviewer start attempt {r}");
        // End only this disposable native job; do not wait through Claude's spawn-retry backoff.
        crate::runtime::stop(s.kids[0].proc.as_mut()).unwrap();
        s.pass().unwrap();
        let log = b.comments(n).unwrap();
        for comment in &log { eprintln!("{}", comment.body); }
        for kid in &mut s.kids { crate::runtime::stop(kid.proc.as_mut()).unwrap(); }
        assert!(log.iter().any(|e| e.body.contains(&format!("taskq: gone attempt {r}"))), "next pass must reap the native terminal job");
        assert!(task(&b, n).result.is_some());
        std::fs::remove_dir_all(&c.install).unwrap();
    }

    type Exits = Rc<RefCell<Vec<(u64, Rc<RefCell<Option<i32>>>)>>>;

    // H-A5: a childless service can still own an uncertain native execution.
    #[test]
    fn project_drain_retains_unresolved_execution_until_release() {
        use std::fs;
        let b=Fake::default();
        let mut c=cfg("mac");
        c.root=std::env::temp_dir().join(format!("taskq-drain-{}",crate::runtime::uuid()));
        c.install=c.root.join("install");
        fs::create_dir_all(crate::dot(&c.root)).unwrap();
        let host=std::sync::Arc::new(std::sync::Mutex::new(crate::host::Host::new(Err("fixture holds admission".into()))));
        host.lock().unwrap().draining.insert(c.root.clone());
        let n=b.create("T","",&[TASK_LABEL.into()]).unwrap();
        let a=b.comment(n,"taskq: spawn worker codex:session mac").unwrap();
        let mut s=Service::new(&b,&c,"fixture",Box::new(|_|panic!("drain must never submit work")));
        s.host=Some(host.clone());
        b.state.borrow_mut().fail_comment_reads=1;
        assert!(crate::stopped_handoff(&mut s,&host,&c).is_err(),"no board observation is not an idle proof");
        s.pass().unwrap();
        assert!(s.kids.is_empty());
        assert!(s.retained_attempts().contains(&a));
        assert!(!crate::stopped_handoff(&mut s,&host,&c).unwrap(),"missing handle must retain the project and its slot");
        assert_eq!(host.lock().unwrap().busy.get(&c.root),Some(&true));
        b.labels(n,&[],&[TASK_LABEL.into()]).unwrap();s.pass().unwrap();
        assert!(s.retained_attempts().contains(&a),"label removal is not native termination");
        b.state.borrow_mut().comments.iter_mut().find(|(_,comment)|comment.id==a).unwrap().1.edited=true;
        s.pass().unwrap();assert!(!s.idle(),"missing execution history cannot release an observed execution");
        b.state.borrow_mut().comments.iter_mut().find(|(_,comment)|comment.id==a).unwrap().1.edited=false;
        b.close(n).unwrap();b.state.borrow_mut().fail_comment_reads=1;
        assert!(s.pass().is_err());
        assert!(s.retained_attempts().contains(&a));
        assert!(!crate::stopped_handoff(&mut s,&host,&c).unwrap(),"failed ownership read cannot certify idle");
        s.pass().unwrap();
        assert!(s.retained_attempts().contains(&a),"leaving the open-issue list is not native termination");
        assert!(!crate::stopped_handoff(&mut s,&host,&c).unwrap());
        release_execution(&b,n,a,a,"exit").unwrap();
        s.tasks_read=Some(std::time::Instant::now()-std::time::Duration::from_secs(c.poll.max(1)));
        assert!(crate::stopped_handoff(&mut s,&host,&c).unwrap(),"read-only monitor must observe terminal delivery and complete drain");
        let n=b.create("Recovered","",&[TASK_LABEL.into()]).unwrap();
        let a=b.comment(n,"taskq: spawn worker codex:recovered mac").unwrap();
        s.pass().unwrap();assert!(s.retained_attempts().contains(&a));
        let exit=Rc::new(RefCell::new(None));
        s.adopt(Kid {execution:a,task:n,attempt:a,rt:"codex".into(),proc:Box::new(FakeProc(exit.clone()))}).unwrap();
        assert!(!crate::stopped_handoff(&mut s,&host,&c).unwrap());
        *exit.borrow_mut()=Some(0);
        assert!(crate::stopped_handoff(&mut s,&host,&c).unwrap(),"reduced monitoring must retire the recovered reservation after exact release");
        b.state.borrow_mut().fail_comments=1;
        s.ownership_known=false;
        assert!(crate::stopped_handoff(&mut s,&host,&c).unwrap(),"read-only recovery can prepare without resuming ordinary board writes");
        assert_eq!(b.state.borrow().fail_comments,1,"recovery must not try an ordinary task write");
        drop(s);fs::remove_dir_all(&c.root).unwrap();
    }

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

    // Chain: Resume process -> project wakeup -> separate scheduler process -> durable admission evidence.
    // Board and process adapters are fakes; real command functions, scheduler and shared collector run unchanged.
    #[test]
    fn resume_handoff_links_the_service_decision() {
        let dir = std::env::temp_dir().join(format!("qh-{}", std::process::id()));
        for held in [true, false] {
            std::fs::create_dir_all(dir.join(".taskq")).unwrap();
            std::fs::create_dir_all(dir.join("i")).unwrap();
            std::fs::write(dir.join(".taskq/focus"), "off").unwrap();
            std::fs::write(dir.join(".taskq/focus.paused"), "all").unwrap();
            if held { std::fs::write(dir.join("i/hold"), "private owner reason").unwrap(); }
            for mode in ["resume", "service"] {
                let child = std::process::Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", "service::tests::resume_handoff_child", "--ignored"])
                    .env("TASKQ_HANDOFF_ROOT", &dir).env("TASKQ_HANDOFF_MODE", mode)
                    .env("TASKQ_INSTALL", dir.join("i")).env("TASKQ_ROOT", &dir).env("TASKQ_INCIDENT_REPORTS", "off")
                    .output().unwrap();
                assert!(child.status.success(), "{mode}: {}\n{}", String::from_utf8_lossy(&child.stdout), String::from_utf8_lossy(&child.stderr));
            }
            let reader = coreq::telemetry::Reader::open(&dir.join("i/telemetry.log/universal.sqlite3")).unwrap();
            let mut observations = Vec::new(); let mut after = 0;
            loop {
                let page = reader.export(0, after, 1024).unwrap();
                let rows = page["events"].as_array().unwrap();
                if rows.is_empty() { break; }
                let next = rows.last().unwrap()["seq"].as_i64().unwrap();
                assert!(next > after, "export cursor did not advance");
                after = next;
                observations.extend(rows.iter().map(|r| r["observation"].clone()));
            }
            let terminal = |name: &str| observations.iter().find(|r| r["operation"] == name && r["kind"] == "method").unwrap();
            let command = terminal("taskq.resume");
            let poke = terminal("taskq.poke");
            let write = terminal("taskq.operation.wake.write");
            let wake = terminal("taskq.continuation");
            assert_eq!(poke["trace"], command["trace"]);
            assert_eq!(write["trace"], command["trace"]);
            assert_eq!(write["parent"], poke["span"]);
            assert_eq!(write["outcome"], "ok");
            assert_eq!(wake["trace"], command["trace"]);
            assert_eq!(wake["parent"], write["span"]);
            assert_eq!(terminal("taskq.service.Service.pass")["trace"], command["trace"]);
            if held {
                assert!(observations.iter().any(|r| r["operation"] == "taskq.service.Service.may_launch" && r["kind"] == "method"
                    && r["trace"] == command["trace"] && r["outcome"] == "refused" && r["error"] == "hold"));
                assert!(!observations.iter().any(|r| r["operation"] == "taskq.service.Service.launch"));
                assert_eq!(std::fs::read_to_string(dir.join("i/hold")).unwrap(), "private owner reason");
            } else {
                let launch = terminal("taskq.service.Service.launch");
                assert_eq!(launch["trace"], command["trace"]);
                assert_eq!(launch["outcome"], "ok");
            }
            if !held {
                assert!(observations.iter().any(|r|r["kind"]=="stage"&&r["operation"]=="start"&&r["trace"]==command["trace"]));
                assert!(observations.iter().any(|r|r["kind"]=="stage"&&r["open"]==true&&r["version"]=="fixture"));
            }
            assert!(!dir.join("i/telemetry.log/telemetry.sqlite3").exists(),"scheduler must not open a second live sink");
            let text = serde_json::to_string(&observations).unwrap();
            assert!(!text.contains("private owner reason") && !text.contains(dir.to_str().unwrap()));
            drop(reader);
            std::fs::remove_dir_all(&dir).unwrap();
        }
    }

    #[test]
    #[ignore = "subprocess helper exercised by resume_handoff_links_the_service_decision"]
    fn resume_handoff_child() {
        let dir = PathBuf::from(std::env::var_os("TASKQ_HANDOFF_ROOT").unwrap());
        crate::profile::init(&dir.join("i"));
        let _runtime = coreq::telemetry::capture::Runtime::install_bounded(&dir.join("i/telemetry.log/universal.sqlite3"), "taskq", "fixture",crate::telemetry::budget(&dir.join("i"),"telemetry.sqlite3").unwrap()).unwrap();
        if std::env::var("TASKQ_HANDOFF_MODE").unwrap() == "resume" {
            crate::resume(&dir).unwrap();
        } else {
            let mut seen = None;
            let link = crate::wake_context(&dir.join(".taskq/poke"), &mut seen).unwrap().unwrap();
            assert!(crate::wake_context(&dir.join(".taskq/poke"), &mut seen).unwrap().is_none());
            let _wakeup = link.span().entered();
            let mut c = cfg("fixture"); (c.root, c.install) = (dir.clone(), dir.join("i"));
            let b = Fake::default(); let n = b.create("Wake fixture", "", &["taskq".into()]).unwrap();
            let e: Exits = Default::default(); let mut s = service(&b, &c, e.clone());
            s.focus = crate::data::focus(&dir).unwrap(); s.pass().unwrap();
            assert_eq!(task(&b, n).state, if c.install.join("hold").exists() { State::Ready } else { State::Doing });
            assert_eq!(e.borrow().len(), usize::from(!c.install.join("hold").exists()));
        }
    }

    // Chain: Resume during a pass -> wake -> fresh admission; a host hold still refuses the launch.
    // Risk: a poke received during work becomes the sleep baseline, delaying Resume by another poll.
    #[test]
    fn resume_during_pass_wakes_the_next_admission_check() {
        crate::with_test_capture(|_, _| {
            let dir = std::env::temp_dir().join(format!("qr-wake-{}", std::process::id()));
            std::fs::create_dir_all(dir.join(".taskq")).unwrap();
            let mut c = cfg("fixture");
            (c.root, c.install) = (dir.clone(), dir.join("i"));
            std::fs::create_dir_all(&c.install).unwrap();
            let b = Fake::default();
            let n = b.create("Wake fixture", "", &["taskq".into()]).unwrap();
            let e: Exits = Default::default();
            let mut s = service(&b, &c, e.clone());
            std::fs::write(c.install.join("hold"), "owner-held fixture").unwrap();
            for held in [true, false] {
                std::fs::write(dir.join(".taskq/focus"), "all").unwrap();
                crate::pause(&dir,false).unwrap();
                std::fs::remove_file(dir.join(".taskq/poke")).unwrap();
                let pokes = [dir.join(".taskq/poke"), c.install.join("poke")];
                let poke_start = crate::poke_times(&pokes);
                s.focus = crate::data::focus(&dir).unwrap();
                s.pass().unwrap();
                assert_eq!(task(&b, n).state, State::Ready);
                crate::resume(&dir).unwrap();
                let mut ticks = 0;
                crate::sleep_until_poke(&pokes, poke_start, std::time::Instant::now()+std::time::Duration::from_secs(1), &mut || ticks += 1);
                assert_eq!(ticks, 0, "Resume during the pass must skip the next poll wait");
                s.focus = crate::data::focus(&dir).unwrap();
                s.pass().unwrap();
                if held {
                    assert_eq!(task(&b, n).state, State::Ready);
                    assert!(e.borrow().is_empty());
                    assert_eq!(s.capacity_json()["reasons"][0]["code"], "hold");
                    assert_eq!(std::fs::read_to_string(c.install.join("hold")).unwrap(), "owner-held fixture");
                    std::fs::remove_file(c.install.join("hold")).unwrap();
                } else {
                    assert_eq!(task(&b, n).state, State::Doing);
                    assert_eq!(e.borrow().len(), 1);
                }
            }
            let pokes = [dir.join(".taskq/poke"), c.install.join("poke")];
            let poke_start = crate::poke_times(&pokes);
            crate::touch(&pokes[1]);
            crate::sleep_until_poke(&pokes, poke_start, std::time::Instant::now()+std::time::Duration::from_secs(1), &mut || panic!("host poke during a pass was lost"));
            // A poke during sleep is observed after the current tick, without waiting another second.
            let poke_start = crate::poke_times(&pokes);
            let mut ticks = 0;
            crate::sleep_until_poke(&pokes, poke_start, std::time::Instant::now()+std::time::Duration::from_secs(2), &mut || {
                ticks += 1;
                std::fs::remove_file(&pokes[1]).unwrap();
            });
            assert_eq!(ticks, 1);
            drop(s);
            std::fs::remove_dir_all(dir).unwrap();
        });
    }

    // Chain A2: project-thread unwind -> rebuilt board cache -> quarantine with live lease, beside healthy admission.
    // Risk: a panic drops a child/cursor, stops the other project or collides on equal task/attempt numbers.
    #[test]
    fn host_project_panic_preserves_children_and_other_project_admission() {
        use std::sync::{Arc, Mutex, Barrier};
        let policy = crate::host::Policy::parse(json!({"limits": {"claude": 2}, "admission": {"max": 2}, "load_gate": {"enabled": false}}), "invented").unwrap();
        let host = Arc::new(Mutex::new(crate::host::Host::new(Ok(policy.clone()))));
        let barrier = Arc::new(Barrier::new(2));
        let joins: Vec<_> = [true, false].into_iter().map(|panicking| {
            let (host, barrier, policy) = (host.clone(), barrier.clone(), policy.clone());
            std::thread::spawn(move || {
                let b = Fake::default(); let n = b.create("Same number", "", &["taskq".into()]).unwrap();
                let mut c = cfg("invented"); c.root = PathBuf::from(if panicking { "/invented/panic" } else { "/invented/healthy" });
                let exits: Exits = Default::default(); let mut s = service(&b, &c, exits.clone());
                // Seed the panicking project's existing child without consuming a new host launch interval.
                if panicking { s.pass().unwrap(); s.delivered.insert(s.kids[0].attempt, 99); }
                s.host = Some(host.clone()); s.reload(Ok(policy.cap.clone())); s.sync_host();
                barrier.wait();
                if !panicking { s.pass().unwrap(); }
                assert_eq!(s.kids.len(), 1);
                let a = s.kids[0].attempt;
                let mut panics = crate::host::Panics::default();
                for now in [0, 1, 11] {
                    if panicking {
                        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            s.pass().unwrap();
                            panic!("invented pass panic");
                        }));
                        assert!(r.is_err()); panics.record(now, "invented pass panic"); s.rebuild();
                        s.reduced().unwrap();
                        assert_eq!(s.delivered(a), 99, "the inbox cursor survives every rebuild");
                    } else { s.pass().unwrap(); }
                    assert_eq!(s.kids.len(), 1); assert_eq!(*exits.borrow()[0].1.borrow(), None);
                }
                if panicking {
                    assert!(panics.quarantined()); assert_eq!(panics.reason.as_deref(), Some("quarantined: panic:other"));
                    let before = b.comments(n).unwrap().len();
                    b.advance(900); s.mono = 900;
                    s.reduced().unwrap(); assert_eq!(b.comments(n).unwrap().len(), before, "quarantine writes no task transitions");
                }
                let hb = b.issues(None).unwrap().into_iter().find(is_host).unwrap();
                assert!(hb.body.lines().any(|l| l == format!("#{n} {a}")), "child lease remains listed");
                assert!(b.now().unwrap() - hb.updated < c.ttl, "quarantine refreshes the lease beyond ttl using board time");
                (n, a, s.kids.len())
            })
        }).collect();
        let results: Vec<_> = joins.into_iter().map(|j| j.join().unwrap()).collect();
        assert_eq!(results[0], results[1], "equal issue and attempt numbers across projects stay distinct");
        assert_eq!(host.lock().unwrap().sync(std::path::Path::new("unused"), vec![]), 2);
    }

    // Chain: § 3 poisoned authority -> each project's retained handles -> verified slot rebuild.
    // Risk: clearing poison early drops another project's occupancy and allows a launch above the host limit.
    #[test]
    fn host_poison_holds_until_every_project_reconciles_owned_handles() {
        let install = std::env::temp_dir().join(format!("taskq-host-poison-{}", std::process::id()));
        std::fs::create_dir_all(&install).unwrap();
        let raw = json!({"limits": {"claude": 2}, "admission": {"max": 2}, "load_gate": {"enabled": false}});
        std::fs::write(install.join("host.json"), raw.to_string()).unwrap();
        let p = crate::host::Policy::parse(raw, "invented").unwrap();
        let host = std::sync::Arc::new(std::sync::Mutex::new(crate::host::Host::new(Ok(p.clone()))));
        let (a, b) = (Fake::default(), Fake::default());
        let mut ca = cfg("invented"); ca.root = install.join("a"); ca.install = install.clone();
        let mut cb = ca.clone(); cb.root = install.join("b");
        let mut sa = service(&a, &ca, Default::default()); let mut sb = service(&b, &cb, Default::default());
        for s in [&mut sa, &mut sb] {
            s.host = Some(host.clone()); s.reload(Ok(p.cap.clone()));
            s.kids.push(Kid { execution: 7, task: 1, attempt: 7, rt: "claude".into(), proc: Box::new(FakeProc(Default::default())) });
            host.lock().unwrap().busy.insert(s.cfg.root.clone(), true);
            s.sync_host();
        }
        let poisoned = host.clone();
        assert!(std::thread::spawn(move || { let _guard = poisoned.lock().unwrap(); panic!("invented admission panic"); }).join().is_err());
        sa.sync_host(); assert!(host.is_poisoned()); assert!(!sa.reserve(2, 8, "claude"));
        sb.sync_host(); assert!(!host.is_poisoned());
        assert_eq!(host.lock().unwrap().sync(std::path::Path::new("unused"), vec![]), 2);
        assert_eq!((sa.kids.len(), sb.kids.len()), (1, 1));
        std::fs::remove_dir_all(install).unwrap();
    }

    // Chain A2: unavailable project leaves round-robin -> multiple healthy grants; failed reduced loop -> safe update.
    // Risk: stale readiness starves later work, or a stopped project can never satisfy the host handoff barrier.
    #[test]
    fn host_quarantine_does_not_starve_and_stopped_handles_can_prepare_restart() {
        let root = std::env::temp_dir().join(format!("taskq-host-stopped-{}", std::process::id()));
        std::fs::create_dir_all(crate::dot(&root)).unwrap();
        let p = crate::host::Policy::parse(json!({"limits": {"claude": 2}, "load_gate": {"enabled": false}}), "invented").unwrap();
        let host = std::sync::Arc::new(std::sync::Mutex::new(crate::host::Host::new(Ok(p.clone()))));
        let b = Fake::default(); let mut c = cfg("invented"); c.root = root.clone(); c.install = root.clone();
        let mut s = service(&b, &c, Default::default()); s.host = Some(host.clone());
        let healthy = root.join("healthy");
        { let mut h = host.lock().unwrap(); h.ready(&root, true); h.ready(&healthy, true); }
        s.rebuild(); s.reduced().unwrap();
        for n in 1..=3 {
            let mut h = host.lock().unwrap();
            h.reserve(&healthy, n, n, "claude", &p.cap, true, (n - 1) * 30).unwrap();
            h.sync(&healthy, vec![]);
        }
        let n = b.create("Owned", "", &[TASK_LABEL.into()]).unwrap();
        let a = b.comment(n, "taskq: spawn worker claude:fixture invented").unwrap();
        let exit = Rc::new(RefCell::new(None));
        s.kids.push(Kid { execution: a, task: n, attempt: a, rt: "claude".into(), proc: Box::new(FakeProc(exit.clone())) });
        s.pass().unwrap();s.rebuild();
        { let mut h = host.lock().unwrap(); h.busy.insert(root.clone(), true); h.restart = true; }
        // This is the monitor entered after a reduced-loop panic: no board pass and no lease renewal.
        assert!(!crate::stopped_handoff(&mut s, &host, &c).unwrap());
        assert!(!host.lock().unwrap().prepared.contains(&root)); assert_eq!(s.kids.len(), 1);
        *exit.borrow_mut() = Some(0);
        assert!(!crate::stopped_handoff(&mut s, &host, &c).unwrap());
        let h = host.lock().unwrap();
        assert!(h.busy.keys().all(|p| h.prepared.contains(p)), "coordinator can restart after verified exit");
        assert!(core::executions(n, &entries(&b,n).unwrap()).iter().all(|x| x.release.is_some()), "only confirmed release permits handoff");
        drop(h); std::fs::remove_dir_all(root).unwrap();
    }

    // Chain: cross-host recovery. Risk: inbox age or an unlisted lease is mistaken for exit.
    #[test]
    fn cross_host_recovery_needs_exit_regardless_of_inbox_grace() {
        for (entry, trusted, edited, pending_input) in [
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
            assert_eq!(task(&b, n).attempt, Some(a), "{entry}, trusted={trusted}, edited={edited}");
            b.advance(301);
            b.edit(hb, "version 4.0.0\nat fresh").unwrap();
            s.pass().unwrap();
            assert_eq!(task(&b, n).attempt, Some(a));
            release_execution(&b, n, a, a, "exit").unwrap();
            s.pass().unwrap();
            assert_eq!(task(&b, n).attempt, pending_input.then_some(a), "confirmed exit permits recovery but cannot discard pending input");
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
        assert_eq!(task(&b, n).attempt, Some(a), "the continuation remains owned after heartbeat expiry");
        assert_eq!(core::executions(n, &entries(&b,n).unwrap()).iter().filter(|x| x.release.is_none()).count(), 1);
    }

    // Chain: § 4 step 1 turn end, A1/A2 of #1034. Risk: a worker whose `taskq result` was rate-limited finishes its
    // turn, its process lingers, and the task shows `doing` for hours; or the `gone` hides the worker's last message.
    #[test]
    fn finished_turn_without_result_is_gone_with_last_message() {
        for paused in [false, true] {
            turn_end(paused);
        }
    }

    // Chain: service launch -> failed native checkpoint -> repair -> observation -> terminal receipt.
    // Risk: persistence failure loses a live handle or resubmits the task instead of saving its identity.
    #[cfg(unix)]
    #[test]
    fn native_checkpoint_failure_retains_execution_and_retries_only_storage() {
        let root=std::env::temp_dir().join(format!("taskq-checkpoint-{}",crate::runtime::uuid()));
        let install=root.join("install");
        std::fs::create_dir_all(&install).unwrap();
        std::fs::write(install.join("native"),"blocked directory").unwrap();
        let b=Fake::default();
        let n=b.create("T","",&[TASK_LABEL.into()]).unwrap();
        let c=Config {root:root.clone(),install:install.clone(),limits:BTreeMap::from([("codex".into(),1)]),..cfg("mac")};
        let count=Rc::new(RefCell::new(0)); let launched=count.clone();
        let mut s=Service::new(&b,&c,"fixture",Box::new(|l| {
            *launched.borrow_mut()+=1;
            let log=crate::runtime::log_path(&root,l);
            std::fs::create_dir_all(log.parent().unwrap()).unwrap();
            std::fs::write(&log,"").unwrap();
            let child=std::process::Command::new("sleep").arg("30").spawn().unwrap();
            let native=coreq::process::identity(child.id()).unwrap();
            Ok(Box::new(crate::runtime::CliTurn {proc:Box::new(child),log,pos:0,start:0,stopping:false,accepted:false,read_error:None,native,recovery:None}))
        }));
        s.focus=vec!["all".into()];
        s.pass().unwrap();
        let a=task(&b,n).attempt.unwrap();
        assert_eq!(s.kids.len(),1);
        s.reap_owned();
        assert!(s.unknown[&a].contains("checkpoint failed"));
        let pid=s.kids[0].proc.id();
        assert!(coreq::process::identity(pid).unwrap().is_some());
        std::fs::remove_file(install.join("native")).unwrap();
        s.reap_owned();
        let rows=crate::kids_file(&install,&root).unwrap();
        assert_eq!(rows.len(),1);
        assert_eq!((rows[0].task,rows[0].attempt,rows[0].execution,rows[0].process.pid),(n,a,a,pid));
        assert_eq!(*count.borrow(),1,"retry storage, never work");
        assert!(!s.unknown.contains_key(&a));
        let held=std::mem::take(&mut s.kids);
        let discarded=crate::save_kids(&s,&root);
        s.kids=held;
        assert!(discarded.unwrap_err().contains("unresolved"));
        assert_eq!(crate::kids_file(&install,&root).unwrap(),rows,"a missing handle cannot erase recovery evidence");
        // A second failed checkpoint must not prevent emergency cancellation.
        let l=Launch {execution:None,task:n,title:String::new(),attempt:a,role:"worker".into(),rt:"codex".into(),session:"-".into(),url:String::new(),resume:None};
        let path=crate::kids_path(&install,&root).unwrap();
        std::fs::write(&path,"corrupt").unwrap();
        assert!(s.kids[0].proc.checkpoint(&c,&l,a).is_err());
        s.kids[0].proc.kill().unwrap();
        let until=std::time::Instant::now()+std::time::Duration::from_secs(3);
        while !s.kids.is_empty() {
            s.reap_owned();
            assert!(std::time::Instant::now()<until,"stop must remain available during storage failure");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(*count.borrow(),1);
        assert_eq!(core::executions(n,&crate::board::entries(&b,n).unwrap())[0].release,Some(core::Release::Exit));
        assert_eq!(std::fs::read_to_string(path).unwrap(),"corrupt","failed recovery evidence is preserved");
        std::fs::remove_dir_all(&root).unwrap();
    }

    fn turn_end(paused: bool) {
        use std::io::Write;
        let root = std::env::temp_dir().join(format!("taskq-turn-end-{paused}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let b = Fake::default();
        let n = b.create("T", "", &[TASK_LABEL.into()]).unwrap();
        let c = Config { root: root.clone(), ..cfg("win") };
        let flags: Exits = Default::default();
        let (seen, r) = (flags.clone(), root.clone());
        let mut s = Service::new(&b, &c, "4.0.0", Box::new(move |l: &Launch| {
            let flag = Rc::new(RefCell::new(None));
            seen.borrow_mut().push((l.task, flag.clone()));
            let log = crate::runtime::log_path(&r, l);
            std::fs::create_dir_all(log.parent().unwrap()).unwrap();
            std::fs::write(&log, "{\"type\":\"thread.started\",\"thread_id\":\"t\"}\n").unwrap();
            Ok(Box::new(crate::runtime::CliTurn { proc: Box::new(FakeProc(flag)), log, pos: 0, start: 0, stopping: false, accepted: false, read_error: None, native: None, recovery: None }) as Box<dyn Proc>)
        }));
        s.focus = vec!["all".into()];
        s.pass().unwrap();
        let a = task(&b, n).attempt.unwrap();
        s.pass().unwrap();
        assert_eq!(task(&b, n).attempt, Some(a), "a running turn stays doing");

        // the worker's result is rejected by the board rate limit, then its turn ends and its process lingers
        b.state.borrow_mut().fail_comments = 1;
        assert!(b.comment(n, &core::entry("result", &["d3e3479077ac2df8db8071b5bdf12cdacaf10ab4"], Some(a), "Changed: x")).is_err());
        let log = crate::runtime::attempt_log(&root, "worker", n, a);
        let mut f = std::fs::File::options().append(true).open(&log).unwrap();
        let text = format!("Task handoff is incomplete: GitHub rate limits rejected `taskq result`.\n\nSee [log]({}/.taskq/x.log).", root.display());
        writeln!(f, "{}", json!({"type": "item.completed", "item": {"type": "agent_message", "text": text}})).unwrap();
        writeln!(f, "{{\"type\":\"turn.completed\"}}").unwrap();
        if paused {
            // A failed release keeps the exited handle and retries only its evidence write.
            b.state.borrow_mut().fail_comments = 1;
            s.pass().unwrap();
            assert_eq!(s.kids.len(), 1);
            assert!(s.unknown.contains_key(&a));
        }
        s.pass().unwrap();
        assert_eq!(*flags.borrow()[0].1.borrow(), Some(-9), "the lingering process tree is stopped");
        let t = task(&b, n);
        let gone = b.comments(n).unwrap().into_iter().rfind(|c| c.body.starts_with("taskq: gone")).unwrap().body;
        assert_eq!((t.state, t.attempt, t.retries), (State::Ready, None, 1), "paused={paused}");
        let reason = "exit -9";
        assert!(gone.ends_with(&format!("\n{reason}\nLast message: Task handoff is incomplete: GitHub rate limits rejected `taskq result`. See [log](.taskq/x.log).")), "{gone}");
        let _ = std::fs::remove_dir_all(&root);
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
            let v = views(&b, &mut HashMap::new(), &mut HashMap::new()).unwrap().0.remove(0);
            s.claim(&v, "worker", "claude").unwrap();
            assert_eq!(task(&b, n).state, State::Ready);
            assert!(b.comments(n).unwrap().last().unwrap().body.contains("start failed: private failure text"));
            assert_eq!(crate::incident::take_detected(), if reports { vec!["dispatch.start"] } else { vec![] });
        }
    }

    // Chain: § 4 budget, github.com list without assignees -> `assignee-only` admission and report filter. Risk: an
    // assigned task never starts, or a listed assignee costs a second read.
    #[test]
    fn assignees_missing_from_the_list_are_read_per_issue() {
        let b = Fake::default();
        let n = b.create("t", "", &["assignee-only".into()]).unwrap();
        let listed = b.issue(n).unwrap();
        b.state.borrow_mut().issues[0].assignees = vec!["me".into()];
        assert_eq!((assigned(&b, &listed, "me"), assigned(&b, &listed, "you")), (Ok(true), Ok(false)));
        b.state.borrow_mut().fail_issue_read = Some((n, 1));
        assert!(assigned(&b, &listed, "me").is_err());
        let full = b.issue(n).unwrap();
        b.state.borrow_mut().fail_issue_read = Some((n, 1));
        assert_eq!(assigned(&b, &full, "me"), Ok(true), "a listed assignee needs no read");
    }

    // Chain: incident lease heartbeat (#1140). Risk: a replaced attempt still listed by its host's heartbeat until
    // that host's next pass reported as a second live attempt (#1132, #1137), or a launched lost claim missed.
    #[test]
    fn incident_heartbeats_flag_only_leases_newer_than_the_attempt() {
        for (lease, age, expected) in [("replaced", 0, false), ("current", 0, false), ("lost", 0, true), ("lost", 301, false)] {
            crate::incident::take_detected();
            let b = Fake::default();
            let n = b.create("task", "", &[TASK_LABEL.into()]).unwrap();
            let replaced = b.comment(n, "taskq: spawn worker claude:a other").unwrap();
            b.comment(n, &format!("taskq: requeue attempt {replaced}")).unwrap();
            let current = b.comment(n, "taskq: spawn worker claude:b first").unwrap();
            let lost = b.comment(n, "taskq: spawn worker claude:c other").unwrap();
            let a = match lease { "replaced" => replaced, "current" => current, _ => lost };
            b.create("taskq host other", &format!("#{n} {a}"), &[HOST_LABEL.into()]).unwrap();
            b.advance(age);
            b.create("taskq host first", &format!("#{n} {current}"), &[HOST_LABEL.into()]).unwrap();
            let c = cfg("local");
            let mut s = service(&b, &c, Rc::new(RefCell::new(vec![])));
            s.pass().unwrap();
            assert_eq!(crate::incident::take_detected(), if expected { vec!["invariant.double_attempt"] } else { vec![] }, "{lease} lease, age {age}");
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
            s.kids.push(Kid { execution: if rejected { lost } else { accepted }, task: n, attempt: if rejected { lost } else { accepted }, rt: "claude".into(), proc: Box::new(FakeProc(flag.clone())) });
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
            let v = views(&b, &mut HashMap::new(), &mut HashMap::new()).unwrap().0.remove(0);
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

    // Chain: § 4 heartbeat body, DashQ host join (dashq docs/design/hosts.md H2). Risk: a changed or missing line
    // splits one machine into several Hosts rows or breaks the one-service-per-host check.
    #[test]
    fn heartbeat_body_lists_version_service_machine_focus() {
        let b = Fake::default();
        let c = cfg("win");
        let mut s = service(&b, &c, Default::default());
        s.instance = "abc123".into();
        s.machine = "DESKTOP wsl".into();
        s.focus = vec![];
        assert_eq!(s.heartbeat_body(), "version 4.0.0\nservice abc123\nmachine DESKTOP wsl\nfocus off");
        s.machine = String::new();
        s.focus = vec!["all".into()];
        assert_eq!(s.heartbeat_body(), "version 4.0.0\nservice abc123");
        // #1204: a live Claude child publishes its account-scoped URL; every host's snapshot reads it back by host and attempt
        s.pending.insert((7, 42));
        s.sessions.insert(42, "https://claude.ai/code/session_014abc".into());
        assert_eq!(s.heartbeat_body(), "version 4.0.0\nservice abc123\n#7 42\nsession #7 42 https://claude.ai/code/session_014abc");
        let host = |body: String| crate::board::Issue { number: 1, title: "taskq host win".into(), body, labels: vec![], assignees: vec![], updated: 0, open: true, trusted: true, reason: None };
        let urls = crate::snapshot::session_urls(&[host(s.heartbeat_body()), host("session #8 43 https://evil.example/x".into())]);
        assert_eq!(urls.into_iter().collect::<Vec<_>>(), vec![(("win".to_string(), 42), "https://claude.ai/code/session_014abc".to_string())]);
    }

    // Chain: native observation -> aggregate heartbeat -> remote read -> durable terminal after lost response.
    #[test]
    fn execution_heartbeat_is_fenced_visible_and_never_reopens_terminal() {
        let b=Fake::default();let c=cfg("mac");let remote=cfg("win");
        let n=b.create("T","",&[TASK_LABEL.into()]).unwrap();
        let exits:Exits=Default::default();let mut s=service(&b,&c,exits.clone());
        s.pass().unwrap();s.pass().unwrap();let a=task(&b,n).attempt.unwrap();
        let read=||crate::snapshot::read(&b,&remote,&json!({}),&[]).tasks.into_iter().find(|r|r.n==n).unwrap().json();
        let host=b.issues(None).unwrap().into_iter().find(|i|i.title=="taskq host mac").unwrap();
        let running=host.body.clone();
        assert_eq!(read()["executions"][0]["state"],"running");
        for fault in ["foreign","execution","input","duplicate","malformed","partial-duplicate","invalid-state","untrusted","stale","future"] {
            let mut body=running.clone();
            if fault=="foreign" {body=body.replace("execution ","ignored ");}
            if matches!(fault,"execution"|"input"|"invalid-state") {
                let mut row:Value=serde_json::from_str(running.lines().find_map(|l|l.strip_prefix("execution ")).unwrap()).unwrap();
                match fault {"execution"=>row["id"]=json!(a+999),"input"=>row["through"]=json!(a+999),_=>row["state"]=json!("finished")};
                body=running.lines().filter(|l|!l.starts_with("execution ")).chain(std::iter::once("")).collect::<Vec<_>>().join("\n")+&format!("execution {row}");
            }
            if fault=="duplicate" {body+=&format!("\n{}",running.lines().find(|l|l.starts_with("execution ")).unwrap());}
            if fault=="partial-duplicate" {body+="\nexecution {";}
            if fault=="malformed" {body=running.lines().filter(|l|!l.starts_with("execution ")).collect::<Vec<_>>().join("\n")+"\nexecution {";}
            b.edit(host.number,&body).unwrap();
            {let mut st=b.state.borrow_mut();let now=st.clock;let h=st.issues.iter_mut().find(|i|i.number==host.number).unwrap();
                h.trusted=fault!="untrusted";if fault=="stale" {h.updated=now-61;}if fault=="future" {h.updated=now+60;}}
            let row=read();assert_eq!(row["executions"][0]["state"],"unknown","{fault}: {row}");
            assert!(row["execution_wait"].is_string(),"{fault}");
            b.state.borrow_mut().issues.iter_mut().find(|i|i.number==host.number).unwrap().trusted=true;
        }
        b.edit(host.number,&running).unwrap();
        s.stopping.insert(a);s.beat().unwrap();assert_eq!(read()["executions"][0]["state"],"waiting");s.stopping.remove(&a);
        *exits.borrow().iter().find(|(id,_)|*id==n).unwrap().1.borrow_mut()=Some(-999);s.pass().unwrap();
        assert!(read()["executions"][0]["reason"].as_str().unwrap().contains("read failed"));
        assert!(s.snapshot.as_ref().unwrap().tasks.iter().find(|r|r.n==n).unwrap().json()["execution_wait"].as_str().unwrap().contains("read failed"));
        *exits.borrow().iter().find(|(id,_)|*id==n).unwrap().1.borrow_mut()=Some(0);b.state.borrow_mut().fail_comments=1;
        s.reap_owned();s.beat().unwrap();assert_eq!(s.kids.len(),1);
        assert_eq!(read()["executions"][0]["state"],"unknown","failed final delivery retains ownership");
        b.state.borrow_mut().lose_comment_response=true;s.reap_owned();assert!(s.kids.is_empty());
        b.state.borrow_mut().lose_comment_response=false;
        let log=entries(&b,n).unwrap();assert_eq!(core::executions(n,&log)[0].release,Some(core::Release::Exit));
        assert_eq!(log.iter().filter(|e|e.body.starts_with("taskq: released")).count(),1);
        b.edit(host.number,&running).unwrap();let row=read();assert!(row.get("executions").is_none(),"late heartbeat cannot reopen terminal");
        let mut fresh=service(&b,&c,Default::default());fresh.focus.clear();fresh.pass().unwrap();
        assert!(!fresh.heartbeat_body().lines().any(|l|l.starts_with("execution ")),"restart reads durable terminal");
    }

    // Chain: slow native children -> bounded observation round -> honest heartbeat -> fair next round.
    #[test]
    fn slow_observations_defer_without_releasing_and_rotate_fairly() {
        struct Slow {task:u64,calls:Rc<RefCell<Vec<u64>>>}
        impl Proc for Slow {
            fn accepted(&mut self)->Result<bool,String> {Ok(true)}
            fn exited(&mut self)->Result<Option<i32>,String> {
                self.calls.borrow_mut().push(self.task);std::thread::sleep(std::time::Duration::from_millis(25));Ok(None)
            }
            fn kill(&mut self)->Result<(),String> {Err("fixture must stay owned".into())}
            fn id(&self)->u32 {0}
        }
        let b=Fake::default();let mut c=cfg("mac");c.limits.insert("claude".into(),3);
        for _ in 0..3 {b.create("slow fixture","",&[TASK_LABEL.into()]).unwrap();}
        let mut s=service(&b,&c,Default::default());s.pass().unwrap();s.pass().unwrap();
        assert_eq!(s.kids.len(),3);
        let calls=Rc::new(RefCell::new(vec![]));
        for kid in &mut s.kids {kid.proc=Box::new(Slow {task:kid.task,calls:calls.clone()});}
        for round in 1..=3 {
            s.pass_inner(std::time::Duration::from_millis(10)).unwrap();
            assert_eq!(calls.borrow().len(),round,"a slow poll must not delay status behind all children");
            assert_eq!(s.kids.len(),3);
            assert_eq!(s.unknown.len(),2,"deferred observations must be unknown, not freshly healthy");
            let heartbeat=b.issue(s.host_issue.unwrap()).unwrap();
            assert!(heartbeat.body.contains("native observation deferred"));
        }
        assert_eq!(calls.borrow().iter().collect::<std::collections::HashSet<_>>().len(),3,"slow first child starved another execution");
        calls.borrow_mut().clear();
        for round in 1..=3 {
            s.reap_owned_within(std::time::Duration::from_millis(10));
            assert_eq!(calls.borrow().len(),round);assert_eq!(s.kids.len(),3);assert_eq!(s.unknown.len(),2);
        }
        assert_eq!(calls.borrow().iter().collect::<std::collections::HashSet<_>>().len(),3,"reduced pass starved an execution");
        let tasks=s.kids.iter().map(|k|k.task).collect::<Vec<_>>();
        s.kids.clear();calls.borrow_mut().clear();
        let recovery_calls=calls.clone();
        s.recover=Some(Box::new(move |task,_| {
            recovery_calls.borrow_mut().push(task);std::thread::sleep(std::time::Duration::from_millis(25));Err("fixture: native recovery unknown".into())
        }));
        for round in 1..=3 {
            s.pass_inner(std::time::Duration::from_millis(10)).unwrap();assert_eq!(calls.borrow().len(),round);
            assert_eq!(s.execution_waits.len(),3);assert!(s.kids.is_empty(),"unknown recovery cannot replay work");
        }
        assert_eq!(calls.borrow().iter().collect::<std::collections::HashSet<_>>().len(),3,"cold recovery starved an execution");
        for task in tasks {assert!(core::executions(task,&entries(&b,task).unwrap()).iter().all(|x|x.release.is_none()));}
    }

    // Chain: § 4 board budget, heartbeat. Risk: a PATCH every pass burns the shared budget, or a sparse heartbeat
    // lets another host expire a live lease.
    #[test]
    fn idle_heartbeat_is_batched_at_sixty_seconds_and_stays_fresh() {
        let b = Fake::default();
        let c = cfg("mac");
        let mut s = service(&b, &c, Default::default());
        let beat = |b: &Fake| b.issues(None).unwrap().into_iter().find(|i| i.title == "taskq host mac").map(|i| i.updated).unwrap();
        s.pass().unwrap();
        let (mut writes, mut last) = (0, beat(&b));
        for _ in 0..360 {
            b.advance(10);
            assert!(b.now().unwrap() - beat(&b) < c.ttl, "lease stays fresh between passes");
            s.pass().unwrap();
            if beat(&b) != last { writes += 1; last = beat(&b); }
        }
        assert_eq!(writes, 60, "one periodic write per minute, not one per fast pass");
    }

    // Chain: R2 two hosts -> lost heartbeat -> still-live executor -> confirmed exit -> replacement.
    #[test]
    fn two_hosts_wait_for_confirmed_exit_before_replacement() {
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
        // A missing heartbeat does not kill the old executor.
        b.advance(301);
        w.pass().unwrap();
        assert_eq!(host_of(&task(&b, n)), Some("mac"));
        assert_eq!(ea.borrow().iter().chain(eb.borrow().iter()).filter(|(_, p)| p.borrow().is_none()).count(), 1);
        assert!(w.execution_waits[&n].contains("termination unconfirmed"));
        let snapshot = crate::snapshot::read(&b, &cb, &json!({}), &[]);
        assert!(snapshot.tasks.iter().find(|r| r.n == n).unwrap().json()["execution_wait"].as_str().unwrap().contains("termination unconfirmed"));
        assert!(snapshot.text(&cb, &json!({})).unwrap().contains("termination unconfirmed"));
        assert!(eb.borrow().is_empty());
        *ea.borrow()[0].1.borrow_mut() = Some(0);
        a.focus.clear();
        a.pass().unwrap();
        assert_eq!(task(&b, n).state, State::Ready);
        w.pass().unwrap();
        let t = task(&b, n);
        assert_eq!((t.state, host_of(&t)), (State::Doing, Some("win")));
        assert_eq!(t.retries, 1);
    }

    // Chain: #1226 open stage -> vanished child -> normal gone -> terminal record and freed lease.
    // Existing exit checks do not cross the retained profile store or selftest boundary.
    #[test]
    fn missing_terminal_record_repaired_only_by_normal_recovery() {
        let dir = std::env::temp_dir().join(format!("taskq-terminal-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (_capture,_guard)=crate::profile::tests::collecting(&dir);
        let b = Fake::default();
        let n = b.create("T", "", &["taskq".into()]).unwrap();
        let a = b.comment(n, &core::entry("spawn", &["worker", "claude:s", "mac"], None, "")).unwrap();
        let mut c = cfg("mac");
        c.install = dir.clone();
        c.root = dir.clone();
        let record=json!({"project":c.repo,"host":"mac","stage":"build","open":true,"task":n,"attempt":a,"rt":"claude",
            "outcome":"unknown","error":"none","at_ms":1,"unknown":"open-stage"});
        drop(crate::profile::log_lock(&dir.join("taskq.log")).unwrap());
        crate::telemetry::append(&dir,&format!("taskq-stage {record}"),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64).unwrap();
        assert!(crate::profile::selftest(&dir, &c.repo, &vec![], 300).unwrap_err().contains("FAIL"));
        // A live attempt stays exempt even after ttl; the check never mutates board state.
        assert!(crate::profile::selftest(&dir, &c.repo, &vec![(c.repo.clone(), a)], 300).is_ok());
        assert_eq!(task(&b, n).attempt, Some(a));
        let report = crate::profile::report(&dir);
        assert!(report.contains(&format!("| o/r | #{n} | {a} | claude | mac |")), "{report}");
        let mut s = service(&b, &c, Default::default());
        s.focus.clear();
        s.pass().unwrap();
        assert_eq!(task(&b, n).attempt, Some(a), "missing child is unknown, not exited");
        assert!(s.execution_waits.contains_key(&n));
        release_execution(&b, n, a, a, "exit").unwrap();
        s.pass().unwrap();
        assert!(b.comments(n).unwrap().iter().any(|c| c.body.starts_with(&format!("taskq: gone attempt {a}"))));
        assert_eq!(task(&b, n).attempt, None);
        assert!(!s.heartbeat_body().lines().any(|l| l == format!("#{n} {a}")));
        let repaired = crate::profile::read_log(&dir).unwrap();
        assert!(repaired.contains("\"outcome\":\"failed\",\"error\":\"exit\""), "{repaired}");
        assert!(crate::profile::selftest(&dir, &c.repo, &vec![], 300).is_ok());
        s.pass().unwrap();
        assert_eq!(crate::profile::read_log(&dir).unwrap(), repaired, "repair is idempotent");

        drop(_guard);drop(_capture);std::fs::remove_dir_all(dir).unwrap();
    }

    // Chain: §4 ended review -> left-board -> close -> next-pass repair -> selftest.
    // Risk: historical attempts absent from open views permanently block unrelated workers.
    #[test]
    fn closed_or_left_board_attempt_gets_terminal_record() {
        for closed in [true, false] {
            let dir = std::env::temp_dir().join(format!("taskq-ended-{}-{closed}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let (_capture,_guard)=crate::profile::tests::collecting(&dir);
            let b = Fake::default();
            let n = b.create("T", "", &["taskq".into()]).unwrap();
            let w = b.comment(n, "taskq: spawn worker codex:w mac").unwrap();
            b.comment(n, &format!("taskq: result {} attempt {w}", "a".repeat(40))).unwrap();
            let a = b.comment(n, "taskq: spawn reviewer codex:r mac").unwrap();
            assert_eq!(task(&b, n).attempt, Some(a));
            let mut c = cfg("mac");
            c.install = dir.clone();
            c.root = dir.clone();
            let path = dir.join("taskq.log");
            let record = json!({"project": c.repo, "host":"mac", "stage":"review", "task":n,
                "attempt":a, "rt":"codex", "outcome":"unknown", "error":"left-board", "at_ms":1});
            std::fs::write(&path, format!("taskq-stage {record}\n")).unwrap();
            if closed {
                b.comment(n, "taskq: close").unwrap();
                b.close(n).unwrap();
            } else {
                b.labels(n, &[], &["taskq".into()]).unwrap();
            }
            assert!(crate::profile::selftest(&dir, &c.repo, &vec![], 300).is_err());
            let mut s = service(&b, &c, Default::default());
            s.focus.clear();
            s.pass().unwrap();
            assert!(crate::profile::selftest(&dir, &c.repo, &vec![], 300).is_ok());
            let repaired = crate::profile::read_log(&dir).unwrap();
            s.pass().unwrap();
            assert_eq!(crate::profile::read_log(&dir).unwrap(), repaired);
            assert!(!b.comments(n).unwrap().iter().any(|c| c.body.starts_with("taskq: gone")));
            drop(_guard);drop(_capture);std::fs::remove_dir_all(dir).unwrap();
        }
    }

    // Chain: §4 historical no-host review -> fresh ended-board proof -> one-pass repair -> selftest.
    // Risk: #1252 stays blocked, or another project/host receives fabricated terminal evidence.
    #[test]
    fn historical_no_host_terminal_repair_requires_local_ownership() {
        for closed in [true, false] {
            for evidence in ["local", "foreign-project", "foreign-host", "missing-log", "empty-log", "unreadable-log", "ignored-spawn", "untrusted-spawn", "runtime-mismatch", "unreadable-board", "unreadable-comments"] {
                let dir = std::env::temp_dir().join(format!("taskq-no-host-{}-{closed}-{evidence}", std::process::id()));
                std::fs::create_dir_all(dir.join(".taskq")).unwrap();
                let b = Fake::default();
                let n = b.create("T", "", &["taskq".into()]).unwrap();
                let w = b.comment(n, "taskq: spawn worker codex:w mac").unwrap();
                b.comment(n, &format!("taskq: result {} attempt {w}", "a".repeat(40))).unwrap();
                let host = if evidence == "foreign-host" { "win" } else { "mac" };
                let spawn = format!("taskq: spawn reviewer codex:r {host}");
                let a = b.raw(n, &spawn, evidence != "untrusted-spawn", evidence == "ignored-spawn");
                let mut c = cfg("mac");
                c.install = dir.clone();
                c.root = dir.clone();
                let path = dir.join("taskq.log");
                let project = if evidence == "foreign-project" { "other/project" } else { &c.repo };
                let rt = if evidence == "runtime-mismatch" { "claude" } else { "codex" };
                let mut original = String::new();
                for (stage, outcome, error) in [("claim", "ok", "none"), ("start", "ok", "none"), ("checks", "ok", "none"), ("close", "failed", "other"), ("review", "unknown", "left-board")] {
                    let record = json!({"project":project, "stage":stage, "task":n, "attempt":a,
                        "rt":rt, "outcome":outcome, "error":error, "at_ms":1, "unknown":"no-usage"});
                    original += &format!("taskq-stage {record}\n");
                }
                std::fs::write(&path, &original).unwrap();
                let attempt_log = crate::runtime::attempt_log(&dir, "reviewer", n, a);
                if evidence == "unreadable-log" {
                    std::fs::create_dir(&attempt_log).unwrap();
                } else if evidence != "missing-log" {
                    std::fs::write(&attempt_log, if evidence == "empty-log" { "" } else { "original reviewer output\n" }).unwrap();
                }
                if closed {
                    b.comment(n, &format!("taskq: close attempt {a}")).unwrap();
                    b.close(n).unwrap();
                } else {
                    b.labels(n, &[], &["taskq".into()]).unwrap();
                }
                if evidence == "unreadable-board" { b.state.borrow_mut().fail_issue_read = Some((n, 1)); }
                if evidence == "unreadable-comments" { b.state.borrow_mut().fail_comment_reads = 1; }
                // Prepare the legacy fixture before collection; this case proves ownership, not import contention.
                crate::profile::prune(&dir).unwrap();
                let (_capture,_guard)=crate::profile::tests::collecting(&dir);
                assert!(crate::profile::selftest(&dir, project, &vec![], 300).is_err());
                let mut s = service(&b, &c, Default::default());
                s.focus.clear();
                s.pass().unwrap();
                let repaired = crate::profile::read_log(&dir).unwrap();
                assert!(repaired.starts_with(&original), "original history must survive");
                if evidence == "local" {
                    assert!(crate::profile::selftest(&dir, project, &vec![], 300).is_ok(), "{closed}: {repaired}");
                    let record: serde_json::Value = serde_json::from_str(repaired.lines().last().unwrap().strip_prefix("taskq-stage ").unwrap()).unwrap();
                    assert_eq!(record["host"], "mac");
                    assert_eq!(record["stage"], "review");
                    assert_eq!(record["outcome"], if closed { "ok" } else { "cancelled" });
                    assert!(record["us"].is_null() && record["calls"].is_null() && record["tokens"].is_null());
                    s.pass().unwrap();
                    assert_eq!(crate::profile::read_log(&dir).unwrap(), repaired, "idempotent repair");
                } else {
                    assert_eq!(repaired, original, "{closed}: {evidence} must stay unknown");
                    assert!(crate::profile::selftest(&dir, project, &vec![], 300).is_err());
                }
                if attempt_log.is_file() { assert_eq!(std::fs::read_to_string(attempt_log).unwrap(), if evidence == "empty-log" { "" } else { "original reviewer output\n" }); }
                assert!(!b.comments(n).unwrap().iter().any(|c| c.body.starts_with("taskq: gone")));
                drop(_guard);drop(_capture);std::fs::remove_dir_all(dir).unwrap();
            }
        }
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

    // Chain: native terminal reviewer -> same-pass gone -> retained result and classified stage.
    // Risk: native failure detail is lost between adapter and stage record.
    #[test]
    fn terminal_reviewer_records_native_error_and_releases_lease() {
        struct Terminal { root: PathBuf }
        impl Proc for Terminal {
            fn exited(&mut self) -> Result<Option<i32>, String> { Ok(Some(-1)) }
            fn exit_error(&self) -> Option<&'static str> {
                Some(crate::runtime::claude_failure(&serde_json::json!({"id":"abcdef12", "state":"failed"}),
                    &self.root, "abcdef12", Some(&self.root)))
            }
            fn kill(&mut self) -> Result<(), String> { Ok(()) }
            fn id(&self) -> u32 { 0 }
        }
        let b = Fake::default();
        let n = b.create("T", "", &["taskq".into()]).unwrap();
        let a = b.comment(n, "taskq: spawn worker claude:invented win").unwrap();
        b.comment(n, &core::entry("result", &["0123456789abcdef0123456789abcdef01234567"], Some(a), "ok")).unwrap();
        release_execution(&b, n, a, a, "exit").unwrap();
        let c = cfg("win");
        let root = std::env::temp_dir().join(format!("taskq-native-failure-{}", crate::runtime::uuid()));
        std::fs::create_dir_all(root.join("jobs/abcdef12")).unwrap();
        std::fs::write(root.join("jobs/abcdef12/state.json"), serde_json::json!({
            "cwd": root, "state":"failed", "detail":"ENAMETOOLONG: name too long, uv_spawn"
        }).to_string()).unwrap();
        let mut s = Service::new(&b, &c, "4.4.0", Box::new(|_| Ok(Box::new(Terminal { root: root.clone() }))));
        s.focus = vec!["all".into()];
        s.pass().unwrap();
        let r = task(&b, n).attempt.unwrap();
        crate::profile::STAGED.with(|v| v.borrow_mut().clear());
        s.focus.clear();
        s.pass().unwrap();
        assert!(s.kids.is_empty());
        assert!(b.comments(n).unwrap().iter().any(|e| e.body.contains(&format!("taskq: gone attempt {r}"))));
        assert_eq!((task(&b, n).state, task(&b, n).result.is_some()), (State::Review, true));
        crate::profile::STAGED.with(|v| assert!(v.borrow().contains(&format!("review {n} {r} claude failed argument-too-long"))));
        let failure = |row: serde_json::Value, id: &str, project: &std::path::Path| crate::runtime::claude_failure(&row, project, id, Some(&root));
        assert_eq!(failure(serde_json::json!({"state":"failed", "detail":"permission denied"}), "abcdef12", &root), "permission");
        assert_eq!(failure(serde_json::json!({"state":"stopped"}), "abcdef12", &root), "other");
        assert_eq!(failure(serde_json::json!({"state":"failed"}), "../abcdef12", &root), "other");
        assert_eq!(failure(serde_json::json!({"state":"failed"}), "abcdef12", &root.join("jobs")), "other");
        std::fs::remove_file(root.join("jobs/abcdef12/state.json")).unwrap();
        assert_eq!(failure(serde_json::json!({"state":"failed"}), "abcdef12", &root), "other");
        std::fs::remove_dir_all(&root).unwrap();
    }

    // Chain: § 3 claim across hosts to § 2 retry limit (#913, #1123). Risk: two hosts claim one result's review, the
    // loser writes more than its spawn, two reviewers run, or the limit parks the task on a card without options.
    #[test]
    fn lost_reviewer_claim_writes_nothing_more_and_the_gone_limit_asks_with_options() {
        let b = Fake::default();
        let n = b.create("T", "", &["taskq".into()]).unwrap();
        let w = b.comment(n, &core::entry("spawn", &["worker", "claude:s", "linux"], None, "")).unwrap();
        b.comment(n, &core::entry("result", &["0123456789abcdef0123456789abcdef01234567"], Some(w), "ok")).unwrap();
        release_execution(&b, n, w, w, "exit").unwrap();
        let (cm, cw) = (cfg("mac"), cfg("win"));
        let (em, ew): (Exits, Exits) = Default::default();
        let (mut mac, mut win) = (service(&b, &cm, em.clone()), service(&b, &cw, ew.clone()));
        // mac reads the task in review before win's spawn lands: both hosts post a reviewer spawn for one result
        let stale = views(&b, &mut HashMap::new(), &mut HashMap::new()).unwrap().0.into_iter().find(|v| v.issue.number == n).unwrap();
        win.pass().unwrap();
        let r = task(&b, n).attempt.unwrap();
        let (labels, comments) = (b.issue(n).unwrap().labels, b.comments(n).unwrap().len());
        mac.claim(&stale, "reviewer", "claude").unwrap();
        // Fresh ownership read prevents even a losing spawn for the stale view.
        assert_eq!((b.issue(n).unwrap().labels, b.comments(n).unwrap().len(), mac.kids.len(), em.borrow().len()), (labels, comments, 0, 0));
        assert_eq!((task(&b, n).attempt, task(&b, n).state), (Some(r), State::Review));
        // A3: later passes of both hosts start no second reviewer for this result
        mac.pass().unwrap();
        win.pass().unwrap();
        assert_eq!((em.borrow().len(), ew.borrow().len(), task(&b, n).attempt), (0, 1, Some(r)));
        // the reviewer exits twice without a verdict: the 2nd gone asks with the limit card
        *ew.borrow()[0].1.borrow_mut() = Some(1);
        win.pass().unwrap();
        win.pass().unwrap(); // the gone task is left alone for the rest of its pass
        assert_eq!((task(&b, n).state, task(&b, n).retries, ew.borrow().len()), (State::Review, 1, 2));
        *ew.borrow()[1].1.borrow_mut() = Some(1);
        win.pass().unwrap();
        let t = task(&b, n);
        let card = t.card.clone().unwrap();
        assert_eq!((t.state, t.retries, t.attempt), (State::Ask, 2, None));
        assert!(card.starts_with("exit 1") && card.contains("Problem: retry limit 2 reached by this gone"), "{card}");
        assert_eq!(core::options(&card).into_iter().map(|o| o.1.split(':').next().unwrap().to_string()).collect::<Vec<_>>(), ["requeue", "later"]);
        win.pass().unwrap(); // labels follow the fold on the next pass (§ 2)
        assert!(b.issue(n).unwrap().labels.contains(&"q-ask".to_string()), "{:?}", b.issue(n).unwrap().labels);
    }

    // Chain: § 2 retry limit after a failed start (#1123). Risk: a start failure whose comment read fails posts a
    // gone without a fresh fold, so the limit parks the task on a card without options.
    #[test]
    fn start_failure_with_failed_read_posts_nothing_and_the_next_pass_asks_with_the_card() {
        let b = Fake::default();
        let n = b.create("T", "", &["taskq".into()]).unwrap();
        let w = b.comment(n, &core::entry("spawn", &["worker", "claude:s", "linux"], None, "")).unwrap();
        b.comment(n, &core::entry("gone", &[], Some(w), "exit 1")).unwrap();
        release_execution(&b, n, w, w, "exit").unwrap();
        assert_eq!((task(&b, n).state, task(&b, n).retries), (State::Ready, 1));
        let c = cfg("mac");
        let mut s = Service::new(&b, &c, "4.0.0", Box::new(|_: &Launch| {
            b.state.borrow_mut().fail_comment_reads = 1; // the read after the failed start fails
            Err(crate::runtime::StartError::io(std::io::Error::other("boom"), Some("claude")))
        }));
        s.focus = vec!["all".into()];
        let comments = b.comments(n).unwrap().len();
        let e = s.pass().unwrap_err();
        assert!(e.contains("comment read failed"), "{e}");
        assert_eq!((b.comments(n).unwrap().len(), task(&b, n).state), (comments + 1, State::Doing), "only the spawn");
        s.pass().unwrap();
        let t = task(&b, n);
        let card = t.card.clone().unwrap();
        assert_eq!((t.state, t.retries), (State::Ask, 2));
        assert!(card.contains("confirmed NoStart") && card.contains("Problem: retry limit 2 reached by this gone"), "{card}");
        assert_eq!(core::options(&card).len(), 2);
    }

    // Chain: § 4 Filter, any-runtime and prefer-* admission over tests/golden/admit.json. Risk: the first listed runtime takes every slot while the other idles; a preference blocks a task or beats run-*.
    #[test]
    fn any_runtime_admission_follows_golden_traces() {
        let traces: Vec<serde_json::Value> = serde_json::from_str(include_str!("../tests/golden/admit.json")).unwrap();
        let rts = |v: &serde_json::Value| -> Vec<(String, u32)> { v.as_object().unwrap().iter().map(|(k, n)| (k.clone(), n.as_u64().unwrap() as u32)).collect() };
        let mut first_free_fails = false;
        for tr in &traces {
            let (limits, running, tasks) = (rts(&tr["limits"]), rts(&tr["running"]), tr["tasks"].as_u64().unwrap());
            let want: Vec<Option<String>> = serde_json::from_value(tr["expect"].clone()).unwrap();
            let b = Fake::default();
            // Running children hold doing tasks of this host.
            let held: Vec<(u64, u64, String)> = running.iter().flat_map(|(rt, n)| (0..*n).map(move |_| rt.clone())).map(|rt| {
                let n = b.create("T", "", &[TASK_LABEL.into()]).unwrap();
                (n, b.comment(n, &format!("taskq: spawn worker {rt}:s mac")).unwrap(), rt)
            }).collect();
            let mut labels = vec![TASK_LABEL.to_string()];
            labels.extend(serde_json::from_value::<Vec<String>>(tr.get("labels").cloned().unwrap_or(json!([]))).unwrap());
            let ns: Vec<u64> = (0..tasks).map(|_| b.create("T", "", &labels).unwrap()).collect();
            let mut c = cfg("mac");
            c.limits = limits.iter().cloned().collect();
            let mut s = service(&b, &c, Default::default());
            for (task, attempt, rt) in held {
                s.kids.push(Kid { execution: attempt, task, attempt, rt, proc: Box::new(FakeProc(Rc::new(RefCell::new(None)))) });
            }
            s.pass().unwrap();
            let got: Vec<Option<String>> = ns.iter().map(|&n| rt_of(&task(&b, n)).map(|(rt, _)| rt.to_string())).collect();
            assert_eq!(got, want, "{}", tr["name"]);
            let ignored = s.log.iter().filter(|l| l.contains("conflicting prefer-*")).count();
            assert_eq!(ignored, tr["ignored"].as_u64().unwrap_or(0) as usize, "{}", tr["name"]);
            // The old rule: every task takes the first listed runtime with a free slot.
            let mut used: BTreeMap<String, u32> = running.iter().cloned().collect();
            let old: Vec<Option<String>> = (0..tasks).map(|_| limits.iter().find(|(rt, n)| used.get(rt).copied().unwrap_or(0) < *n).map(|(rt, _)| {
                *used.entry(rt.clone()).or_default() += 1;
                rt.clone()
            })).collect();
            first_free_fails |= old != want;
        }
        assert!(first_free_fails, "the traces must reject the first-free rule");
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
            if worker == "codex-app" && !cfg!(any(target_os = "macos", windows)) {
                assert_eq!(t.state, State::Ready, "§ 9: app workers are macOS and Windows only");
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

    // H-A5: legacy Fixed/Auto drain forbids new workers/reviewers, but preserves and resumes its existing attempt.
    #[test]
    fn legacy_handoff_preserves_owned_answers_and_blocks_new_claims() {
        let b=Fake::default();let n=b.create("Owned","",&[TASK_LABEL.into()]).unwrap();
        let mut c=cfg("mac");c.limits.insert("claude".into(),3);
        let exits:Exits=Default::default();let mut s=service(&b,&c,exits.clone());
        s.pass().unwrap();let attempt=task(&b,n).attempt.unwrap();
        s.legacy_handoff=true;
        let ready=b.create("New worker","",&[TASK_LABEL.into()]).unwrap();
        let review=b.create("New reviewer","",&[TASK_LABEL.into()]).unwrap();
        let previous=b.comment(review,"taskq: spawn worker codex:invented mac").unwrap();
        b.comment(review,&core::entry("result",&["0123456789abcdef0123456789abcdef01234567"],Some(previous),"candidate")).unwrap();
        release_execution(&b,review,previous,previous,"exit").unwrap();
        s.pass().unwrap();
        assert_eq!(exits.borrow().len(),1,"handoff must refuse fresh worker and reviewer claims even with spare Fixed capacity");
        assert_eq!(task(&b,ready).state,State::Ready);assert_eq!(task(&b,review).attempt,None);
        b.comment(n,&core::entry("ask",&[],Some(attempt),"Choose?\n1. x ★\n2. y")).unwrap();
        b.comment(n,&core::entry("answer",&["1"],None,"x")).unwrap();
        *exits.borrow()[0].1.borrow_mut()=Some(0);s.pass().unwrap();
        assert_eq!(exits.borrow().len(),2,"the same owned attempt must still receive its answer");
        assert_eq!((task(&b,n).state,task(&b,n).attempt,task(&b,n).retries),(State::Doing,Some(attempt),0));
        *exits.borrow()[1].1.borrow_mut()=Some(-999);s.pass().unwrap();
        assert!(!s.idle());assert!(s.retained_attempts().contains(&attempt));
        assert!(s.capacity_json()["reasons"].as_array().unwrap().iter().any(|r|r["code"]=="handoff"));
        assert_eq!(task(&b,ready).state,State::Ready);assert_eq!(task(&b,review).attempt,None);
    }

    // H-A5: ready is a latch; an answer arriving after preparation belongs to the future host owner.
    #[test]
    fn prepared_legacy_drain_defers_late_answers_without_replaying_work() {
        let b=Fake::default();let n=b.create("Owned","",&[TASK_LABEL.into()]).unwrap();let c=cfg("mac");
        let exits:Exits=Default::default();let mut s=service(&b,&c,exits.clone());s.pass().unwrap();
        let attempt=task(&b,n).attempt.unwrap();s.legacy_handoff=true;
        b.comment(n,&core::entry("ask",&[],Some(attempt),"Choose?\n1. x ★\n2. y")).unwrap();
        *exits.borrow()[0].1.borrow_mut()=Some(0);s.pass().unwrap();assert!(s.idle());
        s.legacy_prepared=true;
        b.comment(n,&core::entry("answer",&["1"],None,"x")).unwrap();
        for _ in 0..3 {s.pass().unwrap();}
        assert_eq!(exits.borrow().len(),1,"prepared service cannot resume after its drain proof");
        assert_eq!(task(&b,n).attempt,Some(attempt));assert_eq!(task(&b,n).retries,0);
        assert!(entries(&b,n).unwrap().iter().any(|entry|entry.body.starts_with("taskq: answer")),"input remains on the board for the new host");
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

    // Chain: § 1 `Depends on <repo>#N` (#1126). Risk: a task blocked by another repository never starts, starts on a
    // close that did not complete, or reports no linked reason.
    #[test]
    fn cross_board_dependency_waits_with_link_until_completed() {
        let b = Fake::default();
        let dashq = |open: bool, reason: Option<&str>| ("dashq".to_string(), Issue { number: 36, open, reason: reason.map(String::from), ..Default::default() });
        b.state.borrow_mut().foreign.push(dashq(true, None));
        let dep = b.create("dep", "", &[]).unwrap();
        let n = b.create("T", "Depends on dashq#36\nDepends on owner/gone#5", &["taskq".into()]).unwrap();
        let same = b.create("S", &format!("Depends on #{dep}"), &["taskq".into()]).unwrap();
        // § 8 cutover: N-1 reads only `Depends on #N`; a held task has no `taskq` label, so N-1 (same `is_task`) skips it
        assert!(core::depends("Depends on dashq#36").iter().all(|d| d.repo.is_some()));
        let held = b.create("H", "Depends on dashq#36", &["q-held".into()]).unwrap();
        assert!(!is_task(&b.issue(held).unwrap()) && is_held(&b.issue(held).unwrap()));
        let mut c = cfg("mac");
        // A shared absolute path may contain real profile records on the test host.
        c.root = std::env::temp_dir().join(format!("taskq-cross-board-{}", crate::runtime::uuid()));
        c.install = c.root.clone();
        std::fs::create_dir_all(&c.root).unwrap();
        let raw = json!({});
        let report = || crate::snapshot::read(&b, &c, &raw, &[]).text(&c, &raw).unwrap();
        let mut s = service(&b, &c, Default::default());
        s.pass().unwrap();
        assert!(b.issue(n).unwrap().labels.contains(&"q-blocked".to_string()));
        assert!(report().contains("blocked ([dashq#36](https://board/dashq/36) open, [owner/gone#5](https://board/owner/gone/5) unknown: HTTP 404 owner/gone#5)"), "{}", report());
        b.state.borrow_mut().foreign = vec![dashq(false, Some("not_planned")), ("owner/gone".into(), Issue { number: 5, ..Default::default() })];
        b.state.borrow_mut().issues[dep as usize - 1].reason = Some("not_planned".into());
        b.state.borrow_mut().issues[dep as usize - 1].open = false;
        s.pass().unwrap();
        let blocked = |t: u64| b.issue(t).unwrap().labels.contains(&"q-blocked".to_string());
        assert!(blocked(n) && blocked(same), "not completed stays unknown, not done");
        assert!(report().contains("[dashq#36](https://board/dashq/36) unknown: closed not planned)"), "{}", report());
        b.state.borrow_mut().foreign[0] = dashq(false, Some("completed"));
        b.state.borrow_mut().reads = 0;
        s.pass().unwrap();
        assert_eq!(task(&b, n).state, State::Doing, "eligible within one pass after the blocker completed");
        assert_eq!(b.state.borrow().reads, 1, "closed dependencies already read are cached: only dashq#36 read again");
        drop(s);
        std::fs::remove_dir_all(&c.root).unwrap();
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
        let os = cfg!(any(target_os = "macos", windows));
        assert_eq!((task(&b, app).state, task(&b, other).state), (if os { State::Doing } else { State::Ready }, State::Ready), "only macOS and Windows admit app turns; unlabelled tasks never get the app");
        if os { assert!(task(&b, app).session.unwrap().starts_with("codex-app:- mac")); }
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

    // Chain: resource policy write (shared-resource-controls.md § Host adapter) -> lower limit -> service reload ->
    // pass. Risk: a lowered limit stops or drops the lease of an attempt that is already running.
    #[test]
    fn lowered_limit_keeps_running_attempts() {
        let b = Fake::default();
        let n = b.create("T", "", &["taskq".into()]).unwrap();
        let m = b.create("M", "", &["taskq".into()]).unwrap();
        let mut c = cfg("mac");
        c.limits.insert("claude".into(), 2);
        let mut s = service(&b, &c, Default::default());
        s.pass().unwrap();
        let (a, am) = (task(&b, n).attempt, task(&b, m).attempt);
        let o = b.create("O", "", &["taskq".into()]).unwrap();
        s.reload(Ok(Capacity::fixed(BTreeMap::from([("claude".to_string(), 0)]), GATE_DEFAULT)));
        b.advance(400);
        s.pass().unwrap();
        assert_eq!((s.kids.len(), task(&b, n).state, task(&b, m).state), (2, State::Doing, State::Doing), "both attempts run on");
        assert_eq!((task(&b, n).attempt, task(&b, m).attempt), (a, am), "leases kept");
        assert_eq!(task(&b, o).state, State::Ready, "no new claim under the lower limit");
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

    // Chain: #1173 logged state change -> pass replay -> `taskq-transition` record -> report. Risk: a transition
    // missed, doubled across passes or a restart, recorded by another host's service, or a zero entry time.
    #[test]
    fn a_pass_records_each_logged_transition_once() {
        let b = Fake::default();
        let (x, y) = (b.create("X", "", &["taskq".into()]).unwrap(), b.create("Y", "", &["taskq".into()]).unwrap());
        let z = b.create("Z", "", &["taskq".into()]).unwrap();
        b.comment(z, &core::entry("hold", &[], None, "")).unwrap();
        let mut c = cfg("mac");
        c.limits.insert("claude".into(), 1);
        let e: Exits = Default::default();
        let mut s = service(&b, &c, e.clone());
        s.pass().unwrap();
        let _ = crate::profile::take_moves();
        let ax = task(&b, x).attempt.unwrap();
        // another host claims y: its service records that move, not this one
        b.comment(y, &core::entry("spawn", &["worker", "codex:s", "win"], None, "")).unwrap();
        b.comment(x, &core::entry("result", &["0123456789abcdef0123456789abcdef01234567"], Some(ax), "ok")).unwrap();
        // the result and the requeue happen between two passes: both are recorded; the requeue left `review` without
        // an attempt, so every service records it without host
        b.comment(x, &core::entry("requeue", &[], None, "again")).unwrap();
        // `held` is outside the queue: neither `hold` nor this `start` is a transition
        b.comment(z, &core::entry("start", &[], None, "")).unwrap();
        *e.borrow()[0].1.borrow_mut() = Some(0);
        s.pass().unwrap();
        let created = |id: u64| b.state.borrow().comments.iter().find(|(_, c)| c.id == id).unwrap().1.created;
        let ids: Vec<u64> = core::changes(x, &entries(&b, x).unwrap()).iter().map(|e| e.0).collect();
        let (records, report) = crate::profile::take_moves();
        let r: Vec<serde_json::Value> = records.lines().map(|l| serde_json::from_str(l.strip_prefix("taskq-transition ").unwrap()).unwrap()).collect();
        let row = |v: &serde_json::Value| (v["task"].as_u64().unwrap(), v["from"].as_str().unwrap().to_string(), v["to"].as_str().unwrap().to_string(), v["prev_s"].as_u64(), v["host"].as_str().map(String::from));
        let mac = Some("mac".to_string());
        assert_eq!(r.iter().map(row).collect::<Vec<_>>()[..3], [(x, "ready".into(), "doing".into(), None, mac.clone()),
            (x, "doing".into(), "review".into(), Some((created(ids[1]) - created(ids[0])) as u64), mac.clone()),
            (x, "review".into(), "ready".into(), Some((created(ids[2]) - created(ids[1])) as u64), None)], "{records}");
        assert!(r.iter().all(|v| v["from"] != "held" && v["to"] != "held"), "{records}");
        assert!(r.iter().all(|v| v["task"] != y) && r[..2].iter().all(|v| v["rt"] == "claude") && r[2]["rt"].is_null(), "{records}");
        assert!(report.contains("| doing | review | claude | 1 |") && report.contains("| ready | doing | claude |"), "{report}");
        // the next pass and a restarted service record none of them again
        s.pass().unwrap();
        let again = crate::profile::take_moves().0;
        assert!(!again.contains("\"to\":\"review\""), "{again}");
        let mut restarted = service(&b, &c, Default::default());
        restarted.pass().unwrap();
        restarted.pass().unwrap();
        assert!(!crate::profile::take_moves().0.contains("\"to\":\"review\""));
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
        // a `gone` entry after the first pass is one `gone` event, once
        let v = b.create("V", "", &["taskq".into()]).unwrap();
        again.pass().unwrap();
        let av = task(&b, v).attempt.unwrap();
        b.comment(v, &core::entry("gone", &[], Some(av), "lost")).unwrap();
        again.pass().unwrap();
        again.pass().unwrap();
        let ev = std::fs::read_to_string(root.join(".taskq/events")).unwrap();
        assert_eq!(ev.lines().filter(|l| l.ends_with(&format!("#{v} gone {av}"))).count(), 1, "{ev}");
        let _ = std::fs::remove_dir_all(&root);
    }

    // Chain: closed task -> execution release -> worktree cleanup. Risk: deleting dirty or still-owned work.
    #[test]
    fn closed_tasks_lose_clean_worktrees_only() {
        let root = std::env::temp_dir().join(format!("taskq-prune-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let git = |args: &[&str]| assert!(std::process::Command::new("git").arg("-C").arg(&root).args(["-c", "user.name=t", "-c", "user.email=t@t"]).args(args).output().unwrap().status.success(), "{args:?}");
        git(&["init", "-q"]);
        git(&["commit", "-q", "--allow-empty", "-m", "init"]);
        for n in ["1", "2", "3", "4"] {
            git(&["worktree", "add", "-q", "-b", &format!("taskq-{n}"), &format!(".worktrees/taskq-{n}")]);
        }
        std::fs::write(root.join(".worktrees/taskq-2/wip"), "x").unwrap();
        let b = Fake::default();
        for _ in 0..4 {
            b.create("T", "", &["taskq".into()]).unwrap();
        }
        let execution = b.comment(4, "taskq: spawn worker claude:owned disconnected").unwrap();
        b.close(4).unwrap();
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
        assert!(wt("4"), "even forced cleanup preserves an unresolved execution's worktree");
        release_execution(&b, 4, execution, execution, "exit").unwrap();
        cleanup(&b, &root, |_| false, false);
        assert!(!wt("4"), "confirmed release permits cleanup");
        assert!(std::fs::read_to_string(root.join(".taskq/trash/taskq-2.patch")).unwrap().contains("wip"), "dirty work saved");
        let _ = std::fs::remove_dir_all(&root);
    }
}
