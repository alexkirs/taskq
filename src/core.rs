//! The task model of taskq.md § 2: one pure fold from log entries to a task. No I/O.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    #[default]
    Ready,
    Waiting,
    Doing,
    Review,
    Ask,
    Later,
    Failed,
    Closed,
    Held,
}

impl State {
    pub fn name(self) -> &'static str {
        use State::*;
        match self {
            Ready => "ready",
            Waiting => "waiting",
            Doing => "doing",
            Review => "review",
            Ask => "ask",
            Later => "later",
            Failed => "failed",
            Closed => "closed",
            Held => "held",
        }
    }
}

/// One `Depends on` reference (§ 1): issue `n` of this board (`repo` none) or of another repository on its host.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Dep {
    pub repo: Option<String>,
    pub n: u64,
}

impl Dep {
    /// `#N` or `<repo>#N`, as written in the issue text.
    pub fn name(&self) -> String {
        format!("{}#{}", self.repo.as_deref().unwrap_or_default(), self.n)
    }
}

/// `Depends on #N` and `Depends on <repo>#N` lines of an issue text (§ 1); `<repo>` is `name` or `owner/name`.
pub fn depends(body: &str) -> Vec<Dep> {
    body.lines().filter_map(|l| {
        let (repo, n) = l.trim().strip_prefix("Depends on ")?.split_whitespace().next()?.split_once('#')?;
        let path = |s: &str| s.split('/').all(|p| !p.is_empty() && !p.starts_with('.') && p.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)));
        Some(Dep { repo: (!repo.is_empty()).then(|| repo.to_string()).filter(|r| path(r)), n: n.parse().ok()? }).filter(|d| repo.is_empty() || d.repo.is_some())
    }).collect()
}

/// A log entry's comment body (§ 2).
pub fn entry(kind: &str, args: &[&str], attempt: Option<u64>, text: &str) -> String {
    let mut head = std::iter::once(kind).chain(args.iter().copied()).collect::<Vec<_>>().join(" ");
    if let Some(a) = attempt {
        head += &format!(" attempt {a}");
    }
    if text.is_empty() { format!("taskq: {head}") } else { format!("taskq: {head}\n{text}") }
}

/// § 2 retry limits: an attempt `requeue` that makes `retries` this, or a `gone` that makes it [`GONE_MAX`], asks.
pub const REQUEUE_MAX: u32 = 3;
pub const GONE_MAX: u32 = 2;

/// The text of attempt `kind` (`requeue`, `gone`) on `t` (§ 2 limit card, #1123): when it reaches the retry limit, the
/// reason plus a card naming the limit with requeue and later options, so the parked task never waits on a card
/// without choices. The writer adds it, so every version folds the same card.
pub fn limit_text(n: u64, t: &Task, kind: &str, text: &str) -> String {
    let max = match kind {
        "requeue" => REQUEUE_MAX,
        "gone" => GONE_MAX,
        _ => return text.into(),
    };
    let is_card = text.lines().any(|l| l.starts_with("Problem: ")) && options(text).iter().filter(|o| o.2).count() == 1;
    if !matches!(t.state, State::Doing | State::Review) || t.retries + 1 < max || is_card {
        return text.into();
    }
    // the reason's own numbered lines (a fix list) must not become options of the card
    let reason = text.lines().map(|l| {
        let body = l.trim_start();
        match body.split_once(". ") {
            Some((k, rest)) if !k.is_empty() && k.bytes().all(|b| b.is_ascii_digit()) => format!("{}{k}) {rest}", &l[..l.len() - body.len()]),
            _ => l.to_string(),
        }
    });
    let card = [
        format!("Problem: retry limit {max} reached by this {kind}; the task waits for the owner."),
        "1. requeue: retry with a fresh attempt ★".into(),
        "2. later: park the task until the owner requeues it".into(),
        // a bare comment `K` folds as a plain answer (ready), so `later` needs the CLI form (§ 2)
        format!("To answer: `taskq answer {n}.K`; a bare comment `K` makes the task ready, not later."),
    ];
    reason.filter(|s| !s.trim().is_empty()).chain(card).collect::<Vec<_>>().join("\n")
}

/// Appends the optional § 2 fields to an entry body: `op-id`, then `revision` (written only with an op id).
pub fn with_fields(body: String, op: Option<&str>, revision: Option<u64>) -> String {
    match (op, revision) {
        (Some(o), Some(r)) => format!("{body}\nop-id: {o}\nrevision: {r}"),
        (Some(o), None) => format!("{body}\nop-id: {o}"),
        _ => body,
    }
}

