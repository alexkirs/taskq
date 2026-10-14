//! The `taskq` command line (taskq.md § 5). Every write is one `taskq:` comment that `fold` accepts.

mod admission;
mod board;
mod core;
mod data;
mod dispatch;
mod enrollment;
mod incident;
mod labels;
mod operator;
mod platform;
mod retro;
mod runtime;
mod service;
mod snapshot;
#[cfg(any(target_os = "macos", windows))]
mod icon;
#[cfg(any(target_os = "macos", windows))]
mod tray;

use board::{Board, entries};
use core::{Entry, State, entry, fold};
use dispatch::{Op, Receipt, act, answer_picks, close_one, current, deps_open};
use serde_json::{Value, json};
use service::{Config, Kid, Service};
use snapshot::in_scope;
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

pub type Res<T> = Result<T, String>;

const USAGE: &str = "taskq agents
 taskq labels
 taskq add <title> --goal G --acceptance A [--acceptance B..] [--scope P..] [--deps N..] [--type T] [--runtime R] [--priority 0|1|2] [--host H] [--later|--hold] [--text T]
 taskq start N
 taskq ask N --text Q --option O.. --recommend K [--link URL..]
 taskq answer N --text A | taskq answer N.K [M.K..]
 taskq result N --sha SHA --checks C.. --text T [--option O.. --recommend K --link URL..]
 taskq requeue N [--text T] | taskq later N [--text T]
 taskq steer N --text T | taskq failed N --text T
 taskq close N [M..] [--text VERDICT]
 taskq status [--only L,..] [--json] | taskq inbox N
 taskq retro | taskq add --retro K [--type T ..]
 taskq wait [--window MIN] [--only L,..]
 taskq cleanup [--force] | taskq update [--check]
 taskq service install|run|pause|resume|status|restart|stop|uninstall|list
 taskq service focus [all|off|<label..>] [--request TEXT]
 taskq service capacity [--host H] [--mode auto|fixed] [--max N|none] [--claude N] [--codex N] [--codex-app N] [--load-gate on|off] [--cpu-max P] [--memory-min P]
 taskq tray [--snapshot DIR] [--icon FILE] | taskq selftest
