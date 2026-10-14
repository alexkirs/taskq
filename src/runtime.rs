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
    /// The exit code once the child has exited; `Err` is an unknown read (§ 4 reconciliation), never an exit.
    fn exited(&mut self) -> Result<Option<i32>, String>;
    fn kill(&mut self);
    fn id(&self) -> u32;
}

impl Proc for Child {
    fn exited(&mut self) -> Result<Option<i32>, String> {
        match self.try_wait() {
            Ok(s) => Ok(s.map(|s| s.code().unwrap_or(-1))),
            Err(e) => Err(format!("wait failed: {e}")),
        }
    }
    fn kill(&mut self) {
        // Windows: the child is `cmd.exe` running `codex.cmd`; node and `codex.exe` below it survive a plain kill (#933)
        #[cfg(windows)]
        let _ = crate::platform::quiet(Command::new("taskkill").args(["/F", "/T", "/PID", &Child::id(self).to_string()])).output();
        let _ = Child::kill(self);
        let _ = self.wait();
    }
    fn id(&self) -> u32 {
        Child::id(self)
    }
}

/// A process with this pid runs (§ 4 one service per root); pid reuse reads live, a ceiling accepted for a 60 s window.
#[cfg(unix)]
pub fn alive(pid: u32) -> bool {
    let Ok(pid) = i32::try_from(pid) else { return false };
    unsafe { libc::kill(pid, 0) == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM) }
}

#[cfg(windows)]
pub fn alive(pid: u32) -> bool {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut std::ffi::c_void;
        fn GetExitCodeProcess(h: *mut std::ffi::c_void, code: *mut u32) -> i32;
        fn CloseHandle(h: *mut std::ffi::c_void) -> i32;
    }
    unsafe {
        let h = OpenProcess(0x1000, 0, pid); // PROCESS_QUERY_LIMITED_INFORMATION
        if h.is_null() {
            return false;
        }
        let mut code = 0;
        let ok = GetExitCodeProcess(h, &mut code) != 0 && code == 259; // STILL_ACTIVE
        CloseHandle(h);
        ok
    }
}

/// A child of this process image's predecessor, kept across the update re-exec (§ 8 step 3). The PID is unchanged by
/// `exec`, so it is still our child and `waitpid` works.
#[cfg(unix)]
pub struct Adopted(pub i32);