/// The op id a log entry of task `n` carries (§ 2); a bare comment carries none.
pub fn op_id(n: u64, body: &str) -> Option<&str> {
    parse(n, body)?.op
}

/// Whether `text` ends with lines that would read as the § 2 fields: such text is refused before writing.
pub fn has_fields(text: &str) -> bool {
    fields(text).1.is_some()
}

/// Text without its trailing fields and the op id: `op-id: <id>`, optionally followed by `revision: <R>`. Without
/// the op id line a trailing `revision:` stays text, so historical entries fold unchanged (R7).
fn fields(text: &str) -> (&str, Option<&str>) {
    let t = text.trim_end();
    let (rest, last) = t.rsplit_once('\n').unwrap_or(("", t));
    let (rest, last) = match last.trim().strip_prefix("revision: ").and_then(|r| r.trim().parse::<u64>().ok()) {
        Some(_) => rest.trim_end().rsplit_once('\n').unwrap_or(("", rest.trim_end())),
        None => (rest, last),
    };
    match last.trim().strip_prefix("op-id: ") {
        Some(o) => (rest.trim_end(), Some(o.trim())),
        None => (text, None),
    }
}

/// An owner gate (§ 2): `required` until the owner approves the current result, `stale` once a later result or a
/// move to `ready` invalidated an approval.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Gate {
    Required,
    Approved,
    Stale,
}

impl Gate {
    pub fn name(self) -> &'static str {
        match self {
            Gate::Required => "required",
            Gate::Approved => "approved",
            Gate::Stale => "stale",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Task {
    pub state: State,
    pub attempt: Option<u64>,
    pub role: Option<String>,
    pub session: Option<String>,
    pub retries: u32,
    pub card: Option<String>,
    pub result: Option<String>,
    pub inbox: Vec<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gate: Option<Gate>,
}

/// A comment by a trusted author; the board adapter drops everyone else's.
#[derive(Clone, Debug, Deserialize)]
pub struct Entry {
    pub id: u64,
    pub body: String,
    #[serde(default)]
    pub edited: bool,
}

struct Line<'a> {
    kind: &'a str,
    args: Vec<&'a str>,
    attempt: Option<u64>,
    text: &'a str,
    bare: bool,
    op: Option<&'a str>,
}

/// The first line of a log entry of task `n`; `taskq answer K|N.K` (the CLI form) counts as `taskq: answer K`, any other
/// comment as a bare answer that `pick` resolves against the open card.
fn parse(n: u64, body: &str) -> Option<Line<'_>> {
    let (head, text) = body.split_once('\n').unwrap_or((body, ""));
    let (text, op) = fields(text);
    if let Some(arg) = head.trim().strip_prefix("taskq answer ") {
        let k = match arg.trim().split_once('.') {
            Some((m, k)) => (m.trim_start_matches('#').parse() == Ok(n)).then_some(k)?,
            None => arg.trim(),
        };
        k.parse::<usize>().ok()?;
        return Some(Line { kind: "answer", args: vec![k], attempt: None, text: text.trim(), bare: false, op });
    }
    let Some(head) = head.trim().strip_prefix("taskq:") else {
        return Some(Line { kind: "answer", args: vec![body.trim()], attempt: None, text: "", bare: true, op: None });
    };
    let mut words: Vec<&str> = head.split_whitespace().collect();
    let mut attempt = None;
    if words.len() >= 3 && words[words.len() - 2] == "attempt" {
        attempt = Some(words.pop()?.parse().ok()?);
        words.pop();
    }
    let kind = words.first().copied()?;
    Some(Line { kind, args: words[1..].to_vec(), attempt, text: text.trim(), bare: false, op })
}

