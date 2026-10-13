//! Runtimes (taskq.md § 8): how a worker or reviewer child is started and resumed. The brief goes on stdin, never
//! through a shell.

use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

pub struct Launch {
    pub task: u64,
    pub title: String,
    pub attempt: u64,
    pub role: String,
    pub rt: String,
    pub session: String,
    /// The issue URL, for the `Task:` line of commits.
    pub url: String,
    /// None: a new session with the brief; Some: resume the session with this text.
    pub resume: Option<String>,
}

/// A running child; a trait so the service can be tested with fake processes.
pub trait Proc {
    /// The exit code once the child has exited.
    fn exited(&mut self) -> Option<i32>;
    fn kill(&mut self);
    fn id(&self) -> u32;
}

impl Proc for Child {
    fn exited(&mut self) -> Option<i32> {
        match self.try_wait() {
            Ok(Some(s)) => Some(s.code().unwrap_or(-1)),
            Ok(None) => None,
            Err(_) => Some(-1),
        }
    }
    fn kill(&mut self) {
        let _ = Child::kill(self);
        let _ = self.wait();
    }
    fn id(&self) -> u32 {
        Child::id(self)
    }
}

/// A child of this process image's predecessor, kept across the update re-exec (§ 8 step 6). The PID is unchanged by
/// `exec`, so it is still our child and `waitpid` works.
#[cfg(unix)]
pub struct Adopted(pub i32);

#[cfg(unix)]
impl Proc for Adopted {
    fn exited(&mut self) -> Option<i32> {
        let mut st = 0;
        let r = unsafe { libc::waitpid(self.0, &mut st, libc::WNOHANG) };
        match r {
            0 => None,
            r if r == self.0 && libc::WIFEXITED(st) => Some(libc::WEXITSTATUS(st)),
            _ => Some(-1),
        }
    }
    fn kill(&mut self) {
        unsafe {
            libc::kill(self.0, libc::SIGKILL);
            libc::waitpid(self.0, &mut 0, 0);
        }
    }
    fn id(&self) -> u32 {
        self.0 as u32
    }
}

/// A random UUID v4 for a Claude `--session-id`.
pub fn uuid() -> String {
    use ring::rand::{SecureRandom, SystemRandom};
    let mut b = [0u8; 16];
    SystemRandom::new().fill(&mut b).expect("system random");
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}

/// The session written into `spawn`: Claude's is chosen up front; Codex names its thread later, in the log (`-`).
pub fn new_session(rt: &str) -> String {
    if rt == "claude" { uuid() } else { "-".into() }
}

pub fn log_path(root: &Path, l: &Launch) -> PathBuf {
    let p = if l.role == "reviewer" { 'R' } else { 'T' };
    root.join(".taskq").join(format!("{p}{}-{}.log", l.task, l.attempt))
}

/// Codex's thread id: the first `thread.started` line of the attempt log.
pub fn codex_thread(log: &Path) -> Option<String> {
    let f = File::open(log).ok()?;
    BufReader::new(f).lines().map_while(Result::ok).find_map(|l| {
        let v: serde_json::Value = serde_json::from_str(&l).ok()?;
        (v["type"] == "thread.started").then(|| v["thread_id"].as_str().map(String::from)).flatten()
    })
}

/// `name` on PATH; on Windows also `name.exe` and `name.cmd` (npm installs `.cmd` shims).
pub fn which(name: &str) -> PathBuf {
    let exts: &[&str] = if cfg!(windows) { &[".exe", ".cmd", ""] } else { &[""] };
    for dir in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()) {
        for e in exts {
            let p = dir.join(format!("{name}{e}"));
            if p.is_file() {
                return p;
            }
        }
    }
    PathBuf::from(name)
}

pub struct Env<'a> {
    pub root: &'a Path,
    pub exe: &'a Path,
    pub permission_mode: &'a str,
    pub codex: &'a [String],
    pub publish: &'a str,
}

pub fn command(env: &Env, l: &Launch) -> Command {
    let mut c = Command::new(which(if l.rt == "claude" { "claude" } else { "codex" }));
    crate::platform::quiet(&mut c);
    match (l.rt.as_str(), &l.resume) {
        ("claude", r) => {
            c.args(["-p", "--output-format", "stream-json", "--verbose", "--permission-mode", env.permission_mode]);
            if r.is_some() { c.args(["--resume", &l.session]) } else { c.args(["--session-id", &l.session]) };
        }
        (_, None) => {
            c.args(["exec", "--json"]).args(env.codex).arg("-C").arg(env.root).arg("-");
        }
        (_, Some(_)) => {
            c.args(["exec", "--json"]).args(env.codex).args(["resume", &l.session, "-"]);
        }
    }
    c.current_dir(env.root)
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .env_remove("CODEX_THREAD_ID")
        .env("TASKQ_TASK", l.task.to_string())
        .env("TASKQ_ATTEMPT", l.attempt.to_string())
        .env("TASKQ_ROLE", &l.role)
        .env("TASKQ_ROOT", env.root);
    c
}