#[cfg(unix)]
impl Proc for Adopted {
    fn exited(&mut self) -> Result<Option<i32>, String> {
        let mut st = 0;
        let r = unsafe { libc::waitpid(self.0, &mut st, libc::WNOHANG) };
        match r {
            0 => Ok(None),
            r if r == self.0 && libc::WIFEXITED(st) => Ok(Some(libc::WEXITSTATUS(st))),
            r if r == self.0 => Ok(Some(-1)),
            _ => {
                let e = std::io::Error::last_os_error();
                // not waitable: only an observed missing pid proves the exit
                if unsafe { libc::kill(self.0, 0) } != 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
                    Ok(Some(-1))
                } else {
                    Err(format!("waitpid {}: {e}", self.0))
                }
            }
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

/// A random UUID v4 for the pre-launch Claude identity and qualification fixtures.
pub fn uuid() -> String {
    use ring::rand::{SecureRandom, SystemRandom};
    let mut b = [0u8; 16];
    SystemRandom::new().fill(&mut b).expect("system random");
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}

/// Claude has a pre-launch alias; the log resolves it (and Codex `-`) to the native session.
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
    /// `<install>`: its `worker-policy.json` grants access (§ 8 Access).
    pub install: &'a Path,
    pub permission_mode: &'a str,
    pub codex: &'a [String],
    /// The § 7 delivery preset: `auto-merge`, `review-pr` or `direct`.
    pub publish: &'a str,
    /// Secret values: command environment only, never brief, argv or log.
    pub board_env: &'a [(String, String)],
}

fn executable(bin: PathBuf, access: &crate::policy::Effective) -> Command {
    match &access.profile {
        Some(p) => {
            let mut c = Command::new("/usr/bin/sandbox-exec");
            c.arg("-p").arg(p).arg(bin);
            c
        }
        None => Command::new(bin),
    }
}

pub fn command(env: &Env, l: &Launch, access: &crate::policy::Effective) -> Command {
    let mut c = executable(which(if l.rt == "claude" { "claude" } else { "codex" }), access);
    crate::platform::quiet(&mut c);
    let codex = access.codex.as_deref().unwrap_or(env.codex);
    let mode = access.claude.as_deref().unwrap_or(env.permission_mode);
    match (l.rt.as_str(), &l.resume) {
        ("claude", r) => {
            c.args(["-p", "--output-format", "stream-json", "--verbose", "--permission-mode", mode]);
            if r.is_some() { c.args(["--resume", &l.session]) } else { c.args(["--session-id", &l.session]) };
        }
        (_, None) => {
            c.args(["exec", "--json"]).args(codex).arg("-C").arg(env.root).arg("-");
        }
        (_, Some(_)) => {
            c.args(["exec", "--json"]).args(codex).args(["resume", &l.session, "-"]);
        }
    }
    environment(env, l, &mut c);
    c
}

fn environment(env: &Env, l: &Launch, c: &mut Command) {
    c.current_dir(env.root)
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .env_remove("CODEX_THREAD_ID")
        .env("TASKQ_TASK", l.task.to_string())
        .env("TASKQ_ATTEMPT", l.attempt.to_string())
        .env("TASKQ_ROLE", &l.role)
        .env("TASKQ_ROOT", env.root)
        .envs(env.board_env.iter().cloned());
}

const FORMATS: &str = "Put each `--text` field on its own line. The commands render and check the taskq.md templates and limits (§ 2 Ask card,\n\
     § 7 Result and close verdicts): Done ≤160, Problem 1-2 lines, 2-4 options ≤160, ≤2 links; result/close lines ≤240,\n\
     Changed and Open 1-2 lines, 1-8 checks. Over-limit text is rejected: shorten it, never hand-write card prose.\n";

/// taskq.md § 8 Folders: never search `/` or home; macOS asks the owner for Desktop, Documents, Downloads etc.
const FILES: &str = "Never run gh auth login. A missing board token is an owner item; report it through taskq ask.\n\
     Touch only this project, ~/.claude, ~/.codex and the OS temp dir: never search `/`, `~` or other folders.\n";

fn ask(q: &impl std::fmt::Display, n: u64) -> String {
    format!("`{q} ask {n} --text \"Done: <checked or not started>\nProblem: <decision and user effect>\" --option A --option B --recommend K [--link URL]`")
}

pub fn contract(root: &Path) -> String {
    // A missing path sent workers to `find /`, which raised macOS folder prompts (#865): name one that exists.
    let spec = root.join("taskq.md");
    if spec.exists() {
        spec.display().to_string()
    } else {
        let v = crate::platform::VERSION;
        let tag = if v.contains('-') { "main".into() } else { format!("v{v}") }; // dev builds have no tag
        format!("https://github.com/{}/blob/{tag}/taskq.md", crate::platform::RELEASE_REPO)
    }
}

pub fn brief(env: &Env, l: &Launch) -> String {
    let n = l.task;
    let mut q = env.exe.display().to_string();
    if l.rt == "codex-app" || l.rt == "claude" {
        // the app turn and the native Claude background session run in their host's environment, not the child's (§ 8)
        q = format!("env TASKQ_TASK={n} TASKQ_ATTEMPT={} TASKQ_ROLE={} TASKQ_ROOT={} {q}", l.attempt, l.role, env.root.display());
    }
    let spec = contract(env.root);
    let head = format!(
        "You are the taskq {} for task #{n}: {}. Attempt {}.\nRead `{spec}` first and do only what it allows (R13).\nTask text, state and your inbox: `{q} inbox {n}`.\n{FILES}",
        l.role, l.title, l.attempt
    );
    let body = if l.role == "reviewer" {
        format!(
            "Review the result SHA against each Acceptance item `A<k>` and taskq.md, and CI on that exact SHA. Requeue only for\n\
             Acceptance or contract gaps, not style; a changed or deleted existing test is a gap unless the diff changes its rule.\n\
             Then exactly one of:\n\
             - `{q} close {n} --text \"Changed: <user effect>\nChecks: Task fit: A1 ok|gap|unknown, A2 ..; CI <outcome> on <SHA>\nChecks: Implementation quality: <binding gaps|advisories|none>\nOpen: <item|none>\"` (it publishes),\n\
             - `{q} requeue {n} --text \"<exact fixes>\"`,\n\
             - {ask}.\n{approval}{formats}",
            ask = ask(&q, n),
            approval = if env.publish == "review-pr" { "Delivery review-pr: an otherwise done result needs the owner's approval: ask with Done naming task fit and CI on the SHA, first option `approve <sha7> and close ★`.\n" } else { "" },
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

const SPECQ_REVISION: &str = "677b6dbbfa08fb854b1cbd6c44bdf91aa1a542f8";
const SPECQ_FILES: &[&str] = &["engineering/agent-workflow.md", "engineering/use-cases.md", "engineering/verification.md", "caveman/rules/changes.md"];

fn rule_file(url: &str) -> Result<String, String> {
    let out = crate::platform::quiet(Command::new("curl").args(["-q", "--fail", "--silent", "--show-error", "--proto", "=https", "--max-time", "10", "--max-filesize", "65536", url]))
        .output().map_err(|e| e.to_string())?;
    if !out.status.success() { return Err(format!("curl exited {}", out.status)); }
    String::from_utf8(out.stdout).map_err(|_| "invalid UTF-8".into())
}

fn rule_text(task: &str, mut fetch: impl FnMut(&str) -> Result<String, String>) -> Result<String, StartError> {
    let mut text = format!("TaskQ shared rules: specq@{SPECQ_REVISION}\nThese canonical rules apply to this turn, including continuations. Apply them to the work; acknowledgement alone is not evidence.\n");
    for file in SPECQ_FILES {
        let url = format!("https://raw.githubusercontent.com/taskqorg/specq/{SPECQ_REVISION}/{file}");
        let body = fetch(&url).and_then(|body| {
            if body.trim().is_empty() || body.len() > 65536 { Err("empty or over 64 KiB".into()) } else { Ok(body) }
        }).map_err(|e| StartError { message: format!("specq@{SPECQ_REVISION} {file}: {e}"), reportable: false })?;
        text.push_str(&format!("\n--- specq/{file} ({url}) ---\n{body}\n"));
    }
    text.push_str(&format!("\nTaskQ task\n{task}"));
    Ok(text)
}

pub fn instructions(task: &str) -> Result<String, StartError> {
    rule_text(task, rule_file)
}

fn turn_text(env: &Env, l: &Launch, fetch: impl FnMut(&str) -> Result<String, String>) -> Result<String, StartError> {
    let mut task = brief(env, l);
    if let Some(answer) = &l.resume { task.push_str(&format!("\nContinue this same attempt with the following inbox message:\n{answer}")); }
    rule_text(&task, fetch)
}

/// Keep OS failure classes until the service decides whether a start failure is TaskQ's fault.
#[derive(Debug)]
pub struct StartError {
    message: String,
    pub reportable: bool,
}

impl StartError {
    pub fn io(error: std::io::Error, runtime: Option<&str>) -> Self {
        use std::io::ErrorKind;
        let reportable = matches!(error.kind(), ErrorKind::Other | ErrorKind::InvalidData | ErrorKind::UnexpectedEof | ErrorKind::BrokenPipe);
        let message = runtime.map_or_else(|| error.to_string(), |rt| format!("{rt}: {error}"));
        Self { message, reportable }
    }
}

impl std::fmt::Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { self.message.fmt(f) }
}

/// § 8 Access of `rt` here, resolved again for every start and resume; a refusal starts nothing.
pub fn access(env: &Env, rt: &str) -> Result<crate::policy::Effective, StartError> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from).unwrap_or_default();
    let a = crate::policy::resolve(&crate::policy::load(env.install), &crate::policy::Host::here(env.install, env.root, &home), rt, env.permission_mode, env.codex);
    match &a.refused {
        Some(r) => Err(StartError { message: format!("access refused: {r}"), reportable: false }),
        None => Ok(a),
    }
}

/// Start the child with its stdout and stderr in the attempt log; `codex-app` starts its handshake child (§ 8).
pub fn spawn(env: &Env, l: &Launch) -> Result<Box<dyn Proc>, StartError> {
    let access = access(env, &l.rt)?;
    log!("#{} attempt {} access {}", l.task, l.attempt, access.line());
    let text = turn_text(env, l, rule_file)?;
    log!("specq@{SPECQ_REVISION} loaded for {} #{} attempt {} ({})", l.role, l.task, l.attempt, if l.resume.is_some() { "resume" } else { "start" });
    let log = log_path(env.root, l);
    if l.rt == "codex-app" {
        let mut turn = AppTurn { hand: None, log, text, thread: l.session.clone(), done: 0, rollout: None, running: app_running };
        match &l.resume {
            Some(_) => {
                turn.queue().map_err(|message| StartError { message, reportable: false })?;
            }
            None => turn.hand = Some(Box::new(start(env, l, &turn.log, HANDSHAKE, &access)?)),
        }
        return Ok(Box::new(turn));
    }
    if l.rt == "claude" && (l.resume.is_none() || claude_background(env.root, &l.session).map_err(|message| StartError { message, reportable: false })?) {
        return ClaudeTurn::start(env, l, &log, &text, &access).map(|p| Box::new(p) as Box<dyn Proc>).map_err(|message| StartError { message, reportable: false });
    }
    Ok(Box::new(start(env, l, &log, &text, &access)?))
}

fn start(env: &Env, l: &Launch, log: &Path, text: &str, access: &crate::policy::Effective) -> Result<Child, StartError> {
    std::fs::create_dir_all(log.parent().unwrap()).map_err(|e| StartError::io(e, None))?;
    let out = File::options().create(true).append(true).open(log).map_err(|e| StartError::io(e, None))?;
    let err = out.try_clone().map_err(|e| StartError::io(e, None))?;
    let mut c = command(env, l, access);
    let mut child = c.stdin(Stdio::piped()).stdout(out).stderr(err).spawn().map_err(|e| StartError::io(e, Some(&l.rt)))?;
    let mut stdin = child.stdin.take().unwrap();
    if let Err(e) = stdin.write_all(text.as_bytes()) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(StartError::io(e, Some(&l.rt)));
    }
    Ok(child)
}

