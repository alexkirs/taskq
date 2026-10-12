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
}

fn parse(body: &str) -> Option<Line<'_>> {
    let (head, text) = body.split_once('\n').unwrap_or((body, ""));
    let mut words: Vec<&str> = head.trim().strip_prefix("taskq:")?.split_whitespace().collect();
    let mut attempt = None;
    if words.len() >= 3 && words[words.len() - 2] == "attempt" {
        attempt = Some(words.pop()?.parse().ok()?);
        words.pop();
    }
    let kind = words.first().copied()?;
    Some(Line { kind, args: words[1..].to_vec(), attempt, text: text.trim() })
}

fn sha(s: &str) -> bool {
    s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn text(s: &str) -> Option<String> {
    (!s.is_empty()).then(|| s.to_string())
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

pub fn fold(entries: &[Entry], deps_open: bool) -> Task {
    let mut log: Vec<&Entry> = entries.iter().filter(|e| !e.edited).collect();
    log.sort_by_key(|e| e.id);
    let mut t = Task::default();
    let mut resume = None; // the state an `ask` entry left; None when a retry limit asked
    for e in log {
        if let Some(l) = parse(&e.body) {
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
        (Ask, "answer", _) => {
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
    deps_open: bool,
    entries: Vec<Entry>,
    expect: Task,
}

/// Fold every golden trace; Ok(count) or the first mismatch.
pub fn golden(json: &str) -> Result<usize, String> {
    let traces: Vec<Trace> = serde_json::from_str(json).map_err(|e| e.to_string())?;
    for tr in &traces {
        let got = fold(&tr.entries, tr.deps_open);
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
                let (before, after) = (super::fold(&log[..i], tr.deps_open), super::fold(&log[..=i], tr.deps_open));
                if let (Some(l), true) = (super::parse(&log[i].body), before != after) {
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
}