const FORMATS: &str = "Put each `--text` field on its own line. The commands render and check the taskq.md templates and limits (§ 2 Ask card,\n\
     § 7 Result and close verdicts): Done ≤160, Problem 1-2 lines, 2-4 options ≤160, ≤2 links; result/close lines ≤240,\n\
     Changed and Open 1-2 lines, 1-8 checks. Over-limit text is rejected: shorten it, never hand-write card prose.\n";

fn ask(q: &impl std::fmt::Display, n: u64) -> String {
    format!("`{q} ask {n} --text \"Done: <checked or not started>\nProblem: <decision and user effect>\" --option A --option B --recommend K [--link URL]`")
}

pub fn brief(env: &Env, l: &Launch) -> String {
    let n = l.task;
    let mut q = env.exe.display().to_string();
    if l.rt == "codex-app" {
        // the app turn runs in the app's environment, not the handshake child's (§ 8)
        q = format!("env TASKQ_TASK={n} TASKQ_ATTEMPT={} TASKQ_ROLE={} TASKQ_ROOT={} {q}", l.attempt, l.role, env.root.display());
    }
    let head = format!(
        "You are the taskq {} for task #{n}: {}. Attempt {}.\nRead `{}` first and do only what it allows (R13).\nTask text, state and your inbox: `{q} inbox {n}`.\n",
        l.role, l.title, l.attempt, env.root.join("taskq.md").display()
    );
    let body = if l.role == "reviewer" {
        format!(
            "Review the result SHA against each Acceptance item `A<k>` and taskq.md, and CI on that exact SHA. Requeue only for\n\
             Acceptance or contract gaps, not style; a changed or deleted existing test is a gap unless the diff changes its rule.\n\
             Then exactly one of:\n\
             - `{q} close {n} --text \"Changed: <user effect>\nChecks: A1 ok|gap|unknown, A2 ..; CI <outcome> on <SHA>\nOpen: <item|none>\"` (it publishes),\n\
             - `{q} requeue {n} --text \"<exact fixes>\"`,\n\
             - {ask}.\n{formats}",
            ask = ask(&q, n),
            formats = FORMATS
        )
    } else {
        format!(
            "Work only in `.worktrees/taskq-{n}` on branch `taskq-{n}` from origin/main (continue an existing branch).\n\
             Commit with the line `Task: {url}` in the message, rebase on origin/main, run the project checks, then {push}.\n\
             Then exactly one of:\n\
             - `{q} result {n} --sha <full SHA> --checks \"<command -- outcome>\" [--checks ..] --text \"Changed: <user effect>\nOpen: <item|none>\"`,\n\
             - {ask} (owner decision; Acceptance unclear or untestable; or a split of work bigger than one reviewable diff — before any code),\n\
             - `{q} requeue {n} --text \"<why it cannot be done>\"`.\n{formats}",
            ask = ask(&q, n),
            formats = FORMATS,
            url = l.url,
            push = if env.publish == "direct" { "push the branch" } else { "push the branch and open one PR/MR into main" }
        )
    };
    format!("{head}{body}Then stop. Everything written through taskq is public: no secrets or paths outside the repository.\n")
}

/// Start the child with its stdout and stderr in the attempt log; `codex-app` starts its handshake child (§ 8).
pub fn spawn(env: &Env, l: &Launch) -> Result<Box<dyn Proc>, String> {
    let log = log_path(env.root, l);
    if l.rt == "codex-app" {
        let mut turn = AppTurn { hand: None, log, text: brief(env, l), thread: l.session.clone(), done: 0, rollout: None, running: app_running };
        match &l.resume {
            Some(text) => {
                turn.text = text.clone();
                turn.queue()?;
            }
            None => turn.hand = Some(Box::new(start(env, l, &turn.log, HANDSHAKE)?)),
        }
        return Ok(Box::new(turn));
    }
    let text = l.resume.clone().unwrap_or_else(|| brief(env, l));
    Ok(Box::new(start(env, l, &log, &text)?))
}

