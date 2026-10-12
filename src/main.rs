//! The `taskq` command line (taskq.md § 5). Every write is one `taskq:` comment that `fold` accepts.

mod board;
mod core;
mod labels;
mod operator;
mod platform;
mod runtime;
mod service;
#[cfg(any(target_os = "macos", windows))]
mod tray;

use board::{Board, Http, Kind, Publish, entries};
use core::{Entry, State, Task, entry, fold, options};
use serde_json::{Value, json};
use service::{Config, Kid, Service};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

type Res<T> = Result<T, String>;

const USAGE: &str = "usage: taskq labels|add|ask|answer|result|requeue|steer|later|failed|close|status|inbox|wait|cleanup|update|service|tray|selftest (see taskq.md § 5)";

// ---------- arguments ----------

#[derive(Default)]
struct Args {
    pos: Vec<String>,
    flags: HashMap<String, Vec<String>>,
}

impl Args {
    fn parse(v: &[String]) -> Res<Args> {
        let mut a = Args::default();
        let mut it = v.iter();
        while let Some(x) = it.next() {
            match x.strip_prefix("--") {
                Some(k) => {
                    let val = if matches!(k, "later" | "check" | "force") { String::new() } else { it.next().cloned().ok_or(format!("--{k} needs a value"))? };
                    a.flags.entry(k.to_string()).or_default().push(val);
                }
                None => a.pos.push(x.clone()),
            }
        }
        Ok(a)
    }
    fn one(&self, k: &str) -> Option<&str> {
        self.flags.get(k).and_then(|v| v.last()).map(String::as_str)
    }
    fn need(&self, k: &str) -> Res<&str> {
        self.one(k).filter(|s| !s.trim().is_empty()).ok_or(format!("--{k} is required"))
    }
    fn all(&self, k: &str) -> Vec<String> {
        self.flags.get(k).cloned().unwrap_or_default()
    }
    fn num(&self, i: usize) -> Res<u64> {
        let s = self.pos.get(i).ok_or("task number missing")?;
        s.trim_start_matches('#').parse().map_err(|_| format!("not a task number: {s}"))
    }
}

// ---------- config ----------

struct Ctx {
    cfg: Config,
    raw: Value,
    board: Box<dyn Board>,
}

/// The project root: `TASKQ_ROOT`, else the nearest folder with `taskq.json` that is not a task worktree.
fn find_root() -> Res<PathBuf> {
    if let Some(r) = std::env::var_os("TASKQ_ROOT") {
        return Ok(r.into());
    }
    let mut d = std::env::current_dir().map_err(|e| e.to_string())?;
    loop {
        let in_worktree = d.parent().and_then(Path::file_name).is_some_and(|n| n == ".worktrees");
        if d.join("taskq.json").is_file() && !in_worktree {
            return Ok(d);
        }
        if !d.pop() {
            return Err("no taskq.json here or above".into());
        }
    }
}

fn hostname() -> String {
    platform::quiet(&mut std::process::Command::new("hostname")).output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default()
}

fn load(root: &Path) -> Res<(Config, Value)> {
    let p = root.join("taskq.json");
    let v: Value = serde_json::from_str(&fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?).map_err(|e| format!("{}: {e}", p.display()))?;
    let str_or = |k: &str, d: &str| v[k].as_str().unwrap_or(d).to_string();
    let secs = |k: &str, d: u64| v[k].as_u64().unwrap_or(d);
    let h = hostname();
    let me = std::env::var("TASKQ_HOST").ok().filter(|s| !s.is_empty()).or_else(|| v["hosts"][&h].as_str().map(String::from)).unwrap_or(h);
    let has = v["has"][&me].as_array().into_iter().flatten().filter_map(|x| x.as_str().map(String::from)).collect();
    let limits: BTreeMap<String, u32> = v["limits"].as_object().into_iter().flatten().map(|(k, n)| (k.clone(), n.as_u64().unwrap_or(0) as u32)).collect();
    let codex = match v["codex"].as_array() {
        Some(a) => a.iter().filter_map(|x| x.as_str().map(String::from)).collect(),
        None => vec!["-s".into(), "danger-full-access".into()],
    };
    let cfg = Config {
        root: root.to_path_buf(),
        install: platform::default_install(),
        board: if v["board"] == "gitlab" { Kind::GitLab } else { Kind::GitHub },
        web_host: str_or("host", "github.com"),
        repo: str_or("repo", ""),
        publish: str_or("publish", "pr"),
        me,
        limits,
        has,
        permission_mode: str_or("permission_mode", "dontAsk"),
        codex,
        ttl: secs("ttl", 300) as i64,
        poll: secs("poll", 60),
        update_every: secs("update_every", 900),
        rollback_after: secs("rollback_after", 600),
    };
    if cfg.repo.is_empty() {
        return Err("taskq.json: repo is required".into());
    }
    Ok((cfg, v))
}

/// Env `TASKQ_TOKEN`, else `GITHUB_TOKEN`/`GITLAB_TOKEN`, else `<install>/token`, else the `gh`/`glab` login for the board host (§ 9).
fn token(install: &Path, kind: Kind, host: &str) -> Res<String> {
    let named = if kind == Kind::GitLab { "GITLAB_TOKEN" } else { "GITHUB_TOKEN" };
    for k in ["TASKQ_TOKEN", named] {
        if let Ok(t) = std::env::var(k)
            && !t.trim().is_empty()
        {
            return Ok(t.trim().to_string());
        }
    }
    let cli = || {
        // only the board host's login: a default-host token never goes to another host
        let (bin, args): (&str, &[&str]) = if kind == Kind::GitLab { ("glab", &["config", "get", "token", "--host"]) } else { ("gh", &["auth", "token", "--hostname"]) };
        let o = platform::quiet(std::process::Command::new(bin).args(args).arg(host)).output().ok()?;
        o.status.success().then(|| String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    fs::read_to_string(install.join("token")).map(|t| t.trim().to_string()).ok().filter(|t| !t.is_empty())
        .or_else(cli).filter(|t| !t.is_empty())
        .ok_or(format!("no board token: set TASKQ_TOKEN or write {}", install.join("token").display()))
}

fn ctx() -> Res<Ctx> {
    let (cfg, raw) = load(&find_root()?)?;
    let tok = token(&cfg.install, cfg.board, &cfg.web_host)?;
    let mut http = Http::new(cfg.board, &cfg.web_host, &cfg.repo, &tok);
    http.set_label_schema(raw["label_schema"].as_str())?;
    let board = Box::new(http);
    Ok(Ctx { cfg, raw, board })
}

fn dot(root: &Path) -> PathBuf {
    root.join(".taskq")
}

fn poke(root: &Path) {
    let _ = fs::create_dir_all(dot(root));
    touch(&dot(root).join("poke"));
}

/// Bump `p`'s mtime. Not `fs::write(p, "")`: on Windows rewriting an empty file keeps its mtime.
fn touch(p: &Path) {
    let _ = fs::write(p, ".");
}

fn project_name(root: &Path) -> String {
    root.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "taskq".into())
}

/// Seconds since `p` was last written, by the local clock (local files only).
fn file_age(p: &Path) -> Option<u64> {
    fs::metadata(p).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).map(|d| d.as_secs())
}

// ---------- writes ----------

fn deps_open(b: &dyn Board, body: &str) -> Res<bool> {
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

fn current(b: &dyn Board, n: u64) -> Res<(board::Issue, Task, bool)> {
    let issue = b.issue(n)?;
    let deps = deps_open(b, &issue.body)?;
    let t = fold(n, &entries(b, n)?, deps);
    Ok((issue, t, deps))
}

/// R4: append one entry that `fold` accepts, set the projected label, poke the local service. A refused entry writes
/// nothing and reports the current state.
fn act(b: &dyn Board, root: &Path, n: u64, kind: &str, args: &[&str], text: &str) -> Res<Task> {
    let issue = b.issue(n)?;
    if !issue.open {
        return Err(format!("#{n} is closed"));
    }
    let deps = deps_open(b, &issue.body)?;
    let mut log = entries(b, n)?;
    let before = fold(n, &log, deps);
    let body = entry(kind, args, own_attempt(n), text);
    log.push(Entry { id: u64::MAX, body: body.clone(), edited: false });
    if fold(n, &log, deps) == before {
        return Err(format!("#{n} is {}: `{kind}` not accepted", before.state.name()));
    }
    b.comment(n, &body)?;
    let t = fold(n, &entries(b, n)?, deps); // re-read: report what the board holds (R12)
    service::project(b, &issue, &t)?;
    poke(root);
    Ok(t)
}

fn numbered(opts: &[String], recommend: Option<usize>) -> String {
    opts.iter().enumerate().map(|(i, o)| format!("{}. {o}{}", i + 1, if recommend == Some(i + 1) { " ★" } else { "" })).collect::<Vec<_>>().join("\n")
}

fn join(parts: &[&str]) -> String {
    parts.iter().filter(|p| !p.trim().is_empty()).copied().collect::<Vec<_>>().join("\n\n")
}

// ---------- message formats (R7, § 1, § 2 ask card, § 7 verdicts) ----------

/// R7: Unicode characters of a line; link targets and `code` spans (exact commands, SHAs, paths) are exempt.
fn width(l: &str) -> usize {
    let mut s: String = l.split('`').step_by(2).collect();
    while let Some(a) = s.find("](") {
        let b = s[a..].find(')').map_or(s.len(), |b| a + b + 1);
        s.replace_range(a + 1..b, "");
    }
    s.split(' ').filter(|w| !w.is_empty() && !w.starts_with("http://") && !w.starts_with("https://")).map(|w| w.chars().count() + 1).sum::<usize>().saturating_sub(1)
}

/// Rejects, with a one-line hint, a field outside `min..=max` lines or with a line over `chars` (R7: never cut).
fn limit(what: &str, lines: &[String], min: usize, max: usize, chars: usize) -> Res<()> {
    match lines.len() {
        n if n > 0 && max == 0 => Err(format!("{what}: not allowed here (only with --option)")),
        n if n < min || n > max => Err(format!("{what}: {n} lines, allowed {min}-{max}; shorten or split")),
        _ => match lines.iter().map(|l| width(l)).find(|w| *w > chars) {
            Some(w) => Err(format!("{what}: a line has {w} characters, limit {chars}; shorten it")),
            None => Ok(()),
        },
    }
}

fn one_line(what: &str, items: &[String]) -> Res<()> {
    if items.iter().any(|s| s.contains('\n') || s.trim().is_empty()) { Err(format!("{what}: one nonempty line each")) } else { Ok(()) }
}

/// Labelled `--text` lines (`Label: ..`; an unlabelled line continues the field above) in `spec` order:
/// (label, min lines, max lines, characters).
fn fields(text: &str, spec: &[(&str, usize, usize, usize)]) -> Res<Vec<Vec<String>>> {
    let names = spec.iter().map(|s| format!("`{}:`", s.0)).collect::<Vec<_>>().join(", ");
    let mut out = vec![vec![]; spec.len()];
    let mut cur: Option<usize> = None;
    for l in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        match spec.iter().position(|s| l.strip_prefix(s.0).is_some_and(|r| r.starts_with(": "))) {
            Some(i) if cur.is_none_or(|c| i > c) => cur = Some(i),
            Some(_) => return Err(format!("--text fields go once each in the order {names}")),
            None if cur.is_none() => return Err(format!("--text starts with one of {names}")),
            None => {}
        }
        out[cur.unwrap()].push(l.to_string());
    }
    for (f, s) in out.iter().zip(spec) {
        limit(s.0, f, s.1, s.2, s.3)?;
    }
    Ok(out)
}