fn sha(s: &str) -> bool {
    s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn text(s: &str) -> Option<String> {
    (!s.is_empty()).then(|| s.to_string())
}

/// Option lines `K. text [★]` of a card.
pub fn options(card: &str) -> Vec<(usize, String, bool)> {
    card.lines()
        .filter_map(|l| {
            let (k, rest) = l.trim().split_once(". ")?;
            let k: usize = k.parse().ok()?;
            let star = rest.trim_end().ends_with('★');
            Some((k, rest.trim_end().trim_end_matches('★').trim().to_string(), star))
        })
        .collect()
}

/// The revision displayed by an approval option; no implicit approval of an unseen result.
pub fn approval_sha(option: &str) -> Option<&str> {
    let mut words = option.split_whitespace();
    if !words.next()?.eq_ignore_ascii_case("approve") { return None; }
    let sha = words.next()?;
    ((7..=40).contains(&sha.len()) && sha.bytes().all(|b| b.is_ascii_hexdigit())).then_some(sha)
}

/// The option a bare comment `s` names on `card` (§ 2): its number, or its text with or without `K. `, case-insensitive;
/// None unless exactly one option matches.
fn pick(card: &str, s: &str) -> Option<usize> {
    let s = s.trim_end_matches('★').trim().to_lowercase();
    let m: Vec<usize> = options(card)
        .into_iter()
        .filter(|(k, o, _)| [k.to_string(), o.to_lowercase(), format!("{k}. {}", o.to_lowercase())].contains(&s))
        .map(|o| o.0)
        .collect();
    (m.len() == 1).then(|| m[0])
}

/// What a resumed child receives for inbox entry `id` (§ 2): its text, else option K of the card it answers, else its
/// first line; never empty.
pub fn delivery(n: u64, entries: &[Entry], id: u64) -> String {
    let Some(e) = entries.iter().find(|e| e.id == id) else { return String::new() };
    let head = e.body.split_once('\n').map_or(e.body.as_str(), |h| h.0);
    let text = parse(n, &e.body).map_or("", |l| l.text);
    if !text.is_empty() {
        return text.to_string();
    }
    let before: Vec<Entry> = entries.iter().filter(|x| x.id < id).cloned().collect();
    let card = fold(n, &before, false).card.unwrap_or_default();
    let k = parse(n, &e.body).and_then(|l| if l.bare { pick(&card, l.args[0]) } else { l.args.first()?.parse::<usize>().ok() });
    options(&card).into_iter().find(|o| Some(o.0) == k).map(|o| o.1).unwrap_or_else(|| head.trim().to_string())
}

/// Whether attempt `a` wrote `ask`, `result` or `requeue` after entry `after`: its child then exits clean (§ 4 step 1).
pub fn handed_in(n: u64, entries: &[Entry], a: u64, after: u64) -> bool {
    entries.iter().filter(|e| e.id > after && !e.edited).filter_map(|e| parse(n, &e.body)).any(|l| l.attempt == Some(a) && matches!(l.kind, "ask" | "result" | "requeue"))
}

/// The kind of attempt `a`'s last `ask`, `result` or `requeue` after entry `after` that `fold` accepted: invalid,
/// duplicate and wrong-state entries change nothing and count as no hand-in.
fn hand_in(n: u64, entries: &[Entry], a: u64, after: u64) -> Option<&'static str> {
    let accepted = replay(n, entries, false).3;
    let mut log: Vec<&Entry> = entries.iter().filter(|e| e.id > after && accepted.contains(&e.id)).collect();
    log.sort_by_key(|e| e.id);
    log.into_iter().filter_map(|e| parse(n, &e.body)).filter(|l| l.attempt == Some(a))
        .filter_map(|l| ["ask", "result", "requeue"].into_iter().find(|k| *k == l.kind)).last()
}

/// Outcome and error class of attempt `a`'s `build`/`review` stage when its child exits or is stopped (§ 4 Profiling):
/// its hand-in after entry `after` wins over a stop; `log` is `None` once the issue left the board read.
pub fn stage_outcome(n: u64, log: Option<(&[Entry], bool)>, a: u64, after: u64, reviewer: bool) -> (&'static str, &'static str) {
    match log.and_then(|(entries, _)| hand_in(n, entries, a, after)) {
        Some("result") => ("ok", "none"),
        Some("ask") => ("ask", "none"),
        Some(_) => ("retried", "none"),
        // still the live attempt: the child ended without hand-in
        None if log.is_some_and(|(_, live)| live) => ("failed", "exit"),
        // the issue closed under a live reviewer: its own `close`
        None if log.is_none() && reviewer => ("ok", "none"),
        None => ("cancelled", "none"),
    }
}

fn has_options(s: &str) -> bool {
    s.lines().any(|l| l.split_once(". ").is_some_and(|(n, _)| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())))
}

fn end(t: &mut Task) {
    t.attempt = None;
    t.role = None;
    t.session = None;
    t.inbox.clear();
}

fn ready(t: &mut Task) {
    end(t);
    t.card = None;
    t.result = None;
    t.state = State::Ready;
    invalidate(t);
}