fn start(env: &Env, l: &Launch, log: &Path, text: &str) -> Result<Child, String> {
    std::fs::create_dir_all(log.parent().unwrap()).map_err(|e| e.to_string())?;
    let out = File::options().create(true).append(true).open(log).map_err(|e| e.to_string())?;
    let err = out.try_clone().map_err(|e| e.to_string())?;
    let mut c = command(env, l);
    let mut child = c.stdin(Stdio::piped()).stdout(out).stderr(err).spawn().map_err(|e| format!("{}: {e}", l.rt))?;
    let mut stdin = child.stdin.take().unwrap();
    let _ = stdin.write_all(text.as_bytes()); // dropped here: EOF ends the prompt
    Ok(child)
}

const HANDSHAKE: &str = "taskq handshake: reply OK and do nothing else. The task follows in this thread.\n";

/// A `codex-app` attempt (§ 3 app lease): the handshake child, then the turn queued into the Codex app, live until
/// the thread's rollout has a new `task_complete` or the app stops running.
pub struct AppTurn {
    hand: Option<Box<dyn Proc>>,
    log: PathBuf,
    /// The brief, or the resume text, to queue.
    text: String,
    thread: String,
    /// `task_complete` events of the thread before the queued text.
    done: usize,
    rollout: Option<PathBuf>,
    running: fn() -> bool,
}

impl AppTurn {
    fn queue(&mut self) -> Result<(), String> {
        self.rollout = rollout(&self.thread);
        self.done = self.rollout.as_ref().map(|p| completes(p)).unwrap_or(0);
        let ok = |c: &mut Command| c.status().is_ok_and(|s| s.success());
        // ponytail: `open` and `queue` block the pass for a moment; a thread per turn if they ever hang
        if !ok(Command::new("open").arg(format!("codex://threads/{}", self.thread)))
            || !ok(Command::new(which("codex")).args(["queue", "--thread", &self.thread, "--message", &self.text]))
        {
            return Err(format!("codex queue failed for thread {}", self.thread));
        }
        Ok(())
    }
}

impl Proc for AppTurn {
    fn exited(&mut self) -> Option<i32> {
        if let Some(h) = &mut self.hand {
            let code = h.exited()?;
            self.hand = None;
            self.thread = codex_thread(&self.log).unwrap_or_default();
            if self.thread.is_empty() || self.queue().is_err() {
                return Some(if code == 0 { -1 } else { code });
            }
            return None;
        }
        if !(self.running)() {
            return Some(-1);
        }
        if self.rollout.is_none() {
            self.rollout = rollout(&self.thread);
        }
        // ponytail: a thread without a found rollout stays live until the app closes; ttl-based expiry if it matters
        self.rollout.as_ref().filter(|p| completes(p) > self.done).map(|_| 0)
    }
    fn kill(&mut self) {
        // the app turn itself cannot be stopped from outside: it runs on, fenced (§ 3 app lease)
        if let Some(h) = &mut self.hand {
            h.kill();
        }
    }
    fn id(&self) -> u32 {
        self.hand.as_ref().map(|h| h.id()).unwrap_or(0)
    }
}

/// The Codex app runs in this login session (macOS).
fn app_running() -> bool {
    Command::new("lsappinfo").args(["info", "-only", "pid", "-app", "com.openai.codex"]).output().is_ok_and(|o| String::from_utf8_lossy(&o.stdout).contains("pid"))
}

