//! The `taskq` command line (taskq.md § 5). Every write is one `taskq:` comment that `fold` accepts.

mod board;
mod core;
mod platform;
mod runtime;
mod service;
#[cfg(any(target_os = "macos", windows))]
mod tray;

use board::{Board, Http, Kind, Publish, entries};
use core::{Entry, State, Task, entry, fold};
use serde_json::{Value, json};
use service::{Config, Kid, Service};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

type Res<T> = Result<T, String>;

const USAGE: &str = "usage: taskq add|ask|answer|result|requeue|steer|later|failed|close|status|inbox|wait|cleanup|update|service|tray|selftest (see taskq.md § 5)";

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
    let board = Box::new(Http::new(cfg.board, &cfg.web_host, &cfg.repo, &tok));
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
    let t = fold(&entries(b, n)?, deps);
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
    let before = fold(&log, deps);
    let body = entry(kind, args, own_attempt(n), text);
    log.push(Entry { id: u64::MAX, body: body.clone(), edited: false });
    if fold(&log, deps) == before {
        return Err(format!("#{n} is {}: `{kind}` not accepted", before.state.name()));
    }
    b.comment(n, &body)?;
    let t = fold(&entries(b, n)?, deps); // re-read: report what the board holds (R12)
    service::project(b, &issue, &t)?;
    poke(root);
    Ok(t)
}

fn numbered(opts: &[String], recommend: Option<usize>) -> String {
    opts.iter().enumerate().map(|(i, o)| format!("{}. {o}{}", i + 1, if recommend == Some(i + 1) { " ★" } else { "" })).collect::<Vec<_>>().join("\n")
}

/// Option lines `K. text [★]` of a card.
fn options(card: &str) -> Vec<(usize, String, bool)> {
    card.lines()
        .filter_map(|l| {
            let (k, rest) = l.trim().split_once(". ")?;
            let k: usize = k.parse().ok()?;
            let star = rest.trim_end().ends_with('★');
            Some((k, rest.trim_end().trim_end_matches('★').trim().to_string(), star))
        })
        .collect()
}