/// Native background sessions are not children of TaskQ. Keep their full identity and
/// observe the completed input before releasing the attempt's capacity.
pub struct ClaudeTurn {
    root: PathBuf,
    log: PathBuf,
    session: String,
    id: String,
    announced: bool,
    copied: usize,
    before: usize,
    mode: String,
    input: String,
    pid: u32,
    done: bool,
}

fn claude_agents(root: &Path) -> Result<Vec<serde_json::Value>, String> {
    let root = root.to_str().ok_or("Claude project path is not UTF-8")?;
    let (ok, out) = probe(&which("claude"), &["agents", "--json", "--all", "--cwd", root]).ok_or("Claude agents unavailable or timed out")?;
    if !ok { return Err(format!("claude agents: {out}")); }
    serde_json::from_str(&out).map_err(|e| format!("claude agents: {e}"))
}

fn claude_transcript(session: &str) -> Option<PathBuf> {
    if session.is_empty() || !session.chars().all(|c| c.is_ascii_hexdigit() || c == '-') { return None; }
    let home = std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from).or_else(||
        std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(|h| PathBuf::from(h).join(".claude")))?;
    std::fs::read_dir(home.join("projects")).ok()?.flatten().find_map(|d| {
        let p = d.path().join(format!("{session}.jsonl"));
        p.is_file().then_some(p)
    })
}