/// § 2: an approval binds to the result it named; the next result or a move to `ready` makes it stale.
fn invalidate(t: &mut Task) {
    if t.gate == Some(Gate::Approved) {
        t.gate = Some(Gate::Stale);
    }
}

pub fn fold(n: u64, entries: &[Entry], deps_open: bool) -> Task {
    replay(n, entries, deps_open).0
}

/// The task's revision (§ 2): the entry that set its open card, else its attempt, else its last accepted entry; 0: none.
pub fn revision(n: u64, entries: &[Entry]) -> u64 {
    replay(n, entries, false).2
}

/// Owner comments (§ 4): ids of the bare comments that the open `ask` ignored, i.e. that name no option of its card.
pub fn owner_comments(n: u64, entries: &[Entry]) -> Vec<u64> {
    replay(n, entries, false).1
}

/// The folded task, the ignored owner comments, the revision and the ids of the entries that changed the task.
fn replay(n: u64, entries: &[Entry], deps_open: bool) -> (Task, Vec<u64>, u64, Vec<u64>) {
    let mut log: Vec<&Entry> = entries.iter().filter(|e| !e.edited).collect();
    log.sort_by_key(|e| e.id);
    let mut t = Task::default();
    let mut resume = None; // the state an `ask` entry left; None when a retry limit asked
    let (mut free, mut accepted) = (vec![], vec![]);
    let (mut ops, mut card_at, mut last) = (std::collections::HashSet::new(), None, 0);
    for e in log {
        if let Some(l) = parse(n, &e.body) {
            // § 2: a repeated op id is ignored; a stale revision is the writer's to undo (dispatch::act)
            if l.op.is_some_and(|o| !ops.insert(o)) {
                continue;
            }
            let asked = t.state == State::Ask;
            let before = t.clone();
            step(&mut t, &mut resume, e.id, &l, last == 0);
            if t != before {
                last = e.id;
                accepted.push(e.id);
                if t.card.is_some() && t.card != before.card {
                    card_at = Some(e.id);
                }
            }
            match (asked, t.state == State::Ask) {
                (true, true) if l.bare && !e.body.trim().is_empty() => free.push(e.id),
                (_, false) => free.clear(),
                _ => {}
            }
        }
    }
    // ponytail: dependencies are judged at read time only, so a hand-typed spawn while waiting is accepted
    if t.state == State::Ready && deps_open {
        t.state = State::Waiting;
    }
    let rev = t.card.as_ref().and(card_at).or(t.attempt).unwrap_or(last);
    (t, free, rev, accepted)
}

