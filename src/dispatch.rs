//! The operation dispatcher (taskq.md § 5): every task write goes through `act`, which checks the expected revision and
//! the op id, appends one entry that `fold` accepts, reads it back and returns a receipt.

use crate::board::{self, Board, Publish, entries};
use crate::core::{self, Entry, Gate, State, Task, entry, fold, options};
use crate::service::{self, Config};
use crate::{Res, join, poke};
use serde::Serialize;
use std::path::Path;

/// Whether a `Depends on` issue of `body` is not done (§ 1); an unreadable one counts as not done.
pub fn deps_open(b: &dyn Board, body: &str) -> bool {
    !service::blockers(b, body, None, &mut Default::default()).is_empty()
}

/// This process's `TASKQ_TASK` and `TASKQ_ATTEMPT`.
type ChildEnv = (Option<String>, Option<String>);

/// `TASKQ_ATTEMPT` when this process is a child working on task `n` (§ 3 fencing).
fn own_attempt(n: u64, (task, attempt): &ChildEnv) -> Option<u64> {
    let task: u64 = task.as_deref()?.parse().ok()?;
    (task == n).then(|| attempt.as_deref()?.parse().ok()).flatten()
}

/// § 5: a child's `result`, `ask`, `requeue` or `gone` must come from the live attempt of its own task; a reused
/// session's stale `TASKQ_TASK`/`TASKQ_ATTEMPT` is refused by name instead of writing unfenced or ignored.
fn stale_child(n: u64, kind: &str, live: Option<u64>, (task, attempt): &ChildEnv) -> Option<String> {
    let attempt = attempt.as_deref()?;
    if !matches!(kind, "result" | "ask" | "requeue" | "gone") {
        return None;
    }
    let task = task.as_deref().unwrap_or_default();
    if task.parse() != Ok(n) {
        return Some(format!("TASKQ_TASK {task:?} is not #{n}: stale session environment"));
    }
    (attempt.parse().ok() != live).then(|| format!("TASKQ_ATTEMPT {attempt} is not the live attempt {}: stale session environment", live.map_or("none".into(), |a| a.to_string())))
}

pub fn current(b: &dyn Board, n: u64) -> Res<(board::Issue, Task, bool)> {
    let issue = b.issue(n)?;
    let deps = deps_open(b, &issue.body);
    let t = fold(n, &entries(b, n)?, deps);
    Ok((issue, t, deps))
}

/// `--op-id` and `--revision` of one command (§ 5).
#[derive(Clone, Default)]
pub struct Op {
    pub id: Option<String>,
    pub revision: Option<u64>,
}

/// What a command did (§ 5 receipt): `accepted` with the readback, `rejected`, `stale` or `unknown`.
#[derive(Debug, Serialize)]
pub struct Receipt {
    pub task: u64,
    pub op_id: Option<String>,
    pub outcome: &'static str,
    pub state: Option<&'static str>,
    pub revision: Option<u64>,
    pub reason: String,
    pub next: String,
    /// Extra success lines, e.g. the publication of `close`.
    #[serde(skip)]
    pub extra: String,
}