/// `--link`: `[label](url)` as given, else the URL labelled `#N` by its trailing number, else `link`.
fn evidence(l: &str) -> String {
    if l.starts_with('[') && l.contains("](") {
        return l.to_string();
    }
    let tail = l.trim_end_matches('/').rsplit('/').next().unwrap_or_default();
    format!("[{}]({l})", if !tail.is_empty() && tail.bytes().all(|b| b.is_ascii_digit()) { format!("#{tail}") } else { "link".into() })
}

/// § 2 ask card: Done, Problem, optional Evidence, 2-4 options with one ★, the answer line.
fn ask_card(n: u64, text: &str, opts: &[String], recommend: Option<&str>, links: &[String]) -> Res<String> {
    let mut lines = fields(text, &[("Done", 1, 1, 160), ("Problem", 1, 2, 240)])?.concat();
    one_line("--option", opts)?;
    if !(2..=4).contains(&opts.len()) || opts.iter().any(|o| o.contains('★')) {
        return Err(format!("{} options, allowed 2-4 without ★; --recommend adds it", opts.len()));
    }
    let k = recommend.and_then(|k| k.parse().ok()).filter(|k| (1..=opts.len()).contains(k)).ok_or(format!("--recommend needs one option number 1-{}", opts.len()))?;
    if links.len() > 2 {
        return Err("Evidence: at most two --link".into());
    }
    let ev: Vec<String> = links.iter().map(|l| evidence(l)).collect();
    if ev.iter().any(|e| e.split_once("](").is_some_and(|(l, _)| l.chars().count() > 41)) {
        return Err("Evidence: link labels at most 40 characters".into());
    }
    if !ev.is_empty() {
        lines.push(format!("Evidence: {}", ev.join(", ")));
    }
    let opts: Vec<String> = numbered(opts, Some(k)).lines().map(String::from).collect();
    limit("option", &opts, 2, 4, 160)?;
    lines.extend(opts);
    lines.push(format!("To answer: `taskq answer {n}.K` or comment `K` or the option text."));
    Ok(lines.join("\n"))
}

/// § 7 result: Changed, one `Checks:` line per check, Open; a decision appends the § 2 card (Done/Problem in `--text`).
fn result_text(n: u64, text: &str, checks: &[String], opts: &[String], recommend: Option<&str>, links: &[String]) -> Res<String> {
    let d = !opts.is_empty() as usize;
    if d == 0 && (recommend.is_some() || !links.is_empty()) {
        return Err("--recommend and --link need --option".into());
    }
    let f = fields(text, &[("Changed", 1, 2, 240), ("Open", 1, 2, 240), ("Done", d, d, 160), ("Problem", d, 2 * d, 240)])?;
    let checks: Vec<String> = checks.iter().flat_map(|c| c.lines()).map(str::trim).filter(|c| !c.is_empty()).map(|c| format!("Checks: {}", c.trim_start_matches("Checks: "))).collect();
    limit("Checks", &checks, 1, 8, 240)?;
    let verdict = [f[0].clone(), checks, f[1].clone()].concat();
    limit("result", &verdict, 3, 12, 240)?;
    let card = if d == 1 { ask_card(n, &[f[2].clone(), f[3].clone()].concat().join("\n"), opts, recommend, links)? } else { String::new() };
    Ok(join(&[&verdict.join("\n"), &card]))
}

/// § 7 close verdict: Changed, Checks naming every Acceptance item `A<k> ok|gap|unknown` and CI, Open.
fn close_verdict(text: &str, body: &str) -> Res<()> {
    let f = fields(text, &[("Changed", 1, 2, 240), ("Checks", 1, 8, 240), ("Open", 1, 2, 240)])?;
    limit("verdict", &f.concat(), 3, 12, 240)?;
    let checks = f[1].join(" ");
    for k in body.lines().filter_map(|l| l.trim().strip_prefix('A')?.split_once('.')?.0.parse::<u32>().ok()) {
        if !["ok", "gap", "unknown"].iter().any(|v| checks.contains(&format!("A{k} {v}"))) {
            return Err(format!("Checks: name A{k} ok|gap|unknown"));
        }
    }
    if checks.contains("CI") { Ok(()) } else { Err("Checks: name the CI outcome on the result SHA".into()) }
}

/// Selftest of the rendered forms: a result, a close verdict, and an ask card answered by the hand-typed form (§ 2).
fn formats_selftest() -> Res<String> {
    let checks = ["cargo test --locked -- pass".to_string()];
    result_text(1, "Changed: x\nOpen: none", &checks, &[], None, &[])?;
    close_verdict("Changed: x\nChecks: A1 ok; CI green on the SHA\nOpen: none", "A1. y")?;
    let card = ask_card(1, "Done: not started\nProblem: pick one", &["left".into(), "right".into()], Some("2"), &[])?;
    let log: Vec<Entry> = [entry("spawn", &["worker", "claude:s", "mac"], None, ""), entry("ask", &[], Some(1), &card), "taskq answer 2".into()]
        .into_iter().enumerate().map(|(i, body)| Entry { id: i as u64 + 1, body, edited: false }).collect();
    match (fold(1, &log, false).state, core::delivery(1, &log, 3)) {
        (State::Doing, d) if d == "right" => Ok("formats: ask card, result, close verdict PASS".into()),
        (s, d) => Err(format!("formats: FAIL hand-typed answer gave {} and {d:?}", s.name())),
    }
}

/// § 7: publish a reviewed result, then `close`; later and failed tasks close without publication.
fn close_one(b: &dyn Board, cfg: &Config, n: u64, verdict: &str) -> Res<String> {
    let (issue, t, _) = current(b, n)?;
    match t.state {
        State::Later | State::Failed => {
            act(b, &cfg.root, n, "close", &[], verdict)?;
            Ok(format!("closed #{n}"))
        }
        State::Review => {
            let sha = t.result.ok_or(format!("#{n} has no result"))?;
            match b.publish(n, &sha, &cfg.publish, &issue.title, &format!("Task: {}", b.url(n))) {
                Publish::Done(m) => {
                    act(b, &cfg.root, n, "close", &[], &join(&[verdict, &m]))?;
                    Ok(format!("closed #{n}: {m}"))
                }
                Publish::Pending(m) => Err(format!("#{n} not closed: {m}")),
                Publish::Failed(m) => {
                    act(b, &cfg.root, n, "requeue", &[], &format!("publication failed: {m}"))?;
                    Err(format!("#{n} requeued: {m}"))
                }
            }
        }
        s => Err(format!("#{n} is {}: `close` not accepted", s.name())),
    }
}

/// `answer N.K [M.K..]`: every pick is checked before any is written (§ 5).
fn answer_picks(b: &dyn Board, cfg: &Config, picks: &[String]) -> Res<Vec<String>> {
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
        let r = match (state, close) {
            (State::Ask, _) => act(b, &cfg.root, n, "answer", &[&k.to_string()], &opt).and_then(|t| {
                if close && t.state == State::Review { close_one(b, cfg, n, &opt) } else { Ok(format!("#{n} {}", t.state.name())) }
            }),
            (_, true) => close_one(b, cfg, n, &opt),
            (_, false) => act(b, &cfg.root, n, "requeue", &[], &opt).map(|t| format!("#{n} {}", t.state.name())),
        };
        out.push(r.unwrap_or_else(|e| format!("error: {e}")));
    }
    Ok(out)
}

// ---------- report ----------

fn cell(s: &str) -> String {
    s.replace('\\', "\\\\").replace('|', "\\|").replace('[', "\\[").replace(']', "\\]").replace(['\r', '\n'], " ")
}

fn age(secs: i64) -> String {
    match secs {
        s if s < 120 => format!("{s}s"),
        s if s < 7200 => format!("{}m", s / 60),
        s if s < 172800 => format!("{}h", s / 3600),
        s => format!("{}d", s / 86400),
    }
}

