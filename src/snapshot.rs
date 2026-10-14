//! Canonical read model (R6, § 4, § 5): one snapshot of the open tasks renders `status`, `status --json`, the
//! `status.json` tasks and totals, `service status` and the tray counts.

use crate::board::{self, Board};
use crate::core::{State, Task, options};
use crate::service::{self, Config, View};
use crate::{Res, age, cell, compact, runtime};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};

pub const SCHEMA: u64 = 1;
/// Every open state; `totals` names each one, zero included.
pub const STATES: [&str; 8] = ["held", "ready", "waiting", "doing", "review", "ask", "later", "failed"];
/// What a reader finds in a snapshot of each source.
const BOARD_CAPS: [&str; 4] = ["tasks", "totals", "questions", "hosts"];
const HOST_CAPS: [&str; 6] = ["tasks", "totals", "focus", "capacity", "agents", "events"];

pub struct Row {
    pub n: u64,
    pub title: String,
    pub labels: Vec<String>,
    pub url: String,
    pub task: Task,
    /// § 2 revision: the expected `--revision` of the next command.
    pub revision: u64,
    /// `Depends on` issues that keep it waiting (§ 1).
    pub deps: Vec<service::Blocker>,
    /// Text of the last trusted `failed` entry.
    pub failed: Option<String>,
}

pub struct Host {
    pub name: String,
    pub version: String,
    pub updated: i64,
    /// `off` or `unknown` when the host admits nothing by focus (§ 4).
    pub focus: Option<String>,
}

pub struct Snapshot {
    /// `board`: this command read the board; `host`: the service's `status.json`.
    pub source: &'static str,
    /// Board time of the read.
    pub observed: i64,
    /// Why the snapshot is incomplete; `None`: complete.
    pub reason: Option<String>,
    pub board: Value,
    pub tasks: Vec<Row>,
    pub hosts: Vec<Host>,
}

/// The one snapshot function: open tasks by priority then number, plus host heartbeats.
pub fn from_views(source: &'static str, b: &dyn Board, cfg: &Config, observed: i64, views: &[View], hosts: &[board::Issue]) -> Snapshot {
    let mut open: Vec<&View> = views.iter().filter(|v| v.task.state != State::Closed).collect();
    open.sort_by_key(|v| (service::priority(&v.issue), v.issue.number));
    let tasks = open.into_iter().map(|v| Row {
        n: v.issue.number,
        title: v.issue.title.clone(),
        labels: v.issue.labels.clone(),
        url: b.url(v.issue.number),
        task: v.task.clone(),
        revision: crate::core::revision(v.issue.number, &v.log),
        deps: v.blockers.clone(),
        failed: (v.task.state == State::Failed).then(|| v.comments.iter().rev().find(|c| c.trusted && c.body.starts_with("taskq: failed")).and_then(|c| c.body.split_once('\n')).map(|x| x.1.to_string()).unwrap_or_default()),
    }).collect();
    let hosts = hosts.iter().map(|h| Host {
        name: h.title.trim_start_matches("taskq host ").to_string(),
        version: h.body.lines().find_map(|l| l.strip_prefix("version ")).unwrap_or("?").to_string(),
        updated: h.updated,
        focus: h.body.lines().find_map(|l| l.strip_prefix("focus ")).map(str::to_string),
    }).collect();
    Snapshot { source, observed, reason: None, board: identity(b, cfg), tasks, hosts }
}

fn identity(b: &dyn Board, cfg: &Config) -> Value {
    json!({"kind": cfg.board.name, "host": cfg.web_host, "repo": cfg.repo, "url": b.url(0).trim_end_matches("/0")})
}