fn claude_stop(root: &Path, id: &str, known_pid: u32) -> Result<(), String> {
    let rows = claude_agents(root)?;
    let row = rows.iter().find(|v| v["id"] == id).ok_or("Claude stop identity unavailable")?;
    let pid = row["pid"].as_u64().unwrap_or(known_pid as u64) as u32;
    if row["pid"].as_u64().is_none() && (pid == 0 || !alive(pid)) && row["state"] != "working" { return Ok(()); }
    let (ok, out) = probe(&which("claude"), &["stop", id]).ok_or("Claude stop unavailable or timed out; state unknown")?;
    if !ok { return Err(format!("claude stop: {out}")); }
    // Native stop acknowledges the request before the process has actually stopped.
    for _ in 0..50 {
        let rows = claude_agents(root)?;
        let row = rows.iter().find(|v| v["id"] == id).ok_or("stopped Claude session disappeared; state unknown")?;
        if row["pid"].as_u64().is_none() && (pid == 0 || !alive(pid)) { return Ok(()); }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    Err("Claude stop still pending; do not launch another turn".into())
}

impl ClaudeTurn {
    pub fn start(env: &Env, l: &Launch, log: &Path, text: &str, access: &crate::policy::Effective) -> Result<Self, String> {
        let permission_mode = access.claude.as_deref().unwrap_or(env.permission_mode);
        let root = env.root.canonicalize().map_err(|e| e.to_string())?;
        let mut expected_id = None;
        let mut before = 0;
        if l.resume.is_some() {
            let rows = claude_agents(&root)?;
            let row = rows.iter().find(|v| v["sessionId"] == l.session).ok_or("Claude background session unavailable; no replacement")?;
            if row["pid"].as_u64().is_some() { return Err("Claude session is still running; no copy started".into()); }
            expected_id = Some(row["id"].as_str().ok_or("Claude background ID missing")?.to_string());
            let path = claude_transcript(&l.session).ok_or("Claude saved permissions unavailable")?;
            let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
            before = text.lines().count();
            let mode = text.lines().rev().filter_map(|s| serde_json::from_str::<serde_json::Value>(s).ok())
                .find_map(|v| v["permissionMode"].as_str().map(String::from)).ok_or("Claude saved permission mode unknown")?;
            if mode != permission_mode { return Err("Claude saved permission mode differs; a new attempt is required".into()); }
        }
        std::fs::create_dir_all(log.parent().ok_or("missing log directory")?).map_err(|e| e.to_string())?;
        let mut file = File::options().create(true).append(true).open(log).map_err(|e| e.to_string())?;
        let mut c = executable(which("claude"), access);
        environment(env, l, &mut c);
        c.current_dir(&root).arg("--bg");
        if l.resume.is_some() { c.args(["--resume", &l.session]); }
        else { c.args(["--remote-control", &l.title, "--permission-mode", permission_mode]); }
        if l.role == "operator" { for k in ["TASKQ_TASK", "TASKQ_ATTEMPT", "TASKQ_ROLE"] { c.env_remove(k); } }
        let mut child = crate::platform::quiet(&mut c).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| e.to_string())?;
        if let Err(e) = child.stdin.take().unwrap().write_all(text.as_bytes()) {
            return Err(format!("Claude background input: {e}; launch outcome unknown"));
        }
        let out = child.wait_with_output().map_err(|e| e.to_string())?;
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        writeln!(file, "{}", serde_json::json!({"type":"claude.launch", "stdout":stdout, "stderr":stderr})).map_err(|e| e.to_string())?;
        if !out.status.success() { return Err(format!("Claude background launch failed: {stderr}")); }
        let id = stdout.lines().find_map(|line| line.strip_prefix("backgrounded · ").and_then(|s| s.split_whitespace().next())).ok_or("Claude returned no background ID; launch outcome unknown")?;
        if expected_id.as_deref().is_some_and(|expected| expected != id) {
            claude_stop(&root, id, 0)?;
            return Err("Claude created a copy; stopped owned copy, original conversation preserved".into());
        }
        Ok(Self { root, log: log.into(), session: if l.resume.is_some() { l.session.clone() } else { String::new() }, id: id.into(),
            announced: false, copied: 0, before, mode: permission_mode.into(), input: text.trim().into(), pid: 0, done: false })
    }
}