fn session_link(cfg: &Config, raw: &Value, t: &Task, n: u64) -> (String, String) {
    let Some((rt, id)) = t.session.as_deref().and_then(|s| s.split(' ').next()).and_then(|s| s.split_once(':')) else { return (String::new(), String::new()) };
    let host = t.session.as_deref().and_then(|s| s.rsplit(' ').next()).unwrap_or_default();
    let mut id = id.to_string();
    if id == "-" && host == cfg.me
        && let Some(a) = t.attempt
    {
        let l = runtime::Launch { task: n, title: String::new(), attempt: a, role: t.role.clone().unwrap_or_default(), rt: rt.into(), session: id.clone(), url: String::new(), resume: None };
        id = runtime::codex_thread(&runtime::log_path(&cfg.root, &l)).unwrap_or(id);
    }
    let link = match rt {
        _ if id == "-" => format!("{host}: starting"),
        "claude" => format!("[{host}](https://claude.ai/code/session_{id})"),
        _ if std::env::var("TASKQ_CLIENT").as_deref() == Ok("codex") => format!("[{host}](codex://threads/{id})"),
        _ => match raw["pages"].as_str() {
            Some(p) => format!("[{host}]({}/open.html#codex://threads/{id})", p.trim_end_matches('/')),
            None => format!("{host}: `codex://threads/{id}`"),
        },
    };
    (rt.to_string(), link)
}

/// A PM's scope (§ 5 `--only`): no labels means every task.
fn in_scope(i: &board::Issue, only: &[String]) -> bool {
    only.is_empty() || only.iter().any(|f| labels::matches(&i.labels, f))
}

fn report(b: &dyn Board, cfg: &Config, raw: &Value, only: &[String]) -> Res<String> {
    let (mut views, hosts) = service::views(b, &mut HashMap::new())?;
    views.retain(|v| in_scope(&v.issue, only));
    if let Some(who) = raw["assignee"].as_str() {
        let who = if who == "me" { b.me()? } else { who.to_string() };
        views.retain(|v| v.issue.assignees.contains(&who));
    }
    let now = b.now()?;
    views.sort_by_key(|v| (service::priority(&v.issue), v.issue.number));
    let project = cfg.root.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let base = b.url(0);
    let mut out = vec![format!("{project} · [board]({})", base.trim_end_matches("/0"))];
    let count = |f: &dyn Fn(State) -> bool| views.iter().filter(|v| f(v.task.state)).count();
    out.push(format!(
        "In work {} · Waiting for answer {} · Ready {}",
        count(&|s| matches!(s, State::Doing | State::Review)),
        count(&|s| s == State::Ask),
        count(&|s| s == State::Ready)
    ));
    let link = |v: &service::View| format!("[#{} {}]({})", v.issue.number, cell(&v.issue.title.chars().take(60).collect::<String>()), b.url(v.issue.number));
    let open: Vec<u64> = views.iter().map(|v| v.issue.number).collect();
    let rows: Vec<String> = views
        .iter()
        .filter(|v| !matches!(v.task.state, State::Ask | State::Later | State::Failed))
        .map(|v| {
            let state = if v.task.state == State::Waiting {
                let deps: Vec<String> = core::depends(&v.issue.body).into_iter().filter(|d| open.contains(d)).map(|d| format!("#{d}")).collect();
                format!("blocked ({} open)", deps.join(", "))
            } else {
                v.task.state.name().to_string()
            };
            let (rt, s) = session_link(cfg, raw, &v.task, v.issue.number);
            format!("| {} | {state} | {rt} | {s} |", link(v))
        })
        .collect();
    if !rows.is_empty() {
        out.push(String::new());
        out.push("| Task | State | Runtime | Session |\n|---|---|---|---|".into());
        out.extend(rows);
    }
    let qs: Vec<String> = views
        .iter()
        .filter(|v| v.task.state == State::Ask || (v.task.state == State::Review && v.task.card.is_some()))
        .map(|v| {
            let card = v.task.card.clone().unwrap_or_default();
            let opts = options(&card);
            let (q, why): (Vec<&str>, Vec<&str>) = card.lines().filter(|l| !opts.iter().any(|o| l.trim().starts_with(&format!("{}. ", o.0)))).partition(|_| true);
            let q = q.first().copied().unwrap_or("");
            let why = why.get(1..).map(|w| w.join(" ")).unwrap_or_default();
            let n = v.issue.number;
            let o: Vec<String> = opts.iter().map(|(k, t, star)| format!("{n}.{k} {}{}", cell(t), if *star { " ★" } else { "" })).collect();
            format!("| {} {} | {} | {} |", link(v), cell(q), cell(&why), o.join(" · "))
        })
        .collect();
    if !qs.is_empty() {
        out.push(String::new());
        out.push("Questions (answer N.M):\n| Question | Brief reason | Options |\n|---|---|---|".into());
        out.extend(qs);
    }
    let later: Vec<String> = views.iter().filter(|v| v.task.state == State::Later).map(link).collect();
    if !later.is_empty() {
        out.push(String::new());
        out.push(format!("Later: {}", later.join(", ")));
    }
    let failed: Vec<String> = views
        .iter()
        .filter(|v| v.task.state == State::Failed)
        .map(|v| {
            let why = v.comments.iter().rev().find(|c| c.trusted && c.body.starts_with("taskq: failed")).and_then(|c| c.body.split_once('\n')).map(|x| x.1).unwrap_or("");
            format!("| {} | {} |", link(v), cell(why))
        })
        .collect();
    if !failed.is_empty() {
        out.push(String::new());
        out.push("Failed:\n| Task | Reason |\n|---|---|".into());
        out.extend(failed);
    }
    let hs: Vec<String> = hosts
        .iter()
        .map(|h| {
            let v = h.body.lines().find_map(|l| l.strip_prefix("version ")).unwrap_or("?");
            format!("{} v{v} {}", h.title.trim_start_matches("taskq host "), age(now - h.updated))
        })
        .collect();
    if !hs.is_empty() {
        out.push(String::new());
        out.push(format!("Hosts: {}", hs.join(", ")));
    }
    Ok(out.join("\n"))
}

fn inbox(b: &dyn Board, n: u64) -> Res<String> {
    let (issue, t, _) = current(b, n)?;
    let mut out = vec![format!("#{n} {}\n{}", issue.title, b.url(n)), issue.body.trim().to_string(), format!("Folded: {}", serde_json::to_string(&t).unwrap_or_default()), "Log:".into()];
    for e in entries(b, n)? {
        out.push(format!("[{}{}] {}", e.id, if e.edited { " edited, ignored" } else { "" }, e.body.trim()));
    }
    Ok(out.join("\n\n"))
}

// ---------- wait ----------

/// PM wait (§ 5): the cursor `<root>/.taskq/pm-cursor` holds the board time of the last read and the last seen state
/// per task. Only moves into `ask`, `review` with options, closed or `failed` are printed.
fn wait(b: &dyn Board, cfg: &Config, window_min: u64, only: &[String]) -> Res<Vec<String>> {
    let path = dot(&cfg.root).join("pm-cursor");
    let mut since = None;
    let mut seen: HashMap<u64, String> = HashMap::new();
    if let Ok(text) = fs::read_to_string(&path) {
        let mut lines = text.lines();
        since = lines.next().and_then(|l| l.parse::<i64>().ok());
        seen = lines.filter_map(|l| l.split_once(' ').and_then(|(n, s)| Some((n.parse().ok()?, s.to_string())))).collect();
    }
    let deadline = Instant::now() + Duration::from_secs(window_min * 60);
    loop {
        let now = b.now()?;
        let issues = b.issues(since.map(|s| s - 1).or(Some(now)))?;
        let open: Vec<u64> = b.issues(None)?.iter().map(|i| i.number).collect();
        let mut moved = vec![];
        for i in issues.iter().filter(|i| service::is_task(i) && in_scope(i, only)) {
            let t = fold(i.number, &entries(b, i.number)?, core::depends(&i.body).iter().any(|d| open.contains(d)));
            let state = if !i.open { "closed".to_string() } else if t.state == State::Review && t.card.is_some() { "review*".into() } else { t.state.name().into() };
            if seen.get(&i.number) != Some(&state) && matches!(state.as_str(), "ask" | "review*" | "closed" | "failed") {
                moved.push(format!("{} #{}", state.trim_end_matches('*'), i.number));
            }
            seen.insert(i.number, state);
        }
        since = Some(now);
        let mut text = format!("{now}\n");
        let mut keep: Vec<_> = seen.iter().filter(|(_, s)| *s != "closed").collect();
        keep.sort();
        for (n, s) in keep {
            text += &format!("{n} {s}\n");
        }
        fs::create_dir_all(dot(&cfg.root)).map_err(|e| e.to_string())?;
        platform::write_atomic(&path, &text)?;
        if !moved.is_empty() {
            return Ok(moved);
        }
        if Instant::now() >= deadline {
            return Ok(vec!["tick".into()]);
        }
        std::thread::sleep(Duration::from_secs(cfg.poll.clamp(5, 60)).min(deadline.saturating_duration_since(Instant::now()) + Duration::from_secs(1)));
    }
}

// ---------- service ----------

fn installed(install: &Path) -> bool {
    std::env::current_exe().ok().and_then(|e| e.canonicalize().ok()).zip(install.join("releases").canonicalize().ok()).is_some_and(|(e, r)| e.starts_with(r))
}

fn launcher(install: &Path) -> PathBuf {
    let l = install.join(platform::exe_name());
    if l.is_file() { l } else { std::env::current_exe().unwrap_or_else(|_| PathBuf::from("taskq")) }
}

/// Sleep until a poke or `secs`; `tick` runs every second (the load gate's sampling, § 3).
fn sleep_until_poke(poke: &Path, secs: u64, tick: &mut dyn FnMut()) {
    let mtime = |p: &Path| fs::metadata(p).and_then(|m| m.modified()).ok();
    let start = mtime(poke);
    for _ in 0..secs {
        std::thread::sleep(Duration::from_secs(1));
        tick();
        if mtime(poke) != start {
            return;
        }
    }
}