Writes to a task take [--op-id ID] [--revision R] [--json] (taskq.md section 5).
Text templates and limits: taskq.md sections 1, 2, 5, 7.";

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
                    let val = if matches!(k, "later" | "hold" | "check" | "force" | "help" | "json" | "apply") { String::new() } else { it.next().cloned().ok_or(format!("--{k} needs a value"))? };
                    a.flags.entry(k.to_string()).or_default().push(val);
                }
                None => a.pos.push(x.clone()),
            }
        }
        let used = match a.pos.first().map(String::as_str) {
            Some("close") => a.pos.len(),
            Some("answer") if a.pos.get(1).is_some_and(|p| p.contains('.')) => a.pos.len(),
            Some("service") if a.pos.get(1).is_some_and(|p| p == "focus") => a.pos.len(),
            Some("service" | "add" | "start" | "ask" | "answer" | "result" | "requeue" | "later" | "failed" | "steer" | "inbox") => 2,
            _ => 1,
        };
        if a.pos.len() > used {
            return Err(format!("unused positional arguments: {}", a.pos[used..].join(" ")));
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
    let board = board::kind(&str_or("board", "github"))?;
    let cfg = Config {
        root: root.to_path_buf(),
        install: platform::default_install(),
        board,
        web_host: str_or("host", board.default_host),
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

fn ctx() -> Res<Ctx> {
    let (cfg, raw) = load(&find_root()?)?;
    let board = cfg.board.open(&cfg.web_host, &cfg.repo, raw["label_schema"].as_str(), &platform::tool(&cfg.install))?;
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

use coreq::status::age as file_age;

// ---------- writes ----------

fn numbered(opts: &[String], recommend: Option<usize>) -> String {
    opts.iter().enumerate().map(|(i, o)| format!("{}. {o}{}", i + 1, if recommend == Some(i + 1) { " ★" } else { "" })).collect::<Vec<_>>().join("\n")
}

fn join(parts: &[&str]) -> String {
    parts.iter().filter(|p| !p.trim().is_empty()).copied().collect::<Vec<_>>().join("\n\n")
}

// ---------- message formats (R7, § 1, § 2 ask card, § 7 verdicts) ----------

/// R7: Unicode characters of a line; link targets and `code` spans (exact commands, SHAs, paths) are exempt.
fn width(l: &str) -> usize {
    let mut s = String::new();
    let mut rest = l;
    while let Some(a) = rest.find('`') {
        s.push_str(&rest[..a]);
        match rest[a + 1..].find('`') {
            Some(b) => rest = &rest[a + b + 2..],
            None => { rest = &rest[a..]; break; }
        }
    }
    s.push_str(rest);
    while let Some(a) = s.find("](") {
        let b = s[a..].find(')').map_or(s.len(), |b| a + b + 1);
        s.replace_range(a + 1..b, "");
    }
    s.split_inclusive(char::is_whitespace).filter(|w| !w.starts_with("http://") && !w.starts_with("https://")).map(|w| w.chars().count()).sum()
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
    if items.iter().any(|s| s.contains(['\r', '\n']) || s.trim().is_empty()) { Err(format!("{what}: one nonempty line each")) } else { Ok(()) }
}

fn task_text(cmd: &str, text: &str) -> Res<()> {
    if matches!(cmd, "requeue" | "later" | "failed") {
        if text.is_empty() && cmd != "failed" { return Ok(()); }
        let lines: Vec<String> = text.lines().filter(|l| !l.trim().is_empty()).map(String::from).collect();
        limit(cmd, &lines, 1, 2, 240)?;
        fields(text, &[("Reason", 1, 1, 240), ("Next", 0, 1, 240)])?;
    } else {
        let lines: Vec<String> = text.lines().filter(|l| !l.trim().is_empty()).map(String::from).collect();
        limit(cmd, &lines, 1, snapshot::ANSWER_LINES, snapshot::ANSWER_CHARS)?;
    }
    Ok(())
}

/// Labelled `--text` lines (`Label: ..`; an unlabelled line continues the field above) in `spec` order:
/// (label, min lines, max lines, characters).
fn fields(text: &str, spec: &[(&str, usize, usize, usize)]) -> Res<Vec<Vec<String>>> {
    let names = spec.iter().map(|s| format!("`{}:`", s.0)).collect::<Vec<_>>().join(", ");
    let mut out = vec![vec![]; spec.len()];
    let mut cur: Option<usize> = None;
    for l in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        match spec.iter().position(|s| l.strip_prefix(s.0).is_some_and(|r| r.starts_with(": "))) {
            Some(i) if cur.is_none_or(|c| i > c || (i == c && spec[i].0 == "Checks")) => cur = Some(i),
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
    one_line("--link", links)?;
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
    let lines: Vec<String> = text.lines().filter(|l| !l.trim().is_empty()).map(String::from).collect();
    limit("verdict", &lines, 3, 12, 240)?;
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

// ---------- report ----------

pub fn cell(s: &str) -> String {
    s.replace('\\', "\\\\").replace('|', "\\|").replace('[', "\\[").replace(']', "\\]").replace(['\r', '\n'], " ")
}

/// R6: compact display only; the issue link retains the complete stored text.
pub fn compact(s: &str, max: usize) -> String {
    let s = s.replace(['\r', '\n'], " ");
    if s.chars().count() > max { s.chars().take(max - 1).chain(['…']).collect() } else { s }
}

pub fn age(secs: i64) -> String {
    match secs {
        s if s < 120 => format!("{s}s"),
        s if s < 7200 => format!("{}m", s / 60),
        s if s < 172800 => format!("{}h", s / 3600),
        s => format!("{}d", s / 86400),
    }
}

#[cfg(test)]
fn report(b: &dyn Board, cfg: &Config, raw: &Value, only: &[String]) -> Res<String> {
    snapshot::read(b, cfg, raw, only).text(cfg, raw)
}

fn inbox(b: &dyn Board, n: u64) -> Res<String> {
    let (issue, t, _) = current(b, n)?;
    let mut out = vec![format!("#{n} {}\n{}", issue.title, b.url(n)), issue.body.trim().to_string(), format!("Folded: {}", serde_json::to_string(&t).unwrap_or_default())];
    let log = entries(b, n)?;
    out.extend([format!("Revision: {}", core::revision(n, &log)), "Log:".into()]);
    for e in log {
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
    let l = install.join(platform::tool(install).exe_name());
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
/// a crashed process is ignored and its attempts go `gone` (§ 4 restart). A malformed file is unknown: nothing is
/// adopted and the file stays (§ 4 Data).
fn kids_file(root: &Path) -> Vec<data::KidLine> {
    let path = dot(root).join("kids");
    let parsed = match fs::read_to_string(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return vec![],
        Err(e) => Err(format!("kids: unreadable ({e}); file kept")),
        Ok(t) => data::kids(&t),
    };
    match parsed {
        Err(e) => {
            eprintln!("{e}; its attempts go gone");
            vec![]
        }
        Ok((pid, kids)) => {
            let _ = fs::remove_file(&path);
            if pid == std::process::id() { kids } else { vec![] }
        }
    }
}

fn adopt(s: &mut Service, root: &Path) {
    let kids = kids_file(root);
    #[cfg(unix)]
    for (task, attempt, rt, pid, delivered) in kids {
        s.adopt(Kid { task, attempt, rt, proc: Box::new(runtime::Adopted(pid)) }, delivered);
    }
    #[cfg(not(unix))]
    let _ = (s, kids);
}

fn service_run(c: &Ctx) -> Res<()> {
    let cfg = &c.cfg;
    let exe = launcher(&cfg.install);
    let board_env = cfg.board.worker_env(&platform::tool(&cfg.install), &cfg.web_host)?;
    let env = runtime::Env { board_env: &board_env, root: &cfg.root, exe: &exe, permission_mode: &cfg.permission_mode, codex: &cfg.codex, publish: &cfg.publish };
    let start = |l: &runtime::Launch| runtime::spawn(&env, l);
    let mut s = Service::new(c.board.as_ref(), cfg, platform::VERSION, Box::new(start));
    s.instance = enrollment::installation_id(&cfg.install).map(|id| service_id(&id))?;
    let d = dot(&cfg.root);
    fs::create_dir_all(&d).map_err(|e| e.to_string())?;
    // § 4 one service per root: wait, not exit, so the unit sees no crash loop
    while let Some(pid) = other_service(&cfg.root) {
        eprintln!("service pid {pid} already runs this project; waiting");
        std::thread::sleep(Duration::from_secs(cfg.poll));
    }
    adopt(&mut s, &cfg.root);
    let is_installed = installed(&cfg.install);
    let began = Instant::now();
    eprintln!("{}", incident::disclosure(Some(&cfg.root)));
    let mut checked: Option<Instant> = None;
    let mut update: Option<String> = None;
    let mut restart_empty: Option<u64> = None;
    let _ = fs::remove_file(d.join("restart"));
    operator::recover(&cfg.root);
    // ponytail: the operator takes no slot of `limits`; count it when operators crowd out workers
    let mut tray = None;
    let project = project_name(&cfg.root);
    let issues = c.board.url(0).strip_suffix("/0").unwrap_or_default().to_string();
    let log = coreq::service::log_path(&platform::tool(&cfg.install), &project);
    eprintln!("taskq {} service on {} for {}", platform::VERSION, cfg.me, cfg.repo);
    let mut sampled: Option<Instant> = None;
    let mut agents: Option<(Instant, Value)> = None;
    let mut focus_error: Option<String> = None;
    let mut pruned: Option<Instant> = None;
    loop {
        // § 3: capacity reloads before each admission decision; children are untouched
        let cap = fs::read_to_string(cfg.root.join("taskq.json")).map_err(|e| e.to_string())
            .and_then(|t| serde_json::from_str::<Value>(&t).map_err(|e| format!("taskq.json: {e}")))
            .and_then(|v| service::capacity(&v, &cfg.me));
        if let Err(e) = &cap {
            eprintln!("capacity: {e}; launches held");
        }
        let op_rt = cap.as_ref().ok().and_then(|c| ["claude", "codex"].into_iter().find(|rt| c.limits.get(*rt).copied().unwrap_or(if c.auto { u32::MAX } else { 0 }) > 0));
        s.reload(cap);
        s.mono = began.elapsed().as_secs();
        s.wall = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        // § 4 Data: an unreadable focus admits nothing and reads unknown with its reason
        let focus = data::focus(&cfg.root);
        if focus.as_ref().err() != focus_error.as_ref() {
            if let Err(e) = &focus { eprintln!("{e}; nothing admitted"); }
            focus_error = focus.as_ref().err().cloned();
        }
        s.focus = focus.unwrap_or_default();
        let r = s.pass();
        s.sync_host();
        for l in s.log.drain(..) {
            eprintln!("{l}");
        }
        if let Err(e) = &r {
            eprintln!("pass: {e}");
        }
        let at = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let kids: Vec<Value> = s.kids.iter().map(|k| {
            let unknown = s.unknown.get(&k.attempt);
            json!({"task": k.task, "attempt": k.attempt, "rt": k.rt, "pid": k.proc.id(), "live": if unknown.is_some() { "unknown" } else { "running" }, "reason": unknown})
        }).collect();
        let request = fs::read_to_string(d.join("focus.request")).ok();
        // § 4: runtime probes at most every 5 minutes
        if agents.as_ref().is_none_or(|(t, _)| t.elapsed().as_secs() >= 300) {
            agents = Some((Instant::now(), json!({"at": at, "rows": runtime::agents()})));
        }
        let tool = platform::tool(&cfg.install);
        let error = r.as_ref().err().cloned();
        let mut status = coreq::status::common(&tool, &cfg.me, &project, at, r.err(), update.as_deref());
        // § 4 snapshot keys; before the first successful pass tasks are unknown, not an empty list
        match &s.snapshot {
            Some(snap) => for (k, v) in snap.status_keys(error.as_deref()) { status[k] = v; },
            None => { status["complete"] = json!(false); status["reason"] = json!(error.as_deref().unwrap_or("no board read yet")); status["tasks"] = json!([]); }
        }
        for (k, v) in [("focus", if focus_error.is_some() { Value::Null } else { json!(s.focus) }), ("focus_error", json!(focus_error)), ("kids", json!(kids)), ("issues", json!(issues)), ("log", json!(log)),
            ("tasks_age_seconds", json!(s.tasks_read.map(|t| t.elapsed().as_secs()))), ("request", json!(request)), ("events_since", json!(s.events_since)),
            ("capacity", s.capacity_json()), ("hold", json!(s.hold)), ("agents", json!(agents.as_ref().map(|a| &a.1))),
            ("incident", incident::status(&cfg.root)), ("incident_notice", json!(incident::notice(&cfg.root)))]
        {
            status[k] = v;
        }
        let _ = coreq::status::write(&tool, &cfg.root, &status);
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
            if is_installed {
                match platform::service_update(&cfg.install) {
                    Ok(Some(_)) => {
                        update = None;
                        reexec(&s, cfg)?;
                    }
                    Ok(None) => update = None,
                    Err(e) => eprintln!("update: {e}"),
                }
            } else {
                match platform::update(&cfg.install, true) {
                    Ok(found) => update = found,
                    Err(e) => eprintln!("update: {e}"),
                }
            }
            if asked {
                eprintln!("update requested: {}", update.as_deref().map_or("up to date".into(), |v| format!("{v} found")));
            }
        }
        restart_watch(&mut restart_empty, cfg!(windows) && d.join("restart").exists() && s.kids.is_empty(), began.elapsed().as_secs(), cfg.update_every);
        if is_installed && d.join("restart").exists() {
            // Windows re-exec waits for the live children; the request stays until it can go
            if !operator::running(&cfg.root) && ((cfg!(unix) && !s.kids.iter().any(|k| k.rt == "codex-app")) || s.kids.is_empty()) {
                let _ = fs::remove_file(d.join("restart"));
                eprintln!("restart requested");
            }
            reexec(&s, cfg)?;
        }
        // § 4 Data: the pruners run hourly
        if pruned.is_none_or(|t| t.elapsed().as_secs() >= 3600) {
            pruned = Some(Instant::now());
            let live: Vec<u64> = s.kids.iter().map(|k| k.attempt).collect();
            for l in data::prune(&cfg.root, &cfg.install, &project, &live) {
                eprintln!("{l}");
            }
        }
        operator::owner_comments(&cfg.root, &project, &issues, &s.owner_comments);
        operator::step(&cfg.root, &cfg.install, &exe, op_rt);
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
                s.gate.host_mem = platform::host_memory();
                s.gate.observe(s.mono, wall as i64, platform::cpu_ticks(), platform::memory_available());
            }
            s.wall = wall as i64;
            s.sync_host();
            status["tasks_age_seconds"] = json!(s.tasks_read.map(|t| t.elapsed().as_secs()));
            status["capacity"] = s.capacity_json();
            status["at"] = json!(wall);
            let _ = coreq::status::write(&tool, &cfg.root, &status);
        };
        tick();
        sleep_until_poke(&d.join("poke"), cfg.poll, &mut tick);
    }
}

// ---------- capacity (§ 3, § 5 `service capacity`, § 9) ----------

/// One owner edit of a host's capacity: runtime limits and load gate fields; `None` keeps a field.
#[derive(Default, Clone)]
struct CapEdit {
    /// `admission.mode` auto (true) or fixed (false), § 9
    mode: Option<bool>,
    /// `admission.max`: `Some(None)` removes it
    max: Option<Option<u32>>,
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
        e.mode = match a.one("mode") {
            None => None,
            Some("auto") => Some(true),
            Some("fixed") => Some(false),
            Some(x) => return Err(format!("--mode: auto or fixed, not {x}")),
        };
        e.max = match a.one("max") {
            None => None,
            Some("none") => Some(None),
            Some(_) => Some(num("max")?),
        };
        e.gate = match a.one("load-gate") {
            None => None,
            Some("on") => Some(true),
            Some("off") => Some(false),
            Some(x) => return Err(format!("--load-gate: on or off, not {x}")),
        };
        Ok(e)
    }

    fn is_empty(&self) -> bool {
        self.mode.is_none() && self.max.is_none() && self.limits.is_empty() && self.gate.is_none() && self.cpu_max.is_none() && self.memory_min.is_none()
    }
}

use service::RUNTIMES;

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
        return Err("nothing to change: give --mode, --max, --claude, --codex, --codex-app, --load-gate, --cpu-max or --memory-min".into());
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
    if e.mode.is_some() || e.max.is_some() {
        // the host's object replaces the top-level one (§ 9): carry the unedited field over
        h["admission"] = json!({"mode": if e.mode.unwrap_or(old.auto) { "auto" } else { "fixed" }});
        if let Some(m) = e.max.unwrap_or(old.max) {
            h["admission"]["max"] = json!(m);
        }
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
    let mode = |a: bool| if a { "Auto" } else { "Fixed" };
    if old.auto != new.auto {
        out.push(format!("Mode {} → {}", mode(old.auto), mode(new.auto)));
    }
    if old.max != new.max {
        let m = |x: Option<u32>| x.map_or("none".into(), |x| x.to_string());
        out.push(format!("Maximum {} → {}", m(old.max), m(new.max)));
    }
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
        Some("held") => format!("Waiting: {}", reasons(&["cpu", "memory", "cooldown", "max", "spacing", "version"]).into_iter().map(|r| match r["code"].as_str() { Some("cpu") => "CPU", Some("memory") => "memory", Some("max") => "host maximum", Some("spacing") => "host spacing", Some("version") => "old service", _ => "recovery" }).collect::<Vec<_>>().join(", ")),
        Some(_) => format!("Unknown: {}", reasons(&["sample", "config", "host"]).into_iter().filter_map(|r| r["text"].as_str()).collect::<Vec<_>>().join("; ")),
        None => "Unknown: service has no capacity status".into(),
    }
}

/// The admission line of a `status.json` capacity (§ 3): mode, host ceiling and use, own reservations.
fn admission_text(c: &Value) -> String {
    let n = |v: &Value| v.as_u64().map_or("unknown".into(), |n| n.to_string());
    let a = &c["admission"];
    let mode = match a["mode"].as_str() {
        Some("auto") => format!("Auto · host {} / {} held", n(&c["host_used"]), a["max"].as_u64().map_or("no max".into(), |m| m.to_string())),
        Some(_) => "Fixed".into(),
        None => "unknown (service older than this release)".into(),
    };
    let res: Vec<String> = c["reservations"].as_array().into_iter().flatten().map(|r| format!("#{} {}", r["task"], r["state"].as_str().unwrap_or("?"))).collect();
    format!("Admission: {mode}{}", if res.is_empty() { String::new() } else { format!(" · {}", res.join(", ")) })
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
            (Some(c), _) => {
                println!("Load gate: {}", gate_text(c));
                println!("{}", admission_text(c));
            }
            (None, Err(e)) => println!("Unknown: {e}"),
            (None, Ok(_)) if host != cfg.me => println!("Unknown: no local snapshot of {host}"),
            (None, Ok(_)) => println!("Unknown: service has no fresh capacity status"),
        }
        if let (Some(c), Ok(s)) = (&c, &saved)
            && (c["limits"] != s.json()["limits"] || c["load_gate"] != s.json()["load_gate"] || c["admission"] != s.json()["admission"])
        {
            println!("Saved config differs: {}", cap_changes(&service::Capacity { limits: serde_json::from_value(c["limits"].clone()).unwrap_or_default(), ..s.clone() }, s));
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
            && st["capacity"]["admission"] == saved.json()["admission"]
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
    let t = platform::tool(install);
    let Some(v) = t.current() else { return };
    let mut exe = t.release_exe(&v);
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
fn pause(root: &Path) -> Res<()> {
    let d = dot(root);
    let f = data::focus(root)?.join(" ");
    if !f.is_empty() {
        fs::write(d.join("focus.paused"), f).map_err(|e| e.to_string())?;
    }
    fs::write(d.join("focus"), "off").map_err(|e| e.to_string())?;
    poke(root);
    Ok(())
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

/// `service install` hands over instead of reloading the unit (§ 8 step 5) while this project's fresh `status.json` lists
/// live children or is unreadable (unknown keeps them, § 4 Data).
fn handover(root: &Path) -> bool {
    if operator::running(root) { return true; }
    let p = dot(root).join("status.json");
    file_age(&p).is_some_and(|a| a <= 60) && data::json(&p).map_or(true, |s| s.is_some_and(|s| s["kids"].as_array().is_some_and(|k| !k.is_empty())))
}

/// § 8 step 5: live children pass to the new image through the step 3 re-exec; a unit reload would end them. True
/// when the running service was asked to re-exec instead of the unit being reloaded.
fn load_unit(u: &platform::Unit, root: &Path) -> Res<bool> {
    if handover(root) {
        fs::write(dot(root).join("restart"), "").map_err(|e| e.to_string())?;
        poke(root);
        return Ok(true);
    }
    platform::run_all(&u.unload, true)?;
    kill_service(root); // Windows: `/End` left the old service running beside the new one (#933)
    platform::run_all(&u.load, false)?;
    Ok(false)
}

/// The heartbeat's `service <id>` (§ 4): public, so a hash of the installation id, never the id itself.
fn service_id(installation: &str) -> String {
    let d = ring::digest::digest(&ring::digest::SHA256, installation.as_bytes());
    d.as_ref()[..6].iter().map(|b| format!("{b:02x}")).collect()
}

/// Another live process that wrote this root's fresh `status.json` (§ 4 one service per root).
fn other_service(root: &Path) -> Option<u32> {
    let p = dot(root).join("status.json");
    if file_age(&p).is_none_or(|a| a > 60) {
        return None;
    }
    let pid = data::json(&p).ok()??["pid"].as_u64()? as u32;
    (pid != std::process::id() && runtime::alive(pid)).then_some(pid)
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
    // App turns have no adoptable child; keep their lease until completion.
    if s.kids.iter().any(|k| k.rt == "codex-app") || operator::running(&cfg.root) {
        return fs::write(dot(&cfg.root).join("restart"), "").map_err(|e| e.to_string());
    }
    let mut text = format!("pid {}\n", std::process::id());
    for k in &s.kids {
        text += &format!("{} {} {} {} {}\n", k.task, k.attempt, k.rt, k.proc.id(), s.delivered(k.attempt));
    }
    platform::write_atomic(&dot(&cfg.root).join("kids"), &text)?;
    let e = std::process::Command::new(cfg.install.join(platform::tool(&cfg.install).exe_name())).args(["service", "run"]).exec();
    incident::detect("dispatch.reexec", concat!("src/main.rs:", line!()), "none", &[], &["reexec:failed"]);
    Err(format!("re-exec: {e}"))
}

#[cfg(not(unix))]
fn reexec(s: &Service, cfg: &Config) -> Res<()> {
    // ponytail: Windows cannot hand children to a new image; exit once no child is live and the unit restarts us.
    // The new `current` is already written, so the update waits only for the running attempts.
    if s.kids.is_empty() && !operator::running(&cfg.root) {
        std::process::exit(75);
    }
    // later update checks see no newer version: the restart request carries the re-exec until the children end
    fs::write(dot(&cfg.root).join("restart"), "").map_err(|e| e.to_string())
}

fn service_fields(root: &Path) -> String {
    let d = dot(root);
    let mut out = vec![format!("Project: `{}`", root.display())];
    match data::focus(root) {
        _ if !d.join("focus").exists() => out.push("Focus: unknown (focus file unavailable)".into()),
        Ok(f) if f.is_empty() => out.push("Focus: off".into()),
        Ok(f) => out.extend(f.iter().map(|l| format!("Focus: {l}"))),
        Err(e) => out.push(format!("Focus: unknown ({e})")),
    }
    let status = data::json(&d.join("status.json"));
    if let Err(e) = &status {
        out.push(format!("Status: unknown ({e})"));
    }
    let status = status.ok().flatten();
    let ttl = load(root).map(|(c, _)| c.ttl.max(0) as u64).unwrap_or(300);
    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).ok().map(|d| d.as_secs());
    let fresh = status.as_ref().and_then(|s| {
        s["tasks_age_seconds"].as_u64()?.checked_add(now?.checked_sub(s["at"].as_u64()?)?)
    }).filter(|a| *a <= ttl);
    match (status, fresh) {
        (Some(s), Some(a)) if snapshot::complete(&s) => {
            out.push(snapshot::counts_line(&snapshot::status_totals(&s).unwrap()));
            out.push(format!("Freshness: {} (cached status)", age(a as i64)));
            out.push("Service: unknown (process not checked)".into());
        }
        _ => out.push("Service: unknown (status missing, stale or failed)\nFreshness: unknown (no fresh successful status)\nCounts: unknown (no fresh successful read)".into()),
    }
    out.join("\n")
}

fn service(args: &Args) -> Res<()> {
    let sub = args.pos.get(1).map(String::as_str).unwrap_or("");
    let projects = platform::default_install().join("projects");
    if sub == "list" {
        // one project root per line, written by `service install` (§ 5); a PM or ARM covers several projects from it
        for r in fs::read_to_string(&projects).map_err(|e| format!("{}: {e}", projects.display()))?.lines().filter(|l| !l.is_empty()) {
            println!("{}", service_fields(Path::new(r)));
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
                println!("{}", service_fields(&root));
                for h in &c.cfg.has { println!("Has: {h}"); }
                for (l, k) in n { println!("Label: {l} ({k})"); }
                return Ok(());
            }
            set_focus(&root, &f, args.one("request"))?;
            println!("{}", service_fields(&root));
            Ok(())
        }
        "pause" => {
            fs::create_dir_all(&d).map_err(|e| e.to_string())?;
            pause(&root)?;
            println!("Paused {}", project_name(&root));
            Ok(())
        }
        "resume" => {
            resume(&root)?;
            println!("{}", service_fields(&root));
            Ok(())
        }
        "capacity" => capacity_cmd(&root, args),
        "hold" | "release" => hold_cmd(sub == "hold", args.one("text")),
        "recover" => recover_cmd(&root, args.flags.contains_key("apply"), args.flags.contains_key("json")),
        "restart" => {
            fs::create_dir_all(&d).map_err(|e| e.to_string())?;
            fs::write(d.join("restart"), "").map_err(|e| e.to_string())?;
            poke(&root);
            println!("Restart requested for {}", project_name(&root));
            Ok(())
        }
        "status" => {
            let text = fs::read_to_string(d.join("status.json")).map_err(|_| "no service status: is `taskq service run` running here?".to_string())?;
            println!("{text}");
            let status = data::json(&d.join("status.json"))?.unwrap_or_default();
            println!("Update: {}", status["update"].as_str().unwrap_or("unknown"));
            println!("{}", incident::line(&incident::status(&root)));
            if status["incident_notice"] == true || incident::notice(&root) { println!("Incident reports: notice"); }
            Ok(())
        }
        "install" | "uninstall" | "stop" => {
            let (cfg, _) = load(&root)?;
            let u = platform::unit(&platform::tool(&cfg.install), &root, &project_name(&root));
            if sub == "stop" {
                platform::run_all(&u.stop, false)?;
                kill_service(&root);
                println!("Stop requested for {}; starts again at the next logon", project_name(&root));
                return Ok(());
            }
            if sub == "install" {
                coreq::update::install_self(&platform::tool(&cfg.install))?;
                platform::write_unit(&u)?;
                let handover = load_unit(&u, &root)?;
                let mut all: Vec<String> = fs::read_to_string(&projects).unwrap_or_default().lines().map(String::from).collect();
                if !all.contains(&root.display().to_string()) {
                    all.push(root.display().to_string());
                    fs::write(&projects, all.join("\n") + "\n").map_err(|e| e.to_string())?;
                }
                println!("installed {}{}", u.path.display(), if handover { "; live children kept: the running service re-execs" } else { "" });
            } else {
                uninstall_unit(&u, &root, &projects)?;
                println!("removed {}", u.path.display());
                kill_service(&root);
            }
            Ok(())
        }
        _ => Err("usage: taskq service install|run|focus|pause|resume|capacity|hold|release|recover|status|restart|stop|uninstall|list".into()),
    }
}

/// § 4 emergency hold: write or remove `<install>/hold`, then poke every registered project so its service reads it.
fn hold_cmd(on: bool, reason: Option<&str>) -> Res<()> {
    let install = platform::default_install();
    let path = install.join("hold");
    if on {
        let reason = reason.unwrap_or("emergency hold");
        one_line("--text", &[reason.to_string()])?;
        platform::write_atomic(&path, &format!("{reason}\n"))?;
    } else if let Err(e) = fs::remove_file(&path).or_else(|e| if e.kind() == std::io::ErrorKind::NotFound { Ok(()) } else { Err(e) }) {
        return Err(format!("hold file: {e}; still held"));
    }
    for r in fs::read_to_string(install.join("projects")).unwrap_or_default().lines().filter(|l| !l.is_empty()) {
        poke(Path::new(r));
    }
    if !on {
        println!("Hold released on {}; launches follow each project's focus", hostname());
        return Ok(());
    }
    println!("Hold on {}: new launches refused until `taskq service release`; live children continue", hostname());
    for o in admission::old_services(&install) {
        println!("Not enforced: {o} is older than {}", admission::MIN_SERVICE);
    }
    Ok(())
}

/// § 4 recovery: list this host's interrupted attempts and their fencing ops; `--apply` writes them through the
/// dispatcher, one receipt each. Only under a hold.
fn recover_cmd(root: &Path, apply: bool, json: bool) -> Res<()> {
    let c = ctx()?;
    let hold = admission::hold(&c.cfg.install).ok_or("no emergency hold on this host; run `taskq service hold` first")?;
    // the service's children, from a status.json at most 60 s old (§ 4); else every row is unknown
    let p = dot(root).join("status.json");
    let kids = match (data::json(&p), file_age(&p)) {
        (Ok(Some(s)), Some(a)) if a <= 60 => Ok(s["kids"].as_array().cloned().unwrap_or_default()),
        (Err(e), _) => Err(e),
        (Ok(Some(_)), Some(a)) => Err(format!("status.json {a} s old")),
        _ => Err("no service status".to_string()),
    };
    let rows = dispatch::proposal(&*c.board, &c.cfg.me, &kids)?;
    println!("Hold: {hold}");
    if rows.is_empty() {
        println!("No interrupted attempts on {}", c.cfg.me);
    }
    for r in &rows {
        println!("{}", r.text());
    }
    if !apply {
        return Ok(());
    }
    emit(&dispatch::recover(&*c.board, root, &rows), json)
}

#[cfg(any(target_os = "macos", windows))]
fn tray(install: PathBuf, snapshot: Option<&str>, icon: Option<&str>) -> Res<()> {
    if let Some(f) = icon {
        let draft: serde_json::Value = serde_json::from_str(&fs::read_to_string(f).map_err(|e| format!("{f}: {e}"))?).map_err(|e| format!("{f}: {e}"))?;
        tray::use_icon(&install, &draft).map_err(|e| format!("{f}: {}; tray.json unchanged", e.join("; ")))?;
        println!("custom icon saved: {}", install.join("tray.json").display());
        return Ok(());
    }
    tray::run(install, snapshot.map(PathBuf::from))
}

#[cfg(not(any(target_os = "macos", windows)))]
fn tray(_install: PathBuf, _snapshot: Option<&str>, _icon: Option<&str>) -> Res<()> {
    Err("taskq tray: macOS and Windows only".into())
}

// ---------- commands ----------

/// § 5 receipts: JSON lines with `--json`, else the text lines; any receipt not accepted fails after all are shown.
fn emit(rs: &[Receipt], json: bool) -> Res<()> {
    for r in rs {
        match (json, r.ok()) {
            (true, _) => println!("{}", r.json()),
            (false, true) => println!("{}", r.text()),
            (false, false) => eprintln!("{}", r.text()),
        }
    }
    if rs.iter().all(Receipt::ok) { Ok(()) } else { Err(String::new()) } // already shown: exit 1 quietly
}

/// An accepted `ask`, `answer` or `requeue` goes to the request log (§ 4), then the receipt is shown.
fn logged(root: &Path, source: &str, text: &str, r: Receipt, json: bool) -> Res<()> {
    if r.ok() {
        retro::record(root, source, r.task, text);
    }
    emit(&[r], json)
}

fn uninstall_unit(u: &platform::Unit, root: &Path, projects: &Path) -> Res<()> {
    let registered = fs::read_to_string(projects).map_err(|e| format!("{}: {e}", projects.display()))?;
    let all: Vec<String> = registered.lines().filter(|l| *l != root.display().to_string()).map(|l| format!("{l}\n")).collect();
    platform::run_all(&u.unload, false)?;
    if let Err(e) = fs::remove_file(&u.path) && e.kind() != std::io::ErrorKind::NotFound { return Err(e.to_string()); }
    fs::write(projects, all.concat()).map_err(|e| e.to_string())
}

fn golden_selftest(traces: &str, published: bool) -> Res<usize> {
    let result = core::golden(traces);
    if published && result.is_err() {
        incident::detect("invariant.golden", concat!("src/main.rs:", line!()), "none", &["fold.json"], &["golden:mismatch"]);
    }
    result
}

fn restart_watch(since: &mut Option<u64>, empty_restart: bool, elapsed: u64, interval: u64) {
    if !empty_restart { *since = None; return; }
    let start = *since.get_or_insert(elapsed);
    if elapsed.saturating_sub(start) >= interval.saturating_mul(2) {
        incident::detect("dispatch.reexec", concat!("src/main.rs:", line!()), "none", &[], &["restart:stuck"]);
        *since = Some(u64::MAX);
    }
}

fn run(argv: &[String]) -> Res<()> {
    let a = Args::parse(argv)?;
    if a.pos.is_empty() || a.pos.first().is_some_and(|s| s == "help") || a.pos.iter().any(|s| s == "-h") || a.flags.contains_key("help") {
        println!("{USAGE}");
        return Ok(());
    }
    let cmd = a.pos.first().map(String::as_str).unwrap_or("");
    match cmd {
        "selftest" => {
            let n = golden_selftest(core::GOLDEN, !platform::VERSION.contains('-')).map_err(|e| format!("fold: FAIL {e}"))?;
            println!("fold: {n} golden traces PASS");
            println!("{}", formats_selftest()?);
            if std::env::var_os("TASKQ_SELFTEST_OFFLINE").is_some() {
                println!("board: unknown (TASKQ_SELFTEST_OFFLINE set; no board read)");
            } else if let Ok(root) = find_root() {
                let c = ctx()?;
                let open = c.board.issues(None)?.len();
                println!("board: {} as {} on {}", c.cfg.repo, c.board.me()?, c.cfg.web_host);
                println!("open issues: {open}");
                println!("host: {}", c.cfg.me);
                println!("root: `{}`", root.display());
            } else {
                println!("board: unknown (no project config found; no board read)");
            }
            Ok(())
        }
        "update" => {
            let install = platform::default_install();
            let check = a.flags.contains_key("check");
            println!("{}", incident::disclosure(find_root().ok().as_deref()));
            match platform::update(&install, check)? {
                Some(v) if check => println!("update available: {} -> {v}", platform::tool(&install).current().unwrap_or_else(|| platform::VERSION.into())),
                Some(v) => println!("updated to {v}; the service restarts on its next update check"),
                None => println!("up to date ({})", platform::tool(&install).current().unwrap_or_else(|| platform::VERSION.into())),
            }
            Ok(())
        }
        // § 5: read-only probes on this host, no board
        "agents" => {
            for r in runtime::agents().as_array().into_iter().flatten() {
                let s = |k: &str, none: &str| r[k].as_str().unwrap_or(none).to_string();
                let login = match (s("login", "unknown").as_str(), r["account"].as_str()) {
                    ("logged in", Some(a)) => format!("logged in as {a}"),
                    ("logged in", None) => "logged in, account unknown".into(),
                    (l, _) => l.to_string(),
                };
                println!("{}  {}  {}  {login}", s("runtime", "?"), s("path", "not installed"), if r["path"].is_null() { "-".into() } else { s("version", "unknown") });
            }
            Ok(())
        }
        "service" => service(&a),
        // § 5: the operator over the request log; no board
        "retro" => {
            let (cfg, raw) = load(&find_root()?)?;
            let cap = service::capacity(&raw, &cfg.me)?;
            let rt = ["claude", "codex"].into_iter().find(|rt| cap.limits.get(*rt).copied().unwrap_or(if cap.auto { u32::MAX } else { 0 }) > 0);
            print!("{}", retro::run(&cfg.root, rt)?);
            Ok(())
        }
        "tray" => tray(platform::default_install(), a.one("snapshot"), a.one("icon")),
        "" => Err("command missing; use `taskq help`".into()),
        _ => command(&ctx()?, cmd, &a),
    }
}

fn command(c: &Ctx, cmd: &str, a: &Args) -> Res<()> {
    let (b, cfg, root) = (c.board.as_ref(), &c.cfg, c.cfg.root.as_path());
    let text = a.one("text").unwrap_or("");
    if matches!(cmd, "requeue" | "later" | "failed" | "steer") || (cmd == "answer" && !a.pos.get(1).is_some_and(|p| p.contains('.'))) {
        task_text(cmd, text)?;
    }
    if cmd == "add" && a.flags.contains_key("later") { task_text("later", text)?; }
    let revision = a.one("revision").map(|r| r.parse::<u64>().map_err(|_| format!("bad --revision {r}"))).transpose()?;
    let op = Op { id: a.one("op-id").map(String::from), revision };
    if op.id.as_deref().is_some_and(|i| i.is_empty() || i.contains(char::is_whitespace)) {
        return Err("--op-id: one word".into());
    }
    let json = a.flags.contains_key("json");
    if revision.is_some() && (cmd == "close" && a.pos.len() > 2 || cmd == "answer" && a.pos.len() > 2) {
        return Err("--revision names one task".into());
    }
    match cmd {
        "labels" => {
            let mut names = labels::FIXED.iter().map(|s| s.to_string()).collect::<Vec<_>>();
            names.extend(b.issues(None)?.into_iter().flat_map(|i| i.labels));
            b.ensure_labels(&names)?;
            println!("Label catalog verified for {}", project_name(root));
            Ok(())
        }
        "add" => {
            // § 1 task body: Goal, Acceptance, Scope, dependencies; limits checked before any write
            // § 5 `--retro K`: title, goal and acceptance of the last retro report's recommendation K
            let picked = a.one("retro").map(|k| retro::pick(root, k)).transpose()?;
            if picked.is_some() && (a.pos.len() > 1 || a.flags.contains_key("goal") || a.flags.contains_key("acceptance")) {
                return Err("--retro K takes title, goal and acceptance from the report; drop them".into());
            }
            let title = match &picked { Some((t, _, _)) => t, None => a.pos.get(1).filter(|t| !t.trim().is_empty()).ok_or("title missing")? };
            if title.contains(['\r', '\n']) || title.chars().count() > 80 {
                return Err("title: one line, at most 80 characters; shorten it".into());
            }
            let goal: Vec<String> = match &picked { Some((_, g, _)) => g.as_str(), None => a.need("goal")? }.lines().map(str::trim).filter(|l| !l.is_empty()).map(String::from).collect();
            limit("Goal", &goal, 1, 2, 240)?;
            let (acc, scope) = (picked.as_ref().map_or_else(|| a.all("acceptance"), |p| p.2.clone()), a.all("scope"));
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
            for (flag, allowed) in [("type", &["code", "docs", "research", "asset"][..]), ("runtime", &["codex", "claude", "codex-app"][..]), ("priority", &["0", "1", "2"][..])] {
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
            let hold = a.flags.contains_key("hold");
            if hold && a.flags.contains_key("later") {
                return Err("--hold and --later exclude each other".into());
            }
            // § 1: an open issue carrying this op id is the first create; return its receipt, write nothing
            if let Some(id) = &op.id {
                body += &format!("\nop-id: {id}\n");
                if let Some(i) = b.issues(None)?.into_iter().find(|i| i.trusted && i.body.trim_end().ends_with(&format!("\nop-id: {id}"))) {
                    let log = entries(b, i.number)?;
                    let t = fold(i.number, &log, deps_open(b, &i.body)?);
                    let state = if service::is_held(&i) { State::Held } else { t.state };
                    return emit(&[Receipt::accepted(i.number, &op, state, core::revision(i.number, &log))], json);
                }
            }
            let blocked = deps_open(b, &body)?;
            // a held task gets `q-held` instead of `taskq`: no service of any version admits it before `start`
            let mut labels = if hold { vec![b.state_label("held")?] } else { vec![service::TASK_LABEL.to_string(), b.state_label(if blocked { "waiting" } else { "ready" })?] };
            for (flag, prefix) in [("type", ""), ("runtime", "run-"), ("priority", if b.canonical_labels()? { "p" } else { "priority-" }), ("host", "host-")] {
                if let Some(v) = a.one(flag) {
                    labels.push(format!("{prefix}{v}"));
                }
            }
            labels::validate(&labels)?;
            let n = b.create(title, &body, &labels)?;
            let r = match (hold, a.flags.contains_key("later")) {
                (true, _) => act(b, root, n, "hold", &[], "", &Op::default()),
                (_, true) => act(b, root, n, "later", &[], text, &Op::default()),
                _ => Receipt::accepted(n, &op, fold(n, &[], blocked).state, 0),
            };
            poke(root);
            emit(&[Receipt { op_id: op.id.clone(), ..r }], json)
        }
        "start" => {
            let n = a.num(1)?;
            let r = act(b, root, n, "start", &[], "", &op);
            // § 1: `taskq` goes on after the `start` entry; a retry adds the label a failed write left off
            let issue = b.issue(n)?;
            let started = entries(b, n)?.iter().any(|e| !e.edited && e.body.starts_with("taskq: start"));
            if started && issue.open && !service::is_task(&issue) {
                b.labels(n, &[service::TASK_LABEL.into()], &[])?;
                let (_, t, _) = current(b, n)?;
                return emit(&[Receipt::accepted(n, &op, t.state, core::revision(n, &entries(b, n)?))], json);
            }
            emit(&[r], json)
        }
        "ask" => {
            let n = a.num(1)?;
            let body = ask_card(n, a.need("text")?, &a.all("option"), a.one("recommend"), &a.all("link"))?;
            logged(root, "ask", &body, act(b, root, n, "ask", &[], &body, &op), json)
        }
        "answer" if a.pos.get(1).is_some_and(|p| p.contains('.')) => {
            let rs = answer_picks(b, cfg, &a.pos[1..], &op)?;
            for (r, pick) in rs.iter().zip(&a.pos[1..]) {
                if r.ok() { retro::record(root, "answer", r.task, pick); }
            }
            emit(&rs, json)
        }
        "answer" => {
            let n = a.num(1)?;
            logged(root, "answer", a.need("text")?, act(b, root, n, "answer", &[], a.need("text")?, &op), json)
        }
        "result" => {
            let n = a.num(1)?;
            let sha = a.need("sha")?;
            let body = result_text(n, text, &a.all("checks"), &a.all("option"), a.one("recommend"), &a.all("link"))?;
            emit(&[act(b, root, n, "result", &[sha], &body, &op)], json)
        }
        "requeue" => {
            let n = a.num(1)?;
            logged(root, "requeue", text, act(b, root, n, cmd, &[], text, &op), json)
        }
        "later" => {
            let n = a.num(1)?;
            emit(&[act(b, root, n, cmd, &[], text, &op)], json)
        }
        "steer" | "failed" => {
            let n = a.num(1)?;
            emit(&[act(b, root, n, cmd, &[], a.need("text")?, &op)], json)
        }
        "close" => {
            let mut out = vec![];
            for i in 1..a.pos.len() {
                let n = a.num(i)?;
                // § 7: a reviewed result closes with the verdict template; later/failed tasks and bare closes keep free text
                let checked = match current(b, n) {
                    Ok((issue, t, _)) if t.state == State::Review && !text.is_empty() => close_verdict(text, &issue.body).and_then(|_| {
                        if t.result.as_deref().is_some_and(|sha| text.lines().any(|l| l.starts_with("Checks:") && l.contains(sha))) { Ok(()) } else { Err("Checks: name the exact result SHA with the CI outcome".into()) }
                    }),
                    Ok(_) => Ok(()),
                    Err(e) => Err(e),
                };
                out.push(match checked {
                    Ok(()) => close_one(b, cfg, n, text, &op),
                    Err(e) => Receipt::rejected(n, &op, e),
                });
            }
            emit(&out, json)
        }
        "status" => {
            let snap = snapshot::read(b, cfg, &c.raw, &a.all("only").iter().flat_map(|x| x.split(',')).map(String::from).collect::<Vec<_>>());
            if a.flags.contains_key("json") {
                println!("{}", serde_json::to_string_pretty(&snap.json()).map_err(|e| e.to_string())?);
            } else {
                println!("{}", snap.text(cfg, &c.raw)?);
            }
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
        _ => Err(format!("unknown command `{cmd}`; use `taskq help`")),
    }
}

fn main() {
    incident::install_panic_hook();
    let args: Vec<String> = std::env::args().skip(1).collect();
    // `<install>/taskq` is the launcher: it runs the current release (coreq.md § Launcher).
    let install = platform::default_install();
    let exe = std::env::current_exe().ok().and_then(|e| e.canonicalize().ok());
    let t = platform::tool(&install);
    let launcher = install.join(t.exe_name()).canonicalize().ok();
    if exe.is_some() && exe == launcher && t.current().is_some() {
        coreq::launcher::launch(&t, &args);
    }
    if let Err(e) = run(&args) {
        if e.is_empty() {
            std::process::exit(1);
        }
        platform::die(&e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use board::Fake;

    // Chain: § 5 capacity edit. Risk: an invalid edit replaces the file.
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
        let changed = edit_capacity(text, "win", "mac", &e, None).unwrap();
        assert!(edit_capacity(&changed, "win", "mac", &e, Some(&old)).is_err(), "lost receipt retries cannot overwrite a concurrent config change");
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

    // Chain: § 5 capacity --mode/--max -> § 9 admission. Risk: a mode edit drops the inherited maximum, `none`
    // leaves it, or Auto saves with the gate off.
    #[test]
    fn capacity_edit_writes_mode_and_maximum() {
        let text = r#"{"admission": {"mode": "auto", "max": 6}, "limits": {"claude": 0}, "hosts": {"a": "win"}}"#;
        let get = |out: &str, h: &str| service::capacity(&serde_json::from_str(out).unwrap(), h).unwrap();
        let fixed = edit_capacity(text, "mac", "mac", &CapEdit { mode: Some(false), ..Default::default() }, None).unwrap();
        let (c, v): (_, Value) = (get(&fixed, "mac"), serde_json::from_str(&fixed).unwrap());
        assert!(!c.auto && c.max == Some(6), "inherited maximum carried over");
        assert_eq!(v["admission"], json!({"mode": "auto", "max": 6}), "top level untouched");
        assert!(get(&fixed, "win").auto, "other hosts keep theirs");
        assert_eq!(c.limits["claude"], 0, "explicit zero kept");
        let none = edit_capacity(text, "mac", "mac", &CapEdit { max: Some(None), ..Default::default() }, None).unwrap();
        assert!(get(&none, "mac").auto && get(&none, "mac").max.is_none());
        assert_eq!(cap_changes(&get(text, "mac"), &get(&none, "mac")), "Maximum 6 → none");
        assert_eq!(cap_changes(&get(text, "mac"), &c), "Mode Auto → Fixed, Load gate on (CPU 90 %, memory 10 %) → off");
        let off = r#"{"load_gate": {"enabled": false}}"#;
        let r = edit_capacity(off, "mac", "mac", &CapEdit { mode: Some(true), ..Default::default() }, None);
        assert!(r.is_err_and(|x| x.contains("needs the load gate")), "Auto with the gate off is refused");
    }

    fn cfg() -> Config {
        Config {
            root: std::env::temp_dir().join("taskq-main-test"),
            install: PathBuf::from("/nonexistent"),
            board: &board::github::KIND,
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

    // Chain: § 5 CLI arguments. Risk: stray words silently dropped from a command.
    #[test]
    fn unused_positionals_are_rejected() {
        let parse = |v: &[&str]| Args::parse(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(parse(&["add", "T", "--goal", "G", "--acceptance", "A", "B", "C"]).err().unwrap(), "unused positional arguments: B C");
        assert_eq!(parse(&["inbox", "1", "extra"]).err().unwrap(), "unused positional arguments: extra");
        assert_eq!(parse(&["selftest", "extra"]).err().unwrap(), "unused positional arguments: extra");
        assert_eq!(parse(&["help", "extra"]).err().unwrap(), "unused positional arguments: extra");
        assert!(parse(&["help"]).is_ok());
        assert_eq!(parse(&["service", "status", "extra"]).err().unwrap(), "unused positional arguments: extra");
        assert_eq!(parse(&["add", "T", "--goal", "G", "--acceptance", "A", "--acceptance", "B"]).unwrap().all("acceptance"), vec!["A", "B"]);
        for v in [vec!["close", "1", "2"], vec!["answer", "1.1", "2.1"], vec!["service", "focus", "code", "docs"]] {
            assert!(parse(&v).is_ok());
        }
    }

    // Chain: § 5 taskq add. Risk: an invalid task is written to the board.
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
        command(&c, "add", &args(&["--runtime", "codex-app"])).unwrap();
        assert!(c.board.issues(None).unwrap().pop().unwrap().labels.contains(&"run-codex-app".into()));
        command(&c, "add", &args(&["--priority", "0"])).unwrap();
        assert_eq!(c.board.issues(None).unwrap().pop().unwrap().labels, vec!["taskq", "q-ready", "p0"]);
        for extra in [&["--priority", "3"][..], &["--runtime", "any"], &["--type", "code", "--type", "docs"], &["--host", "win,mac"]] {
            assert!(command(&c, "add", &args(extra)).is_err());
        }
        assert_eq!(c.board.issues(None).unwrap().len(), 4);
    }

    // Chain: § 1 task body. Risk: over-limit text or a wrong body is written.
    #[test]
    fn held_create_is_idempotent_and_starts_only_on_start() {
        let c = Ctx { cfg: cfg(), raw: json!({}), board: Box::new(Fake::default()) };
        let b = c.board.as_ref();
        let run = |v: &[&str]| command(&c, v[0], &Args::parse(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>()).unwrap());
        let item = ["add", "T", "--goal", "G", "--acceptance", "a", "--hold", "--op-id", "req.1.0"];
        run(&item).unwrap();
        run(&item).unwrap();
        let issues = b.issues(None).unwrap();
        assert_eq!(issues.len(), 1, "a repeated op id writes one issue");
        let n = issues[0].number;
        assert!(issues[0].body.ends_with("\nop-id: req.1.0\n"));
        // N-1: no `taskq` label, so 4.1.11 (same `is_task`) skips the issue; this version lists it as held
        assert_eq!(issues[0].labels, vec!["q-held"]);
        assert!(!service::is_task(&issues[0]) && service::is_held(&issues[0]));
        let (views, _) = service::views(b, &mut HashMap::new()).unwrap();
        assert_eq!(views[0].task.state, State::Held);
        assert_eq!(fold(n, &entries(b, n).unwrap(), false).state, State::Held);
        run(&["add", "U", "--goal", "G", "--acceptance", "a", "--op-id", "req.1.1"]).unwrap();
        assert_eq!(b.issues(None).unwrap().len(), 2, "each item has its own receipt");
        run(&["start", &n.to_string(), "--op-id", "s1"]).unwrap();
        let i = b.issue(n).unwrap();
        assert!(service::is_task(&i) && i.labels.contains(&"q-ready".to_string()) && !i.labels.contains(&"q-held".to_string()), "{:?}", i.labels);
        let log = entries(b, n).unwrap();
        assert_eq!(fold(n, &log, false).state, State::Ready);
        // N-1 reads `hold` and `start` as unknown kinds: the same `ready`
        let old: Vec<Entry> = log.iter().filter(|e| !e.body.starts_with("taskq: hold") && !e.body.starts_with("taskq: start")).cloned().collect();
        assert_eq!(fold(n, &old, false), fold(n, &log, false));
        run(&["start", &n.to_string(), "--op-id", "s1"]).unwrap();
        assert_eq!(entries(b, n).unwrap().len(), log.len(), "a retried start writes nothing");
        assert!(run(&["start", &n.to_string()]).is_err(), "a started task refuses another start");
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

    // Chain: § 2 Ask card. Risk: a card breaks the template or limits.
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
        assert_eq!(width("see [#12](https://very/long/url) and `cargo test --locked` https://x/y"), 15, "link targets and code are exempt; outside whitespace counts");
    }

    // Chain: § 4 service uninstall. Risk: an unreadable registry loses the unit.
    #[test]
    fn unreadable_registry_preserves_unit_before_unload() {
        let root = std::env::temp_dir().join(format!("taskq-registry-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let projects = root.join("projects");
        let registered = b"other-project\n\xff";
        fs::write(&projects, registered).unwrap();
        let u = platform::Unit {
            path: root.join("unit"), text: String::new(), load: vec![], stop: vec![],
            unload: vec![vec!["taskq-must-not-run-unload".into()]],
        };
        fs::write(&u.path, "unit contents").unwrap();
        let error = uninstall_unit(&u, &root, &projects).unwrap_err();
        assert!(error.contains(&projects.display().to_string()));
        assert!(!error.contains("taskq-must-not-run-unload"));
        assert_eq!(fs::read_to_string(&u.path).unwrap(), "unit contents");
        assert_eq!(fs::read(&projects).unwrap(), registered);
        fs::remove_file(&projects).unwrap();
        assert!(uninstall_unit(&u, &root, &projects).unwrap_err().contains(&projects.display().to_string()));
        assert!(u.path.is_file());
        assert!(!projects.exists());
        fs::remove_file(&u.path).unwrap();
        fs::remove_dir(&root).unwrap();
    }

    // Chain: § 4 service uninstall failure. Risk: a failed uninstall loses registration.
    #[test]
    fn failed_uninstall_preserves_unit_and_project_registration() {
        let root = std::env::temp_dir().join(format!("taskq-uninstall-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let projects = root.join("projects");
        let registered = format!("{}\nother\n", root.display());
        fs::write(&projects, &registered).unwrap();
        let u = platform::Unit {
            path: root.join("unit"), text: String::new(), load: vec![], stop: vec![],
            unload: vec![vec![std::env::current_exe().unwrap().display().to_string(), "--invalid-taskq-test-option".into()]],
        };
        fs::write(&u.path, "unit contents").unwrap();
        assert!(uninstall_unit(&u, &root, &projects).unwrap_err().contains("failed"));
        assert_eq!(fs::read_to_string(&u.path).unwrap(), "unit contents");
        assert_eq!(fs::read_to_string(&projects).unwrap(), registered);
        fs::remove_file(&u.path).unwrap();
        fs::remove_file(&projects).unwrap();
        fs::remove_dir(&root).unwrap();
    }

    // Chain: § 7 close verdict. Risk: a padded verdict publishes.
    #[test]
    fn padded_close_verdict_is_rejected_before_publication() {
        let c = Ctx { cfg: cfg(), raw: json!({}), board: Box::new(Fake::default()) };
        let n = c.board.create("T", "A1. observable", &["taskq".into()]).unwrap();
        c.board.comment(n, &entry("spawn", &["worker", "claude:u", "mac"], None, "")).unwrap();
        let attempt = fold(n, &entries(c.board.as_ref(), n).unwrap(), false).attempt;
        let sha = "0123456789abcdef0123456789abcdef01234567";
        c.board.comment(n, &entry("result", &[sha], attempt, "result")).unwrap();
        let before = c.board.comments(n).unwrap();
        for changed in [format!("{}Changed: c", " ".repeat(240)), format!("Changed: c{}", " ".repeat(240))] {
            let verdict = format!("{changed}\nChecks: A1 ok; CI green on {sha}\nOpen: none");
            let args = Args::parse(&["close".into(), n.to_string(), "--text".into(), verdict]).unwrap();
            assert!(command(&c, "close", &args).is_err());
            assert!(c.board.issue(n).unwrap().open);
            assert_eq!(c.board.comments(n).unwrap().len(), before.len());
            assert_eq!(fold(n, &entries(c.board.as_ref(), n).unwrap(), false).state, State::Review);
        }
    }

    // Chain: § 7 Result and close templates. Risk: result or close text breaks the template.
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

    // Chain: § 2 card to hand-typed answer. Risk: a hand-typed answer to a rendered card is ignored.
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

    // Chain: § 5 commands to board entries. Risk: a rejected command writes; close does not publish.
    #[test]
    fn commands_write_only_accepted_entries_and_close_publishes() {
        let (b, c) = (Fake::default(), cfg());
        let n = b.create("T", "## Goal\ng", &["taskq".into()]).unwrap();
        assert!(act(&b, &c.root, n, "steer", &[], "x", &Op::default()).reason.contains("ready"), "steer needs a live worker");
        let before = b.comments(n).unwrap().len();
        b.comment(n, &entry("spawn", &["worker", "claude:u", "mac"], None, "")).unwrap();
        let a = fold(n, &entries(&b, n).unwrap(), false).attempt.unwrap();
        b.comment(n, &entry("result", &["0123456789abcdef0123456789abcdef01234567"], Some(a), "1. close: ship it ★\n2. redo the copy")).unwrap();
        assert_eq!(b.comments(n).unwrap().len(), before + 2);
        b.state.borrow_mut().publish = "pending".into();
        assert!(close_one(&b, &c, n, "ok", &Op::default()).reason.contains("pending"));
        assert_eq!(fold(n, &entries(&b, n).unwrap(), false).state, State::Review, "pending CI changes nothing");
        b.state.borrow_mut().publish = String::new();
        assert!(answer_picks(&b, &c, &[format!("{n}.3")], &Op::default()).is_err(), "picks are checked before writing");
        assert_eq!(answer_picks(&b, &c, &[format!("{n}.1")], &Op::default()).unwrap().iter().map(Receipt::text).collect::<Vec<_>>(), vec![format!("#{n} closed\nmerged 0123456789abcdef0123456789abcdef01234567")]);
        let i = b.issue(n).unwrap();
        assert!(!i.open && !i.labels.iter().any(|l| l.starts_with("q-")));
    }

    // Chain: § 4 one service per root (#933). Risk: a second `service run` on one root drops the first one's children.
    #[cfg(unix)]
    #[test]
    fn a_fresh_status_json_of_another_live_service_holds_service_run() {
        let root = std::env::temp_dir().join(format!("taskq-one-service-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(dot(&root)).unwrap();
        let st = dot(&root).join("status.json");
        assert_eq!(other_service(&root), None, "no status.json");
        let mut other = std::process::Command::new("sleep").arg("30").spawn().unwrap();
        fs::write(&st, format!(r#"{{"pid": {}}}"#, other.id())).unwrap();
        assert_eq!(other_service(&root), Some(other.id()), "a live other service");
        fs::write(&st, format!(r#"{{"pid": {}}}"#, std::process::id())).unwrap();
        assert_eq!(other_service(&root), None, "its own status.json after the update re-exec");
        fs::write(&st, format!(r#"{{"pid": {}}}"#, other.id())).unwrap();
        other.kill().unwrap();
        other.wait().unwrap();
        assert_eq!(other_service(&root), None, "a dead writer, e.g. before a restart");
        assert_eq!(service_id("abc").len(), 12);
        assert_ne!(service_id("abc"), "abc");
        let _ = fs::remove_dir_all(&root);
    }

    // Chain: service install, § 8 step 3 kids file, adoption. Risk: an install from a project with a live child ends it
    #[cfg(unix)]
    #[test]
    fn install_with_a_live_child_keeps_it_through_reexec_and_adoption() {
        use runtime::Proc;
        let root = std::env::temp_dir().join(format!("taskq-install-live-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(dot(&root)).unwrap();
        let child = std::process::Command::new("sleep").arg("30").spawn().unwrap();
        let pid = child.id();
        // the unit reload stands for launchctl bootout: it ends the service's process tree, the child included
        let reloaded = root.join("reloaded");
        let u = platform::Unit { path: root.join("unit"), text: String::new(), stop: vec![],
            unload: vec![vec!["kill".into(), "-9".into(), pid.to_string()]],
            load: vec![vec!["touch".into(), reloaded.display().to_string()]] };
        fs::write(dot(&root).join("status.json"), format!(r#"{{"kids": [{{"task": 7, "attempt": 41, "rt": "claude", "pid": {pid}}}]}}"#)).unwrap();
        assert!(load_unit(&u, &root).unwrap(), "live child: hand over");
        assert!(dot(&root).join("restart").exists() && !reloaded.exists(), "the running service re-execs; no reload");
        let mut adopted = runtime::Adopted(pid as i32);
        assert_eq!(adopted.exited(), Ok(None), "the child still runs");
        // the re-exec writes the kids file; the new image adopts the child, not `gone`
        fs::write(dot(&root).join("kids"), format!("pid {}\n7 41 claude {pid} 40\n", std::process::id())).unwrap();
        assert_eq!(kids_file(&root), vec![(7, 41, "claude".to_string(), pid as i32, 40)]);
        assert_eq!(adopted.exited(), Ok(None), "adopted live");
        // A real process stands in for an app turn: restart must stay pending and keep its lease.
        let b = Fake::default();
        let c = Config { root: root.clone(), ..cfg() };
        let mut s = Service::new(&b, &c, "test", Box::new(|_| unreachable!()));
        s.kids.push(Kid { task: 7, attempt: 41, rt: "codex-app".into(), proc: Box::new(runtime::Adopted(pid as i32)) });
        reexec(&s, &c).unwrap();
        assert!(dot(&root).join("restart").exists());
        assert_eq!(s.kids[0].attempt, 41);
        assert_eq!(adopted.exited(), Ok(None));
        s.kids.clear();
        fs::create_dir_all(operator::dir(&root)).unwrap();
        fs::write(operator::dir(&root).join("1.json"), "{}").unwrap();
        fs::write(operator::answer_path(&root, "1"), r#"{"state":"running"}"#).unwrap();
        reexec(&s, &c).unwrap();
        assert!(dot(&root).join("restart").exists() && handover(&root));
        fs::write(operator::answer_path(&root, "1"), r#"{"state":"result"}"#).unwrap();
        // an idle service reloads the unit, which would end a child
        fs::write(dot(&root).join("status.json"), r#"{"kids": []}"#).unwrap();
        assert!(!load_unit(&u, &root).unwrap() && reloaded.exists());
        std::thread::sleep(Duration::from_millis(200));
        assert!(adopted.exited().is_ok_and(|c| c.is_some()), "a reload ends it");
        let _ = fs::remove_dir_all(&root);
    }

    // Chain: service install and the § 8 step 3 handover. Risk: a reinstall or a corrupt kids file ends live children
    #[test]
    fn install_hands_live_children_over_and_corrupt_kids_stay() {
        let root = std::env::temp_dir().join(format!("taskq-handover-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(dot(&root)).unwrap();
        assert!(!handover(&root), "no service: reload");
        let st = dot(&root).join("status.json");
        fs::write(&st, r#"{"kids": []}"#).unwrap();
        assert!(!handover(&root), "idle service: reload");
        fs::write(&st, r#"{"kids": [{"task": 7, "attempt": 41, "rt": "claude", "pid": 1}]}"#).unwrap();
        assert!(handover(&root), "live children: re-exec");
        fs::write(&st, "{\"kids\": [").unwrap();
        assert!(handover(&root), "unknown keeps them");
        fs::File::options().write(true).open(&st).unwrap().set_modified(SystemTime::now() - Duration::from_secs(120)).unwrap();
        assert!(!handover(&root), "a stale status.json: no running service");
        let kids = dot(&root).join("kids");
        assert!(kids_file(&root).is_empty());
        fs::write(&kids, "pid 1\n7 41 claude\n").unwrap();
        assert!(kids_file(&root).is_empty());
        assert_eq!(fs::read_to_string(&kids).unwrap(), "pid 1\n7 41 claude\n", "a malformed kids file stays");
        fs::write(&kids, format!("pid {}\n7 41 claude 123 40\n", std::process::id())).unwrap();
        assert_eq!(kids_file(&root), vec![(7, 41, "claude".to_string(), 123, 40)]);
        assert!(!kids.exists(), "a valid one is consumed");
        fs::write(&kids, "pid 1\n7 41 claude 123 40\n").unwrap();
        assert!(kids_file(&root).is_empty() && !kids.exists(), "another process's file: its attempts go gone");
        // a corrupt focus pauses nothing away and names its reason
        fs::write(dot(&root).join("focus"), b"\xff").unwrap();
        assert!(pause(&root).unwrap_err().contains("not UTF-8"));
        assert_eq!(fs::read(dot(&root).join("focus")).unwrap(), b"\xff");
        assert!(service_fields(&root).contains("Focus: unknown (focus: not UTF-8 text; file kept)"));
        fs::write(&st, "{").unwrap();
        assert!(service_fields(&root).contains("Status: unknown (status.json: corrupt"));
        let _ = fs::remove_dir_all(&root);
    }

    // Chain: § 4 pause and resume focus. Risk: resume widens focus to all.
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
        pause(&root).unwrap();
        pause(&root).unwrap();
        assert_eq!(fs::read_to_string(dot(&root).join("focus")).unwrap(), "off");
        assert_eq!(resume(&root).unwrap(), "area-maps", "a second pause keeps the saved focus");
        let _ = fs::remove_dir_all(&root);
    }

    // Chain: R6 human report. Risk: report breaks the R6 format.
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

    /// A board with one task per report part: ready, ask with a card, blocked, failed, plus a host heartbeat.
    fn snapshot_board() -> Fake {
        let b = Fake::default();
        b.create("Ready one", "", &["taskq".into(), "p1".into()]).unwrap();
        let q = b.create("Question", "", &["taskq".into()]).unwrap();
        b.comment(q, &entry("spawn", &["worker", "claude:abc", "mac"], None, "")).unwrap();
        let a = fold(q, &entries(&b, q).unwrap(), false).attempt.unwrap();
        b.comment(q, &entry("ask", &[], Some(a), "Done: read\nProblem: Which way?\n1. left\n2. right ★")).unwrap();
        let dep = b.create("plain issue", "", &[]).unwrap();
        b.create("Blocked", &format!("Depends on #{dep}"), &["taskq".into()]).unwrap();
        let f = b.create("Broken", "", &["taskq".into()]).unwrap();
        b.comment(f, &entry("failed", &[], None, "Reason: no login")).unwrap();
        b.create("taskq host mac", "version 4.0.0", &[service::HOST_LABEL.into()]).unwrap();
        b.advance(90);
        b
    }

    // Chain: § 5 status read model. Risk: status outputs disagree.
    #[test]
    fn text_json_and_service_status_count_from_one_snapshot() {
        let (b, c) = (snapshot_board(), cfg());
        let snap = snapshot::read(&b, &c, &Value::Null, &[]);
        let text = snap.text(&c, &Value::Null).unwrap();
        let j = snap.json();
        let t = &j["totals"];
        assert_eq!((t["ready"].as_u64(), t["ask"].as_u64(), t["waiting"].as_u64(), t["failed"].as_u64()), (Some(1), Some(1), Some(1), Some(1)));
        let line = format!("In work {} · Waiting for answer {} · Ready {}", t["doing"].as_u64().unwrap() + t["review"].as_u64().unwrap(), t["ask"], t["ready"]);
        assert!(text.contains(&line), "{text}");
        // the service writes the same rows and totals into status.json; `service status` counts them back
        let root = std::env::temp_dir().join(format!("taskq-snap-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(dot(&root)).unwrap();
        fs::write(dot(&root).join("focus"), "all").unwrap();
        let mut status = json!({"error": null, "at": SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs(), "tasks_age_seconds": 0});
        for (k, v) in snap.status_keys(None) { status[k] = v; }
        assert_eq!(status["totals"], j["totals"]);
        assert_eq!(status["tasks"], j["tasks"]);
        fs::write(dot(&root).join("status.json"), status.to_string()).unwrap();
        assert!(service_fields(&root).contains(&line));
        // a failed pass keeps the last rows for N-1 readers but its counts are unknown
        for (k, v) in snap.status_keys(Some("board down")) { status[k] = v; }
        assert_eq!((status["complete"].clone(), status["totals"].clone(), status["reason"].clone()), (json!(false), Value::Null, json!("board down")));
        fs::write(dot(&root).join("status.json"), status.to_string()).unwrap();
        assert!(service_fields(&root).contains("Counts: unknown"));
        let _ = fs::remove_dir_all(&root);
    }

    // Chain: R12 status unknown. Risk: an unreadable board shows zero.
    #[test]
    fn unreadable_board_snapshot_is_incomplete_not_zero() {
        let (b, c) = (Fake::default(), cfg());
        let mut snap = snapshot::read(&b, &c, &Value::Null, &[]);
        snap.reason = Some("board unreadable: 502".into());
        let j = snap.json();
        assert_eq!((j["complete"].clone(), j["reason"].clone(), j["totals"].clone()), (json!(false), json!("board unreadable: 502"), Value::Null));
        assert_eq!(snap.text(&c, &Value::Null).unwrap_err(), "board unreadable: 502");
        assert!(!snapshot::complete(&json!({"error": "board failed", "tasks": []})), "N-1 status.json with an error");
        assert!(snapshot::complete(&json!({"error": null, "tasks": []})), "N-1 status.json after a good pass");
        assert!(!snapshot::complete(&json!({"error": null, "complete": false, "tasks": []})));
    }

    // Chain: § 5 status --json schema. Risk: status JSON drifts from tests/fixtures.
    #[test]
    fn status_json_matches_the_golden_fixture() {
        let snap = snapshot::read(&snapshot_board(), &cfg(), &Value::Null, &[]);
        let got = serde_json::to_string_pretty(&snap.json()).unwrap() + "\n";
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/status-snapshot.json");
        if std::env::var_os("TASKQ_BLESS").is_some() { fs::write(&path, &got).unwrap(); }
        // Parsed compare: a CRLF checkout on Windows must not fail the golden.
        let want: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(snap.json(), want, "TASKQ_BLESS=1 rewrites {}", path.display());
    }

    // Chain: § 2 card -> canonical parsed question -> shared answer card fixtures. Risk: a UI fixture drifts from TaskQ's parse.
    #[test]
    fn answer_card_fixtures_carry_the_canonical_parse() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/answer-card.json");
        let mut fx: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        let mut want = fx.clone();
        for row in want["cases"].as_array_mut().unwrap().iter_mut().flat_map(|c| c["rows"].as_array_mut().unwrap()) {
            row["question"] = row["card"].as_str().map_or(Value::Null, snapshot::question);
        }
        if std::env::var_os("TASKQ_BLESS").is_some() {
            fs::write(&path, serde_json::to_string_pretty(&want).unwrap() + "\n").unwrap();
            fx = want.clone();
        }
        assert_eq!(fx, want, "TASKQ_BLESS=1 rewrites {}", path.display());
        let q = snapshot::question("Done: d\nProblem: p\nmore\nEvidence: [#1](https://x/1), [shot](https://x/s.png)\n1. close: ship ★\n2. redo\nTo answer: `taskq answer 7.K`");
        assert_eq!((q["parsed"].clone(), q["problem"].clone(), q["evidence"][1]["url"].clone(), q["choices"][0]["consequential"].clone()), (json!(true), json!("p more"), json!("https://x/s.png"), json!(true)));
        assert_eq!(snapshot::question("1. a ★\n2. b")["parsed"], json!(false), "no Problem: read only");
        assert_eq!(snapshot::question("Problem: p\n1. a\n2. b")["parsed"], json!(false), "no star: read only");
    }

    // Chain: canonical board read -> approved projection -> duplicate-host coalescing -> freshness.
    #[test]
    fn enrollment_projects_only_approved_board_metadata() {
        use enrollment::{BoardScope, coalesce, project};
        let mut snap = snapshot::read(&snapshot_board(), &cfg(), &Value::Null, &[]);
        snap.board["host"] = json!("github.com");
        snap.board["token"] = json!("forge-secret");
        snap.board["url"] = json!("https://user:forge-secret@github.com/o/r");
        let scope = BoardScope::new("github", "GITHUB.COM", "o/r").unwrap();
        let approved = [scope.clone()];
        let a = project(&snap, &"a".repeat(64), &approved).unwrap();
        let b = project(&snap, &"b".repeat(64), &approved).unwrap();
        let got = serde_json::to_value(&a).unwrap();
        let want: Value = serde_json::from_str(include_str!("../tests/fixtures/enrollment-projection.json")).unwrap();
        assert_eq!(got, want);
        let text = got.to_string();
        for omitted in ["forge-secret", "session", "labels", "title", "Which way?", "no login", "url", "hosts"] {
            assert!(!text.contains(omitted), "{omitted}");
        }
        assert!(project(&snap, &"a".repeat(64), &[]).is_err());
        assert_eq!(scope, BoardScope::new("github", "github.com", "O/R").unwrap());
        for (host, repo) in [("user:secret@github.com", "o/r"), ("github.com", "o/../r"), ("github.com", "o/r?token=x")] {
            assert!(BoardScope::new("github", host, repo).is_err());
        }
        let rows = coalesce(&[a.clone(), b], &approved, 100, 10);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["totals"], got["totals"], "two hosts do not double board totals");
        assert_eq!(rows[0]["freshness"], "fresh");
        for (now, freshness) in [(120, "stale"), (98, "unknown")] {
            let rows = coalesce(&[a.clone()], &approved, now, 10);
            assert_eq!(rows[0]["freshness"], freshness);
            assert!(rows[0]["totals"].is_null());
        }
        assert_eq!(coalesce(&[], &approved, 100, 10)[0]["freshness"], "missing");
        snap.reason = Some("forge-secret and private path".into());
        let incomplete = project(&snap, &"a".repeat(64), &approved).unwrap();
        let rows = coalesce(&[incomplete.clone()], &approved, 100, 10);
        assert_eq!(rows[0]["complete"], false);
        assert_eq!(rows[0]["reason"], "source incomplete");
        assert!(rows[0]["totals"].is_null());
        let rows = coalesce(&[a.clone(), incomplete], &approved, 100, 10);
        assert_eq!(rows[0]["reason"], "conflicting observations");
        assert!(rows[0]["observation"].is_null());
        snap.observed = 90;
        let old = project(&snap, &"a".repeat(64), &approved).unwrap();
        assert_eq!(coalesce(&[a, old], &approved, 100, 10)[0]["totals"], got["totals"]);
        let other = BoardScope::new("gitlab", "gitlab.com", "o/r").unwrap();
        assert!(project(&snap, &"a".repeat(64), &[other.clone()]).is_err());
        assert_eq!(coalesce(&[], &[scope, other], 100, 10).len(), 2);
    }

    // Chain: § 5 CLI text limits. Risk: invalid text is written.
    #[test]
    fn cli_audit_boundaries_and_no_write_on_invalid_text() {
        assert_eq!(width("a  b"), 4);
        assert_eq!(width("a`unclosed"), 10);
        assert_eq!(width("é🦀"), 2);
        assert!(one_line("option", &["a\rb".into()]).is_err());
        let text = "Changed: visible\nChecks: A1 ok\nChecks: A2 ok; CI green on abc\nOpen: none";
        close_verdict(text, "A1. a\nA2. b").unwrap();
        assert!(task_text("requeue", "unlabelled reason").is_err());
        assert!(task_text("requeue", &format!("Reason: ok{}", " ".repeat(240))).is_err());
        assert!(task_text("failed", "Reason: blocked\nNext: repair").is_ok());
        assert!(task_text("steer", "1\n2\n3\n4\n5").is_err());
        let c = Ctx { cfg: cfg(), raw: json!({}), board: Box::new(Fake::default()) };
        let n = c.board.create("T", "", &["taskq".into()]).unwrap();
        c.board.comment(n, &entry("spawn", &["worker", "claude:u", "mac"], None, "")).unwrap();
        let before = c.board.comments(n).unwrap();
        for (cmd, text) in [("requeue", "bad"), ("later", "bad"), ("failed", "bad"), ("answer", "1\n2\n3\n4\n5"), ("steer", "1\n2\n3\n4\n5")] {
            let a = Args::parse(&[cmd.into(), n.to_string(), "--text".into(), text.into()]).unwrap();
            assert!(command(&c, cmd, &a).is_err(), "{cmd}");
        }
        assert_eq!(c.board.comments(n).unwrap().len(), before.len());
        run(&["--help".into()]).unwrap();
        run(&["service".into(), "pause".into(), "--help".into()]).unwrap();
        run(&["close".into(), "12".into(), "--help".into()]).unwrap();
        assert!(USAGE.contains("--recommend K"));
        assert!(USAGE.contains("--snapshot DIR"));
        assert!(USAGE.contains("taskq service capacity [--host H] [--mode auto|fixed] [--max N|none] [--claude N] [--codex N] [--codex-app N] [--load-gate on|off] [--cpu-max P] [--memory-min P]"));
        run(&["service".into(), "capacity".into(), "--help".into()]).unwrap();
        let e = platform::error_text("#7 is ready", None);
        assert!(e.starts_with("Error: #7 is ready\nNext: "));
        assert_eq!(e.lines().count(), 2);
        let dep = c.board.create("ordinary issue", "", &[]).unwrap();
        let blocked = c.board.create("blocked", &format!("Depends on #{dep}"), &["taskq".into(), "selected".into()]).unwrap();
        let report = report(c.board.as_ref(), &c.cfg, &c.raw, &["selected".into()]).unwrap();
        assert!(report.contains(&format!("#{blocked} blocked")));
        assert!(report.contains(&format!("blocked (#{dep} open)")), "non-task dependency remains visible outside filter");
    }

    // Chain: R6 compact report. Risk: compaction drops choices, links or evidence.
    #[test]
    fn compact_report_keeps_choices_links_and_original_evidence() {
        let (b, c) = (Fake::default(), cfg());
        let title = "🦀".repeat(65);
        let n = b.create(&title, "", &["taskq".into()]).unwrap();
        b.comment(n, &entry("spawn", &["worker", "claude:u", "mac"], None, "")).unwrap();
        let reason = "r".repeat(170);
        let option = "x".repeat(125);
        let card = format!("Done: inspected\nProblem: {reason}\nEvidence: [context](https://context)\n1. {option} ★\n2. keep\nTo answer: exact syntax");
        let a = fold(n, &entries(&b, n).unwrap(), false).attempt;
        b.comment(n, &entry("ask", &[], a, &card)).unwrap();
        let r = report(&b, &c, &Value::Null, &[]).unwrap();
        assert!(r.contains(&format!("{}…", "🦀".repeat(59))));
        assert!(r.contains(&format!("[{}…](https://board/{n})", "r".repeat(159))));
        assert!(r.contains(&format!("{n}.1 [{}…](https://board/{n}) ★", "x".repeat(119))));
        assert!(r.contains(&format!("{n}.2 keep")));
        assert!(!r.contains("Done: inspected"));
        assert!(!r.contains("To answer:"));
        assert_eq!(fold(n, &entries(&b, n).unwrap(), false).card.as_deref(), Some(card.as_str()));
        assert_eq!(b.issue(n).unwrap().title, title);
        assert_eq!(snapshot::brief("Old question?\n1. keep\n2. change"), "Old question?");
    }

    // Chain: R12 service status fields. Risk: missing data shown as known; failed pause reported ok.
    #[test]
    fn service_fields_keep_missing_data_unknown_and_pause_reports_failure() {
        let root = std::env::temp_dir().join(format!("taskq-fields-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let s = service_fields(&root);
        assert!(s.contains("Focus: unknown"));
        assert!(s.contains("Counts: unknown"));
        assert!(s.contains("Freshness: unknown"));
        assert!(!s.contains("Ready 0"));
        assert!(pause(&root).is_err(), "missing state folder cannot acknowledge pause");
        fs::create_dir_all(dot(&root)).unwrap();
        fs::write(dot(&root).join("focus"), "one two").unwrap();
        let mut status = json!({"error": null, "at": SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs(), "tasks_age_seconds": 0, "tasks": [{"state":"doing"},{"state":"review"},{"state":"ask"},{"state":"ready"}]});
        fs::write(dot(&root).join("status.json"), status.to_string()).unwrap();
        let s = service_fields(&root);
        assert!(s.contains("Focus: one\nFocus: two"));
        assert!(s.contains("In work 2 · Waiting for answer 1 · Ready 1"));
        assert!(s.contains("cached status"));
        assert!(s.contains("Service: unknown (process not checked)"));
        // Capacity-only publication keeps the file fresh without refreshing the board counts.
        status["tasks_age_seconds"] = json!(301);
        status["capacity"] = json!({"gate": "off"});
        fs::write(dot(&root).join("status.json"), status.to_string()).unwrap();
        assert!(file_age(&dot(&root).join("status.json")).unwrap() <= 1);
        assert!(service_fields(&root).contains("Counts: unknown"));
        status["tasks_age_seconds"] = Value::Null;
        fs::write(dot(&root).join("status.json"), status.to_string()).unwrap();
        assert!(service_fields(&root).contains("Counts: unknown"), "old snapshots lack board freshness");
        fs::write(dot(&root).join("status.json"), r#"{"error":"board failed","tasks":[]}"#).unwrap();
        assert!(service_fields(&root).contains("Counts: unknown"));
        let _ = fs::remove_dir_all(&root);
    }

    // Chain: § 5 error diagnostics. Risk: a compact error loses its full diagnostic.
    #[test]
    fn compact_errors_retain_complete_diagnostics() {
        let root = std::env::temp_dir().join(format!("taskq-error-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let original = format!("#7 {}\nexact evidence", "é".repeat(200));
        let e = platform::error_text(&original, Some(&root));
        assert_eq!(e.lines().count(), 2);
        assert!(width(e.lines().next().unwrap()) <= 160);
        assert!(e.contains("cli-errors.log"));
        assert_eq!(fs::read_to_string(dot(&root).join("cli-errors.log")).unwrap(), format!("{original}\n"));
        let failed = platform::error_text(&original, None);
        assert!(failed.contains(&original), "a failed save never discards the original evidence");
        let _ = fs::remove_dir_all(&root);
    }
}