fn step(t: &mut Task, resume: &mut Option<State>, id: u64, l: &Line, first: bool) {
    use State::*;
    if l.attempt.is_some() && l.attempt != t.attempt {
        return; // fencing: a replaced attempt writes nothing
    }
    let own = l.attempt.is_some();
    let spawn = |t: &mut Task, role: &str| {
        t.attempt = Some(id);
        t.role = Some(role.to_string());
        t.session = Some(format!("{} {}", l.args[1], l.args[2]));
        t.inbox.clear();
    };
    let limit = |t: &mut Task, resume: &mut Option<State>, max: u32| {
        t.retries += 1;
        if t.retries >= max {
            end(t);
            t.card = text(l.text);
            *resume = None;
            t.state = Ask;
            true
        } else {
            false
        }
    };
    match (t.state, l.kind, own) {
        // § 2: `hold` only as the first accepted entry; `start` leaves it
        (Ready, "hold", false) if first => t.state = Held,
        (Held, "start", false) => t.state = Ready,
        // § 2 gate: set before work; only the owner (no attempt) approves, and only the current result; an approval
        // without a gate serves a `review-pr` project (§ 7)
        (Held | Ready, "gate", false) if t.gate.is_none() => t.gate = Some(Gate::Required),
        (Review | Ask, "approve", false) if t.gate != Some(Gate::Approved) && l.args.len() == 1 && t.result.as_deref() == Some(l.args[0]) => {
            t.gate = Some(Gate::Approved)
        }
        (Ready, "spawn", false) if l.args.len() == 3 && l.args[0] == "worker" => {
            spawn(t, "worker");
            t.state = Doing;
        }
        (Review, "spawn", false) if l.args.len() == 3 && l.args[0] == "reviewer" && t.attempt.is_none() => {
            spawn(t, "reviewer")
        }
        (Doing, "result", true) if l.args.len() == 1 && sha(l.args[0]) => {
            end(t);
            t.result = Some(l.args[0].to_string());
            t.card = text(l.text).filter(|s| has_options(s));
            t.state = Review;
            invalidate(t);
        }
        (Doing | Review, "ask", true) => {
            t.card = text(l.text);
            *resume = Some(t.state);
            t.state = Ask;
        }
        (Doing | Review, "requeue", true) => {
            if !limit(t, resume, REQUEUE_MAX) {
                ready(t)
            }
        }
        (Doing | Review, "gone", true) => {
            if !limit(t, resume, GONE_MAX) {
                if t.state == Review { end(t) } else { ready(t) }
            }
        }
        (Doing, "steer", _) => t.inbox.push(id),
        (Review, "close", _) if t.gate.is_none_or(|g| g == Gate::Approved) => {
            end(t);
            t.state = Closed;
        }
        (Later | Failed, "close", false) => {
            end(t);
            t.state = Closed;
        }
        (Ask, "answer", _) if !l.bare || pick(t.card.as_deref().unwrap_or(""), l.args[0]).is_some() => {
            if !own {
                let card = t.card.as_deref().unwrap_or("");
                let k = if l.bare { pick(card, l.args[0]) } else { l.args.first().and_then(|s| s.parse().ok()) };
                if options(card).iter().any(|(n, option, _)| Some(*n) == k
                    && approval_sha(option).is_some_and(|sha| t.result.as_deref().is_some_and(|r| r.starts_with(sha))))
                {
                    t.gate = Some(Gate::Approved);
                }
            }
            t.retries = 0;
            t.card = None;
            match resume.take() {
                Some(s) => {
                    t.state = s;
                    t.inbox.push(id);
                }
                None => ready(t),
            }
        }
        (Ready | Waiting | Doing | Review | Ask | Later | Failed, "requeue", false) => {
            t.retries = 0;
            ready(t);
        }
        (Ready | Waiting | Doing | Review | Ask, "later" | "failed", _) => {
            end(t);
            t.state = if l.kind == "later" { Later } else { Failed };
        }
        _ => {}
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Trace {
    name: String,
    #[serde(default)]
    task: u64,
    #[serde(default)]
    deps_open: bool,
    entries: Vec<Entry>,
    expect: Task,
}

/// Fold every golden trace; Ok(count) or the first mismatch.
pub fn golden(json: &str) -> Result<usize, String> {
    let traces: Vec<Trace> = serde_json::from_str(json).map_err(|e| e.to_string())?;
    for tr in &traces {
        let got = fold(tr.task, &tr.entries, tr.deps_open);
        if got != tr.expect {
            return Err(format!("{}: got {:?}, want {:?}", tr.name, got, tr.expect));
        }
    }
    Ok(traces.len())
}

pub const GOLDEN: &str = include_str!("../tests/golden/fold.json");

#[cfg(test)]
mod tests {
    // Chain: § 2 fold over tests/golden. Risk: fold drifts from the traces; a wrong expectation still passes.
    #[test]
    fn golden_traces() {
        assert!(super::golden(super::GOLDEN).unwrap() > 20);
        let wrong = super::GOLDEN.replacen("\"retries\": 2}", "\"retries\": 1}", 1);
        assert!(super::golden(&wrong).is_err(), "a wrong expectation must fail");
    }

    // Chain: § 2 limit card (#1123). Risk: a card below the limit, a card added twice, or fix lines read as options.
    #[test]
    fn limit_text_adds_the_card_only_at_the_limit() {
        let t = |state, retries| super::Task { state, retries, ..Default::default() };
        let card = "Done: d\nProblem: p\n1. a ★\n2. b\nTo answer: x";
        assert_eq!(super::limit_text(7, &t(super::State::Review, 1), "requeue", "1. a"), "1. a");
        assert_eq!(super::limit_text(7, &t(super::State::Review, 2), "requeue", card), card);
        assert_eq!(super::limit_text(7, &t(super::State::Ask, 2), "requeue", "r"), "r");
        assert_eq!(super::limit_text(7, &t(super::State::Doing, 2), "steer", "r"), "r");
        let got = super::limit_text(7, &t(super::State::Doing, 1), "gone", "exit 1\n  2. b");
        assert_eq!(got.lines().take(3).collect::<Vec<_>>(), ["exit 1", "  2) b", "Problem: retry limit 2 reached by this gone; the task waits for the owner."]);
        assert_eq!(super::options(&got).iter().map(|o| (o.0, o.2)).collect::<Vec<_>>(), [(1, true), (2, false)]);
        assert!(super::limit_text(7, &t(super::State::Doing, 2), "requeue", "").starts_with("Problem: "));
    }

    /// R8 made mechanical: every (state, entry) pair of the § 2 table is accepted by some golden trace.
    // Chain: R8 § 2 table to golden traces; spec size. Risk: a § 2 row without a trace, taskq.md over 300 lines or dated.
    #[test]
    fn every_table_row_has_a_golden_trace() {
        let spec = include_str!("../taskq.md");
        let mut seen = std::collections::HashSet::new();
        for tr in serde_json::from_str::<Vec<super::Trace>>(super::GOLDEN).unwrap() {
            let mut log = tr.entries.clone();
            log.sort_by_key(|e| e.id);
            for i in 0..log.len() {
                let (before, after) = (super::fold(tr.task, &log[..i], tr.deps_open), super::fold(tr.task, &log[..=i], tr.deps_open));
                if let (Some(l), true) = (super::parse(tr.task, &log[i].body), before != after) {
                    seen.insert((before.state.name().to_string(), l.kind.to_string()));
                }
            }
        }
        let ticks = |cell: &str| -> Vec<String> {
            cell.split('`').skip(1).step_by(2).map(|s| s.split(' ').next().unwrap().to_string()).collect()
        };
        let rows = spec.lines().skip_while(|l| !l.starts_with("| State | Entry |")).skip(2).take_while(|l| l.starts_with('|'));
        let mut missing = vec![];
        for row in rows {
            let cells: Vec<&str> = row.split('|').collect();
            let states = if cells[1].contains("any open") {
                ["ready", "waiting", "doing", "review", "ask"].map(String::from).to_vec()
            } else {
                ticks(cells[1])
            };
            for s in &states {
                for k in ticks(cells[2]) {
                    if !seen.contains(&(s.clone(), k.clone())) {
                        missing.push(format!("{s} + {k}"));
                    }
                }
            }
        }
        assert!(missing.is_empty(), "§ 2 rows without a golden trace: {missing:?}");
        assert!(spec.lines().count() <= 300, "taskq.md over 300 lines");
        let dated = |w: &[u8]| w.len() == 10 && w[4] == b'-' && w[7] == b'-' && w.iter().filter(|b| b.is_ascii_digit()).count() == 8;
        assert!(!spec.as_bytes().windows(10).any(dated), "taskq.md holds a date: history goes to git and issues");
    }

    // Chain: § 2 ask answer to resumed worker. Risk: the worker resumes with the wrong answer text.
    #[test]
    fn delivery_falls_back_to_option_then_first_line() {
        let e = |id, body: &str| super::Entry { id, body: body.into(), edited: false };
        let mut log = vec![e(10, "taskq: spawn worker claude:s1 mac"), e(11, "taskq: ask attempt 10\nWhich?\n1. x\n2. y ★")];
        log.push(e(12, "taskq answer 7.2"));
        assert_eq!(super::delivery(7, &log, 12), "y");
        log[2] = e(12, "taskq: answer 2\nuse y, keep x");
        assert_eq!(super::delivery(7, &log, 12), "use y, keep x");
        log[2] = e(12, "2");
        assert_eq!(super::delivery(7, &log, 12), "y");
        log[2] = e(12, " 2. Y ★");
        assert_eq!(super::delivery(7, &log, 12), "y");
        log[2] = e(12, "taskq answer 3");
        assert_eq!(super::delivery(7, &log, 12), "taskq answer 3");
    }

    // Chain: § 2 ask to owner comment request. Risk: taskq entries or edited text treated as an owner request.
    #[test]
    fn only_free_comments_of_the_open_ask_are_owner_comments() {
        let e = |id, body: &str| super::Entry { id, body: body.into(), edited: false };
        let mut log = vec![e(9, "before any ask"), e(10, "taskq: spawn worker claude:s1 mac"), e(11, "taskq: ask attempt 10\nWhich?\n1. x\n2. y ★")];
        log.extend([e(12, "x or y? prefer z"), e(13, "taskq: steer\nlog entry"), e(14, "  ")]);
        log.push(super::Entry { id: 15, body: "edited".into(), edited: true });
        assert_eq!(super::owner_comments(7, &log), [12]);
        log.push(e(16, "2"));
        assert_eq!(super::owner_comments(7, &log), [0u64; 0], "an option answers the ask: as today, no owner comment");
        assert_eq!(super::fold(7, &log, false).state, super::State::Doing);
    }
}
