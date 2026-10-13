//! The operation dispatcher (taskq.md § 5): every task write goes through `act`, which checks the expected revision and
//! the op id, appends one entry that `fold` accepts, reads it back and returns a receipt.

use crate::board::{self, Board, Publish, entries};
use crate::core::{self, Entry, State, Task, entry, fold, options};
use crate::service::{self, Config};
use crate::{Res, join, poke};
use serde::Serialize;
use std::path::Path;

pub fn deps_open(b: &dyn Board, body: &str) -> Res<bool> {
    for d in core::depends(body) {
        if b.issue(d)?.open {
            return Ok(true);
        }
    }
    Ok(false)
}

/// `TASKQ_ATTEMPT` when this process is a child working on task `n` (§ 3 fencing).
fn own_attempt(n: u64) -> Option<u64> {
    let task: u64 = std::env::var("TASKQ_TASK").ok()?.parse().ok()?;
    (task == n).then(|| std::env::var("TASKQ_ATTEMPT").ok()?.parse().ok()).flatten()
}

pub fn current(b: &dyn Board, n: u64) -> Res<(board::Issue, Task, bool)> {
    let issue = b.issue(n)?;
    let deps = deps_open(b, &issue.body)?;
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
    /// The § 5 text: `#N <state>` (plus extra lines), else `Error:` and `Next:`.
    pub fn text(&self) -> String {
        if self.ok() {
            join(&[&format!("#{} {}", self.task, self.state.unwrap_or("unknown")), &self.extra]).replace("\n\n", "\n")
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
    let read = || -> Res<(board::Issue, bool, Vec<Entry>)> {
        let issue = b.issue(n)?;
        let deps = deps_open(b, &issue.body)?;
        Ok((issue, deps, entries(b, n)?))
    };
    let (issue, deps, mut log) = match read() {
        Ok(x) => x,
        Err(e) => return Receipt::rejected(n, op, e),
    };
    let before = fold(n, &log, deps);
    let rev = core::revision(n, &log);
    let accepted = |s: State, rev: u64| Receipt::accepted(n, op, s, rev);
    if let Some(id) = &op.id
        && log.iter().any(|e| !e.edited && core::op_id(n, &e.body) == Some(id.as_str()))
    {
        // a retry: the first write stands, nothing more is written
        return accepted(if issue.open { before.state } else { State::Closed }, rev);
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
    let body = core::with_fields(entry(kind, args, own_attempt(n), text), op.id.as_deref(), op.revision);
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
    if op.revision.is_some_and(|r| r != core::revision(n, &log[..i])) || fold(n, &log[..i], deps) == fold(n, &log[..=i], deps) {
        // the losing entry edits itself: an edited entry is ignored by this and older versions alike (§ 2, § 8)
        if let Err(e) = b.edit_comment(n, id, &format!("{body}\n\nignored: another write won")) {
            return unknown(e);
        }
        log[i].edited = true;
        let (t, rev) = (fold(n, &log, deps), core::revision(n, &log));
        return Receipt { state: Some(t.state.name()), revision: Some(rev), ..Receipt::new(n, op, "stale", "another write won; this entry is ignored", format!("Read `taskq inbox {n}`; current revision is {rev}.")) };
    }
    let (t, rev) = (fold(n, &log, deps), core::revision(n, &log));
    if let Err(e) = service::project(b, &issue, &t) {
        return unknown(e);
    }
    poke(root);
    accepted(t.state, rev)
}

/// § 7: publish a reviewed result, then `close`; later and failed tasks close without publication.
pub fn close_one(b: &dyn Board, cfg: &Config, n: u64, verdict: &str, op: &Op) -> Receipt {
    let (issue, t, _) = match current(b, n) {
        Ok(x) => x,
        Err(e) => return Receipt::rejected(n, op, e),
    };
    // a retried close returns its first receipt before publishing again
    if t.state == State::Closed || op.id.is_some() && entries(b, n).is_ok_and(|l| l.iter().any(|e| core::op_id(n, &e.body) == op.id.as_deref())) {
        return act(b, &cfg.root, n, "close", &[], verdict, op);
    }
    match t.state {
        State::Later | State::Failed => act(b, &cfg.root, n, "close", &[], verdict, op),
        State::Review => {
            let sha = t.result.unwrap_or_default();
            let rev = entries(b, n).map(|l| core::revision(n, &l)).unwrap_or(0);
            if op.revision.is_some_and(|r| r != rev) {
                return act(b, &cfg.root, n, "close", &[], verdict, op); // the stale receipt, before any publication
            }
            match b.publish(n, &sha, &cfg.publish, &issue.title, &format!("Task: {}", b.url(n))) {
                Publish::Done(m) => {
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

/// `answer N.K [M.K..]`: every pick is checked before any is written (§ 5).
pub fn answer_picks(b: &dyn Board, cfg: &Config, picks: &[String], op: &Op) -> Res<Vec<Receipt>> {
    let mut plan = vec![];
    for p in picks {
        let (n, k) = p.trim_start_matches('#').split_once('.').and_then(|(n, k)| Some((n.parse::<u64>().ok()?, k.parse::<usize>().ok()?))).ok_or(format!("not N.K: {p}"))?;
        let (_, t, _) = current(b, n)?;
        let card = t.card.clone().filter(|_| matches!(t.state, State::Ask | State::Review)).ok_or(format!("#{n} is {} without a question", t.state.name()))?;
        let opt = options(&card).into_iter().find(|o| o.0 == k).ok_or(format!("#{n} has no option {k}"))?.1;
        plan.push((n, k, opt, t.state));
    }
    let mut out = vec![];
    for (n, k, opt, state) in plan {
        let close = opt.to_lowercase().starts_with("close");
        out.push(match (state, close) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::Fake;

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