impl Proc for ClaudeTurn {
    fn exited(&mut self) -> Result<Option<i32>, String> {
        if self.done { return Ok(Some(0)); }
        let rows = claude_agents(&self.root)?;
        let row = rows.iter().find(|v| v["id"] == self.id).ok_or("Claude session unavailable; state unknown")?;
        // On Windows a waking session briefly publishes a different UUID before restore.
        if !self.announced {
            if row["status"].as_str().is_none() { return if row["state"] == "stopped" { Ok(Some(-1)) } else { Ok(None) }; }
            let session = row["sessionId"].as_str().ok_or("Claude session ID unknown")?;
            if !self.session.is_empty() && self.session != session { return Err("Claude resumed another conversation; identity unknown".into()); }
            self.session = session.into();
            let mut file = File::options().append(true).open(&self.log).map_err(|e| e.to_string())?;
            writeln!(file, "{}", serde_json::json!({"type":"thread.started", "thread_id":session, "session_id":session, "runtime":"claude-bg", "permission_mode":self.mode})).map_err(|e| e.to_string())?;
            self.announced = true;
        }
        self.pid = row["pid"].as_u64().unwrap_or(self.pid as u64) as u32;
        let path = claude_transcript(&self.session).ok_or("Claude transcript unavailable; completion unknown")?;
        let content = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        let events: Vec<serde_json::Value> = content.lines().skip(self.before).filter_map(|line| serde_json::from_str(line).ok()).collect();
        let Some(start) = events.iter().rposition(|v| v["type"] == "user" && v["message"]["content"].as_str().is_some_and(|s| s.trim() == self.input)) else { return Ok(None); };
        let turn = &events[start..];
        let mut log = File::options().append(true).open(&self.log).map_err(|e| e.to_string())?;
        for event in turn.iter().skip(self.copied) { writeln!(log, "{event}").map_err(|e| e.to_string())?; }
        self.copied = turn.len();
        if row["status"] != "idle" && row["state"] != "done" && row["state"] != "blocked" {
            return if row["state"] == "stopped" { Ok(Some(-1)) } else { Ok(None) };
        }
        if !turn.iter().any(|v| v["type"] == "system" && v["subtype"] == "turn_duration") { return Ok(None); }
        let answer = turn.iter().rev().filter(|v| v["type"] == "assistant").find_map(|v|
            v["message"]["content"].as_array()?.iter().rev().find_map(|b| (b["type"] == "text").then(|| b["text"].as_str()).flatten())).ok_or("Claude completed without final text; inspect native logs")?;
        if self.pid != 0 { claude_stop(&self.root, &self.id, self.pid)?; }
        writeln!(log, "{}", serde_json::json!({"type":"result", "session_id":self.session, "result":answer})).map_err(|e| e.to_string())?;
        self.done = true;
        Ok(Some(0))
    }
    fn kill(&mut self) { if let Err(e) = claude_stop(&self.root, &self.id, self.pid) { log!("{e}"); } }
    fn id(&self) -> u32 { self.pid }
}