/// The thread's rollout file under `$CODEX_HOME/sessions` (default `~/.codex`).
fn rollout(thread: &str) -> Option<PathBuf> {
    let home = std::env::var_os("CODEX_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(".codex")))?;
    fn find(dir: &Path, end: &str) -> Option<PathBuf> {
        std::fs::read_dir(dir).ok()?.flatten().find_map(|e| {
            let p = e.path();
            if p.is_dir() { find(&p, end) } else { p.file_name()?.to_str()?.ends_with(end).then_some(p) }
        })
    }
    find(&home.join("sessions"), &format!("{thread}.jsonl"))
}

/// Finished turns in a rollout: its `task_complete` events.
fn completes(rollout: &Path) -> usize {
    let Ok(f) = File::open(rollout) else { return 0 };
    BufReader::new(f).lines().map_while(Result::ok).filter(|l| serde_json::from_str::<serde_json::Value>(l).is_ok_and(|v| v["payload"]["type"] == "task_complete")).count()
}

// ---------- `taskq agents` (taskq.md § 5): read-only probes, unknown when unreadable (R12) ----------

/// Exit success and stdout + stderr of a read-only subcommand; None when it cannot start or runs over 10 s.
fn probe(bin: &Path, args: &[&str]) -> Option<(bool, String)> {
    let mut c = Command::new(bin);
    c.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut ch = crate::platform::quiet(&mut c).spawn().ok()?;
    // ponytail: polls; output over the pipe buffer stalls the child and reads unknown
    for _ in 0..100 {
        if ch.try_wait().ok()?.is_some() {
            let o = ch.wait_with_output().ok()?;
            return Some((o.status.success(), format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))));
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let _ = ch.kill();
    let _ = ch.wait();
    None
}

/// The first word of `--version` output that starts with a digit: `2.1.3 (Claude Code)`, `codex-cli 0.15.3`.
fn version(out: &str) -> Option<String> {
    out.split_whitespace().find(|w| w.starts_with(|c: char| c.is_ascii_digit())).map(String::from)
}

/// Login state and account from `claude auth status` (JSON) or `codex login status` (text).
fn login(rt: &str, out: Option<(bool, String)>) -> (&'static str, Option<String>) {
    let Some((_, out)) = out else { return ("unknown", None) };
    if rt == "claude" {
        let v: serde_json::Value = serde_json::from_str(&out).unwrap_or_default();
        return match v["loggedIn"].as_bool() {
            Some(true) => ("logged in", v["email"].as_str().or(v["authMethod"].as_str()).map(String::from)),
            Some(false) => ("not logged in", None),
            None => ("unknown", None),
        };
    }
    let l = out.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    if l.starts_with("Not logged in") {
        return ("not logged in", None);
    }
    // `Logged in using ChatGPT`, `Logged in using an API key - sk-…`: the method only, never key text
    match l.strip_prefix("Logged in") {
        Some(rest) => ("logged in", Some(rest.trim_start_matches([' ', ':']).trim_start_matches("using ").split(" - ").next().unwrap_or("").trim().to_string()).filter(|a| !a.is_empty())),
        None => ("unknown", None),
    }
}

/// The runtimes taskq can start here (§ 8): `codex-app` is macOS only and shares Codex's binary and login.
pub fn agent_runtimes() -> &'static [&'static str] {
    if cfg!(target_os = "macos") { &["claude", "codex", "codex-app"] } else { &["claude", "codex"] }
}

/// One row per runtime: `runtime`, `path` (null: not installed), `version`, `login`, `account` (null: unknown or none).
pub fn agents() -> serde_json::Value {
    let mut seen: std::collections::HashMap<&str, serde_json::Value> = std::collections::HashMap::new();
    let rows = agent_runtimes().iter().map(|rt| {
        let bin = if *rt == "claude" { "claude" } else { "codex" };
        let mut row = seen.entry(bin).or_insert_with(|| {
            let p = which(bin);
            if !p.is_absolute() {
                return serde_json::json!({"path": null, "version": null, "login": "not installed", "account": null});
            }
            let ver = probe(&p, &["--version"]).and_then(|(_, o)| version(&o));
            let (state, account) = login(bin, probe(&p, if bin == "claude" { &["auth", "status"] } else { &["login", "status"] }));
            serde_json::json!({"path": p.display().to_string(), "version": ver, "login": state, "account": account})
        }).clone();
        row["runtime"] = serde_json::json!(rt);
        row
    }).collect();
    serde_json::Value::Array(rows)
}