/// `taskq status`: one board read; a failed read is an incomplete snapshot with its reason, never zero tasks.
pub fn read(b: &dyn Board, cfg: &Config, raw: &Value, only: &[String]) -> Snapshot {
    let attempt = || -> Res<Snapshot> {
        let (mut views, hosts) = service::views(b, &mut HashMap::new(), &mut HashMap::new())?;
        views.retain(|v| in_scope(&v.issue, only));
        if let Some(who) = raw["assignee"].as_str() {
            let who = if who == "me" { b.me()? } else { who.to_string() };
            let mut keep = vec![];
            for v in views { if service::assigned(b, &v.issue, &who)? { keep.push(v); } }
            views = keep;
        }
        Ok(from_views("board", b, cfg, b.now()?, &views, &hosts))
    };
    attempt().unwrap_or_else(|e| Snapshot { source: "board", observed: b.now().unwrap_or(0), reason: Some(e), board: identity(b, cfg), tasks: vec![], hosts: vec![] })
}

/// A PM's scope (§ 5 `--only`): no labels means every task.
pub fn in_scope(i: &board::Issue, only: &[String]) -> bool {
    only.is_empty() || only.iter().any(|f| crate::labels::matches(&i.labels, f))
}

/// Per-state totals of open task states; the one count used by every reader.
pub fn totals<'a>(states: impl IntoIterator<Item = &'a str>) -> BTreeMap<&'static str, usize> {
    let mut t: BTreeMap<&'static str, usize> = STATES.iter().map(|s| (*s, 0)).collect();
    for s in states {
        if let Some(k) = STATES.iter().find(|x| **x == s) {
            *t.get_mut(k).unwrap() += 1;
        }
    }
    t
}

/// R6 line 2 from totals.
pub fn counts_line(t: &BTreeMap<&'static str, usize>) -> String {
    format!("In work {} · Waiting for answer {} · Ready {}", t["doing"] + t["review"], t["ask"], t["ready"])
}

/// A `status.json` is complete when its service's last pass read the board; N-1 files have no `complete` key.
pub fn complete(status: &Value) -> bool {
    status["complete"].as_bool().unwrap_or(status["error"].is_null()) && status["tasks"].is_array()
}

/// Totals of a `status.json`; `None` when it is incomplete (unknown is never zero).
pub fn status_totals(status: &Value) -> Option<BTreeMap<&'static str, usize>> {
    complete(status).then(|| totals(status["tasks"].as_array().into_iter().flatten().filter_map(|t| t["state"].as_str())))
}

impl Row {
    pub fn json(&self) -> Value {
        json!({"n": self.n, "title": self.title, "state": self.task.state.name(), "labels": self.labels, "url": self.url, "revision": self.revision,
            "session": self.task.session, "attempt": self.task.attempt, "role": self.task.role,
            "deps": self.deps.iter().filter(|b| b.dep.repo.is_none() && b.reason == "open").map(|b| b.dep.n).collect::<Vec<_>>(),
            "blocked": self.deps.iter().map(|b| json!({"ref": b.dep.name(), "url": b.url, "reason": b.reason})).collect::<Vec<_>>(),
            "card": self.task.card, "question": self.task.card.as_deref().map(question), "failed": self.failed})
    }
}

/// Free-text answer limits (`answer N --text`, § 2): nonblank lines, characters per line.
pub const ANSWER_LINES: usize = 4;
pub const ANSWER_CHARS: usize = 240;

/// Canonical parsed fields of an open card (§ 2, docs/design/answer-card.md), the one parse every card UI reads.
/// `parsed` false: a reader shows `text` with the board link and no write controls.
pub fn question(card: &str) -> Value {
    let field = |p: &str| card.lines().find_map(|l| l.strip_prefix(p)).map(str::trim);
    let evidence: Vec<Value> = field("Evidence: ").unwrap_or("").split("), ").filter_map(|l| {
        let (label, url) = l.strip_prefix('[')?.split_once("](")?;
        Some(json!({"label": label, "url": url.trim_end_matches(')')}))
    }).collect();
    let o = options(card);
    // an option that starts with `close` closes the task after the answer (dispatch::answer_picks): consequential
    let choices: Vec<Value> = o.iter().map(|(k, t, star)| json!({"id": k, "text": t, "recommended": star, "consequential": t.to_lowercase().starts_with("close")})).collect();
    let parsed = field("Problem: ").is_some() && (2..=4).contains(&o.len()) && o.iter().filter(|x| x.2).count() == 1;
    json!({"parsed": parsed, "text": card, "done": field("Done: "), "problem": field("Problem: ").map(|_| brief(card)), "evidence": evidence,
        "choices": choices, "limits": {"lines": ANSWER_LINES, "chars": ANSWER_CHARS}})
}