/// The kids file carries live children across the update re-exec (unix). It names the PID that wrote it: a file from
/// a crashed process is ignored and its attempts go `gone` (§ 4 restart).
fn adopt(s: &mut Service, root: &Path) {
    let path = dot(root).join("kids");
    let Ok(text) = fs::read_to_string(&path) else { return };
    let _ = fs::remove_file(&path);
    let mut lines = text.lines();
    if lines.next() != Some(&format!("pid {}", std::process::id())) {
        return;
    }
    #[cfg(unix)]
    for l in lines {
        let w: Vec<&str> = l.split_whitespace().collect();
        if let [task, attempt, rt, pid, delivered] = w[..]
            && let (Ok(task), Ok(attempt), Ok(pid), Ok(delivered)) = (task.parse(), attempt.parse(), pid.parse(), delivered.parse())
        {
            s.adopt(Kid { task, attempt, rt: rt.into(), proc: Box::new(runtime::Adopted(pid)) }, delivered);
        }
    }
}

fn service_run(c: &Ctx) -> Res<()> {
    let cfg = &c.cfg;
    let exe = launcher(&cfg.install);
    let env = runtime::Env { root: &cfg.root, exe: &exe, permission_mode: &cfg.permission_mode, codex: &cfg.codex, publish: &cfg.publish };
    let start = |l: &runtime::Launch| runtime::spawn(&env, l);
    let mut s = Service::new(c.board.as_ref(), cfg, platform::VERSION, Box::new(start));
    let d = dot(&cfg.root);
    fs::create_dir_all(&d).map_err(|e| e.to_string())?;
    adopt(&mut s, &cfg.root);
    let is_installed = installed(&cfg.install);
    let began = Instant::now();
    let mut checked: Option<Instant> = None;
    let mut update: Option<String> = None;
    let _ = fs::remove_file(d.join("restart"));
    operator::recover(&cfg.root);
    // ponytail: the operator takes no slot of `limits`; count it when operators crowd out workers
    let op_rt = cfg.limits.iter().find(|(_, n)| **n > 0).map(|(rt, _)| rt.clone());
    let mut tray = None;
    let project = project_name(&cfg.root);
    let issues = c.board.url(0).strip_suffix("/0").unwrap_or_default().to_string();
    let log = platform::log_path(&cfg.install, &project);
    eprintln!("taskq {} service on {} for {}", platform::VERSION, cfg.me, cfg.repo);
    let mut sampled: Option<Instant> = None;
    loop {
        // § 3: capacity reloads before each admission decision; children are untouched
        let cap = fs::read_to_string(cfg.root.join("taskq.json")).map_err(|e| e.to_string())
            .and_then(|t| serde_json::from_str::<Value>(&t).map_err(|e| format!("taskq.json: {e}")))
            .and_then(|v| service::capacity(&v, &cfg.me));
        if let Err(e) = &cap {
            eprintln!("capacity: {e}; launches held");
        }
        s.reload(cap);
        s.mono = began.elapsed().as_secs();
        s.focus = fs::read_to_string(d.join("focus")).unwrap_or_default().split_whitespace().filter(|f| *f != "off").map(String::from).collect();
        let r = s.pass();
        for l in s.log.drain(..) {
            eprintln!("{l}");
        }
        if let Err(e) = &r {
            eprintln!("pass: {e}");
        }
        let at = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let kids: Vec<Value> = s.kids.iter().map(|k| json!({"task": k.task, "attempt": k.attempt, "rt": k.rt, "pid": k.proc.id()})).collect();
        let tasks: Vec<Value> = s.tasks.iter().map(|(n, title, state, labels)| json!({"n": n, "title": title, "state": state, "labels": labels})).collect();
        let request = fs::read_to_string(d.join("focus.request")).ok();
        let mut status = json!({"host": cfg.me, "version": platform::VERSION, "focus": s.focus, "error": r.err(), "kids": kids, "at": at,
            "pid": std::process::id(), "project": project, "issues": issues, "log": log, "tasks": tasks,
            "request": request, "update": update, "events_since": s.events_since, "capacity": s.capacity_json()});
        let _ = platform::write_atomic(&d.join("status.json"), &serde_json::to_string_pretty(&status).unwrap_or_default());
        if is_installed && began.elapsed().as_secs() >= cfg.rollback_after {
            let h = cfg.install.join("releases").join(platform::VERSION).join("healthy");
            if !h.exists() {
                let _ = fs::write(h, "");
            }
        }
        // § 4 step 7: a not installed service only reports a newer version; `<root>/.taskq/update` checks at once
        let asked = fs::remove_file(d.join("update")).is_ok();
        if asked || checked.is_none_or(|t| t.elapsed().as_secs() >= cfg.update_every) {
            checked = Some(Instant::now());
            match platform::update(&cfg.install, true) {
                Ok(found) => {
                    update = found;
                    if asked {
                        eprintln!("update requested: {}", update.as_deref().map_or("up to date".into(), |v| format!("{v} found")));
                    }
                }
                Err(e) => eprintln!("update: {e}"),
            }
            if is_installed && update.is_some() {
                match platform::update(&cfg.install, false) {
                    Ok(Some(v)) => {
                        eprintln!("updated to {v}; restarting");
                        update = None;
                        reexec(&s, cfg)?;
                    }
                    Ok(None) => update = None,
                    Err(e) => eprintln!("update: {e}"),
                }
            }
        }
        if is_installed && d.join("restart").exists() {
            // Windows re-exec waits for the live children; the request stays until it can go
            if cfg!(unix) || s.kids.is_empty() {
                let _ = fs::remove_file(d.join("restart"));
                eprintln!("restart requested");
            }
            reexec(&s, cfg)?;
        }
        operator::step(&cfg.root, &cfg.install, &exe, op_rt.as_deref());
        if is_installed {
            keep_tray(&cfg.install, &mut tray);
        }
        // every 30 s, also between board polls: a gate sample (when enabled) and a fresh capacity in status.json (§ 4)
        let mut tick = || {
            if sampled.is_some_and(|t| t.elapsed().as_secs() < 30) {
                return;
            }
            sampled = Some(Instant::now());
            s.mono = began.elapsed().as_secs();
            let wall = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
            if s.gate.enabled() {
                s.gate.observe(s.mono, wall as i64, platform::cpu_ticks(), platform::memory_available());
            }
            status["capacity"] = s.capacity_json();
            status["at"] = json!(wall);
            let _ = platform::write_atomic(&d.join("status.json"), &serde_json::to_string_pretty(&status).unwrap_or_default());
        };
        tick();
        sleep_until_poke(&d.join("poke"), cfg.poll, &mut tick);
    }
}

// ---------- capacity (§ 3, § 5 `service capacity`, § 9) ----------

/// One owner edit of a host's capacity: runtime limits and load gate fields; `None` keeps a field.
#[derive(Default, Clone)]
struct CapEdit {
    limits: BTreeMap<String, u32>,
    gate: Option<bool>,
    cpu_max: Option<u32>,
    memory_min: Option<u32>,
}

impl CapEdit {
    fn from_args(a: &Args) -> Res<CapEdit> {
        let num = |k: &str| a.one(k).map(|x| x.parse::<u32>().map_err(|_| format!("--{k}: not an integer 0..{}", u32::MAX))).transpose();
        let mut e = CapEdit { cpu_max: num("cpu-max")?, memory_min: num("memory-min")?, ..Default::default() };
        for rt in RUNTIMES {
            if let Some(n) = num(rt)? {
                e.limits.insert(rt.into(), n);
            }
        }
        e.gate = match a.one("load-gate") {
            None => None,
            Some("on") => Some(true),
            Some("off") => Some(false),
            Some(x) => return Err(format!("--load-gate: on or off, not {x}")),
        };
        Ok(e)
    }

    fn is_empty(&self) -> bool {
        self.limits.is_empty() && self.gate.is_none() && self.cpu_max.is_none() && self.memory_min.is_none()
    }
}

const RUNTIMES: [&str; 3] = ["claude", "codex", "codex-app"];

/// `taskq.json` text with `e` applied to `host` (§ 9): only that host's `capacity_hosts` entry changes, other keys
/// and their order stay. `expect`: the capacity the owner saw; a different current one is a concurrent edit.
fn edit_capacity(text: &str, host: &str, me: &str, e: &CapEdit, expect: Option<&Value>) -> Res<String> {
    let mut v: Value = serde_json::from_str(text).map_err(|e| format!("taskq.json: {e}"))?;
    let known = host == me || v["hosts"].as_object().is_some_and(|h| h.values().any(|n| n == host));
    if !known {
        return Err(format!("unknown host {host}: use this host ({me}) or a name in hosts"));
    }
    let old = service::capacity(&v, host)?;
    if expect.is_some_and(|x| *x != old.json()) {
        return Err("capacity changed since it was shown; nothing written".into());
    }
    if e.is_empty() {
        return Err("nothing to change: give --claude, --codex, --codex-app, --load-gate, --cpu-max or --memory-min".into());
    }
    if let Some(rt) = e.limits.keys().find(|k| !RUNTIMES.contains(&k.as_str())) {
        return Err(format!("unknown runtime {rt}"));
    }
    if !v["capacity_hosts"].is_object() {
        v["capacity_hosts"] = json!({});
    }
    let h = &mut v["capacity_hosts"][host];
    for (rt, n) in &e.limits {
        h["limits"][rt] = json!(n);
    }
    if e.gate.is_some() || e.cpu_max.is_some() || e.memory_min.is_some() {
        let g = old.gate;
        h["load_gate"] = json!({"enabled": e.gate.unwrap_or(g.enabled), "cpu_max": e.cpu_max.unwrap_or(g.cpu_max), "memory_min": e.memory_min.unwrap_or(g.memory_min)});
    }
    service::capacity(&v, host)?;
    // keep the file's indent
    let indent = text.lines().nth(1).map(|l| l.len() - l.trim_start().len()).filter(|n| *n > 0).unwrap_or(2);
    let (mut out, pad) = (vec![], " ".repeat(indent));
    let mut ser = serde_json::Serializer::with_formatter(&mut out, serde_json::ser::PrettyFormatter::with_indent(pad.as_bytes()));
    serde::Serialize::serialize(&v, &mut ser).map_err(|e| e.to_string())?;
    Ok(String::from_utf8_lossy(&out).into_owned() + if text.ends_with('\n') { "\n" } else { "" })
}