fn join(parts: &[&str]) -> String {
    parts.iter().filter(|p| !p.trim().is_empty()).copied().collect::<Vec<_>>().join("\n\n")
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
    only.is_empty() || i.labels.iter().any(|l| only.contains(l))
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
            let t = fold(&entries(b, i.number)?, core::depends(&i.body).iter().any(|d| open.contains(d)));
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

fn sleep_until_poke(poke: &Path, secs: u64) {
    let mtime = |p: &Path| fs::metadata(p).and_then(|m| m.modified()).ok();
    let start = mtime(poke);
    for _ in 0..secs {
        std::thread::sleep(Duration::from_secs(1));
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
    let start = |l: &runtime::Launch| runtime::spawn(&env, l).map(|ch| Box::new(ch) as Box<dyn runtime::Proc>);
    let mut s = Service::new(c.board.as_ref(), cfg, platform::VERSION, Box::new(start));
    let d = dot(&cfg.root);
    fs::create_dir_all(&d).map_err(|e| e.to_string())?;
    adopt(&mut s, &cfg.root);
    let is_installed = installed(&cfg.install);
    let began = Instant::now();
    let mut checked: Option<Instant> = None;
    let _ = fs::remove_file(d.join("restart"));
    let mut tray = None;
    let project = project_name(&cfg.root);
    let issues = c.board.url(0).strip_suffix("/0").unwrap_or_default().to_string();
    let log = platform::log_path(&cfg.install, &project);
    eprintln!("taskq {} service on {} for {}", platform::VERSION, cfg.me, cfg.repo);
    loop {
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
        let tasks: Vec<Value> = s.tasks.iter().map(|(n, title, state)| json!({"n": n, "title": title, "state": state})).collect();
        let status = json!({"host": cfg.me, "version": platform::VERSION, "focus": s.focus, "error": r.err(), "kids": kids, "at": at,
            "pid": std::process::id(), "project": project, "issues": issues, "log": log, "tasks": tasks});
        let _ = platform::write_atomic(&d.join("status.json"), &serde_json::to_string_pretty(&status).unwrap_or_default());
        if is_installed && began.elapsed().as_secs() >= cfg.rollback_after {
            let h = cfg.install.join("releases").join(platform::VERSION).join("healthy");
            if !h.exists() {
                let _ = fs::write(h, "");
            }
        }
        if is_installed && checked.is_none_or(|t| t.elapsed().as_secs() >= cfg.update_every) {
            checked = Some(Instant::now());
            match platform::update(&cfg.install, false) {
                Ok(Some(v)) => {
                    eprintln!("updated to {v}; restarting");
                    reexec(&s, cfg)?;
                }
                Ok(None) => {}
                Err(e) => eprintln!("update: {e}"),
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
        if is_installed {
            keep_tray(&cfg.root, &mut tray);
        }
        sleep_until_poke(&d.join("poke"), cfg.poll);
    }
}

/// Start `taskq tray` when its heartbeat `<root>/.taskq/tray` is stale (§ 4); reap the one this image started.
fn keep_tray(root: &Path, tray: &mut Option<std::process::Child>) {
    if !cfg!(any(target_os = "macos", windows)) {
        return;
    }
    if tray.as_mut().is_some_and(|c| !matches!(c.try_wait(), Ok(None))) {
        *tray = None;
    }
    if tray.is_some() || file_age(&dot(root).join("tray")).is_some_and(|a| a < 30) {
        return;
    }
    let Ok(me) = std::env::current_exe() else { return };
    let mut c = std::process::Command::new(me);
    let err = fs::File::create(dot(root).join("tray.log")).map(std::process::Stdio::from).unwrap_or_else(|_| std::process::Stdio::null());
    c.arg("tray").current_dir(root).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(err);
    *tray = platform::quiet(&mut c).spawn().ok();
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
    for k in &s.kids {
        text += &format!("{} {} {} {} {}\n", k.task, k.attempt, k.rt, k.proc.id(), s.delivered(k.attempt));
    }
    platform::write_atomic(&dot(&cfg.root).join("kids"), &text)?;
    let e = std::process::Command::new(cfg.install.join(platform::exe_name())).args(["service", "run"]).exec();
    Err(format!("re-exec: {e}"))
}

#[cfg(not(unix))]
fn reexec(s: &Service, _cfg: &Config) -> Res<()> {
    // ponytail: Windows cannot hand children to a new image; exit once no child is live and the unit restarts us.
    // The new `current` is already written, so the update waits only for the running attempts.
    if s.kids.is_empty() {
        std::process::exit(75);
    }
    Ok(())
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
            fs::create_dir_all(&d).map_err(|e| e.to_string())?;
            fs::write(d.join("focus"), &f).map_err(|e| e.to_string())?;
            poke(&root);
            println!("focus: {f}");
            Ok(())
        }
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
        _ => Err("usage: taskq service install|run|focus|status|restart|stop|uninstall|list".into()),
    }
}

#[cfg(any(target_os = "macos", windows))]
fn tray(root: PathBuf) -> Res<()> {
    tray::run(root)
}

#[cfg(not(any(target_os = "macos", windows)))]
fn tray(_root: PathBuf) -> Res<()> {
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
            match platform::update(&install, a.flags.contains_key("check"))? {
                Some(v) => println!("updated to {v}; the service restarts on its next update check"),
                None => println!("up to date ({})", platform::current(&install).unwrap_or_else(|| platform::VERSION.into())),
            }
            Ok(())
        }
        "service" => service(&a),
        "tray" => tray(find_root()?),
        "" | "help" | "-h" | "--help" => Err(USAGE.into()),
        _ => command(&ctx()?, cmd, &a),
    }
}