/// The menu and status line of a row: `<runtime> <version> · <account>` or `· not logged in` etc.
pub fn agent_text(r: &serde_json::Value) -> String {
    let rt = r["runtime"].as_str().unwrap_or("?");
    let tail = match (r["login"].as_str(), r["account"].as_str()) {
        (Some("not installed"), _) => return format!("{rt} · not installed"),
        (Some("logged in"), Some(a)) => a.to_string(),
        (Some("logged in"), None) => "logged in, account unknown".into(),
        (Some("not logged in"), _) => "not logged in".into(),
        _ => "login unknown".into(),
    };
    format!("{rt} {} · {tail}", r["version"].as_str().unwrap_or("unknown"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_probes_read_version_and_login_or_say_unknown() {
        assert_eq!(version("2.1.296 (Claude Code)\n").as_deref(), Some("2.1.296"));
        assert_eq!(version("codex-cli 0.159.3").as_deref(), Some("0.159.3"));
        assert_eq!(version("error"), None);
        let ok = |s: &str| Some((true, s.to_string()));
        assert_eq!(login("claude", ok(r#"{"loggedIn": true, "email": "a@b.c"}"#)), ("logged in", Some("a@b.c".into())));
        assert_eq!(login("claude", ok(r#"{"loggedIn": false}"#)), ("not logged in", None));
        assert_eq!(login("claude", ok("Unknown command: auth")), ("unknown", None));
        assert_eq!(login("codex", ok("Logged in using ChatGPT\n")), ("logged in", Some("ChatGPT".into())));
        assert_eq!(login("codex", ok("Logged in using an API key - sk-ab***cd")), ("logged in", Some("an API key".into())));
        assert_eq!(login("codex", Some((false, "Not logged in\n".into()))), ("not logged in", None));
        assert_eq!(login("codex", None), ("unknown", None));
        let row = |l: &str, a: serde_json::Value| serde_json::json!({"runtime": "codex", "version": "0.1", "login": l, "account": a});
        assert_eq!(agent_text(&row("logged in", "ChatGPT".into())), "codex 0.1 · ChatGPT");
        assert_eq!(agent_text(&row("not installed", serde_json::Value::Null)), "codex · not installed");
        assert_eq!(agent_text(&row("unknown", serde_json::Value::Null)), "codex 0.1 · login unknown");
    }

    #[test]
    fn app_turn_ends_on_a_new_task_complete_or_a_closed_app() {
        let p = std::env::temp_dir().join(format!("taskq-rollout-{}.jsonl", std::process::id()));
        let done = "{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_complete\"}}\n";
        std::fs::write(&p, format!("{done}{{\"type\":\"event_msg\",\"payload\":{{\"type\":\"user_message\"}}}}\n")).unwrap();
        let mut t = AppTurn { hand: None, log: p.clone(), text: String::new(), thread: "th".into(), done: completes(&p), rollout: Some(p.clone()), running: || true };
        assert_eq!((t.done, t.exited()), (1, None), "the handshake's task_complete does not end the turn");
        std::fs::OpenOptions::new().append(true).open(&p).unwrap().write_all(done.as_bytes()).unwrap();
        assert_eq!(t.exited(), Some(0));
        t.done = 2;
        t.running = || false;
        assert_eq!(t.exited(), Some(-1), "a closed app is a child exit");
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn commands_follow_the_runtime_table() {
        let codex = vec!["-s".to_string(), "workspace-write".to_string()];
        let env = Env { root: Path::new("/p"), exe: Path::new("/i/taskq"), permission_mode: "dontAsk", codex: &codex, publish: "pr" };
        let mut l = Launch { task: 7, title: "T".into(), attempt: 42, role: "worker".into(), rt: "codex".into(), session: "-".into(), url: "https://b/7".into(), resume: None };
        let args = |c: &Command| c.get_args().map(|a| a.to_string_lossy().into_owned()).collect::<Vec<_>>().join(" ");
        assert_eq!(args(&command(&env, &l)), "exec --json -s workspace-write -C /p -");
        l.session = "th1".into();
        l.resume = Some("answer".into());
        assert_eq!(args(&command(&env, &l)), "exec --json -s workspace-write resume th1 -");
        l.rt = "claude".into();
        assert_eq!(args(&command(&env, &l)), "-p --output-format stream-json --verbose --permission-mode dontAsk --resume th1");
        let c = command(&env, &l);
        let envs: Vec<_> = c.get_envs().map(|(k, v)| (k.to_string_lossy().into_owned(), v.map(|v| v.to_string_lossy().into_owned()))).collect();
        assert!(envs.contains(&("CODEX_THREAD_ID".into(), None)));
        assert!(envs.contains(&("TASKQ_ATTEMPT".into(), Some("42".into()))));
        l.role = "worker".into();
        let b = brief(&env, &l);
        assert!(b.contains("/i/taskq result 7 --sha") && b.contains("`Task: https://b/7`") && b.contains("open one PR") && b.contains("Changed: <user effect>\nOpen:") && b.contains("§ 2 Ask card"));
        l.rt = "codex-app".into();
        l.resume = None;
        let c = command(&env, &l);
        assert_eq!(c.get_program(), which("codex"));
        assert_eq!(args(&c), "exec --json -s workspace-write -C /p -");
        assert!(brief(&env, &l).contains("`env TASKQ_TASK=7 TASKQ_ATTEMPT=42 TASKQ_ROLE=worker TASKQ_ROOT=/p /i/taskq result 7 --sha"));
        let u = uuid();
        assert_eq!((u.len(), &u[14..15]), (36, "4"));
    }
}