/// Validate, refuse a concurrent edit, replace `taskq.json` atomically, read back: the saved capacity of `host`.
fn write_capacity(root: &Path, host: &str, e: &CapEdit, expect: Option<&Value>) -> Res<(service::Capacity, service::Capacity)> {
    let p = root.join("taskq.json");
    let read = || fs::read_to_string(&p).map_err(|e| format!("taskq.json: {e}"));
    let text = read()?;
    let (cfg, v) = load(root)?;
    let old = service::capacity(&v, host)?;
    let out = edit_capacity(&text, host, &cfg.me, e, expect)?;
    if read()? != text {
        return Err("taskq.json changed while saving; nothing written".into());
    }
    platform::write_atomic(&p, &out)?;
    let saved = service::capacity(&serde_json::from_str(&read()?).map_err(|e| e.to_string())?, host)?;
    if host == cfg.me {
        poke(root);
    }
    Ok((old, saved))
}

fn cap_changes(old: &service::Capacity, new: &service::Capacity) -> String {
    let mut out = vec![];
    for rt in old.limits.keys().chain(new.limits.keys()).collect::<std::collections::BTreeSet<_>>() {
        let (a, b) = (old.limits.get(rt), new.limits.get(rt));
        if a != b {
            let n = |x: Option<&u32>| x.map_or("none".into(), |x| x.to_string());
            out.push(format!("{} {} → {}", runtime_name(rt), n(a), n(b)));
        }
    }
    if old.gate != new.gate {
        let g = |g: service::GateCfg| if g.enabled { format!("on (CPU {} %, memory {} %)", g.cpu_max, g.memory_min) } else { "off".into() };
        out.push(format!("Load gate {} → {}", g(old.gate), g(new.gate)));
    }
    if out.is_empty() { "no change".into() } else { out.join(", ") }
}