impl Snapshot {
    pub fn totals(&self) -> BTreeMap<&'static str, usize> {
        totals(self.tasks.iter().map(|r| r.task.state.name()))
    }

    /// `status --json` (§ 5).
    pub fn json(&self) -> Value {
        let caps: &[&str] = if self.source == "host" { &HOST_CAPS } else { &BOARD_CAPS };
        json!({"schema": SCHEMA, "source": self.source, "observed": self.observed, "complete": self.reason.is_none(), "reason": self.reason,
            "board": self.board, "totals": self.reason.is_none().then(|| self.totals()), "tasks": self.tasks.iter().map(Row::json).collect::<Vec<_>>(),
            "hosts": self.hosts.iter().map(|h| json!({"name": h.name, "version": h.version, "updated": h.updated})).collect::<Vec<_>>(),
            "capabilities": caps})
    }

    /// The snapshot keys of `status.json` (§ 4) after a pass; `error`: the pass failed, so tasks are the last read's.
    pub fn status_keys(&self, error: Option<&str>) -> [(&'static str, Value); 7] {
        let reason = error.map(String::from).or(self.reason.clone());
        [("schema", json!(SCHEMA)), ("source", json!("host")), ("observed", json!(self.observed)), ("complete", json!(reason.is_none())),
            ("reason", json!(reason)), ("totals", json!(reason.is_none().then(|| self.totals()))),
            ("tasks", json!(self.tasks.iter().map(Row::json).collect::<Vec<_>>()))]
    }

    /// R6 text report.
    pub fn text(&self, cfg: &Config, raw: &Value) -> Res<String> {
        if let Some(r) = &self.reason {
            return Err(r.clone());
        }
        let base = self.board["url"].as_str().unwrap_or_default();
        let project = cfg.root.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let mut out = vec![format!("{project} · [board]({base})"), counts_line(&self.totals())];
        let link = |r: &Row| format!("[#{} {}]({})", r.n, cell(&compact(&r.title, 60)), r.url);
        let rows: Vec<String> = self.tasks.iter()
            .filter(|r| !matches!(r.task.state, State::Ask | State::Later | State::Failed))
            .map(|r| {
                let state = if r.task.state == State::Waiting {
                    let deps: Vec<String> = r.deps.iter().map(|b| format!("[{}]({}) {}", b.dep.name(), b.url, cell(&compact(&b.reason, 80)))).collect();
                    format!("blocked ({})", deps.join(", "))
                } else {
                    r.task.state.name().to_string()
                };
                let (rt, s) = session_link(cfg, raw, &r.task, r.n);
                format!("| {} | {state} | {} | {s} |", link(r), cell(&rt))
            })
            .collect();
        if !rows.is_empty() {
            out.push(String::new());
            out.push("| Task | State | Runtime | Session |\n|---|---|---|---|".into());
            out.extend(rows);
        }
        let qs: Vec<String> = self.tasks.iter()
            .filter(|r| r.task.state == State::Ask || (r.task.state == State::Review && r.task.card.is_some()))
            .map(|r| {
                let card = r.task.card.clone().unwrap_or_default();
                let (n, url) = (r.n, &r.url);
                let o: Vec<String> = options(&card).iter().map(|(k, t, star)| {
                    let label = cell(&compact(t, 120));
                    let label = if t.chars().count() > 120 { format!("[{label}]({url})") } else { label };
                    format!("{n}.{k} {label}{}", if *star { " ★" } else { "" })
                }).collect();
                // § 2: an `ask` card without options is an owner action
                let o = if o.is_empty() && r.task.state == State::Ask { cell(&format!("waiting for owner: {}", compact(&brief(&card), 120))) } else { o.join(" · ") };
                format!("| {} | [{}]({}) | {o} |", link(r), cell(&compact(&brief(&card), 160)), url)
            })
            .collect();
        if !qs.is_empty() {
            out.push(String::new());
            out.push("Questions (answer N.M):\n| Question | Brief reason | Options |\n|---|---|---|".into());
            out.extend(qs);
        }
        let later: Vec<String> = self.tasks.iter().filter(|r| r.task.state == State::Later).map(link).collect();
        if !later.is_empty() {
            out.push(String::new());
            out.push(format!("Later: {}", later.join(", ")));
        }
        let failed: Vec<String> = self.tasks.iter().filter(|r| r.task.state == State::Failed)
            .map(|r| format!("| {} | [{}]({}) |", link(r), cell(&compact(r.failed.as_deref().unwrap_or(""), 160)), r.url))
            .collect();
        if !failed.is_empty() {
            out.push(String::new());
            out.push("Failed:\n| Task | Reason |\n|---|---|".into());
            out.extend(failed);
        }
        let hs: Vec<String> = self.hosts.iter().map(|h| format!("{} v{} {}{}", h.name, h.version, age(self.observed - h.updated), h.focus.as_ref().map(|f| format!(" focus {f}")).unwrap_or_default())).collect();
        if !hs.is_empty() {
            out.push(String::new());
            out.push(format!("Hosts: {}", hs.join(", ")));
        }
        Ok(out.join("\n"))
    }
}

