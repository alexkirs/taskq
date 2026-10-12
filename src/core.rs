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
        }
    }
}

/// `Depends on #N` lines of an issue text (§ 1).
pub fn depends(body: &str) -> Vec<u64> {
    body.lines().filter_map(|l| l.trim().strip_prefix("Depends on #")?.split_whitespace().next()?.parse().ok()).collect()
}

/// A log entry's comment body (§ 2).
pub fn entry(kind: &str, args: &[&str], attempt: Option<u64>, text: &str) -> String {
    let mut head = std::iter::once(kind).chain(args.iter().copied()).collect::<Vec<_>>().join(" ");
    if let Some(a) = attempt {
        head += &format!(" attempt {a}");
    }
    if text.is_empty() { format!("taskq: {head}") } else { format!("taskq: {head}\n{text}") }
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
}

/// The first line of a log entry of task `n`; `taskq answer K|N.K` (the CLI form) counts as `taskq: answer K`, any other
/// comment as a bare answer that `pick` resolves against the open card.
fn parse(n: u64, body: &str) -> Option<Line<'_>> {
    let (head, text) = body.split_once('\n').unwrap_or((body, ""));
    if let Some(arg) = head.trim().strip_prefix("taskq answer ") {
        let k = match arg.trim().split_once('.') {
            Some((m, k)) => (m.trim_start_matches('#').parse() == Ok(n)).then_some(k)?,
            None => arg.trim(),
        };
        k.parse::<usize>().ok()?;
        return Some(Line { kind: "answer", args: vec![k], attempt: None, text: text.trim(), bare: false });
    }
    let Some(head) = head.trim().strip_prefix("taskq:") else {
        return Some(Line { kind: "answer", args: vec![body.trim()], attempt: None, text: "", bare: true });
    };
    let mut words: Vec<&str> = head.split_whitespace().collect();
    let mut attempt = None;
    if words.len() >= 3 && words[words.len() - 2] == "attempt" {
        attempt = Some(words.pop()?.parse().ok()?);
        words.pop();
    }
    let kind = words.first().copied()?;
    Some(Line { kind, args: words[1..].to_vec(), attempt, text: text.trim(), bare: false })
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
    let (head, text) = e.body.split_once('\n').unwrap_or((&e.body, ""));
    if !text.trim().is_empty() {
        return text.trim().to_string();
    }
    let before: Vec<Entry> = entries.iter().filter(|x| x.id < id).cloned().collect();
    let card = fold(n, &before, false).card.unwrap_or_default();
    let k = parse(n, &e.body).and_then(|l| if l.bare { pick(&card, l.args[0]) } else { l.args.first()?.parse::<usize>().ok() });
    options(&card).into_iter().find(|o| Some(o.0) == k).map(|o| o.1).unwrap_or_else(|| head.trim().to_string())
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
}

pub fn fold(n: u64, entries: &[Entry], deps_open: bool) -> Task {
    let mut log: Vec<&Entry> = entries.iter().filter(|e| !e.edited).collect();
    log.sort_by_key(|e| e.id);
    let mut t = Task::default();
    let mut resume = None; // the state an `ask` entry left; None when a retry limit asked
    for e in log {
        if let Some(l) = parse(n, &e.body) {
            step(&mut t, &mut resume, e.id, &l);
        }
    }
    // ponytail: dependencies are judged at read time only, so a hand-typed spawn while waiting is accepted
    if t.state == State::Ready && deps_open {
        t.state = State::Waiting;
    }
    t
}

fn step(t: &mut Task, resume: &mut Option<State>, id: u64, l: &Line) {
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
        }
        (Doing | Review, "ask", true) => {
            t.card = text(l.text);
            *resume = Some(t.state);
            t.state = Ask;
        }
        (Doing | Review, "requeue", true) => {
            if !limit(t, resume, 3) {
                ready(t)
            }
        }
        (Doing | Review, "gone", true) => {
            if !limit(t, resume, 2) {
                if t.state == Review { end(t) } else { ready(t) }
            }
        }
        (Doing, "steer", _) => t.inbox.push(id),
        (Review, "close", _) | (Later | Failed, "close", false) => {
            end(t);
            t.state = Closed;
        }
        (Ask, "answer", _) if !l.bare || pick(t.card.as_deref().unwrap_or(""), l.args[0]).is_some() => {
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
    #[test]
    fn golden_traces() {
        assert!(super::golden(super::GOLDEN).unwrap() > 20);
        let wrong = super::GOLDEN.replacen("\"retries\": 2}", "\"retries\": 1}", 1);
        assert!(super::golden(&wrong).is_err(), "a wrong expectation must fail");
    }

    /// R8 made mechanical: every (state, entry) pair of the § 2 table is accepted by some golden trace.
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
}