fn runtime_name(rt: &str) -> String {
    let mut c = rt.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

/// The gate line of a `status.json` capacity (§ 4 tray text).
fn gate_text(c: &Value) -> String {
    let reasons = |codes: &[&str]| -> Vec<&Value> { c["reasons"].as_array().into_iter().flatten().filter(|r| codes.iter().any(|x| r["code"] == *x)).collect() };
    match c["gate"].as_str() {
        Some("off") => "Off".into(),
        Some("open") => "Ready".into(),
        Some("held") => format!("Waiting: {}", reasons(&["cpu", "memory", "cooldown"]).into_iter().map(|r| match r["code"].as_str() { Some("cpu") => "CPU", Some("memory") => "memory", _ => "recovery" }).collect::<Vec<_>>().join(", ")),
        Some(_) => format!("Unknown: {}", reasons(&["sample", "config"]).into_iter().filter_map(|r| r["text"].as_str()).collect::<Vec<_>>().join("; ")),
        None => "Unknown: service has no capacity status".into(),
    }
}

/// `taskq service capacity` (§ 5).
fn capacity_cmd(root: &Path, a: &Args) -> Res<()> {
    let (cfg, v) = load(root)?;
    let host = a.one("host").unwrap_or(&cfg.me).to_string();
    let project = project_name(root);
    let status = || -> Option<Value> {
        let st: Value = fs::read_to_string(dot(root).join("status.json")).ok().and_then(|t| serde_json::from_str(&t).ok())?;
        (st["host"] == cfg.me.as_str() && file_age(&dot(root).join("status.json")).is_some_and(|x| x <= 60)).then_some(st)
    };
    let e = CapEdit::from_args(a)?;
    if e.is_empty() {
        let saved = service::capacity(&v, &host);
        println!("Capacity: {project} on {host}");
        let c = status().map(|st| st["capacity"].clone()).filter(|c| !c.is_null() && host == cfg.me);
        let limits = c.as_ref().map(|c| c["limits"].clone()).or_else(|| saved.as_ref().ok().map(|s| s.json()["limits"].clone())).unwrap_or_default();
        let mut rts: Vec<&String> = limits.as_object().into_iter().flatten().map(|(k, _)| k).collect();
        if let Some(c) = &c {
            rts.extend(c["used"].as_object().into_iter().flatten().map(|(k, _)| k).filter(|k| !limits.get(k.as_str()).is_some()));
        }
        for rt in rts {
            let used = c.as_ref().and_then(|c| c["used"][rt].as_u64()).map_or("?".into(), |n| n.to_string());
            println!("{} {used}/{}", runtime_name(rt), limits[rt].as_u64().unwrap_or(0));
        }
        match (&c, &saved) {
            (Some(c), _) => println!("Load gate: {}", gate_text(c)),
            (None, Err(e)) => println!("Unknown: {e}"),
            (None, Ok(_)) if host != cfg.me => println!("Unknown: no local snapshot of {host}"),
            (None, Ok(_)) => println!("Unknown: service has no fresh capacity status"),
        }
        if let (Some(c), Ok(s)) = (&c, &saved)
            && (c["limits"] != s.json()["limits"] || c["load_gate"] != s.json()["load_gate"])
        {
            println!("Saved config differs: {}", cap_changes(&service::Capacity { limits: serde_json::from_value(c["limits"].clone()).unwrap_or_default(), gate: s.gate }, s));
        }
        return Ok(());
    }
    // a write names its host explicitly (§ 5)
    let host = a.need("host")?.to_string();
    let wrote = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let (old, saved) = write_capacity(root, &host, &e, None)?;
    let changes = cap_changes(&old, &saved);
    if host != cfg.me {
        println!("Saved for {host} in {project}: {changes}; applies when {host}'s service reads this config");
        return Ok(());
    }
    // `Applied` only after a fresh service readback of the same values (R12)
    for _ in 0..20 {
        if let Some(st) = status()
            && st["at"].as_u64().is_some_and(|t| t >= wrote)
            && st["capacity"]["limits"] == saved.json()["limits"]
            && st["capacity"]["load_gate"] == saved.json()["load_gate"]
        {
            println!("Applied in {project} on {host}: {changes}; running tasks continue");
            return Ok(());
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    println!("Saved; waiting for service in {project} on {host}: {changes}; running tasks continue");
    Ok(())
}

/// Start the host's one `taskq tray` of the current release when its heartbeat `<install>/tray` is stale (§ 4); reap
/// the one this image started.
fn keep_tray(install: &Path, tray: &mut Option<std::process::Child>) {
    if !cfg!(any(target_os = "macos", windows)) {
        return;
    }
    if tray.as_mut().is_some_and(|c| !matches!(c.try_wait(), Ok(None))) {
        *tray = None;
    }
    if tray.is_some() || file_age(&install.join("tray")).is_some_and(|a| a < 30) {
        return;
    }
    let Some(v) = platform::current(install) else { return };
    let mut exe = platform::release_exe(install, &v);
    if cfg!(windows) {
        // Windows keys the notification-area pin by exe path; a dead tray no longer locks the copy
        let stable = install.join("taskq-tray.exe");
        if fs::copy(&exe, &stable).is_ok() {
            exe = stable;
        }
    }
    let mut c = std::process::Command::new(exe);
    let err = fs::File::create(install.join("tray.log")).map(std::process::Stdio::from).unwrap_or_else(|_| std::process::Stdio::null());
    c.arg("tray").current_dir(install).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(err);
    // its own process group: stopping one project's service (launchctl bootout) leaves the host's tray alone
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut c, 0);
    *tray = platform::quiet(&mut c).spawn().ok();
}

/// Labels of the open tasks in the last `status.json`, for a resume that has no focus to restore (§ 4).
fn status_labels(root: &Path) -> Vec<String> {
    let st: Value = fs::read_to_string(dot(root).join("status.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    let mut l: Vec<String> = st["tasks"].as_array().into_iter().flatten().flat_map(|t| t["labels"].as_array().cloned().unwrap_or_default())
        .filter_map(|x| x.as_str().map(String::from)).filter(|x| !x.starts_with("q-") && x != service::TASK_LABEL).collect();
    l.sort();
    l.dedup();
    l
}

/// Focus (§ 4): `f` admits; `request` keeps the owner's words, none removes them.
fn set_focus(root: &Path, f: &str, request: Option<&str>) -> Res<()> {
    let d = dot(root);
    fs::create_dir_all(&d).map_err(|e| e.to_string())?;
    fs::write(d.join("focus"), f).map_err(|e| e.to_string())?;
    match request {
        Some(r) => fs::write(d.join("focus.request"), r).map_err(|e| e.to_string())?,
        None => {
            let _ = fs::remove_file(d.join("focus.request"));
        }
    }
    poke(root);
    Ok(())
}

/// Pause (§ 4): keep the focus in `focus.paused`, admit nothing.
fn pause(root: &Path) {
    let d = dot(root);
    let f = fs::read_to_string(d.join("focus")).unwrap_or_default();
    if !matches!(f.trim(), "" | "off") {
        let _ = fs::write(d.join("focus.paused"), f);
    }
    let _ = fs::write(d.join("focus"), "off");
    poke(root);
}

/// Resume (§ 4): restore the saved focus; never `all` by default.
fn resume(root: &Path) -> Res<String> {
    let d = dot(root);
    let f = fs::read_to_string(d.join("focus.paused")).unwrap_or_default();
    if matches!(f.trim(), "" | "off") {
        return Err(format!("no saved focus: choose with `taskq service focus all|<label..>`; labels of open tasks: {}", status_labels(root).join(", ")));
    }
    fs::write(d.join("focus"), &f).map_err(|e| e.to_string())?;
    poke(root);
    Ok(f.trim().to_string())
}

/// Windows: `schtasks /End` ends only the launcher; the service, its children and the tray go with this (§ 8).
fn kill_service(root: &Path) {
    let status = dot(root).join("status.json");
    if !cfg!(windows) || file_age(&status).is_none_or(|a| a > 600) {
        return;
    }
    let pid = fs::read_to_string(&status).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).and_then(|v| v["pid"].as_u64());
    if let Some(pid) = pid.filter(|p| *p != std::process::id() as u64) {
        // the image filter guards against a reused pid
        let cmd = ["taskkill", "/F", "/T", "/FI", &format!("PID eq {pid}"), "/FI", "IMAGENAME eq taskq.exe"].map(String::from).to_vec();
        let _ = platform::run_all(&[cmd], true);
    }
}

#[cfg(unix)]
fn reexec(s: &Service, cfg: &Config) -> Res<()> {
    use std::os::unix::process::CommandExt;
    let mut text = format!("pid {}\n", std::process::id());
    // a `codex-app` turn has no process to hand over: its attempt goes `gone` (§ 3 app lease)
    for k in s.kids.iter().filter(|k| k.rt != "codex-app") {
        text += &format!("{} {} {} {} {}\n", k.task, k.attempt, k.rt, k.proc.id(), s.delivered(k.attempt));
    }
    platform::write_atomic(&dot(&cfg.root).join("kids"), &text)?;
    let e = std::process::Command::new(cfg.install.join(platform::exe_name())).args(["service", "run"]).exec();
    Err(format!("re-exec: {e}"))
}

#[cfg(not(unix))]
fn reexec(s: &Service, cfg: &Config) -> Res<()> {
    // ponytail: Windows cannot hand children to a new image; exit once no child is live and the unit restarts us.
    // The new `current` is already written, so the update waits only for the running attempts.
    if s.kids.is_empty() {
        std::process::exit(75);
    }
    // later update checks see no newer version: the restart request carries the re-exec until the children end
    fs::write(dot(&cfg.root).join("restart"), "").map_err(|e| e.to_string())
}

fn service(args: &Args) -> Res<()> {
    let sub = args.pos.get(1).map(String::as_str).unwrap_or("");
    let projects = platform::default_install().join("projects");
    if sub == "list" {
        // one project root per line, written by `service install` (§ 5); a PM or ARM covers several projects from it
        for r in fs::read_to_string(&projects).unwrap_or_default().lines().filter(|l| !l.is_empty()) {
            let f = fs::read_to_string(dot(Path::new(r)).join("focus")).unwrap_or_default();
            println!("{r} focus: {}", if f.trim().is_empty() { "off" } else { f.trim() });
        }
        return Ok(());
    }
    let root = find_root()?;
    let d = dot(&root);
    match sub {
        "run" => service_run(&ctx()?),
        "focus" => {
            let f = args.pos[2..].join(" ");
            if f.is_empty() {
                // the contract an agent maps the owner's words to (§ 4)
                let c = ctx()?;
                let mut n: BTreeMap<String, usize> = BTreeMap::new();
                for i in c.board.issues(None)?.iter().filter(|i| service::is_task(i)) {
                    for l in i.labels.iter().filter(|l| !l.starts_with("q-") && *l != service::TASK_LABEL) {
                        *n.entry(l.clone()).or_default() += 1;
                    }
                }
                let now = fs::read_to_string(d.join("focus")).unwrap_or_default();
                println!("focus: {}", if now.trim().is_empty() { "off" } else { now.trim() });
                println!("has: {}", c.cfg.has.join(" "));
                println!("labels of open tasks: {}", n.iter().map(|(l, k)| format!("{l} ({k})")).collect::<Vec<_>>().join(", "));
                return Ok(());
            }
            set_focus(&root, &f, args.one("request"))?;
            println!("focus: {f}");
            Ok(())
        }
        "pause" => {
            fs::create_dir_all(&d).map_err(|e| e.to_string())?;
            pause(&root);
            println!("focus: off");
            Ok(())
        }
        "resume" => {
            println!("focus: {}", resume(&root)?);
            Ok(())
        }
        "capacity" => capacity_cmd(&root, args),
        "restart" => {
            fs::create_dir_all(&d).map_err(|e| e.to_string())?;
            fs::write(d.join("restart"), "").map_err(|e| e.to_string())?;
            poke(&root);
            println!("restart requested");
            Ok(())
        }
        "status" => {
            let text = fs::read_to_string(d.join("status.json")).map_err(|_| "no service status: is `taskq service run` running here?".to_string())?;
            println!("{text}");
            Ok(())
        }
        "install" | "uninstall" | "stop" => {
            let (cfg, _) = load(&root)?;
            let u = platform::unit(&cfg.install, &root, &project_name(&root));
            if sub == "stop" {
                platform::run_all(&u.stop, true)?;
                kill_service(&root);
                println!("stopped; it starts again at the next logon");
                return Ok(());
            }
            if sub == "install" {
                platform::install_self(&cfg.install)?;
                platform::write_unit(&u)?;
                platform::run_all(&u.unload, true)?;
                platform::run_all(&u.load, false)?;
                let mut all: Vec<String> = fs::read_to_string(&projects).unwrap_or_default().lines().map(String::from).collect();
                if !all.contains(&root.display().to_string()) {
                    all.push(root.display().to_string());
                    fs::write(&projects, all.join("\n") + "\n").map_err(|e| e.to_string())?;
                }
                println!("installed {}", u.path.display());
            } else {
                platform::run_all(&u.unload, true)?;
                let _ = fs::remove_file(&u.path);
                let all: Vec<String> = fs::read_to_string(&projects).unwrap_or_default().lines().filter(|l| *l != root.display().to_string()).map(|l| format!("{l}\n")).collect();
                let _ = fs::write(&projects, all.concat());
                println!("removed {}", u.path.display());
                kill_service(&root);
            }
            Ok(())
        }
        _ => Err("usage: taskq service install|run|focus|pause|resume|capacity|status|restart|stop|uninstall|list".into()),
    }
}

#[cfg(any(target_os = "macos", windows))]
fn tray(install: PathBuf, snapshot: Option<&str>) -> Res<()> {
    tray::run(install, snapshot.map(PathBuf::from))
}

#[cfg(not(any(target_os = "macos", windows)))]
fn tray(_install: PathBuf, _snapshot: Option<&str>) -> Res<()> {
    Err("taskq tray: macOS and Windows only".into())
}

// ---------- commands ----------

fn print_task(n: u64, t: &Task) {
    println!("#{n} {}", t.state.name());
}

fn run(argv: &[String]) -> Res<()> {
    let a = Args::parse(argv)?;
    let cmd = a.pos.first().map(String::as_str).unwrap_or("");
    match cmd {
        "selftest" => {
            let n = core::golden(core::GOLDEN).map_err(|e| format!("fold: FAIL {e}"))?;
            println!("fold: {n} golden traces PASS");
            println!("{}", formats_selftest()?);
            if std::env::var_os("TASKQ_SELFTEST_OFFLINE").is_none()
                && let Ok(root) = find_root()
            {
                let c = ctx()?;
                let open = c.board.issues(None)?.len();
                println!("board: {} as {} on {}, {open} open issues, host {} (root {})", c.cfg.repo, c.board.me()?, c.cfg.web_host, c.cfg.me, root.display());
            }
            Ok(())
        }
        "update" => {
            let install = platform::default_install();
            let check = a.flags.contains_key("check");
            match platform::update(&install, check)? {
                Some(v) if check => println!("update available: {} -> {v}", platform::current(&install).unwrap_or_else(|| platform::VERSION.into())),
                Some(v) => println!("updated to {v}; the service restarts on its next update check"),
                None => println!("up to date ({})", platform::current(&install).unwrap_or_else(|| platform::VERSION.into())),
            }
            Ok(())
        }
        "service" => service(&a),
        "tray" => tray(platform::default_install(), a.one("snapshot")),
        "" | "help" | "-h" | "--help" => Err(USAGE.into()),
        _ => command(&ctx()?, cmd, &a),
    }
}

fn command(c: &Ctx, cmd: &str, a: &Args) -> Res<()> {
    let (b, cfg, root) = (c.board.as_ref(), &c.cfg, c.cfg.root.as_path());
    let text = a.one("text").unwrap_or("");
    match cmd {
        "labels" => {
            let mut names = labels::FIXED.iter().map(|s| s.to_string()).collect::<Vec<_>>();
            names.extend(b.issues(None)?.into_iter().flat_map(|i| i.labels));
            b.ensure_labels(&names)?;
            println!("label catalog readback verified");
            Ok(())
        }
        "add" => {
            // § 1 task body: Goal, Acceptance, Scope, dependencies; limits checked before any write
            let title = a.pos.get(1).filter(|t| !t.trim().is_empty()).ok_or("title missing")?;
            if title.contains('\n') || title.chars().count() > 80 {
                return Err("title: one line, at most 80 characters; shorten it".into());
            }
            let goal: Vec<String> = a.need("goal")?.lines().map(str::trim).filter(|l| !l.is_empty()).map(String::from).collect();
            limit("Goal", &goal, 1, 2, 240)?;
            let (acc, scope) = (a.all("acceptance"), a.all("scope"));
            one_line("--acceptance", &acc)?;
            one_line("--scope", &scope)?;
            let acc: Vec<String> = acc.iter().enumerate().map(|(i, s)| format!("A{}. {s}", i + 1)).collect();
            limit("Acceptance", &acc, 1, 8, 240).map_err(|e| if acc.len() > 8 { format!("{e} (split the task, R2)") } else { e })?;
            let scope: Vec<String> = scope.iter().map(|s| format!("- {s}")).collect();
            limit("Scope", &scope, 0, 4, 240)?;
            let mut body = format!("## Goal\n{}\n\n## Acceptance\n{}\n", goal.join("\n"), acc.join("\n"));
            if !scope.is_empty() {
                body += &format!("\n## Scope\n{}\n", scope.join("\n"));
            }
            let deps: Vec<u64> = a.all("deps").iter().flat_map(|d| d.split([',', ' '])).filter(|d| !d.is_empty()).map(|d| d.trim_start_matches('#').parse().map_err(|_| format!("bad --deps {d}"))).collect::<Res<_>>()?;
            if !deps.is_empty() {
                body += "\n";
                body += &deps.iter().map(|d| format!("Depends on #{d}\n")).collect::<String>();
            }
            for (flag, allowed) in [("type", &["code", "docs", "research", "asset"][..]), ("runtime", &["codex", "claude"][..]), ("priority", &["0", "1", "2"][..])] {
                if a.all(flag).len() > 1 || a.one(flag).is_some_and(|v| !allowed.contains(&v)) {
                    return Err(format!("--{flag} requires one of {}", allowed.join("|")));
                }
            }
            if a.all("host").len() > 1 || a.one("host").is_some_and(|v| v.is_empty() || v.contains(',') || v.chars().any(char::is_whitespace)) {
                return Err("--host requires one nonempty host name".into());
            }
            if a.one("priority") == Some("0") && !b.canonical_labels()? {
                return Err("--priority 0 needs the canonical label schema (§ 9)".into());
            }
            let blocked = deps_open(b, &body)?;
            let mut labels = vec![service::TASK_LABEL.to_string(), b.state_label(if blocked { "waiting" } else { "ready" })?];
            for (flag, prefix) in [("type", ""), ("runtime", "run-"), ("priority", if b.canonical_labels()? { "p" } else { "priority-" }), ("host", "host-")] {
                if let Some(v) = a.one(flag) {
                    labels.push(format!("{prefix}{v}"));
                }
            }
            labels::validate(&labels)?;
            let n = b.create(title, &body, &labels)?;
            let t = if a.flags.contains_key("later") { act(b, root, n, "later", &[], text)? } else { fold(n, &[], blocked) };
            poke(root);
            print_task(n, &t);
            Ok(())
        }
        "ask" => {
            let n = a.num(1)?;
            let body = ask_card(n, a.need("text")?, &a.all("option"), a.one("recommend"), &a.all("link"))?;
            print_task(n, &act(b, root, n, "ask", &[], &body)?);
            Ok(())
        }
        "answer" if a.pos.get(1).is_some_and(|p| p.contains('.')) => {
            for line in answer_picks(b, cfg, &a.pos[1..])? {
                println!("{line}");
            }
            Ok(())
        }
        "answer" => {
            let n = a.num(1)?;
            print_task(n, &act(b, root, n, "answer", &[], a.need("text")?)?);
            Ok(())
        }
        "result" => {
            let n = a.num(1)?;
            let sha = a.need("sha")?;
            let body = result_text(n, text, &a.all("checks"), &a.all("option"), a.one("recommend"), &a.all("link"))?;
            print_task(n, &act(b, root, n, "result", &[sha], &body)?);
            Ok(())
        }
        "requeue" | "later" => {
            let n = a.num(1)?;
            print_task(n, &act(b, root, n, cmd, &[], text)?);
            Ok(())
        }
        "steer" | "failed" => {
            let n = a.num(1)?;
            print_task(n, &act(b, root, n, cmd, &[], a.need("text")?)?);
            Ok(())
        }
        "close" => {
            let mut failed = false;
            for i in 1..a.pos.len() {
                let n = a.num(i)?;
                // § 7: a reviewed result closes with the verdict template; later/failed tasks and bare closes keep free text
                let checked = match current(b, n)? {
                    (issue, t, _) if t.state == State::Review && !text.is_empty() => close_verdict(text, &issue.body),
                    _ => Ok(()),
                };
                match checked.and_then(|_| close_one(b, cfg, n, text)) {
                    Ok(m) => println!("{m}"),
                    Err(e) => {
                        failed = true;
                        println!("error: {e}");
                    }
                }
            }
            if failed { Err("some tasks were not closed".into()) } else { Ok(()) }
        }
        "status" => {
            println!("{}", report(b, cfg, &c.raw, &a.all("only").iter().flat_map(|x| x.split(',')).map(String::from).collect::<Vec<_>>())?);
            Ok(())
        }
        "cleanup" => {
            // a child the local service still runs keeps its worktree, whatever the board says
            let st: Value = fs::read_to_string(dot(root).join("status.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
            let live: Vec<u64> = st["kids"].as_array().into_iter().flatten().filter_map(|k| k["task"].as_u64()).collect();
            let log = service::cleanup(b, root, |n| live.contains(&n), a.flags.contains_key("force"));
            println!("{}", if log.is_empty() { "nothing to clean".into() } else { log.join("\n") });
            Ok(())
        }
        "inbox" => {
            println!("{}", inbox(b, a.num(1)?)?);
            Ok(())
        }
        "wait" => {
            let w = a.one("window").map(|w| w.parse().map_err(|_| "bad --window")).transpose()?.unwrap_or(10);
            for l in wait(b, cfg, w, &a.all("only").iter().flat_map(|x| x.split(',')).map(String::from).collect::<Vec<_>>())? {
                println!("{l}");
            }
            Ok(())
        }
        _ => Err(USAGE.into()),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // `<install>/taskq` is the launcher: it runs the current release (§ 8 step 5).
    let install = platform::default_install();
    let exe = std::env::current_exe().ok().and_then(|e| e.canonicalize().ok());
    let launcher = install.join(platform::exe_name()).canonicalize().ok();
    if exe.is_some() && exe == launcher && platform::current(&install).is_some() {
        platform::launch(&install, &args);
    }
    if let Err(e) = run(&args) {
        platform::die(&e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use board::Fake;

    #[test]
    fn capacity_edit_keeps_the_file_and_validates() {
        let text = "{\n    \"repo\": \"o/r\",\n    \"limits\": {\"claude\": 4, \"codex\": 4},\n    \"hosts\": {\"a\": \"win\"},\n    \"zeta\": 1\n}\n";
        let e = CapEdit { limits: BTreeMap::from([("claude".into(), 2)]), gate: Some(true), ..Default::default() };
        let out = edit_capacity(text, "win", "mac", &e, None).unwrap();
        assert!(out.starts_with("{\n    \"repo\"") && out.ends_with("}\n"), "indent, order and newline kept:\n{out}");
        assert!(out.find("\"zeta\"").unwrap() < out.find("\"capacity_hosts\"").unwrap());
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["limits"], json!({"claude": 4, "codex": 4}), "top level untouched");
        assert_eq!(v["capacity_hosts"]["win"], json!({"limits": {"claude": 2}, "load_gate": {"enabled": true, "cpu_max": 85, "memory_min": 15}}));
        assert_eq!(service::capacity(&v, "mac").unwrap().limits["claude"], 4, "other hosts keep theirs");
        let old = service::capacity(&serde_json::from_str(text).unwrap(), "win").unwrap().json();
        assert!(edit_capacity(text, "win", "mac", &e, Some(&old)).is_ok());
        for (host, e, expect, err) in [
            ("nas", e.clone(), None, "unknown host"),
            ("mac", CapEdit::default(), None, "nothing to change"),
            ("mac", CapEdit { cpu_max: Some(5), ..Default::default() }, None, "cpu_max"),
            ("mac", CapEdit { memory_min: Some(96), ..Default::default() }, None, "memory_min"),
            ("mac", CapEdit { limits: BTreeMap::from([("gpt".into(), 1)]), ..Default::default() }, None, "unknown runtime"),
            ("win", e.clone(), Some(json!({})), "nothing written"),
        ] {
            let r = edit_capacity(text, host, "mac", &e, expect.as_ref());
            assert!(r.as_ref().is_err_and(|x| x.contains(err)), "{host} {err}: {r:?}");
        }
    }

    fn cfg() -> Config {
        Config {
            root: std::env::temp_dir().join("taskq-main-test"),
            install: PathBuf::from("/nonexistent"),
            board: Kind::GitHub,
            web_host: String::new(),
            repo: "o/r".into(),
            publish: "pr".into(),
            me: "mac".into(),
            limits: BTreeMap::new(),
            has: vec![],
            permission_mode: "dontAsk".into(),
            codex: vec![],
            ttl: 300,
            poll: 60,
            update_every: 900,
            rollback_after: 600,
        }
    }

    #[test]
    fn add_uses_canonical_names_defaults_and_validates_before_writing() {
        let c = Ctx { cfg: cfg(), raw: json!({}), board: Box::new(Fake::default()) };
        let args = |extra: &[&str]| {
            let mut v = vec!["add", "T", "--goal", "G", "--acceptance", "observable"];
            v.extend_from_slice(extra); Args::parse(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>()).unwrap()
        };
        command(&c, "add", &args(&[])).unwrap();
        let i = c.board.issues(None).unwrap().pop().unwrap();
        assert_eq!(i.labels, vec!["taskq", "q-ready"]);
        command(&c, "add", &args(&["--priority", "1", "--type", "code", "--deps", &i.number.to_string()])).unwrap();
        let i = c.board.issues(None).unwrap().pop().unwrap();
        assert_eq!(i.labels, vec!["taskq", "q-blocked", "code", "p1"]);
        assert!(in_scope(&i, &["q-waiting".into(), "priority-1".into()]));
        command(&c, "add", &args(&["--priority", "0"])).unwrap();
        assert_eq!(c.board.issues(None).unwrap().pop().unwrap().labels, vec!["taskq", "q-ready", "p0"]);
        for extra in [&["--priority", "3"][..], &["--runtime", "any"], &["--type", "code", "--type", "docs"], &["--host", "win,mac"]] {
            assert!(command(&c, "add", &args(extra)).is_err());
        }
        assert_eq!(c.board.issues(None).unwrap().len(), 3);
    }

    #[test]
    fn add_renders_the_task_body_and_rejects_over_limit_text() {
        let c = Ctx { cfg: cfg(), raw: json!({}), board: Box::new(Fake::default()) };
        let run = |v: &[&str]| command(&c, "add", &Args::parse(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>()).unwrap());
        c.board.create("dep", "", &[]).unwrap();
        run(&["add", "T", "--goal", "G", "--acceptance", "a", "--acceptance", "b", "--scope", "s", "--deps", "1"]).unwrap();
        let i = c.board.issues(None).unwrap().pop().unwrap();
        assert_eq!(i.body, "## Goal\nG\n\n## Acceptance\nA1. a\nA2. b\n\n## Scope\n- s\n\nDepends on #1\n");
        let long = "x".repeat(237);
        assert!(run(&["add", "T", "--goal", "G", "--acceptance", &long]).unwrap_err().contains("241 characters, limit 240"));
        assert!(run(&["add", &"t".repeat(81), "--goal", "G", "--acceptance", "a"]).unwrap_err().contains("80"));
        assert!(run(&["add", "T", "--goal", "1\n2\n3", "--acceptance", "a"]).unwrap_err().contains("Goal: 3 lines"));
        assert!(run(&["add", "T", "--goal", "G", "--acceptance", "a\nb"]).is_err());
        let mut v = vec!["add", "T", "--goal", "G"];
        v.extend(["--acceptance", "a"].repeat(9));
        assert!(run(&v).unwrap_err().contains("split the task"));
        assert_eq!(c.board.issues(None).unwrap().len(), 2, "rejected input writes nothing");
    }

    #[test]
    fn ask_card_follows_section_2() {
        let card = ask_card(7, "Done: checked spec\nProblem: two ways\nboth slow", &["left".into(), "right, slower".into()], Some("2"), &["https://g/o/r/pull/12".into(), "[shot](https://x/y.png)".into()]).unwrap();
        assert_eq!(card, "Done: checked spec\nProblem: two ways\nboth slow\nEvidence: [#12](https://g/o/r/pull/12), [shot](https://x/y.png)\n1. left\n2. right, slower ★\nTo answer: `taskq answer 7.K` or comment `K` or the option text.");
        let o = |n: usize| vec!["o".to_string(); n];
        assert!(ask_card(7, "Problem: p", &o(2), Some("1"), &[]).unwrap_err().contains("Done: 0 lines"));
        assert!(ask_card(7, "question?", &o(2), Some("1"), &[]).unwrap_err().contains("starts with"));
        assert!(ask_card(7, "Done: d", &o(2), Some("1"), &[]).unwrap_err().contains("Problem: 0 lines"));
        assert!(ask_card(7, "Problem: p\nDone: d", &o(2), Some("1"), &[]).unwrap_err().contains("order"));
        assert!(ask_card(7, &format!("Done: {}", "d".repeat(155)), &o(2), Some("1"), &[]).unwrap_err().contains("161 characters, limit 160"));
        assert!(ask_card(7, "Done: d\nProblem: p", &o(1), Some("1"), &[]).is_err());
        assert!(ask_card(7, "Done: d\nProblem: p", &o(5), Some("1"), &[]).is_err());
        assert!(ask_card(7, "Done: d\nProblem: p", &o(2), None, &[]).unwrap_err().contains("--recommend"));
        assert!(ask_card(7, "Done: d\nProblem: p", &o(2), Some("3"), &[]).is_err());
        assert!(ask_card(7, "Done: d\nProblem: p", &o(2), Some("1"), &vec!["u".to_string(); 3]).is_err());
        assert_eq!(width("see [#12](https://very/long/url) and `cargo test --locked` https://x/y"), 13, "link targets and code are exempt");
    }

    #[test]
    fn result_and_close_follow_section_7() {
        let checks = ["cargo test --locked -- pass\ncargo run -- selftest -- pass".to_string()];
        assert_eq!(result_text(7, "Changed: faster\nOpen: none", &checks, &[], None, &[]).unwrap(), "Changed: faster\nChecks: cargo test --locked -- pass\nChecks: cargo run -- selftest -- pass\nOpen: none");
        let r = result_text(7, "Changed: c\nOpen: none\nDone: shipped\nProblem: keep it?", &["cargo test --locked -- pass".into()], &["close: ship".into(), "redo".into()], Some("1"), &[]).unwrap();
        assert_eq!(r, "Changed: c\nChecks: cargo test --locked -- pass\nOpen: none\n\nDone: shipped\nProblem: keep it?\n1. close: ship ★\n2. redo\nTo answer: `taskq answer 7.K` or comment `K` or the option text.");
        assert!(result_text(7, "Changed: c\nOpen: none\nDone: d", &checks, &[], None, &[]).unwrap_err().contains("only with --option"));
        assert!(result_text(7, "Changed: c\nOpen: none", &[], &[], None, &[]).unwrap_err().contains("Checks: 0 lines"));
        assert!(result_text(7, "summary", &checks, &[], None, &[]).is_err());
        let body = "## Acceptance\nA1. a\nA2. b\n";
        assert!(close_verdict("Changed: c\nChecks: A1 ok, A2 gap; CI green on abc\nOpen: A2", body).is_ok());
        assert!(close_verdict("Changed: c\nChecks: A1 ok; CI green\nOpen: none", body).unwrap_err().contains("A2"));
        assert!(close_verdict("Changed: c\nChecks: A1 ok, A2 ok\nOpen: none", body).unwrap_err().contains("CI"));
        assert!(close_verdict("A1 ok, A2 ok; c; open: none", body).is_err(), "the old one-line verdict is rejected");
        assert_eq!(formats_selftest().unwrap(), "formats: ask card, result, close verdict PASS");
    }

    #[test]
    fn rendered_ask_card_resumes_the_worker_through_the_hand_typed_answer() {
        let b = Fake::default();
        let n = b.create("T", "## Goal\ng", &["taskq".into()]).unwrap();
        b.comment(n, &entry("spawn", &["worker", "claude:u", "mac"], None, "")).unwrap();
        let a = fold(n, &entries(&b, n).unwrap(), false).attempt;
        b.comment(n, &entry("ask", &[], a, &ask_card(n, "Done: read spec\nProblem: which way", &["left".into(), "right".into()], Some("1"), &[]).unwrap())).unwrap();
        let card = fold(n, &entries(&b, n).unwrap(), false).card.unwrap();
        assert!(card.ends_with(&format!("To answer: `taskq answer {n}.K` or comment `K` or the option text.")), "{card}");
        b.comment(n, "taskq answer 2").unwrap();
        let log = entries(&b, n).unwrap();
        assert_eq!(fold(n, &log, false).state, State::Doing);
        assert_eq!(core::delivery(n, &log, log.last().unwrap().id), "right");
    }

    #[test]
    fn commands_write_only_accepted_entries_and_close_publishes() {
        let (b, c) = (Fake::default(), cfg());
        let n = b.create("T", "## Goal\ng", &["taskq".into()]).unwrap();
        assert!(act(&b, &c.root, n, "steer", &[], "x").unwrap_err().contains("ready"), "steer needs a live worker");
        let before = b.comments(n).unwrap().len();
        b.comment(n, &entry("spawn", &["worker", "claude:u", "mac"], None, "")).unwrap();
        let a = fold(n, &entries(&b, n).unwrap(), false).attempt.unwrap();
        b.comment(n, &entry("result", &["0123456789abcdef0123456789abcdef01234567"], Some(a), "1. close: ship it ★\n2. redo the copy")).unwrap();
        assert_eq!(b.comments(n).unwrap().len(), before + 2);
        b.state.borrow_mut().publish = "pending".into();
        assert!(close_one(&b, &c, n, "ok").unwrap_err().contains("pending"));
        assert_eq!(fold(n, &entries(&b, n).unwrap(), false).state, State::Review, "pending CI changes nothing");
        b.state.borrow_mut().publish = String::new();
        assert!(answer_picks(&b, &c, &[format!("{n}.3")]).is_err(), "picks are checked before writing");
        assert_eq!(answer_picks(&b, &c, &[format!("{n}.1")]).unwrap(), vec![format!("closed #{n}: merged 0123456789abcdef0123456789abcdef01234567")]);
        let i = b.issue(n).unwrap();
        assert!(!i.open && !i.labels.iter().any(|l| l.starts_with("q-")));
    }

    #[test]
    fn resume_restores_the_saved_focus_and_never_defaults_to_all() {
        let root = std::env::temp_dir().join(format!("taskq-resume-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(dot(&root)).unwrap();
        fs::write(dot(&root).join("status.json"), r#"{"tasks": [{"n": 1, "labels": ["taskq", "q-ready", "area-maps"]}]}"#).unwrap();
        fs::write(dot(&root).join("focus"), "off").unwrap();
        let e = resume(&root).unwrap_err();
        assert!(e.contains("labels of open tasks: area-maps"), "{e}");
        assert_eq!(fs::read_to_string(dot(&root).join("focus")).unwrap(), "off");
        fs::write(dot(&root).join("focus"), "area-maps").unwrap();
        pause(&root);
        pause(&root);
        assert_eq!(fs::read_to_string(dot(&root).join("focus")).unwrap(), "off");
        assert_eq!(resume(&root).unwrap(), "area-maps", "a second pause keeps the saved focus");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn report_follows_r6() {
        let (b, c) = (Fake::default(), cfg());
        let n = b.create("Ship | it", "", &["taskq".into()]).unwrap();
        let q = b.create("Q", "", &["taskq".into()]).unwrap();
        b.comment(q, &entry("spawn", &["worker", "claude:abc", "mac"], None, "")).unwrap();
        let a = fold(q, &entries(&b, q).unwrap(), false).attempt.unwrap();
        b.comment(q, &entry("ask", &[], Some(a), "Which way?\nBecause.\n1. left\n2. right ★")).unwrap();
        b.create("taskq host mac", "version 4.0.0", &[service::HOST_LABEL.into()]).unwrap();
        let r = report(&b, &c, &Value::Null, &[]).unwrap();
        assert!(r.contains("In work 0 · Waiting for answer 1 · Ready 1"), "{r}");
        assert!(r.contains(&format!("| [#{n} Ship \\| it](https://board/{n}) | ready |")), "{r}");
        assert!(r.contains(&format!("{q}.1 left · {q}.2 right ★")), "{r}");
        assert!(r.contains("Hosts: mac v4.0.0"), "{r}");
    }
}