impl Receipt {
    fn new(n: u64, op: &Op, outcome: &'static str, reason: impl Into<String>, next: impl Into<String>) -> Receipt {
        Receipt { task: n, op_id: op.id.clone(), outcome, state: None, revision: None, reason: reason.into(), next: next.into(), extra: String::new() }
    }
    pub fn accepted(n: u64, op: &Op, state: State, revision: u64) -> Receipt {
        Receipt { state: Some(state.name()), revision: Some(revision), ..Receipt::new(n, op, "accepted", "", "") }
    }
    pub fn rejected(n: u64, op: &Op, reason: impl Into<String>) -> Receipt {
        Receipt::new(n, op, "rejected", reason, "Check the input and board/service status, then retry; use `taskq help` for syntax.")
    }
    pub fn ok(&self) -> bool {
        self.outcome == "accepted"
    }
    /// Notification failure cannot undo a confirmed board mutation or invite a duplicate write.
    pub fn wake(mut self, root: &Path) -> Self {
        if let Err(error) = poke(root) {
            if self.ok() {
                self.reason = error;
                self.next = "Board change accepted; check service status; do not repeat the board write.".into();
            }
        }
        self
    }
    /// The § 5 text: `#N <state>` (plus extra lines), else `Error:` and `Next:`.
    pub fn text(&self) -> String {
        if self.ok() {
            join(&[&format!("#{} {}", self.task, self.state.unwrap_or("unknown")), &self.extra, &self.reason, &self.next]).replace("\n\n", "\n")
        } else {
            let why = if self.reason.starts_with('#') { self.reason.clone() } else { format!("#{}: {}", self.task, self.reason) };
            format!("Error: {}\nNext: {}", crate::compact(&why, 153), crate::compact(&self.next, 154))
        }
    }
    pub fn json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

/// R4: append one entry that `fold` accepts, set the projected label, poke the local service. A seen op id returns
/// the first receipt; a stale revision or a refused entry writes nothing and names the current state (§ 5).
pub fn act(b: &dyn Board, root: &Path, n: u64, kind: &str, args: &[&str], text: &str, op: &Op) -> Receipt {
    act_child(b, root, n, kind, args, text, op, child_env())
}

/// The child environment; tests see none, so a worker's own `TASKQ_TASK` cannot change their outcome.
fn child_env() -> ChildEnv {
    if cfg!(test) { return (None, None); }
    (std::env::var("TASKQ_TASK").ok(), std::env::var("TASKQ_ATTEMPT").ok())
}

/// [`act`] for a process whose child environment is `env`.
#[allow(clippy::too_many_arguments)]
fn act_child(b: &dyn Board, root: &Path, n: u64, kind: &str, args: &[&str], text: &str, op: &Op, env: ChildEnv) -> Receipt {
    act_as(b, root, n, kind, args, text, op, own_attempt(n, &env), Some(&env))
}

/// [`act`] for an entry that names `attempt` (emergency recovery fences another process's attempt, § 4).
#[allow(clippy::too_many_arguments)]
fn act_as(b: &dyn Board, root: &Path, n: u64, kind: &str, args: &[&str], text: &str, op: &Op, attempt: Option<u64>, child: Option<&ChildEnv>) -> Receipt {
    let read = || -> Res<(board::Issue, bool, Vec<Entry>)> {
        let issue = b.issue(n)?;
        let deps = deps_open(b, &issue.body);
        Ok((issue, deps, entries(b, n)?))
    };
    let (issue, deps, mut log) = match read() {
        Ok(x) => x,
        Err(e) => return Receipt { outcome: "unknown", ..Receipt::rejected(n, op, e) },
    };
    let projection = core::project(n, &log, deps);
    let (before, rev) = (projection.task, projection.revision);
    let accepted = |s: State, rev: u64| Receipt::accepted(n, op, s, rev);
    if let Some(id) = &op.id
        && log.iter().any(|e| !e.edited && core::op_id(n, &e.body) == Some(id.as_str()))
    {
        // a retry: the first write stands, no further board write (even after it cleared the attempt)
        return accepted(if issue.open { before.state } else { State::Closed }, rev).wake(root);
    }
    if let Some(why) = child.and_then(|env| stale_child(n, kind, before.attempt, env)) {
        return Receipt { state: Some(before.state.name()), revision: Some(rev), next: "Run taskq from a fresh session of the live attempt, or unset TASKQ_TASK and TASKQ_ATTEMPT.".into(), ..Receipt::rejected(n, op, why) };
    }
    if !issue.open {
        return Receipt::rejected(n, op, format!("#{n} is closed"));
    }
    if op.revision.is_some_and(|r| r != rev) {
        let next = format!("Read `taskq inbox {n}`; retry with --revision {rev} if still wanted.");
        return Receipt { state: Some(before.state.name()), revision: Some(rev), ..Receipt::new(n, op, "stale", format!("revision {} is stale; current is {rev}", op.revision.unwrap_or(0)), next) };
    }
    if core::has_fields(text) {
        return Receipt::rejected(n, op, "text ends with an `op-id:` or `revision:` line; reword it");
    }
    let text = if attempt.is_some() && attempt == before.attempt { core::limit_text(n, &before, kind, text) } else { text.into() };
    let body = core::with_fields(entry(kind, args, attempt, &text), op.id.as_deref(), op.revision);
    log.push(Entry { id: u64::MAX, body: body.clone(), edited: false });
    if fold(n, &log, deps) == before {
        return Receipt { state: Some(before.state.name()), revision: Some(rev), ..Receipt::rejected(n, op, format!("#{n} is {}: `{kind}` not accepted", before.state.name())) };
    }
    let unknown = |e: String| Receipt::new(n, op, "unknown", e, format!("Run `taskq inbox {n}`, then retry with the same --op-id."));
    let id = match b.comment(n, &body) {
        Ok(id) => id,
        Err(e) => return unknown(e),
    };
    // re-read: report what the board holds (R12); a concurrent writer may have won the same revision
    let log = match entries(b, n) {
        Ok(l) => l,
        Err(e) => return unknown(e),
    };
    let mut log: Vec<Entry> = log.into_iter().filter(|e| !e.edited).collect();
    log.sort_by_key(|e| e.id);
    let Some(i) = log.iter().position(|e| e.id == id) else { return unknown(format!("comment {id} not read back")) };
    // lost: an earlier entry moved the revision, or `fold` ignores this one (a raced op id included)
    let before_write = core::project(n, &log[..i], deps);
    if op.revision.is_some_and(|r| r != before_write.revision) || before_write.task == fold(n, &log[..=i], deps) {
        // the losing entry edits itself: an edited entry is ignored by this and older versions alike (§ 2, § 8)
        if let Err(e) = b.edit_comment(n, id, &format!("{body}\n\nignored: another write won")) {
            return unknown(e);
        }
        log[i].edited = true;
        let projection = core::project(n, &log, deps);
        let (t, rev) = (projection.task, projection.revision);
        return Receipt { state: Some(t.state.name()), revision: Some(rev), ..Receipt::new(n, op, "stale", "another write won; this entry is ignored", format!("Read `taskq inbox {n}`; current revision is {rev}.")) };
    }
    let projection = core::project(n, &log, deps);
    let (t, rev) = (projection.task, projection.revision);
    if let Err(e) = service::project(b, &issue, &t) {
        return unknown(e);
    }
    accepted(t.state, rev).wake(root)
}

/// § 7: publish a reviewed result, then `close`; later and failed tasks close without publication.
/// § 4 Profiling: the `close` stage times the whole command, from entry through every receipt.
pub fn close_one(b: &dyn Board, cfg: &Config, n: u64, verdict: &str, op: &Op) -> Receipt {
    close_checked(b, cfg, n, verdict, op, &|_, _| Ok(()))
}

/// `close_one` after `check` accepts the task read; the CLI verdict validation runs inside the timed stage.
pub fn close_checked(b: &dyn Board, cfg: &Config, n: u64, verdict: &str, op: &Op, check: &dyn Fn(&board::Issue, &Task) -> Res<()>) -> Receipt {
    let began = std::time::Instant::now();
    let mut seen = (0, "other".to_string(), None);
    let r = close_body(b, cfg, n, verdict, op, check, &mut seen);
    let (outcome, err) = seen.2.unwrap_or(if r.ok() { ("ok", "none") } else { ("failed", crate::profile::class(&r.reason)) });
    crate::profile::stage("close", n, seen.0, &seen.1, Some(began.elapsed()), outcome, err, crate::profile::Usage::NoModel);
    r
}

/// `seen`: the attempt and runtime once read, and an outcome override (a pending publication is retried).
fn close_body(b: &dyn Board, cfg: &Config, n: u64, verdict: &str, op: &Op, check: &dyn Fn(&board::Issue, &Task) -> Res<()>, seen: &mut (u64, String, Option<(&'static str, &'static str)>)) -> Receipt {
    let (issue, t, _) = match current(b, n) {
        Ok(x) => x,
        Err(e) => return Receipt::rejected(n, op, e),
    };
    *seen = (t.attempt.unwrap_or(0), t.session.as_deref().and_then(|s| s.split(':').next()).unwrap_or("other").to_string(), None);
    if let Err(e) = check(&issue, &t) {
        return Receipt::rejected(n, op, e);
    }
    // a retried close returns its first receipt before publishing again
    if t.state == State::Closed || op.id.is_some() && entries(b, n).is_ok_and(|l| l.iter().any(|e| core::op_id(n, &e.body) == op.id.as_deref())) {
        return act(b, &cfg.root, n, "close", &[], verdict, op);
    }
    match t.state {
        State::Later | State::Failed => act(b, &cfg.root, n, "close", &[], verdict, op),
        State::Review if needs_approval(cfg, &t) && t.gate != Some(Gate::Approved) => {
            // § 7: no publication before the owner's approval of this result; the current policy decides (B4 fencing)
            let sha = t.result.unwrap_or_default();
            let r = Receipt::rejected(n, op, format!("gate is {}: the owner has not approved {}", t.gate.map_or("required", Gate::name), &sha[..sha.len().min(7)]));
            Receipt { state: Some(t.state.name()), next: format!("Owner: `taskq approve {n}`, or pick the approve option."), ..r }
        }
        State::Review => {
            let sha = t.result.unwrap_or_default();
            let rev = entries(b, n).map(|l| core::revision(n, &l)).unwrap_or(0);
            if op.revision.is_some_and(|r| r != rev) {
                return act(b, &cfg.root, n, "close", &[], verdict, op); // the stale receipt, before any publication
            }
            // § 4 Profiling: `checks` is the CI verification and merge call
            let began = std::time::Instant::now();
            let published = b.publish(n, &sha, &cfg.publish, &issue.title, &format!("Task: {}", b.url(n)));
            let (outcome, err) = match &published {
                Publish::Done(_) => ("ok", "none"),
                Publish::Pending(_) => ("retried", "none"),
                Publish::Failed(m) => ("failed", crate::profile::class(m)),
            };
            crate::profile::stage("checks", n, seen.0, &seen.1, Some(began.elapsed()), outcome, err, crate::profile::Usage::NoModel);
            if outcome == "retried" { seen.2 = Some((outcome, err)); }
            match published {
                Publish::Done(m) => {
                    // § 7 publication receipt: the policy it ran under and what the board read back
                    let m = format!("delivery {} r{}: {m}", cfg.delivery, cfg.delivery_revision);
                    let mut r = act(b, &cfg.root, n, "close", &[], &join(&[verdict, &m]), op);
                    r.extra = m;
                    r
                }
                Publish::Pending(m) => Receipt::rejected(n, op, format!("not closed: {m}")),
                Publish::Failed(m) => {
                    let r = act(b, &cfg.root, n, "requeue", &[], &format!("publication failed: {m}"), &Op::default());
                    Receipt::rejected(n, op, if r.ok() { format!("requeued: {m}") } else { format!("publication failed: {m}; {}", r.reason) })
                }
            }
        }
        s => Receipt { state: Some(s.name()), ..Receipt::rejected(n, op, format!("#{n} is {}: `close` not accepted", s.name())) },
    }
}

/// § 7: a gated task or a `review-pr` project publishes only the owner's approved result.
fn needs_approval(cfg: &Config, t: &Task) -> bool {
    t.gate.is_some() || cfg.delivery == "review-pr"
}

/// Whether this process is an agent (`TASKQ_ATTEMPT` set): it cannot approve a gate (§ 7).
pub fn agent() -> bool {
    std::env::var_os("TASKQ_ATTEMPT").is_some()
}

/// The owner's gate approval of the current result (§ 7). An agent cannot approve; `sha`, when given, must be that
/// result, so an approval never binds to a revision the owner did not see.
pub fn approve(b: &dyn Board, root: &Path, n: u64, sha: Option<&str>, op: &Op, agent: bool) -> Receipt {
    if agent {
        return Receipt::rejected(n, op, "an agent cannot approve a gate; the owner runs `approve`");
    }
    let t = match current(b, n) {
        Ok((_, t, _)) => t,
        Err(e) => return Receipt::rejected(n, op, e),
    };
    let Some(result) = t.result.clone() else {
        return Receipt { state: Some(t.state.name()), ..Receipt::rejected(n, op, format!("#{n} has no result to approve")) };
    };
    if let Some(s) = sha.filter(|s| !result.starts_with(s) || s.len() < 7) {
        return Receipt { state: Some(t.state.name()), ..Receipt::rejected(n, op, format!("{s} is not the current result {}", &result[..result.len().min(7)])) };
    }
    act_as(b, root, n, "approve", &[&result], "", op, None, None)
}

/// `answer N.K [M.K..]`: every pick is checked before any is written (§ 5).
pub fn answer_picks(b: &dyn Board, cfg: &Config, picks: &[String], op: &Op) -> Res<Vec<Receipt>> {
    let mut plan = vec![];
    for p in picks {
        let (n, k) = p.trim_start_matches('#').split_once('.').and_then(|(n, k)| Some((n.parse::<u64>().ok()?, k.parse::<usize>().ok()?))).ok_or(format!("not N.K: {p}"))?;
        let (_, t, _) = current(b, n)?;
        let log = entries(b, n)?;
        if op.id.as_deref().is_some_and(|id| log.iter().any(|e| !e.edited && core::op_id(n, &e.body) == Some(id)))
            || op.revision.is_some_and(|r| r != core::revision(n, &log))
        {
            plan.push((n, k, None));
            continue;
        }
        let card = t.card.clone().filter(|_| matches!(t.state, State::Ask | State::Review)).ok_or(format!("#{n} is {} without a question", t.state.name()))?;
        let opt = options(&card).into_iter().find(|o| o.0 == k).ok_or(format!("#{n} has no option {k}"))?.1;
        plan.push((n, k, Some((opt, t.state, needs_approval(cfg, &t)))));
    }
    let mut out = vec![];
    for (n, k, card) in plan {
        let Some((opt, state, gated)) = card else {
            out.push(act(b, &cfg.root, n, "answer", &[&k.to_string()], "", op));
            continue;
        };
        let close = opt.to_lowercase().starts_with("close");
        if gated && opt.to_lowercase().starts_with("approve") {
            let Some(sha) = core::approval_sha(&opt) else {
                out.push(Receipt::rejected(n, op, "approval option must name the displayed result SHA"));
                continue;
            };
            // § 7: approve only the displayed revision, then resume the reviewer.
            let first = Op { id: op.id.as_ref().map(|i| format!("{i}.approve")), revision: op.revision };
            let r = approve(b, &cfg.root, n, Some(sha), &first, agent());
            if state == State::Review || !r.ok() {
                out.push(r);
                continue;
            }
            out.push(act(b, &cfg.root, n, "answer", &[&k.to_string()], &opt, &Op { id: op.id.clone(), revision: None }));
            continue;
        }
        out.push(match (state, close) {
            // § 2: an option that starts with `later` parks the task, like the retry limit card's (#1123)
            _ if opt.to_lowercase().starts_with("later") => act(b, &cfg.root, n, "later", &[], &opt, op),
            (State::Ask, _) => {
                let r = act(b, &cfg.root, n, "answer", &[&k.to_string()], &opt, op);
                // the close an option starts runs after the answer, as one more op of the same id
                let then = Op { id: op.id.as_ref().map(|i| format!("{i}.close")), revision: None };
                if close && r.ok() && r.state == Some("review") { close_one(b, cfg, n, &opt, &then) } else { r }
            }
            (_, true) => close_one(b, cfg, n, &opt, op),
            (_, false) => act(b, &cfg.root, n, "requeue", &[], &opt, op),
        });
    }
    Ok(out)
}

/// One interrupted attempt of this host in the emergency recovery proposal (§ 4).
#[derive(Debug, PartialEq)]
pub struct Row {
    pub task: u64,
    pub state: &'static str,
    pub attempt: u64,
    /// `running pid P`, `no child` or `unknown (<reason>)`.
    pub live: String,
    /// The fencing op `gone attempt <attempt>` at this revision; none while liveness is unknown.
    pub revision: Option<u64>,
}

impl Row {
    pub fn op_id(&self) -> String {
        format!("recover.{}.{}", self.task, self.attempt)
    }
    pub fn text(&self) -> String {
        let head = format!("#{} {} attempt {}: {}", self.task, self.state, self.attempt, self.live);
        match self.revision {
            Some(r) => format!("{head}; op: gone attempt {} --revision {r} --op-id {}", self.attempt, self.op_id()),
            None => format!("{head}; no op: blocks relaunch until its liveness reads"),
        }
    }
}

/// The recovery proposal (§ 4): every `doing` or `review` attempt of host `me`, read against the service's children
/// (`status.json` `kids`, `Err`: unreadable or stale). Only a child read running or absent gets a fencing op.
pub fn proposal(b: &dyn Board, me: &str, kids: &Result<Vec<serde_json::Value>, String>) -> Res<Vec<Row>> {
    let mut rows = vec![];
    for i in b.issues(None)?.iter().filter(|i| service::is_task(i)) {
        let n = i.number;
        let log = entries(b, n)?;
        let projection = core::project(n, &log, false);
        let t = &projection.task;
        let (Some(a), Some(h)) = (t.attempt, service::host_of(&t)) else { continue };
        if h != me || !matches!(t.state, State::Doing | State::Review) {
            continue;
        }
        let kid = kids.as_ref().map(|k| k.iter().find(|k| k["attempt"].as_u64() == Some(a)));
        let live = match &kid {
            Err(e) => Err(e.to_string()),
            Ok(None) => Ok("no child".to_string()),
            Ok(Some(k)) if k["live"] == "unknown" => Err(k["reason"].as_str().unwrap_or("no reason").to_string()),
            Ok(Some(k)) => Ok(format!("running pid {}", k["pid"])),
        };
        let (live, revision) = match live {
            Ok(l) => (l, Some(projection.revision)),
            Err(e) => (format!("unknown ({e})"), None),
        };
        rows.push(Row { task: n, state: t.state.name(), attempt: a, live, revision });
    }
    rows.sort_by_key(|r| r.task);
    Ok(rows)
}

/// Apply the proposal's ops through [`act`]: one receipt per op; a repeated apply returns the first receipts.
pub fn recover(b: &dyn Board, root: &Path, rows: &[Row]) -> Vec<Receipt> {
    rows.iter().filter_map(|r| {
        let op = Op { id: Some(r.op_id()), revision: Some(r.revision?) };
        Some(act_as(b, root, r.task, "gone", &[], "emergency recovery: attempt fenced", &op, Some(r.attempt), None))
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::Fake;

    // Chain: committed board mutation -> failed wake -> same op retry; never duplicate the board write.
    #[test]
    fn wake_failure_preserves_acceptance_and_retry_only_notifies() {
        let b = Fake::default();
        let root = std::env::temp_dir().join(format!("taskq-wake-receipt-{}", std::process::id()));
        let path = crate::dot(&root).join("poke");
        std::fs::create_dir_all(&path).unwrap();
        let n = b.create("Wake fixture", "## Goal\ng", &["taskq".into()]).unwrap();
        let op = Op { id: Some("hold-once".into()), revision: None };
        let first = act(&b, &root, n, "hold", &[], "", &op);
        assert!(first.ok(), "{}", first.text());
        assert!(first.text().contains("service wake failed"));
        assert!(first.json().contains("wake-open"));
        let before = entries(&b, n).unwrap();
        assert_eq!(first.state, Some("held"));
        std::fs::remove_dir(&path).unwrap();
        let retry = act(&b, &root, n, "hold", &[], "", &op);
        assert!(retry.ok()); assert!(retry.reason.is_empty());
        assert_eq!(retry.revision, first.revision);
        assert_eq!(entries(&b, n).unwrap().len(), before.len());
        assert!(path.is_file());
        std::fs::remove_dir_all(root).unwrap();
    }

    // Chain: § 5 stale child environment (#1072). Risk: a reused session's TASKQ_TASK/TASKQ_ATTEMPT writes unfenced or ignored.
    #[test]
    fn stale_child_environment_is_refused() {
        let s = |k: &str, task: Option<&str>, attempt: Option<&str>| stale_child(7, k, Some(42), &(task.map(Into::into), attempt.map(Into::into)));
        assert_eq!(s("requeue", Some("7"), Some("42")), None);
        assert_eq!(s("requeue", None, None), None); // the owner, not a child
        assert_eq!(s("hold", Some("9"), Some("1")), None); // a child may add and hold another task
        assert_eq!(s("requeue", Some("9"), Some("42")).unwrap(), "TASKQ_TASK \"9\" is not #7: stale session environment");
        assert_eq!(s("result", None, Some("42")).unwrap(), "TASKQ_TASK \"\" is not #7: stale session environment");
        assert_eq!(s("ask", Some("7"), Some("41")).unwrap(), "TASKQ_ATTEMPT 41 is not the live attempt 42: stale session environment");
        assert!(stale_child(7, "gone", None, &(Some("7".into()), Some("42".into()))).unwrap().contains("live attempt none"));
    }

    // Chain: § 5 op-id replay over § 3 fencing (#1072). Risk: retrying an accepted requeue, whose write cleared the
    // attempt, is refused as stale instead of returning the first receipt.
    #[test]
    fn retried_child_op_id_returns_the_first_receipt_after_its_attempt_clears() {
        let (b, root) = (Fake::default(), std::env::temp_dir().join(format!("taskq-replay-{}", std::process::id())));
        let n = b.create("T", "## Goal
g", &["taskq".into()]).unwrap();
        b.comment(n, &entry("spawn", &["worker", "claude:u", "mac"], None, "")).unwrap();
        let a = fold(n, &entries(&b, n).unwrap(), false).attempt.unwrap();
        let env = (Some(n.to_string()), Some(a.to_string()));
        let op = |id: &str| Op { id: Some(id.into()), revision: None };
        let count = || b.comments(n).unwrap().len();
        let first = act_child(&b, &root, n, "requeue", &[], "blocked", &op("rq"), env.clone());
        assert_eq!((first.outcome, first.state), ("accepted", Some("ready")), "{}", first.text());
        assert_eq!(fold(n, &entries(&b, n).unwrap(), false).attempt, None);
        let before = count();
        let again = act_child(&b, &root, n, "requeue", &[], "blocked", &op("rq"), env.clone());
        assert_eq!((again.outcome, again.state, again.revision, count()), ("accepted", Some("ready"), first.revision, before));
        // a new op from the same, now stale, environment is still refused by name
        let other = act_child(&b, &root, n, "requeue", &[], "blocked", &op("rq2"), env);
        assert_eq!((other.outcome, count()), ("rejected", before));
        assert!(other.text().contains("is not the live attempt none"), "{}", other.text());
    }

    // Chain: § 2 retry limit to the owner's answer (#913, #1123). Risk: a reviewer's 3rd requeue after a lost double
    // claim parks the task in `ask` on a card without options, so no answer moves it.
    #[test]
    fn retry_limit_requeue_asks_with_requeue_and_later_options() {
        let (b, root) = (Fake::default(), std::env::temp_dir().join(format!("taskq-limit-{}", std::process::id())));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("taskq.json"), r#"{"repo":"o/r"}"#).unwrap();
        let (cfg, _) = crate::load(&root).unwrap();
        let sha = "0123456789abcdef0123456789abcdef01234567";
        let at_limit = |reviewers: &[&str]| {
            let n = b.create("T", "## Goal\ng", &["taskq".into()]).unwrap();
            for _ in 0..2 {
                let a = b.comment(n, &entry("spawn", &["worker", "claude:u", "mac"], None, "")).unwrap();
                let r = act_child(&b, &root, n, "requeue", &[], "blocked", &Op::default(), (Some(n.to_string()), Some(a.to_string())));
                assert_eq!(r.state, Some("ready"), "{}", r.text());
            }
            let t = fold(n, &entries(&b, n).unwrap(), false);
            assert_eq!((t.retries, t.card), (2, None), "a requeue below the limit carries no card");
            let a = b.comment(n, &entry("spawn", &["worker", "claude:u", "mac"], None, "")).unwrap();
            b.comment(n, &entry("result", &[sha], Some(a), "ok")).unwrap();
            // both hosts spawn a reviewer for this result; the first wins, the second is ignored
            let ids: Vec<u64> = reviewers.iter().map(|h| b.comment(n, &entry("spawn", &["reviewer", "codex:v", h], None, "")).unwrap()).collect();
            assert_eq!(fold(n, &entries(&b, n).unwrap(), false).attempt, Some(ids[0]));
            let r = act_child(&b, &root, n, "requeue", &[], "Fix:
1. x", &Op::default(), (Some(n.to_string()), Some(ids[0].to_string())));
            assert_eq!(r.state, Some("ask"), "{}", r.text());
            n
        };
        let n = at_limit(&["win", "mac"]);
        let t = fold(n, &entries(&b, n).unwrap(), false);
        let card = t.card.unwrap();
        assert_eq!((t.retries, card.lines().take(3).collect::<Vec<_>>()), (3, vec!["Fix:", "1) x", "Problem: retry limit 3 reached by this requeue; the task waits for the owner."]));
        assert_eq!(options(&card).into_iter().map(|o| (o.0, o.1.split(':').next().unwrap().to_string())).collect::<Vec<_>>(), [(1, "requeue".into()), (2, "later".into())]);
        assert!(b.issue(n).unwrap().labels.contains(&"q-ask".to_string()));
        assert_eq!(card.lines().last(), Some(format!("To answer: `taskq answer {n}.K`; a bare comment `K` makes the task ready, not later.").as_str()));
        // `taskq answer N.2` parks the task; a bare board comment `2` is a plain answer to `ready` (§ 2)
        let r = answer_picks(&b, &cfg, &[format!("{n}.2")], &Op::default()).unwrap().remove(0);
        assert_eq!((r.outcome, r.state), ("accepted", Some("later")), "{}", r.text());
        let n = at_limit(&["win", "mac"]);
        b.comment(n, "2").unwrap();
        assert_eq!(fold(n, &entries(&b, n).unwrap(), false).state, State::Ready);
        // `requeue` retries it with a fresh attempt
        let n = at_limit(&["mac", "win"]);
        let r = answer_picks(&b, &cfg, &[format!("{n}.1")], &Op::default()).unwrap().remove(0);
        assert_eq!((r.outcome, r.state), ("accepted", Some("ready")), "{}", r.text());
        assert_eq!(fold(n, &entries(&b, n).unwrap(), false).retries, 0);
    }

    // Chain: § 5 dispatch receipt and readback. Risk: a stale or retried op writes a second entry.
    #[test]
    fn stale_revisions_and_retried_op_ids_write_nothing() {
        let (b, root) = (Fake::default(), std::env::temp_dir().join(format!("taskq-dispatch-{}", std::process::id())));
        let n = b.create("T", "## Goal\ng", &["taskq".into()]).unwrap();
        b.comment(n, &entry("spawn", &["worker", "claude:u", "mac"], None, "")).unwrap();
        let a = fold(n, &entries(&b, n).unwrap(), false).attempt.unwrap();
        let card = b.comment(n, &entry("ask", &[], Some(a), "Which?\n1. x\n2. y ★")).unwrap();
        let count = || b.comments(n).unwrap().len();
        let before = count();
        let op = |id: &str, r: u64| Op { id: Some(id.into()), revision: Some(r) };
        // A2: a stale revision is refused, names the current one, and the board is unchanged (readback)
        let r = act(&b, &root, n, "answer", &["1"], "x", &op("pm", a));
        assert_eq!((r.outcome, r.revision, count()), ("stale", Some(card), before));
        assert!(r.text().contains(&format!("revision {a} is stale; current is {card}")) && r.text().contains(&format!("--revision {card}")), "{}", r.text());
        assert_eq!(fold(n, &entries(&b, n).unwrap(), false).state, State::Ask);
        // the first answer at the shown revision is accepted; a second client at the same revision is stale
        let r = act(&b, &root, n, "answer", &["1"], "x", &op("pm", card));
        assert_eq!((r.outcome, r.state, count()), ("accepted", Some("doing"), before + 1));
        assert_eq!(act(&b, &root, n, "answer", &["2"], "y", &op("tray", card)).outcome, "stale");
        // A3: the same op id again returns the first receipt and writes no second entry
        let again = act(&b, &root, n, "answer", &["1"], "x", &op("pm", card));
        assert_eq!((again.outcome, again.state, count()), ("accepted", Some("doing"), before + 1));
        // Numbered answers reconcile before looking for the card that the accepted write removed.
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("taskq.json"), r#"{"repo":"o/r"}"#).unwrap();
        let (cfg, _) = crate::load(&root).unwrap();
        let again = answer_picks(&b, &cfg, &[format!("{n}.1")], &op("pm", card)).unwrap().remove(0);
        assert_eq!((again.outcome, count()), ("accepted", before + 1));
        let stale = answer_picks(&b, &cfg, &[format!("{n}.2")], &op("tray2", card)).unwrap().remove(0);
        assert_eq!((stale.outcome, count()), ("stale", before + 1));
        // A5: one JSON object per receipt
        let v: serde_json::Value = serde_json::from_str(&again.json()).unwrap();
        assert_eq!((v["task"].as_u64(), v["op_id"].as_str(), v["outcome"].as_str(), v["state"].as_str()), (Some(n), Some("pm"), Some("accepted"), Some("doing")));
        let spoof = act(&b, &root, n, "steer", &[], "do x\nop-id: pm", &Op::default());
        assert_eq!((spoof.outcome, count()), ("rejected", before + 1), "user text never carries the fields");
        let rej = act(&b, &root, n, "result", &["abc"], "", &Op::default());
        assert_eq!(rej.outcome, "rejected");
        assert!(rej.text().starts_with("Error: ") && rej.text().contains("\nNext: "), "{}", rej.text());
        // a race: another client answers between the check and the write; the loser edits itself, N and N-1 agree
        let n = b.create("T2", "## Goal\ng", &["taskq".into()]).unwrap();
        b.comment(n, &entry("spawn", &["worker", "claude:u", "mac"], None, "")).unwrap();
        let a = fold(n, &entries(&b, n).unwrap(), false).attempt.unwrap();
        let card = b.comment(n, &entry("ask", &[], Some(a), "Which?\n1. x\n2. y ★")).unwrap();
        b.state.borrow_mut().race = Some(core::with_fields(entry("answer", &["1"], None, "x"), Some("pm"), Some(card)));
        let r = act(&b, &root, n, "answer", &["2"], "y", &op("tray", card));
        assert_eq!((r.outcome, r.state), ("stale", Some("doing")), "{}", r.text());
        let log = entries(&b, n).unwrap();
        assert!(log.last().unwrap().edited && fold(n, &log, false).inbox.len() == 1, "the losing entry is edited, so ignored");
        // the same race on a retried op id: the second write edits itself too
        b.comment(n, &entry("ask", &[], Some(a), "And?\n1. a\n2. b ★")).unwrap();
        let rev = core::revision(n, &entries(&b, n).unwrap());
        b.state.borrow_mut().race = Some(core::with_fields(entry("answer", &["1"], None, "a"), Some("pm2"), Some(rev)));
        assert_eq!(act(&b, &root, n, "answer", &["1"], "a", &Op { id: Some("pm2".into()), revision: None }).outcome, "stale");
        assert!(entries(&b, n).unwrap().last().unwrap().edited);
        let _ = std::fs::remove_dir_all(&root);
    }
}