/// Only a URL registered by the native session is a Remote Control link.
pub fn claude_remote(session: &str) -> Option<String> {
    let text = std::fs::read_to_string(claude_transcript(session)?).ok()?;
    text.lines().rev().filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok()).find_map(|v| {
        let url = v["url"].as_str()?;
        let id = url.strip_prefix("https://claude.ai/code/session_")?;
        (v["subtype"] == "bridge_status" && !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric()))
            .then(|| url.to_string())
    })
}

pub fn claude_background(root: &Path, session: &str) -> Result<bool, String> {
    if claude_agents(root)?.iter().any(|v| v["sessionId"] == session) { return Ok(true); }
    // A missing native job must not silently resume a background conversation in print mode.
    Ok(claude_transcript(session).and_then(|p| std::fs::read_to_string(p).ok()).is_some_and(|text|
        text.lines().filter_map(|s| serde_json::from_str::<serde_json::Value>(s).ok()).any(|v| v["sessionKind"] == "bg")))
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
    fn exited(&mut self) -> Result<Option<i32>, String> {
        if let Some(h) = &mut self.hand {
            let Some(code) = h.exited()? else { return Ok(None) };
            self.hand = None;
            self.thread = codex_thread(&self.log).unwrap_or_default();
            if self.thread.is_empty() || self.queue().is_err() {
                return Ok(Some(if code == 0 { -1 } else { code }));
            }
            return Ok(None);
        }
        if !(self.running)() {
            return Ok(Some(-1));
        }
        if self.rollout.is_none() {
            self.rollout = rollout(&self.thread);
        }
        // ponytail: a thread without a found rollout stays live until the app closes; ttl-based expiry if it matters
        Ok(self.rollout.as_ref().filter(|p| completes(p) > self.done).map(|_| 0))
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
    // Drain both pipes while waiting: agents --all can exceed an OS pipe buffer.
    let read = |mut pipe: Box<dyn std::io::Read + Send>| std::thread::spawn(move || {
        let mut bytes = Vec::new();
        pipe.read_to_end(&mut bytes).ok().map(|_| bytes)
    });
    let out = read(Box::new(ch.stdout.take()?));
    let err = read(Box::new(ch.stderr.take()?));
    let mut status = None;
    for _ in 0..100 {
        match ch.try_wait() {
            Ok(Some(s)) => { status = Some(s); break; }
            Err(_) => break,
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(100)),
        }
    }
    if status.is_none() { let _ = ch.kill(); let _ = ch.wait(); }
    let (out, err) = (out.join().ok()??, err.join().ok()??);
    Some((status?.success(), format!("{}{}", String::from_utf8_lossy(&out), String::from_utf8_lossy(&err))))
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

    // Chain: incident runtime start. Risk: environment errors reported as taskq faults.
    #[test]
    fn incident_start_preserves_diagnostic_and_excludes_environment_errors() {
        use std::io::{Error, ErrorKind};
        for kind in [ErrorKind::NotFound, ErrorKind::PermissionDenied, ErrorKind::OutOfMemory, ErrorKind::WouldBlock,
            ErrorKind::TimedOut, ErrorKind::StorageFull, ErrorKind::ResourceBusy, ErrorKind::InvalidInput, ErrorKind::Unsupported] {
            let error = StartError::io(Error::new(kind, "unchanged diagnostic"), Some("claude"));
            assert!(!error.reportable, "{kind:?}");
            assert_eq!(error.to_string(), "claude: unchanged diagnostic");
        }
        assert!(StartError::io(Error::other("internal launch fault"), None).reportable);
    }

    // Chain: § 5 taskq agents probe. Risk: unreadable output shown as a known state.
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

    // Chain: § 8 codex-app turn end. Risk: a turn ends early or never ends.
    #[test]
    fn app_turn_ends_on_a_new_task_complete_or_a_closed_app() {
        let p = std::env::temp_dir().join(format!("taskq-rollout-{}.jsonl", std::process::id()));
        let done = "{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_complete\"}}\n";
        std::fs::write(&p, format!("{done}{{\"type\":\"event_msg\",\"payload\":{{\"type\":\"user_message\"}}}}\n")).unwrap();
        let mut t = AppTurn { hand: None, log: p.clone(), text: String::new(), thread: "th".into(), done: completes(&p), rollout: Some(p.clone()), running: || true };
        assert_eq!((t.done, t.exited()), (1, Ok(None)), "the handshake's task_complete does not end the turn");
        std::fs::OpenOptions::new().append(true).open(&p).unwrap().write_all(done.as_bytes()).unwrap();
        assert_eq!(t.exited(), Ok(Some(0)));
        t.done = 2;
        t.running = || false;
        assert_eq!(t.exited(), Ok(Some(-1)), "a closed app is a child exit");
        let _ = std::fs::remove_file(p);
    }

    // Chain: § 8 Access adapter on the host OS (A3, A4). Risk: a Full grant keeps the runtime's own permission or
    // sandbox flags on start or resume, or a host without enforcement starts a non-Full worker or reviewer.
    #[test]
    fn access_maps_full_and_refuses_unknown_enforcement_before_start() {
        let install = std::env::temp_dir().join(format!("taskq-access-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&install);
        std::fs::create_dir_all(&install).unwrap();
        let codex: Vec<String> = ["-s", "workspace-write", "-m", "gpt-5", "-c", "model_reasoning_effort=high"].map(String::from).into();
        let root = std::env::temp_dir();
        let env = Env { root: &root, exe: Path::new("/i/taskq"), install: &install, permission_mode: "dontAsk", codex: &codex, publish: "pr", board_env: &[] };
        let args = |c: &Command| c.get_args().map(|a| a.to_string_lossy().into_owned()).collect::<Vec<_>>().join(" ");
        let mut l = Launch { task: 7, title: "T".into(), attempt: 42, role: "worker".into(), rt: "claude".into(), session: "s".into(), url: "https://b/7".into(), resume: None };
        crate::policy::grant(&install, "claude", "full", None, "op", 1).unwrap();
        crate::policy::grant(&install, "codex", "full", None, "op", 1).unwrap();
        for resume in [None, Some("answer".to_string())] {
            l.resume = resume;
            l.rt = "claude".into();
            assert!(args(&command(&env, &l, &access(&env, "claude").unwrap())).contains("--permission-mode bypassPermissions"));
            l.rt = "codex".into();
            assert!(args(&command(&env, &l, &access(&env, "codex").unwrap())).starts_with("exec --json -s danger-full-access -m gpt-5 -c model_reasoning_effort=high "));
        }
        // the host without enforcement (every OS but macOS): no grant or a non-Full one refuses before any child or fetch
        if !cfg!(target_os = "macos") {
            for (rt, mode) in [("claude", "none"), ("codex", "workspace"), ("claude", "restricted")] {
                crate::policy::grant(&install, rt, mode, None, "op", 2).unwrap();
                l.rt = rt.into();
                let e = spawn(&env, &l).err().expect("refused");
                assert!(e.message.starts_with("access refused: ") && e.message.contains(std::env::consts::OS), "{}", e.message);
            }
        }
        let _ = std::fs::remove_dir_all(&install);
    }

    // Chain: § 8 rule delivery. Risk: a new/resumed worker or reviewer receives only links or mixed rules.
    #[test]
    fn rules_reach_every_runtime_role_and_resume() {
        let env = Env { root: Path::new("/p"), exe: Path::new("/i/taskq"), install: Path::new("/i"), permission_mode: "dontAsk", codex: &[], publish: "pr", board_env: &[] };
        for rt in ["claude", "codex", "codex-app"] {
            for role in ["worker", "reviewer"] {
                for resume in [None, Some("Owner answer: preserve current work.\nLiteral $HOME stays text.".into())] {
                    let l = Launch { task: 7, title: "T".into(), attempt: 42, role: role.into(), rt: rt.into(), session: "same-session".into(), url: "https://b/7".into(), resume };
                    let mut urls = Vec::new();
                    let text = turn_text(&env, &l, |url| { urls.push(url.to_string()); Ok(format!("# Rule from {url}\nApply this rule.")) }).unwrap();
                    assert_eq!(urls.len(), SPECQ_FILES.len(), "{rt} {role}: rules must actually load");
                    for file in SPECQ_FILES {
                        let url = format!("https://raw.githubusercontent.com/taskqorg/specq/{SPECQ_REVISION}/{file}");
                        assert!(urls.contains(&url) && text.contains(&format!("# Rule from {url}\nApply this rule.")));
                    }
                    assert!(text.find(SPECQ_REVISION).unwrap() < text.find("TaskQ task").unwrap());
                    if let Some(answer) = &l.resume { assert!(text.ends_with(answer)); }
                    else { assert!(text.ends_with(&brief(&env, &l))); }
                    assert_eq!(l.session, "same-session");
                    // Native sessions do not inherit the child env; without the prefix `result` is refused as a non-worker.
                    assert_eq!(brief(&env, &l).contains("env TASKQ_TASK=7 TASKQ_ATTEMPT=42 "), rt != "codex", "{rt} {role}: attempt identity");
                    assert!(text.contains("https://github.com/taskqorg/taskq/blob/") && text.contains(FILES));
                    for bad in [Err("offline".into()), Ok(String::new()), Ok("x".repeat(65537))] {
                        assert!(turn_text(&env, &l, |_| bad.clone()).is_err());
                    }
                }
            }
        }
    }

}