fn command(c: &Ctx, cmd: &str, a: &Args) -> Res<()> {
    let (b, cfg, root) = (c.board.as_ref(), &c.cfg, c.cfg.root.as_path());
    let text = a.one("text").unwrap_or("");
    match cmd {
        "add" => {
            let title = a.pos.get(1).filter(|t| !t.trim().is_empty()).ok_or("title missing")?;
            let acc = a.all("acceptance");
            if acc.is_empty() {
                return Err("--acceptance missing".into());
            }
            let acc: String = acc.iter().enumerate().map(|(i, s)| format!("A{}. {s}\n", i + 1)).collect();
            let mut body = format!("## Goal\n{}\n\n## Acceptance\n{acc}", a.need("goal")?);
            let scope = a.all("scope");
            if !scope.is_empty() {
                body += &format!("\n## Scope\n{}\n", scope.iter().map(|s| format!("- {s}")).collect::<Vec<_>>().join("\n"));
            }
            let deps: Vec<u64> = a.all("deps").iter().flat_map(|d| d.split([',', ' '])).filter(|d| !d.is_empty()).map(|d| d.trim_start_matches('#').parse().map_err(|_| format!("bad --deps {d}"))).collect::<Res<_>>()?;
            if !deps.is_empty() {
                body += "\n";
                body += &deps.iter().map(|d| format!("Depends on #{d}\n")).collect::<String>();
            }
            let blocked = deps_open(b, &body)?;
            let mut labels = vec![service::TASK_LABEL.to_string(), format!("q-{}", if blocked { "waiting" } else { "ready" })];
            for (flag, prefix) in [("type", ""), ("runtime", "run-"), ("priority", "priority-"), ("host", "host-")] {
                if let Some(v) = a.one(flag) {
                    labels.push(format!("{prefix}{v}"));
                }
            }
            let n = b.create(title, &body, &labels)?;
            let t = if a.flags.contains_key("later") { act(b, root, n, "later", &[], text)? } else { fold(&[], blocked) };
            poke(root);
            print_task(n, &t);
            Ok(())
        }
        "ask" => {
            let n = a.num(1)?;
            let k: Option<usize> = a.one("recommend").map(|k| k.parse().map_err(|_| "bad --recommend")).transpose()?;
            let body = join(&[a.need("text")?, &numbered(&a.all("option"), k), &a.all("link").join("\n")]);
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
            let checks = a.one("checks").map(|c| format!("Checks: {c}")).unwrap_or_default();
            print_task(n, &act(b, root, n, "result", &[sha], &join(&[&checks, text, &numbered(&a.all("option"), None)]))?);
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
                match close_one(b, cfg, a.num(i)?, text) {
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
    fn commands_write_only_accepted_entries_and_close_publishes() {
        let (b, c) = (Fake::default(), cfg());
        let n = b.create("T", "## Goal\ng", &["taskq".into()]).unwrap();
        assert!(act(&b, &c.root, n, "steer", &[], "x").unwrap_err().contains("ready"), "steer needs a live worker");
        let before = b.comments(n).unwrap().len();
        b.comment(n, &entry("spawn", &["worker", "claude:u", "mac"], None, "")).unwrap();
        let a = fold(&entries(&b, n).unwrap(), false).attempt.unwrap();
        b.comment(n, &entry("result", &["0123456789abcdef0123456789abcdef01234567"], Some(a), "1. close: ship it ★\n2. redo the copy")).unwrap();
        assert_eq!(b.comments(n).unwrap().len(), before + 2);
        b.state.borrow_mut().publish = "pending".into();
        assert!(close_one(&b, &c, n, "ok").unwrap_err().contains("pending"));
        assert_eq!(fold(&entries(&b, n).unwrap(), false).state, State::Review, "pending CI changes nothing");
        b.state.borrow_mut().publish = String::new();
        assert!(answer_picks(&b, &c, &[format!("{n}.3")]).is_err(), "picks are checked before writing");
        assert_eq!(answer_picks(&b, &c, &[format!("{n}.1")]).unwrap(), vec![format!("closed #{n}: merged 0123456789abcdef0123456789abcdef01234567")]);
        let i = b.issue(n).unwrap();
        assert!(!i.open && !i.labels.iter().any(|l| l.starts_with("q-")));
    }

    #[test]
    fn report_follows_r6() {
        let (b, c) = (Fake::default(), cfg());
        let n = b.create("Ship | it", "", &["taskq".into()]).unwrap();
        let q = b.create("Q", "", &["taskq".into()]).unwrap();
        b.comment(q, &entry("spawn", &["worker", "claude:abc", "mac"], None, "")).unwrap();
        let a = fold(&entries(&b, q).unwrap(), false).attempt.unwrap();
        b.comment(q, &entry("ask", &[], Some(a), "Which way?\nBecause.\n1. left\n2. right ★")).unwrap();
        b.create("taskq host mac", "version 4.0.0", &[service::HOST_LABEL.into()]).unwrap();
        let r = report(&b, &c, &Value::Null, &[]).unwrap();
        assert!(r.contains("In work 0 · Waiting for answer 1 · Ready 1"), "{r}");
        assert!(r.contains(&format!("| [#{n} Ship \\| it](https://board/{n}) | ready |")), "{r}");
        assert!(r.contains(&format!("{q}.1 left · {q}.2 right ★")), "{r}");
        assert!(r.contains("Hosts: mac v4.0.0"), "{r}");
    }
}