/// The `Problem:` text of a card, else its first line.
pub fn brief(card: &str) -> String {
    let mut lines = card.lines();
    while let Some(l) = lines.next() {
        if let Some(p) = l.strip_prefix("Problem: ") {
            return std::iter::once(p).chain(lines.take_while(|l| !l.starts_with("Evidence:") && !l.starts_with("To answer:") && options(l).is_empty())).collect::<Vec<_>>().join(" ");
        }
    }
    card.lines().next().unwrap_or_default().to_string()
}

fn session_link(cfg: &Config, raw: &Value, t: &Task, n: u64) -> (String, String) {
    let Some((rt, id)) = t.session.as_deref().and_then(|s| s.split(' ').next()).and_then(|s| s.split_once(':')) else { return (String::new(), String::new()) };
    let host = t.session.as_deref().and_then(|s| s.rsplit(' ').next()).unwrap_or_default();
    let mut id = id.to_string();
    if (id == "-" || rt == "claude") && host == cfg.me
        && let Some(a) = t.attempt
    {
        let l = runtime::Launch { task: n, title: String::new(), attempt: a, role: t.role.clone().unwrap_or_default(), rt: rt.into(), session: id.clone(), url: String::new(), resume: None };
        id = runtime::codex_thread(&runtime::log_path(&cfg.root, &l)).unwrap_or(id);
    }
    let host = cell(host);
    let link = match rt {
        _ if id == "-" => format!("{host}: starting"),
        "claude" => runtime::claude_remote(&id).filter(|_| host == cell(&cfg.me)).map_or_else(|| format!("{host}: Remote Control unknown"), |url| format!("[{host}]({url})")),
        _ if std::env::var("TASKQ_CLIENT").as_deref() == Ok("codex") => format!("[{host}](codex://threads/{id})"),
        _ => match raw["pages"].as_str() {
            Some(p) => format!("[{host}]({}/open.html#codex://threads/{id})", p.trim_end_matches('/')),
            None => format!("{host}: `codex://threads/{id}`"),
        },
    };
    (rt.to_string(), link)
}
