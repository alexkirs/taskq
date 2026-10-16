//! Runtimes: stdin briefs or file-backed Claude background input (taskq.md section 8).

use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};

#[cfg(any(target_os="macos",windows))]
mod app_control;

#[cfg(windows)]
pub mod windows_process;

fn app_cancel(thread:&str,turn:&str)->Result<(),String> {
    #[cfg(any(target_os="macos",windows))] {app_control::cancel(thread,turn)}
    #[cfg(not(any(target_os="macos",windows)))] {let _=(thread,turn);Err("Codex App native cancellation unavailable on this platform; execution remains reserved".into())}
}

pub struct Launch {
    /// Exact board intent, assigned by the service before native submission; absent for non-task probes.
    pub execution: Option<u64>,
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
    fn job(&self)->Option<String> {None}
    /// Save missing native recovery identity after launch without surrendering the handle on failure.
    fn checkpoint(&mut self, _cfg:&crate::service::Config, _launch:&Launch, _execution:u64)->Result<(),String> { Err("native recovery checkpoint unsupported".into()) }
    fn native_identity(&self)->Result<coreq::process::Identity,String> { Err("native process identity unavailable for handover".into()) }
    fn input_offset(&self) -> Result<u64,String> { Err("native input boundary unavailable for handover".into()) }
    /// Exact input was accepted by the native runtime; a handle or process exit alone is not evidence.
    fn accepted(&mut self) -> Result<bool, String> { Err("native input acceptance unavailable; reconcile this execution".into()) }
    /// The exit code once the child has exited; `Err` is an unknown read (§ 4 reconciliation), never an exit.
    fn exited(&mut self) -> Result<Option<i32>, String>;
    fn exit_error(&self) -> Option<&'static str> { None }
    /// Request termination; success is not exit. The owner retains this handle until `exited` confirms it.
    fn kill(&mut self) -> Result<(), String>;
    fn id(&self) -> u32;
}

/// One owned stop attempt; a pending/failed read never releases the caller's process or capacity.
pub fn stop(proc: &mut dyn Proc) -> Result<i32, String> {
    coreq::telemetry::capture::operation("runtime.stop", || {
        proc.kill()?;
        proc.exited()?.ok_or_else(|| "stop pending; process exit unconfirmed".into())
    }, |_| "stop-unconfirmed")
}

impl Proc for Child {
    fn native_identity(&self)->Result<coreq::process::Identity,String> {coreq::process::identity(Child::id(self)).map_err(|e|e.to_string())?.ok_or("native process already exited".into())}
    fn exited(&mut self) -> Result<Option<i32>, String> {
        match self.try_wait() {
            Ok(s) => Ok(s.map(|s| s.code().unwrap_or(-1))),
            Err(e) => Err(format!("wait failed: {e}")),
        }
    }
    fn kill(&mut self) -> Result<(), String> {
        if self.exited()?.is_some() { return Ok(()); }
        // Windows: the child is `cmd.exe` running `codex.cmd`; node and `codex.exe` below it survive a plain kill (#933)
        #[cfg(windows)]
        coreq::process::output("runtime.taskkill", Command::new("taskkill").args(["/F", "/T", "/PID", &Child::id(self).to_string()]), std::time::Duration::from_secs(10)).map_err(|e| format!("process tree stop unconfirmed: {e}"))?;
        #[cfg(not(windows))]
        Child::kill(self).map_err(|e| format!("process stop unconfirmed: {e}"))?;
        Ok(())
    }
    fn id(&self) -> u32 {
        Child::id(self)
    }
}

/// A process with this pid runs (§ 4 one service per root); pid reuse reads live, a ceiling accepted for a 60 s window.
#[cfg(unix)]
pub fn alive(pid: u32) -> bool {
    liveness(pid).unwrap_or(false)
}

#[cfg(unix)]
pub fn liveness(pid: u32) -> Result<bool, String> {
    let pid = i32::try_from(pid).ok().filter(|p| *p > 0).ok_or("invalid process id")?;
    if unsafe { libc::kill(pid, 0) } == 0 { return Ok(true); }
    let e = std::io::Error::last_os_error();
    match e.raw_os_error() { Some(libc::ESRCH) => Ok(false), Some(libc::EPERM) => Ok(true), _ => Err(format!("process liveness unknown: {e}")) }
}

#[cfg(windows)]
pub fn alive(pid: u32) -> bool {
    liveness(pid).unwrap_or(false)
}

#[cfg(windows)]
pub fn liveness(pid: u32) -> Result<bool, String> {
    coreq::process::identity(pid).map(|identity|identity.is_some()).map_err(|e|format!("process liveness unknown: {e}"))
}

/// Protected native identity recovered after re-exec or cold restart; control stays bound to that incarnation.
#[cfg(unix)]
pub struct Adopted { identity:coreq::process::Identity, ended:Option<i32>, direct:bool }

#[cfg(unix)]
impl Adopted {
    pub fn new(identity:coreq::process::Identity)->Self { Self {identity,ended:None,direct:false} }
}

#[cfg(unix)]
impl Proc for Adopted {
    fn exited(&mut self) -> Result<Option<i32>, String> {
        if self.ended.is_some() { return Ok(self.ended); }
        match coreq::process::identity(self.identity.pid) {
            Ok(identity) if identity.as_ref()!=Some(&self.identity) => { self.ended=Some(-1); return Ok(self.ended); },
            Ok(_) => {},
            Err(_) if self.direct => {}, // An unreaped owned child cannot reuse its PID, including while zombie.
            Err(e) => return Err(format!("process identity unreadable: {e}; native reconciliation required")),
        }
        let pid=self.identity.pid as i32;
        let mut st = 0;
        let r = unsafe { libc::waitpid(pid, &mut st, libc::WNOHANG) };
        match r {
            0 => { self.direct=true; Ok(None) },
            r if r == pid => { self.ended=Some(if libc::WIFEXITED(st) {libc::WEXITSTATUS(st)} else {-1}); Ok(self.ended) },
            _ if std::io::Error::last_os_error().raw_os_error()==Some(libc::ECHILD) => { self.direct=false; Ok(None) },
            _ => Err(format!("waitpid {pid}: {}; direct-child ownership unconfirmed",std::io::Error::last_os_error())),
        }
    }
    fn kill(&mut self) -> Result<(), String> {
        if self.exited()?.is_some() { return Ok(()); }
        if !self.direct { return coreq::process::terminate(&self.identity).map_err(|e|format!("recovered process stop unconfirmed: {e}; verify termination on its host")); }
        if unsafe { libc::kill(self.identity.pid as i32, libc::SIGKILL) } != 0 {
            return Err(format!("adopted process stop unconfirmed: {}", std::io::Error::last_os_error()));
        }
        Ok(())
    }
    fn id(&self) -> u32 {
        self.identity.pid
    }
}

/// The saved execution scope, rather than a vanished launcher's PID, decides whether it remains live.
pub fn execution_live(row:&crate::data::KidLine)->Result<bool,String> {
    #[cfg(windows)] if row.runtime=="codex" {
        let key=row.job.as_deref().ok_or("legacy Windows execution has no native job scope; reconcile descendants before release")?;
        return windows_process::Execution::recover(row.process.clone(),key)?.exited().map(|ended|ended.is_none());
    }
    if row.job.is_some() {return Err("native job scope belongs to another adapter/platform; preserve record".into());}
    coreq::process::identity(row.process.pid).map(|id|id.as_ref()==Some(&row.process)).map_err(|e|e.to_string())
}

/// Cleanup may use an already-attributable board Exit; an absent native scope cannot create that proof.
pub fn released_execution_live(row:&crate::data::KidLine)->Result<bool,String> {
    #[cfg(windows)] if row.runtime=="codex" {
        let key=row.job.as_deref().ok_or("legacy Windows execution has no native job scope; reconcile before cleanup")?;
        return match windows_process::Execution::open(row.process.clone(),key)? {Some(mut scope)=>scope.exited().map(|ended|ended.is_none()),None=>Ok(false)};
    }
    execution_live(row)
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
    attempt_log(root, &l.role, l.task, l.attempt)
}

pub fn attempt_log(root: &Path, role: &str, task: u64, attempt: u64) -> PathBuf {
    let p = if role == "reviewer" { 'R' } else { 'T' };
    root.join(".taskq").join(format!("{p}{task}-{attempt}.log"))
}

/// Model use of the attempt log after byte `pos` (§ 4 Profiling): Claude assistant messages (calls, input tokens
/// with cache reads and writes), Codex `turn.completed` input tokens (cached included; calls not reported).
pub fn usage(log: &Path, pos: u64, rt: &str) -> crate::profile::Usage {
    use crate::profile::Usage;
    use std::io::{Read, Seek, SeekFrom};
    if rt == "codex-app" { return Usage::Unknown("app-turn"); }
    let mut text = String::new();
    let Ok(mut f) = File::open(log) else { return Usage::Unknown("log-unreadable") };
    if f.seek(SeekFrom::Start(pos)).and_then(|_| f.read_to_string(&mut text)).is_err() { return Usage::Unknown("log-unreadable"); }
    let (mut ids, mut tokens, mut turns, mut missing) = (std::collections::HashSet::new(), 0u64, false, false);
    for v in text.lines().filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok()) {
        let u = if v["type"] == "assistant" { &v["message"]["usage"] } else { &v["usage"] };
        let sum = ["input_tokens", "cache_read_input_tokens", "cache_creation_input_tokens"].iter().filter_map(|k| u[k].as_u64()).sum::<u64>();
        // a call without input tokens makes the stage's tokens unknown, never a smaller known sum
        let counted = match v["type"].as_str() {
            // one message spans several lines with the same id and usage
            Some("assistant") => v["message"]["id"].as_str().is_some_and(|id| ids.insert(id.to_string())),
            Some("turn.completed") => { turns = true; true }
            _ => false,
        };
        if counted { if u["input_tokens"].is_u64() { tokens += sum } else { missing = true } }
    }
    let tokens = (!missing).then_some(tokens);
    if !ids.is_empty() { Usage::Known { calls: Some(ids.len() as u64), tokens } }
    else if turns { Usage::Known { calls: None, tokens } }
    else { Usage::Unknown("no-usage") }
}

/// The final text of a turn: Claude's last `result`, else Codex's last `agent_message`.
pub fn final_text(events: &[serde_json::Value]) -> Option<String> {
    events.iter().rev().find_map(|v| {
        if v["type"] == "result" { v["result"].as_str().map(String::from) }
        else if v["type"] == "item.completed" && v["item"]["type"] == "agent_message" { v["item"]["text"].as_str().map(String::from) }
        else { None }
    })
}

/// The worker's last message in an attempt log, for the `gone` reason (§ 4 step 1); None when unreadable.
pub fn last_message(log: &Path) -> Option<String> {
    let text = String::from_utf8_lossy(&std::fs::read(log).ok()?).into_owned();
    final_text(&text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect::<Vec<_>>())
}

/// Worker text as one public line (§ 4 step 1, § 7 rule 6): whitespace collapsed, absolute paths made relative to
/// `root` or replaced by `<path>`, at most `max` characters ending in `…`. The full text stays in the attempt log.
pub fn public_line(text: &str, root: &Path, max: usize) -> String {
    let root = slashed(&root.to_string_lossy());
    let mut out = String::new();
    let mut word = String::new();
    for c in text.chars().chain([' ']) {
        if !(c.is_whitespace() || "()[]<>\"'`".contains(c)) {
            word.push(c);
            continue;
        }
        if !word.is_empty() {
            out += &public_word(&word, &root);
            word.clear();
        }
        if !c.is_whitespace() { out.push(c) } else if !out.is_empty() && !out.ends_with(' ') { out.push(' ') }
    }
    let out = out.trim_end();
    if out.chars().count() <= max { return out.into(); }
    out.chars().take(max.saturating_sub(1)).collect::<String>().trim_end().to_string() + "…"
}

/// A path with `/` separators, without `file://`, `\\?\` or the `/` before a drive letter of a Markdown link.
fn slashed(p: &str) -> String {
    let p = p.replace('\\', "/");
    let p = p.strip_prefix("file://").unwrap_or(&p);
    let p = p.strip_prefix("//?/").unwrap_or(p);
    let b = p.as_bytes();
    let p = if b.len() > 2 && b[0] == b'/' && b[1].is_ascii_alphabetic() && b[2] == b':' { &p[1..] } else { p };
    p.trim_end_matches('/').to_string()
}

fn public_word(word: &str, root: &str) -> String {
    let core = word.trim_end_matches(['.', ',', ';', ':']);
    let (w, tail) = (slashed(core), &word[core.len()..]);
    if !root.is_empty() && w.get(..root.len()).is_some_and(|p| p.eq_ignore_ascii_case(root)) {
        match &w[root.len()..] {
            "" => return format!(".{tail}"),
            rest if rest.starts_with('/') => return format!("{}{tail}", &rest[1..]),
            _ => {}
        }
    }
    let b = w.as_bytes();
    let drive = b.len() > 2 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'/';
    let abs = drive || w.starts_with("~/") || (w.starts_with('/') && w[1..].contains('/'));
    if abs { format!("<path>{tail}") } else { word.into() }
}

/// A CLI child whose log shows its turn ended counts as exited even when its process lingers: Codex on Windows kept
/// running after a rate-limited `taskq result` (#1034). The service then stops its tree (§ 4 step 1, R11).
pub struct CliTurn {
    pub proc: Box<dyn Proc>,
    pub log: PathBuf,
    /// Log bytes already read: a resume appends to the log of earlier turns.
    pub pos: u64,
    pub start: u64,
    pub stopping: bool,
    pub accepted: bool,
    pub read_error: Option<String>,
    pub native: Option<coreq::process::Identity>,
    pub recovery: Option<Recovery>,
}

pub struct Recovery {
    install:PathBuf, root:PathBuf, task:u64, attempt:u64, execution:u64,
}

impl CliTurn {
    fn persist(&mut self)->Result<(),String> {
        let Some(r)=&self.recovery else { return Ok(()) };
        let row=crate::data::KidLine {task:r.task,attempt:r.attempt,runtime:"codex".into(),process:self.native_identity()?,execution:r.execution,offset:self.input_offset()?,job:self.proc.job()};
        crate::checkpoint_kid(&r.install,&r.root,row).map_err(|e|format!("native recovery checkpoint failed: {e}"))?;
        self.recovery=None;
        Ok(())
    }
    fn ended(&mut self) -> Result<bool,String> {
        use std::io::{Read, Seek, SeekFrom};
        let mut rest = vec![];
        let mut f = File::open(&self.log).map_err(|e|format!("native input log unreadable: {e}"))?;
        if f.metadata().map_err(|e|e.to_string())?.len() < self.pos { return Err("native input log truncated; input boundary unknown".into()); }
        f.seek(SeekFrom::Start(self.pos)).and_then(|_| f.read_to_end(&mut rest)).map_err(|e|format!("native input log read failed: {e}"))?;
        let Some(end) = rest.iter().rposition(|b| *b == b'\n') else { return Ok(false) };
        self.pos += end as u64 + 1;
        let mut ended = false;
        for v in String::from_utf8_lossy(&rest[..end]).lines().filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok()) {
            self.accepted |= matches!(v["type"].as_str(), Some("turn.started" | "assistant"));
            ended |= matches!(v["type"].as_str(), Some("turn.completed" | "turn.failed" | "result"));
        }
        Ok(ended)
    }
}

impl Proc for CliTurn {
    fn job(&self)->Option<String> {self.proc.job()}
    fn checkpoint(&mut self,cfg:&crate::service::Config,l:&Launch,execution:u64)->Result<(),String> {
        if l.rt!="codex" { return Ok(()) }
        self.recovery=Some(Recovery {install:cfg.install.clone(),root:cfg.root.clone(),task:l.task,attempt:l.attempt,execution});
        self.persist()
    }
    fn native_identity(&self)->Result<coreq::process::Identity,String> { self.native.clone().ok_or("native process identity was not observed at spawn".into()) }
    fn input_offset(&self)->Result<u64,String> {
        if std::fs::metadata(&self.log).map_err(|e|e.to_string())?.len() < self.pos { return Err("native input log truncated; handover refused".into()); }
        Ok(self.start)
    }
    fn accepted(&mut self) -> Result<bool, String> {
        if !self.accepted { if let Some(e)=&self.read_error { return Err(e.clone()); } }
        Ok(self.accepted)
    }
    fn exited(&mut self) -> Result<Option<i32>, String> {
        let checkpoint=self.persist();
        let ended = self.ended();
        self.read_error=ended.as_ref().err().cloned();
        if let Some(code) = self.proc.exited()? { return Ok(Some(code)); }
        if !self.stopping && ended? {
            self.stopping = true;
            log!("{}: turn ended; stopping its lingering process", self.log.display());
        }
        if self.stopping { self.proc.kill()?; self.proc.exited() } else { checkpoint?; Ok(None) }
    }
    fn kill(&mut self) -> Result<(), String> { self.stopping = true; self.proc.kill() }
    fn id(&self) -> u32 { self.proc.id() }
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

/// An npm launcher is not the executor: SIGKILL cannot reach the native child through Node.
fn codex_native(bin: PathBuf) -> Result<PathBuf,String> {
    if cfg!(windows) || !bin.exists() { return Ok(bin); }
    let bin=bin.canonicalize().map_err(|e|format!("Codex executable unavailable: {e}"))?;
    let native=|path:&Path|->Result<bool,String> {
        let mut magic=[0;4];
        File::open(path).and_then(|mut f|f.read_exact(&mut magic)).map_err(|e|format!("Codex executable unreadable: {e}"))?;
        Ok(matches!(magic,[0x7f,b'E',b'L',b'F']|[0xcf,0xfa,0xed,0xfe]|[0xfe,0xed,0xfa,0xcf]|[0xce,0xfa,0xed,0xfe]|[0xfe,0xed,0xfa,0xce]|[0xca,0xfe,0xba,0xbe]|[0xbe,0xba,0xfe,0xca]))
    };
    if native(&bin)? { return Ok(bin); }
    let package=bin.parent().filter(|p|p.file_name().is_some_and(|n|n=="bin")).and_then(Path::parent)
        .filter(|_|bin.file_name().is_some_and(|n|n=="codex.js"))
        .ok_or("Codex launcher ownership unsupported; select a native Codex executable")?;
    let metadata:serde_json::Value=serde_json::from_reader(File::open(package.join("package.json")).map_err(|e|format!("Codex package unavailable: {e}"))?.take(64*1024))
        .map_err(|e|format!("Codex package invalid: {e}"))?;
    if metadata["name"]!="@openai/codex" { return Err("Codex launcher package identity differs".into()); }
    let (platform,target)=match (std::env::consts::OS,std::env::consts::ARCH) {
        ("macos","aarch64")=>("darwin-arm64","aarch64-apple-darwin"),
        ("macos","x86_64")=>("darwin-x64","x86_64-apple-darwin"),
        ("linux","aarch64")=>("linux-arm64","aarch64-unknown-linux-musl"),
        ("linux","x86_64")=>("linux-x64","x86_64-unknown-linux-musl"),
        _=>return Err("Codex native package unsupported on this platform".into()),
    };
    // Match Node's package lookup along the entrypoint's ancestors; never scan other installations.
    let installed=package.ancestors().map(|p|p.join(format!("node_modules/@openai/codex-{platform}")))
        .find(|p|p.join("package.json").is_file()).unwrap_or_else(||package.to_path_buf());
    let executable=installed.join(format!("vendor/{target}/bin/codex"));
    if !native(&executable)? { return Err("Codex package executor is not a native binary".into()); }
    executable.canonicalize().map_err(|e|format!("Codex native executable unavailable: {e}"))
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

fn executable(bin: PathBuf, access: &crate::policy::Effective, env: &Env) -> Result<Command, String> {
    #[cfg(windows)]
    if access.windows { return crate::windows_sandbox::command(env); }
    if access.linux {
        let mut c = Command::new("/usr/local/libexec/taskq-sandbox");
        c.arg(env.root).arg(env.install).arg(&access.mode).arg(bin);
        return Ok(c);
    }
    Ok(match &access.profile {
        Some(p) => {
            let mut c = Command::new("/usr/bin/sandbox-exec");
            c.arg("-p").arg(p).arg(bin);
            c
        }
        None => Command::new(bin),
    })
}

pub fn command(env: &Env, l: &Launch, access: &crate::policy::Effective) -> Result<Command, String> {
    let bin=if l.rt=="claude" {which("claude")} else {codex_native(which("codex"))?};
    let mut c = executable(bin, access, env)?;
    crate::platform::quiet(&mut c);
    let codex = access.codex.as_deref().unwrap_or(env.codex);
    let mode = access.claude.as_deref().unwrap_or(env.permission_mode);
    match (l.rt.as_str(), &l.resume) {
        ("claude", r) => {
            c.args(["-p", "--output-format", "stream-json", "--verbose", "--permission-mode", mode, "--settings", &compact_settings(env.root, l)]);
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
    Ok(c)
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
     § 7 Result and close verdicts): Done ≤160, Problem 1-2 lines, 2-4 options ≤160 (none: owner action), ≤2 links; result/close lines ≤240,\n\
     Changed and Open 1-2 lines, 1-8 checks. Over-limit text is rejected: shorten it, never hand-write card prose.\n";

/// taskq.md § 8 Folders: never search `/` or home; macOS asks the owner for Desktop, Documents, Downloads etc.
const FILES: &str = "Never run gh auth login. A missing board token is an owner item; report it through taskq ask.\n\
     Touch only this project, ~/.claude, ~/.codex and the OS temp dir: never search `/`, `~` or other folders.\n";

/// taskq.md § 7 rule 7 CI wait: every host's agents share one board user's hourly requests (§ 4 board budget).
const CI_WAIT: &str = "Board budget: poll CI at most once a minute (`gh pr checks <PR> --watch --interval 60`); never `gh run watch`\n\
     (it polls every 3 s) or `gh run view` without `--json`.\n";

/// taskq.md § 7: a sandbox limit is unknown locally; publication still needs exact-SHA CI.
const SANDBOX_CHECKS: &str = "Host tests blocked by the worker sandbox: record sandbox-blocked in Checks, never as PASS; do not requeue for them.\n\
     Their Acceptance gate remains green CI on the exact result SHA (R12).\n";

fn ask(q: &impl std::fmt::Display, n: u64) -> String {
    format!("`{q} ask {n} --text \"Done: <checked or not started>\nProblem: <decision and user effect>\" --option A --option B --recommend K [--link URL]`")
}

pub fn contract(root: &Path) -> String {
    match project_contract(root) {
        Ok(Some(spec)) => spec.display().to_string(),
        Ok(None) => command_contract(),
        Err(e) => format!("project contract unavailable: {e}"),
    }
}

fn command_contract() -> String {
    let v = crate::platform::VERSION;
    let tag = if v.contains('-') { "main".into() } else { format!("v{v}") }; // dev builds have no tag
    format!("https://github.com/{}/blob/{tag}/taskq.md", crate::platform::RELEASE_REPO)
}

fn project_contract(root: &Path) -> Result<Option<PathBuf>, StartError> {
    let config = match std::fs::read(root.join("taskq.json")) {
        Ok(bytes) => Some(serde_json::from_slice::<serde_json::Value>(&bytes).map_err(|e| refused("project registration", e))?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(refused("project registration", e)),
    };
    if let Some(config) = config {
        let repo = config["repo"].as_str().ok_or_else(|| refused("project registration", "missing repository"))?;
        let name = repo.rsplit('/').next().filter(|name| !name.is_empty() && !matches!(*name,"."|"..")
            && name.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)))
            .ok_or_else(|| refused("project registration", "invalid repository name"))?;
        let path = root.join(format!("{name}.md"));
        if repo == crate::platform::RELEASE_REPO && !path.try_exists().map_err(|e| refused("project contract", e))? { return Ok(None); }
        Ok(Some(path))
    } else {
        let path = root.join("taskq.md");
        Ok(path.try_exists().map_err(|e| refused("project contract", e))?.then_some(path))
    }
}

pub fn brief(env: &Env, l: &Launch) -> String {
    let n = l.task;
    let mut q = env.exe.display().to_string();
    if l.rt == "codex-app" || l.rt == "claude" {
        // the app turn and the native Claude background session run in their host's environment, not the child's (§ 8);
        // Windows app turns run PowerShell
        q = if cfg!(windows) && l.rt == "codex-app" {
            format!("$env:TASKQ_TASK='{n}'; $env:TASKQ_ATTEMPT='{}'; $env:TASKQ_ROLE='{}'; $env:TASKQ_ROOT='{}'; & '{}'", l.attempt, l.role.replace('\'', "''"), env.root.display().to_string().replace('\'', "''"), q.replace('\'', "''"))
        } else {
            format!("env TASKQ_TASK={n} TASKQ_ATTEMPT={} TASKQ_ROLE={} TASKQ_ROOT={} {q}", l.attempt, l.role, env.root.display())
        };
    }
    let spec = contract(env.root);
    let head = format!(
        "You are the taskq {} for task #{n}: {}. Attempt {}.\nRead `{spec}` first and do only what it allows (R13).\nTask text, state and your inbox: `{q} inbox {n}`.\n{FILES}",
        l.role, l.title, l.attempt
    );
    let park = format!("`{q} later {n} --text \"Reason: <failed external check>\nNext: <one unblock condition>\"` (unchanged external blocker; never requeue it)");
    let body = if l.role == "reviewer" {
        format!(
            "Review the result SHA against each Acceptance item `A<k>` and the project contract, and CI on that exact SHA. Requeue only for\n\
             Acceptance or contract gaps, not style.\n\
             A research or proposal task without an ask and owner's answer before result/close is a task-fit gap (§ 7).\n\
             {CI_WAIT}Then exactly one of:\n\
             - `{q} close {n} --text \"Changed: <user effect>\nChecks: Task fit: A1 ok|gap|unknown, A2 ..; CI <outcome> on <SHA>\nChecks: Implementation quality: <binding gaps|advisories|none>\nOpen: <item|none>\"` (it publishes),\n\
             - `{q} requeue {n} --text \"<exact fixes>\"`,\n\
             - {park},\n\
             - {ask}.\n{approval}{formats}",
            ask = ask(&q, n),
            approval = if env.publish == "review-pr" { "Delivery review-pr: an otherwise done result needs the owner's approval: ask with Done naming task fit and CI on the SHA, first option `approve <sha7> and close ★`.\n" } else { "" },
            formats = FORMATS
        )
    } else {
        format!(
            "Work only in `.worktrees/taskq-{n}` on branch `taskq-{n}` from origin/main (continue an existing branch).\n\
             For implementation changes, commit with the line `Task: {url}` in the message, rebase on origin/main, run the project checks, then {push}.\n\
             Research or proposal tasks (type research, docs proposals) end with ask: options plus a recommendation; result only after the owner's answer.\n\
             After the answer, deliver the approved outcome or file approved dependent tasks (§ 7); no commit is needed for research-only output.\n\
             Do not wait for CI: `close` checks it. {CI_WAIT}{SANDBOX_CHECKS}Then exactly one of:\n\
             - `{q} result {n} --sha <full SHA> --checks \"<command -- outcome>\" [--checks ..] --text \"Changed: <user effect>\nOpen: <item|none>\"`,\n\
             - {ask} (owner decision; Acceptance unclear or untestable; or a split of work bigger than one reviewable diff — before any code),\n\
             - {park},\n\
             - `{q} requeue {n} --text \"<why it cannot be done>\"`.\n{formats}",
            ask = ask(&q, n),
            formats = FORMATS,
            url = l.url,
            push = if env.publish == "direct" { "push the branch" } else { "push the branch and open one PR/MR into main" }
        )
    };
    format!("{head}Test changes follow specq engineering/testing.md: name the defect and retained risk protection in --text; change a rule only when accepted behavior or protection changes (section 7 step 4).\n{body}Then stop. Everything written through taskq is public: no secrets or paths outside the repository.\n")
}

#[cfg(test)]
const SPECQ_REVISION: &str = "2d030bdea5e8fd9095daea2069954ff6091ea90e";
const SPECQ_REFS: &str = "https://github.com/taskqorg/specq.git/info/refs?service=git-upload-pack";
const SPECQ_TREES: &str = "https://api.github.com/repos/taskqorg/specq/git/trees/";
/// The specq shared core, sent whole at every start and continuation (§ 8 Rule delivery).
const SPECQ_CORE: &[&str] = &[
    "engineering/principles.md", "engineering/spec-first.md", "engineering/ponytail.md", "engineering/agent-workflow.md",
    "engineering/use-cases.md", "engineering/verification.md", "engineering/security.md",
    "caveman/rules/core.md", "caveman/rules/documentation.md", "caveman/rules/changes.md",
];
/// Frozen published layout used by test source fixtures; production discovers the selected tree.
#[cfg(test)]
const SPECQ_INDEX: &[&str] = &[
    "caveman/SOURCES.md", "caveman/examples/messages.md", "caveman/rules/agents.md", "caveman/rules/indicators.md",
    "caveman/rules/notifications.md", "design/dashboards.md", "design/implementation-fidelity.md", "design/labels.md",
    "design/operator-ui.md", "design/workflow-review.md", "design/workflow.md", "engineering/data.md", "engineering/performance.md",
    "engineering/permission-authority.md", "engineering/product-connections.md", "engineering/profiling.md",
    "engineering/release-contract.md",
];
/// One cap for the injected header, contract, core and index (§ 8).
const RULES_CAP: usize = 128 * 1024;
/// This release's TaskQ tool contract, used only for the documented setup/TaskQ fallback.
const CONTRACT: &str = include_str!("../taskq.md");
const INDEX: &str = "\nIndex of other shared rules";

fn rule_file(url: &str) -> Result<String, String> {
    let out = match coreq::process::output("rules.download", Command::new("curl").args(["-q", "--fail", "--silent", "--show-error", "--proto", "=https", "--max-time", "10", "--max-filesize", "131072", "--write-out", "\n%{http_code}", url]), std::time::Duration::from_secs(15)) {
        Ok(out) | Err(coreq::process::Failure::Exit(out)) => out,
        Err(e) => return Err(e.to_string()),
    };
    let text=String::from_utf8(out.stdout).map_err(|_| "invalid UTF-8")?;
    let (body,status)=text.rsplit_once('\n').ok_or("missing HTTP status")?;
    if !out.status.success() || status!="200" {return Err(format!("HTTP {status}; curl exited {}",out.status));}
    Ok(body.into())
}

fn specq_dir(root: &Path, revision: &str) -> PathBuf {
    root.join(".taskq").join("specq").join(revision)
}

fn refused(what: &str, e: impl std::fmt::Display) -> StartError {
    StartError { message: format!("rules {what}: {e}"), reportable: false, no_start: true }
}

fn revision_id(text: &str) -> bool {
    if text.len()!=40 {return false;}
    let mut nonzero=false;
    for byte in text.bytes() {
        if !(byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)) {return false;}
        nonzero|=byte!=b'0';
    }
    nonzero
}

fn advertised_revision(text: &str) -> Result<String, StartError> {
    let invalid=||refused("shared revision","invalid Git reference advertisement");
    if !text.starts_with("001e# service=git-upload-pack\n0000") {return Err(invalid());}
    let mut input=text.as_bytes();let mut selected=None;let mut flushed=false;
    while !input.is_empty() {
        let length=std::str::from_utf8(input.get(..4).ok_or_else(invalid)?).ok()
            .and_then(|h|usize::from_str_radix(h,16).ok()).ok_or_else(invalid)?;
        input=&input[4..];
        if length==0 {flushed=true;continue;}
        if !(4..=65520).contains(&length) {return Err(invalid());}
        let packet=input.get(..length-4).ok_or_else(invalid)?;input=&input[length-4..];flushed=false;
        let mut end=0;while end<packet.len() && packet[end]!=0 {end+=1;}
        let reference=&packet[..end];
        let reference=reference.strip_suffix(b"\n").unwrap_or(reference);
        if reference.get(41..)==Some(b"refs/heads/main") {
            let sha=std::str::from_utf8(&reference[..40]).map_err(|_|invalid())?;
            if reference[40]!=b' ' || !revision_id(sha) || selected.is_some() {return Err(invalid());}
            selected=Some(sha.to_string());
        }
    }
    if !flushed {return Err(invalid());}
    selected.ok_or_else(||refused("shared revision","refs/heads/main is absent"))
}

fn shared_revision(root: &Path, fetch: &mut impl FnMut(&str)->Result<String,String>) -> Result<String,StartError> {
    let now=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|e|refused("shared revision clock",e))?.as_secs();
    shared_revision_at(root,now,fetch)
}

fn shared_revision_at(root: &Path, now: u64, fetch: &mut impl FnMut(&str)->Result<String,String>) -> Result<String,StartError> {
    let path=root.join(".taskq/specq/source.json");let mut cached=String::new();
    if File::open(&path).and_then(|f|f.take(1025).read_to_string(&mut cached)).is_ok() && cached.len()<=1024 {
        if let Ok(value)=serde_json::from_str::<serde_json::Value>(&cached) {
            if let (Some(revision),Some(checked))=(value["revision"].as_str(),value["checked_at"].as_u64()) {
                if revision_id(revision) && now.checked_sub(checked).is_some_and(|age|age<60) {return Ok(revision.into());}
            }
        }
    }
    let advertised=fetch(SPECQ_REFS).map_err(|e|refused("shared revision freshness",e))?;
    if advertised.len()>RULES_CAP {return Err(refused("shared revision","oversized advertisement"));}
    let revision=advertised_revision(&advertised)?;
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e|refused("shared revision cache",e))?;
    publish_rule(&path,&serde_json::json!({"revision":revision,"checked_at":now}).to_string()).map_err(|e|refused("shared revision cache",e))?;
    Ok(revision)
}

/// A pinned specq file from `<root>/.taskq/specq/<revision>/`, fetched once when missing or invalid there (R1: a cache).
fn pinned(root: &Path, revision: &str, file: &str, expected: &str, fetch: &mut impl FnMut(&str) -> Result<String, String>) -> Result<String, StartError> {
    cached_rule(root,revision,file,&format!("https://raw.githubusercontent.com/taskqorg/specq/{revision}/{file}"),|body| {
        if git_blob_hash(body)!=expected {Err(refused(file,format!("Git blob mismatch at {revision}; cache preserved")))} else {Ok(())}
    },fetch)
}

fn cached_rule(root: &Path, revision: &str, file: &str, url: &str, check: impl Fn(&str)->Result<(),StartError>, fetch: &mut impl FnMut(&str) -> Result<String, String>) -> Result<String, StartError> {
    let path = specq_dir(root,revision).join(file);
    let metadata = path.with_extension("cache.json");
    let mut body = String::new();
    if File::open(&path).and_then(|f| f.take(RULES_CAP as u64+1).read_to_string(&mut body)).is_ok()
        && !body.trim().is_empty() && body.len()<=RULES_CAP {
        let mut stored = String::new();
        if File::open(&metadata).and_then(|f|f.take(1025).read_to_string(&mut stored)).is_ok() && stored.len()<=1024
            && serde_json::from_str::<serde_json::Value>(&stored).ok()==Some(rule_cache(revision,file,&body)) && check(&body).is_ok() {
            return Ok(body);
        }
    }
    let body = fetch(url).map_err(|e| refused(file, e))?;
    if body.trim().is_empty() || body.len()>RULES_CAP { return Err(refused(file, "empty or oversized")); }
    check(&body)?;
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e|refused(file,e))?;
    publish_rule(&path,&body).and_then(|_|publish_rule(&metadata,&rule_cache(revision,file,&body).to_string())).map_err(|e|refused(file,e))?;
    Ok(body)
}

fn rule_cache(revision: &str, file: &str, body: &str) -> serde_json::Value {
    let digest=ring::digest::digest(&ring::digest::SHA256,body.as_bytes());
    serde_json::json!({"revision":revision,"file":file,"sha256":digest_hex(digest)})
}

fn digest_hex(digest: ring::digest::Digest) -> String {
    use std::fmt::Write as _;
    let mut hex=String::with_capacity(64);
    for byte in digest.as_ref() {write!(&mut hex,"{byte:02x}").expect("writing to String cannot fail");}
    hex
}

fn git_blob_hash(body: &str) -> String {
    let mut hash=ring::digest::Context::new(&ring::digest::SHA1_FOR_LEGACY_USE_ONLY);
    hash.update(format!("blob {}\0",body.len()).as_bytes());hash.update(body.as_bytes());digest_hex(hash.finish())
}

fn shared_files(root: &Path, revision: &str, fetch: &mut impl FnMut(&str)->Result<String,String>) -> Result<std::collections::BTreeMap<String,String>,StartError> {
    let text=cached_rule(root,revision,"tree.json",&format!("{SPECQ_TREES}{revision}?recursive=1"),|body|parse_shared_tree(body).map(|_|()),fetch)?;
    parse_shared_tree(&text)
}

fn parse_shared_tree(text: &str) -> Result<std::collections::BTreeMap<String,String>,StartError> {
    let bad=||refused("shared tree","invalid, incomplete or incompatible tree");
    if text.len()>RULES_CAP {return Err(bad());}
    let value:serde_json::Value=serde_json::from_str(text).map_err(|_|bad())?;
    if value["truncated"]!=false || !value["sha"].as_str().is_some_and(revision_id) {return Err(bad());}
    let entries=value["tree"].as_array().ok_or_else(bad)?;
    if entries.len()>1024 {return Err(bad());}
    let mut files=std::collections::BTreeMap::new();
    let mut portable=std::collections::BTreeSet::new();
    for entry in entries {
        let kind=entry["type"].as_str().ok_or_else(bad)?;
        if !matches!(kind,"blob"|"tree"|"commit") {return Err(bad());}
        let path=entry["path"].as_str().ok_or_else(bad)?;
        if path.is_empty() {return Err(bad());}
        if kind!="blob" {continue;}
        if !path.ends_with(".md") {continue;}
        let parts=path.split('/').collect::<Vec<_>>();
        if path.contains(['\\',':','\0']) || parts.iter().any(|p|matches!(*p,""|"."|"..")) {return Err(bad());}
        if matches!(parts.last().copied(),Some("README.md"|"AGENTS.md"|"SECURITY.md")) || parts.iter().any(|p|matches!(*p,"scripts"|"vendor"|"audits"|"docs")) {continue;}
        for byte in path.bytes() {if !(byte.is_ascii_alphanumeric() || b"-_./".contains(&byte)) {return Err(bad());}}
        for part in &parts {
            let base=part.split('.').next().unwrap_or_default().to_ascii_uppercase();
            if part.ends_with('.') || matches!(base.as_str(),"CON"|"PRN"|"AUX"|"NUL")
                || matches!(base.as_bytes(),[b'C',b'O',b'M',b'1'..=b'9']|[b'L',b'P',b'T',b'1'..=b'9']) {return Err(bad());}
        }
        if !portable.insert(path.to_ascii_lowercase()) {return Err(bad());}
        if !matches!(entry["mode"].as_str(),Some("100644"|"100755")) {return Err(bad());}
        let hash=entry["sha"].as_str().filter(|s|revision_id(s)).ok_or_else(bad)?;
        if files.insert(path.to_string(),hash.to_string()).is_some() {return Err(bad());}
    }
    for required in SPECQ_CORE {if !files.contains_key(*required) {return Err(bad());}}
    Ok(files)
}

fn publish_rule(path: &Path, body: &str) -> Result<(),String> {
    // The shared atomic writer has a fixed temporary suffix; stage under a unique basename for concurrent launches.
    let stage=path.with_file_name(format!(".{}.{}",uuid(),path.file_name().unwrap().to_string_lossy()));
    let result=coreq::write_atomic(&stage,body).and_then(|_|std::fs::rename(&stage,path).map_err(|e|e.to_string()));
    if result.is_err() {let _=std::fs::remove_file(&stage);let _=std::fs::remove_file(stage.with_extension("tmp"));}
    result
}

fn heading(body: &str) -> &str {
    body.lines().find(|l| l.starts_with('#')).map_or("(no heading)", |l| l.trim_start_matches('#').trim())
}

/// `docs/` Markdown files below `root` with their first heading, sorted by `/` path; an unreadable one refuses the launch.
fn docs(root: &Path) -> Result<Vec<(String, String)>, StartError> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, String)>) -> std::io::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let (path, kind) = (entry.path(), entry.file_type()?);
            if kind.is_dir() {
                walk(root, &path, out)?;
            } else if kind.is_file() && path.extension().is_some_and(|e| e == "md") {
                let rel = path.strip_prefix(root).unwrap().components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/");
                let body = String::from_utf8(std::fs::read(&path)?).map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{rel}: invalid UTF-8")))?;
                out.push((rel, heading(&body).to_string()));
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    let dir = root.join("docs");
    if dir.is_dir() { walk(root, &dir, &mut out).map_err(|e| refused("docs index", e))?; }
    out.sort();
    Ok(out)
}

/// § 8 Rule delivery: current header, contract, core and index before every task/continuation.
fn rule_text(root: &Path, task: &str, resume: bool, fetch: impl FnMut(&str) -> Result<String, String>) -> Result<String, StartError> {
    Ok(format!("{}\nTaskQ task\n{task}", rules(root, resume, fetch)?))
}

/// The mandatory bodies are repeated so continuation/compaction cannot silently retain old rules.
fn rules(root: &Path, resume: bool, mut fetch: impl FnMut(&str) -> Result<String, String>) -> Result<String, StartError> {
    let revision=shared_revision(root,&mut fetch)?;
    let _snapshot=tracing::info_span!("taskq.rules.snapshot",shared_spec=revision.as_str()).entered();
    let dir = specq_dir(root,&revision);
    let selected = project_contract(root)?;
    let spec = selected.as_ref().map_or_else(command_contract, |p|p.display().to_string());
    let contract = match selected {
        Some(path) => String::from_utf8(std::fs::read(&path).map_err(|e| refused("project contract", e))?).map_err(|_| refused("contract", "invalid UTF-8"))?,
        None => CONTRACT.into(),
    };
    if contract.trim().is_empty() { return Err(refused("contract", "empty")); }
    // the header names the cache: every pinned file is there, at start and at resume
    let files=shared_files(root,&revision,&mut fetch)?;
    let core = SPECQ_CORE.iter().map(|f| pinned(root, &revision, f, &files[*f], &mut fetch)).collect::<Result<Vec<_>, _>>()?;
    let mut text = format!(
        "TaskQ rules: specq@{revision} at `{}`; contract `{spec}`.\n\
         Precedence: the project contract overrides shared rules where they conflict.\n\
         These rules apply to this turn, including continuations. Apply them to the work; acknowledgement alone is not evidence.\n\
         Evidence: cite the applied rule as R<n> or § n; a fix records the failure before and the pass after.\n",
        dir.display()
    );
    text.push_str(&format!("TaskQ command contract (tool reference): {}\n",command_contract()));
    if resume {
        text.push_str("Continuation: these current contract and core bodies supersede earlier versions.\n");
    }
    text.push_str(&format!("\n--- contract {spec} ---\n{contract}\n"));
    for (file, body) in SPECQ_CORE.iter().zip(&core) { text.push_str(&format!("\n--- specq/{file} ---\n{body}\n")); }
    text.push_str(&format!("{INDEX} (read on demand under `{}`):\n", dir.display()));
    for (file,hash) in &files {
        if !SPECQ_CORE.contains(&file.as_str()) {text.push_str(&format!("- {file}: {}\n", heading(&pinned(root,&revision,file,hash,&mut fetch)?)));}
    }
    text.push_str("Index of project docs (read on demand in your worktree):\n");
    for (path, title) in docs(root)? { text.push_str(&format!("- {path}: {title}\n")); }
    if text.len() > RULES_CAP { return Err(refused("rules", format!("{} bytes over the {RULES_CAP}-byte cap", text.len() - RULES_CAP))); }
    Ok(text)
}

/// § 8 Rule delivery after a Claude compaction: restore the current mandatory bodies and index.
pub fn compact(root: &Path, role: &str, task: u64, attempt: u64) -> Result<String, StartError> {
    let text = rules(root, true, rule_file)?;
    log!("rules loaded for {role} #{task} attempt {attempt} (compact, {} bytes)", text.len());
    rules_line(&attempt_log(root, role, task, attempt), "compact", &text).map_err(|e| StartError::io(e, None))?;
    Ok(text)
}

/// § 8 Rule delivery to a PM session: the SessionStart hook restores the same current mandatory bodies and index
/// after compaction (`phase` `compact`) as a worker continuation.
pub fn pm(root: &Path, phase: &str) -> Result<String, StartError> {
    pm_rules(root, phase, rule_file)
}

fn pm_rules(root: &Path, phase: &str, fetch: impl FnMut(&str) -> Result<String, String>) -> Result<String, StartError> {
    let text = rules(root, phase == "compact", fetch)?;
    log!("rules loaded for pm ({phase}, {} bytes)", text.len());
    Ok(text)
}

/// `--settings` for a Claude turn: a SessionStart hook that re-sends the rules after compaction.
fn compact_settings(root: &Path, l: &Launch) -> String {
    let q = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
    let exe = std::env::current_exe().unwrap_or_else(|_| "taskq".into());
    let cmd = format!("{} rules compact {} {} {} {}", q(&exe.to_string_lossy()), q(&root.to_string_lossy()), q(&l.role), l.task, l.attempt);
    serde_json::json!({"hooks": {"SessionStart": [{"matcher": "compact", "hooks": [{"type": "command", "command": cmd}]}]}}).to_string()
}

/// The operator's current rules and turn text (§ 8 Rule delivery).
pub fn instructions(root: &Path, task: &str, resume: bool) -> Result<String, StartError> {
    rule_text(root, task, resume, rule_file)
}

// Claude --bg forwards stdin as child argv; send only a file reference.
fn claude_reference(path:&Path)->Result<String,String> {
    Ok(format!("Before any work, use Read to read {} in full. It contains this turn's TaskQ rules and task. Apply those rules and carry out that task. If Read truncates the file, read the remaining parts before proceeding.", serde_json::to_string(&path.to_string_lossy()).map_err(|e| e.to_string())?))
}

fn app_reference(path:&Path)->Result<String,String> {
    Ok(format!("Before any work, read {} in full using your file tools. It contains this turn's TaskQ rules and task. Apply those rules and carry out that task. Read all remaining parts if a tool truncates the file. If the file cannot be read completely, report the failure and do not proceed with the task.", serde_json::to_string(&path.to_string_lossy()).map_err(|e| e.to_string())?))
}

fn claude_input(root:&Path,text:&str,launch:Option<&Launch>)->Result<(String,crate::data::InputLock,PathBuf),String> {
    let (path,file)=turn_input(root,text,launch)?;
    Ok((claude_reference(&path)?,file,path))
}

fn app_input(root:&Path,text:&str,launch:Option<&Launch>)->Result<(String,crate::data::InputLock,PathBuf),String> {
    let (path,file)=turn_input(root,text,launch)?;
    Ok((app_reference(&path)?,file,path))
}

fn execution_input(root:&Path,task:u64,attempt:u64,execution:u64)->Result<PathBuf,String> {
    if task==0 || attempt==0 || execution<attempt {return Err("invalid execution identity for native input".into());}
    Ok(root.canonicalize().map_err(|e|e.to_string())?.join(".taskq/inputs").join(format!("execution-{task}-{attempt}-{execution}.txt")))
}

fn turn_input(root: &Path, text: &str, launch:Option<&Launch>) -> Result<(PathBuf, crate::data::InputLock), String> {
    let dir = root.join(".taskq/inputs");
    let path=match launch.filter(|l|l.task>0) {
        Some(l)=>execution_input(root,l.task,l.attempt,l.execution.ok_or("exact execution intent missing before native input creation")?)?,
        None=>dir.join(format!("{}.txt",uuid())),
    };
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let mut options = File::options();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(&path).map_err(|e| e.to_string())?;
    file.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())?;
    let file = crate::data::InputLock::new(file).map_err(|e| e.to_string())?;
    Ok((path, file))
}

fn record_input(log:&Path,record:&serde_json::Value)->Result<(),String> {
    let mut file=File::options().create(true).append(true).open(log).map_err(|e|e.to_string())?;
    writeln!(file,"{record}").and_then(|_|file.sync_all()).map_err(|e|format!("native input binding not persisted: {e}"))
}

/// Recovery locator only: neither a before-call marker nor its presence proves submission or no-start.
fn bind_input(log:&Path,l:&Launch,path:&Path,input:&str,native:serde_json::Value)->Result<serde_json::Value,String> {
    let execution=if l.task==0 {0} else {l.execution.filter(|id|l.attempt>0 && *id>=l.attempt).ok_or("exact execution intent missing before native input submission")?};
    let record=serde_json::json!({"type":"taskq.input","task":l.task,"attempt":l.attempt,"execution":execution,"runtime":l.rt,
        "path":path.to_str().ok_or("native input path is not UTF-8")?,"input":input,"native":native});
    record_input(log,&record)?;
    Ok(record)
}

/// The log supplies native lookup hints; the board-derived input reference is the identity boundary.
fn recovery_binding(root:&Path,task:u64,x:&crate::core::Execution)->Result<(PathBuf,serde_json::Value,crate::data::InputLock),String> {
    let path=execution_input(root,task,x.attempt,x.id)?;
    if !std::fs::symlink_metadata(&path).map_err(|_|"original execution input unavailable; inspect legacy or missing native binding")?.is_file() {
        return Err("original execution input is not a regular file".into());
    }
    let input=match x.runtime.as_str() {"claude"=>claude_reference(&path)?,"codex-app"=>app_reference(&path)?,_=>return Err("native input recovery unsupported for this runtime".into())};
    let worker=attempt_log(root,"worker",task,x.attempt);let reviewer=attempt_log(root,"reviewer",task,x.attempt);
    let log=match (worker.is_file(),reviewer.is_file()) {(true,false)=>worker,(false,true)=>reviewer,_=>return Err("native attempt log missing or ambiguous; preserve the execution".into())};
    let mut binding=None;let mut reader=NativeHistory::default();
    let until=std::time::Instant::now()+std::time::Duration::from_secs(5);
    loop {
        let complete=reader.lines(&log,|line| {
            if !line.trim_start().starts_with('{') {return Ok(());}
            let v:serde_json::Value=serde_json::from_str(line).map_err(|_|"native attempt record incomplete or invalid")?;
            if v["type"]!="taskq.input" || v["execution"].as_u64()!=Some(x.id) {return Ok(());}
            if v["task"].as_u64()!=Some(task) || v["attempt"].as_u64()!=Some(x.attempt) || v["runtime"]!=x.runtime || v["path"].as_str()!=path.to_str() || v["input"]!=input || !v["native"].is_object() {
                return Err("native input binding differs from the board execution; inspect original submission".into());
            }
            binding=Some(v);Ok(())
        })?;
        if complete {break;}
        if std::time::Instant::now()>=until {return Err("native input binding scan exceeded 5 seconds; original execution retained".into());}
    }
    let binding=binding.ok_or("exact execution input binding missing; inspect original submission")?;
    let file=File::open(&path).map_err(|e|e.to_string())?;
    let lock=crate::data::InputLock::new(file).map_err(|e|e.to_string())?;
    Ok((log,binding,lock))
}

/// Reconnect observation only. This path cannot submit a prompt, queue a turn or change access.
pub fn recover(root:&Path,install:&Path,task:u64,x:&crate::core::Execution)->Result<Box<dyn Proc>,String> {
    if x.release.is_some() {return Err("released execution cannot be recovered as live".into());}
    let root=root.canonicalize().map_err(|e|e.to_string())?;
    let saved=crate::kids_file(install,&root)?.into_iter().find(|k|k.task==task);
    if saved.as_ref().is_some_and(|k|k.execution!=x.id || k.attempt!=x.attempt || k.runtime!=x.runtime) {
        return Err("protected native recovery identity differs from board execution; preserve the record".into());
    }
    let (log,binding,input_file)=recovery_binding(&root,task,x)?;
    let input=binding["input"].as_str().ok_or("native recovery input missing")?.to_owned();
    match x.runtime.as_str() {
        "claude"=>Ok(Box::new(ClaudeTurn {root:root.clone(),log,session:if x.id==x.attempt {String::new()} else {binding["native"]["session"].as_str().unwrap_or_default().to_owned()},
            id:String::new(),announced:false,accepted:false,history:ClaudeHistory::default(),before:0,mode:"unknown: recovered observer".into(),input,_input_file:input_file,pid:0,done:false,failure:None,
            native:saved.map(|k|k.process),recovery:Some(Recovery {install:install.into(),root:root.clone(),task,attempt:x.attempt,execution:x.id}),
            launch_error:Some("recovered execution; original native input acceptance pending".into())})),
        "codex-app"=>{
            let thread=binding["native"]["thread"].as_str().filter(|s|s.len()>1 && s.bytes().all(|b|b.is_ascii_alphanumeric() || b==b'-')).ok_or("native App thread identity missing; inspect original handshake, never requeue")?.to_owned();
            Ok(Box::new(AppTurn {root,history:AppHistory::default(),hand:None,log,text:input,_input:Some(input_file),binding:Some(binding),thread,done:0,rollout:None,running:app_running,control:app_cancel,cancelled:false,
                queue_error:Some("recovered execution; original App input acceptance pending".into()),accepted:false}))
        },
        _=>Err("native input recovery unsupported for this runtime".into()),
    }
}

fn turn_text(env: &Env, l: &Launch, fetch: impl FnMut(&str) -> Result<String, String>) -> Result<String, StartError> {
    let mut task = brief(env, l);
    if let Some(answer) = &l.resume { task.push_str(&format!("\nContinue this same attempt with the following inbox message:\n{answer}")); }
    rule_text(env.root, &task, l.resume.is_some(), fetch)
}

/// Keep OS failure classes until the service decides whether a start failure is TaskQ's fault.
#[derive(Debug)]
pub struct StartError {
    message: String,
    pub reportable: bool,
    pub no_start: bool,
}

impl StartError {
    pub fn io(error: std::io::Error, runtime: Option<&str>) -> Self {
        use std::io::ErrorKind;
        let reportable = matches!(error.kind(), ErrorKind::Other | ErrorKind::InvalidData | ErrorKind::UnexpectedEof | ErrorKind::BrokenPipe);
        let message = runtime.map_or_else(|| error.to_string(), |rt| format!("{rt}: {error}"));
        Self { message, reportable, no_start: true }
    }
}

impl From<String> for StartError {
    fn from(message: String) -> Self { Self { message, reportable: false, no_start: true } }
}
impl From<&str> for StartError {
    fn from(message: &str) -> Self { message.to_string().into() }
}
impl std::error::Error for StartError {}

impl std::fmt::Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { self.message.fmt(f) }
}

/// Prepare stdin before spawn: a child that never reads cannot block its execution owner.
pub fn start_command(command: &mut Command, root: &Path, text: &str) -> Result<Box<dyn Proc>, StartError> {
    coreq::telemetry::capture::operation("runtime.launch", || {
        let (path, _input) = turn_input(root, text, None).map_err(|message| StartError { message, reportable: false, no_start: true })?;
        let input = File::open(path).map_err(|e| StartError::io(e, None))?;
        coreq::telemetry::capture::inherit_context(command);
        command.stdin(input);
        #[cfg(windows)] {windows_process::Execution::spawn(command).map(|child|Box::new(child) as Box<dyn Proc>)}
        #[cfg(not(windows))] {command.spawn().map(|child|Box::new(child) as Box<dyn Proc>).map_err(|e|StartError::io(e,None))}
    }, |_| "launch")
}

/// § 8 Access of `rt` here, resolved again for every start and resume; a refusal starts nothing.
pub fn access(env: &Env, rt: &str) -> Result<crate::policy::Effective, StartError> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from).unwrap_or_default();
    let a = crate::policy::resolve(&crate::policy::load(env.install), &crate::policy::Host::here(env.install, env.root, &home), rt, env.permission_mode, env.codex);
    match &a.refused {
        Some(r) => Err(StartError { message: format!("access refused: {r}"), reportable: false, no_start: true }),
        None => Ok(a),
    }
}

/// Start the child with its stdout and stderr in the attempt log; `codex-app` starts its handshake child (§ 8).
pub fn spawn(env: &Env, l: &Launch) -> Result<Box<dyn Proc>, StartError> {
    crate::profile::measure("spawn", || spawn_inner(env, l))
}

fn spawn_inner(env: &Env, l: &Launch) -> Result<Box<dyn Proc>, StartError> {
    let access = access(env, &l.rt)?;
    log!("#{} attempt {} access {}", l.task, l.attempt, access.line());
    let text = turn_text(env, l, rule_file)?;
    let phase = if l.resume.is_some() { "resume" } else { "start" };
    log!("rules loaded for {} #{} attempt {} ({phase}, {} bytes)", l.role, l.task, l.attempt, text.len());
    let log = log_path(env.root, l);
    rules_line(&log, phase, &text).map_err(|e| StartError::io(e, None))?;
    if l.rt == "codex-app" {
        let (text, input_file, path) = app_input(env.root, &text, Some(l)).map_err(|message| StartError { message, reportable: false, no_start: true })?;
        let binding=bind_input(&log,l,&path,&text,serde_json::json!({"thread":l.session}))?;
        let mut turn = AppTurn { root: env.root.canonicalize().map_err(|e| StartError::io(e,None))?, history:AppHistory::default(), hand: None, log, text, _input: Some(input_file), binding:Some(binding), thread: l.session.clone(), done: 0, rollout: None, running: app_running, control:app_cancel, cancelled: false, queue_error: None, accepted: false };
        match &l.resume {
            Some(_) => {
                if let Err(message) = turn.queue() { turn.queue_error = Some(format!("{message}; submission outcome unknown")); }
            }
            None => turn.hand = Some(start(env, l, &turn.log, HANDSHAKE, &access)?),
        }
        return Ok(Box::new(turn));
    }
    if l.rt == "claude" && (l.resume.is_none() || claude_background(env.root, &l.session).map_err(|message| StartError { message, reportable: false, no_start: true })?) {
        return ClaudeTurn::start(env, l, &log, &text, &access).map(|p| Box::new(p) as Box<dyn Proc>);
    }
    let pos = std::fs::metadata(&log).map_or(0, |m| m.len());
    let child = start(env, l, &log, &text, &access)?;
    let native=match child.native_identity() {
        Ok(identity)=>Some(identity),
        Err(e)=>{log!("#{} process recovery identity unknown: {e}",l.task);None}
    };
    Ok(Box::new(CliTurn { proc: child, log, pos, start: pos, stopping: false, accepted: false, read_error: None, native, recovery: None }))
}

/// The attempt log's evidence of § 8 Rule delivery: the injected header and part counts, one JSON line.
pub fn rules_line(log: &Path, phase: &str, text: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(log.parent().unwrap())?;
    let rules = text.split("\nTaskQ task\n").next().unwrap_or_default();
    let header = rules.split("\n\n").next().unwrap_or_default();
    let bodies = rules.matches("\n--- contract ").count() + rules.matches("\n--- specq/").count();
    let index = rules.find(INDEX).map_or(0, |i| rules[i..].lines().filter(|l| l.starts_with("- ")).count());
    let revision=rules.strip_prefix("TaskQ rules: specq@").and_then(|s|s.split_whitespace().next()).filter(|s|revision_id(s));
    let line = serde_json::json!({"type": "taskq.rules", "phase": phase, "header": header, "shared_revision": revision, "bodies": bodies, "index": index, "bytes": rules.len()});
    writeln!(File::options().create(true).append(true).open(log)?, "{line}")
}

fn start(env: &Env, l: &Launch, log: &Path, text: &str, access: &crate::policy::Effective) -> Result<Box<dyn Proc>, StartError> {
    std::fs::create_dir_all(log.parent().unwrap()).map_err(|e| StartError::io(e, None))?;
    let out = File::options().create(true).append(true).open(log).map_err(|e| StartError::io(e, None))?;
    let err = out.try_clone().map_err(|e| StartError::io(e, None))?;
    let mut c = command(env, l, access).map_err(|message| StartError { message, reportable: false, no_start: true })?;
    start_command(c.stdout(out).stderr(err), env.root, text)
}

/// Native background sessions are not children of TaskQ. Keep their full identity and
/// observe the completed input before releasing the attempt's capacity.
pub struct ClaudeTurn {
    root: PathBuf,
    log: PathBuf,
    session: String,
    id: String,
    announced: bool,
    accepted: bool,
    history: ClaudeHistory,
    before: usize,
    mode: String,
    input: String,
    _input_file: crate::data::InputLock,
    pid: u32,
    native: Option<coreq::process::Identity>,
    recovery: Option<Recovery>,
    done: bool,
    failure: Option<&'static str>,
    launch_error: Option<String>,
}

fn claude_agents(root: &Path) -> Result<Vec<serde_json::Value>, String> {
    let root = root.to_str().ok_or("Claude project path is not UTF-8")?;
    let (ok, out) = probe("runtime.agents", &which("claude"), &["agents", "--json", "--all", "--cwd", root]).ok_or("Claude agents unavailable or timed out")?;
    if !ok { return Err(format!("claude agents: {out}")); }
    serde_json::from_str(&out).map_err(|e| format!("claude agents: {e}"))
}

fn claude_home() -> Option<PathBuf> {
    std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from).or_else(||
        std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(|h| PathBuf::from(h).join(".claude")))
}

pub(crate) fn claude_failure(row: &serde_json::Value, root: &Path, id: &str, home: Option<&Path>) -> &'static str {
    let detail = row["detail"].as_str().map(String::from).or_else(|| {
        if id.is_empty() || !id.chars().all(|c| c.is_ascii_hexdigit() || c == '-') { return None; }
        let text = std::fs::read_to_string(home?.join("jobs").join(id).join("state.json")).ok()?;
        let job: serde_json::Value = serde_json::from_str(&text).ok()?;
        if job["state"] != row["state"] || Path::new(job["cwd"].as_str()?).canonicalize().ok()? != root.canonicalize().ok()? { return None; }
        job["detail"].as_str().map(String::from)
    });
    crate::profile::class(detail.as_deref().unwrap_or_default())
}

fn claude_transcript(session: &str) -> Option<PathBuf> {
    claude_transcript_in(&claude_home()?,session).ok()
}

fn claude_transcript_in(home:&Path,session:&str)->Result<PathBuf,String> {
    if session.len()!=36 || !native_job_id(session) {return Err("Claude session identity invalid".into());}
    let mut found=None;
    for (i,entry) in std::fs::read_dir(home.join("projects")).map_err(|e|e.to_string())?.enumerate() {
        if i>=4096 {return Err("Claude transcript directory scan exceeds 4096 entries".into());}
        let directory=entry.map_err(|e|e.to_string())?.path();
        match std::fs::metadata(&directory) {
            Ok(meta) if !meta.is_dir()=>continue,
            Ok(_)=>{},
            Err(e) if e.kind()==std::io::ErrorKind::NotFound=>continue,
            Err(e)=>return Err(format!("Claude transcript directory unreadable: {e}")),
        }
        let path=directory.join(format!("{session}.jsonl"));
        match std::fs::metadata(&path) {
            Ok(meta) if meta.is_file()=>{
                if found.is_some() {return Err("Claude session has multiple transcript paths; inspect native history".into());}
                found=Some(path);
            }
            Ok(_)=>return Err("Claude transcript is not a regular file".into()),
            Err(e) if e.kind()==std::io::ErrorKind::NotFound=>{},
            Err(e)=>return Err(format!("Claude transcript lookup failed: {e}")),
        }
    }
    found.ok_or_else(||"Claude transcript unavailable; completion unknown".into())
}

fn native_job_id(id:&str)->bool {
    (id.len()==8 && id.bytes().all(|b|b.is_ascii_hexdigit())) ||
        (id.len()==36 && id.bytes().enumerate().all(|(i,b)|if [8,13,18,23].contains(&i) {b==b'-'} else {b.is_ascii_hexdigit()}))
}

fn claude_input_event(event:&serde_json::Value,input:&str)->bool {
    if event["type"]!="user" {return false;}
    let content=&event["message"]["content"];
    content.as_str().or_else(||content.as_array().filter(|a|a.len()==1 && a[0]["type"]=="text")
        .and_then(|a|a[0]["text"].as_str())).is_some_and(|s|s.trim()==input)
}

/// One pre-submission permission/baseline read, using the same bounded native decoder.
fn claude_history(path:&Path,before:usize,mut visit:impl FnMut(serde_json::Value))->Result<usize,String> {
    let mut reader=NativeHistory::default();let mut count=0;
    let until=std::time::Instant::now()+std::time::Duration::from_secs(5);
    loop {
        let complete=reader.read(path,|v| {if count>=before {visit(v);}count+=1;Ok(())})?;
        if complete {
            if count<before {return Err("Claude history truncated before input boundary".into());}
            return Ok(count);
        }
        if std::time::Instant::now()>=until {return Err("Claude baseline scan exceeded 5 seconds; no input submitted".into());}
    }
}

#[derive(Default)]
struct ClaudeHistory {
    reader: NativeHistory,
    count: usize,
    found: bool,
    ended: bool,
    answer: Option<String>,
}

impl ClaudeHistory {
    fn read(&mut self,path:&Path,before:usize,input:&str,log:&Path)->Result<bool,String> {
        let mut output=File::options().create(true).append(true).open(log).map_err(|e|e.to_string())?;
        let Self {reader,count,found,ended,answer}=self;
        let complete=reader.read(path,|v| {
            let starts=*count>=before && claude_input_event(&v,input);
            if starts && *found {return Err("Claude original input appears in multiple turns; inspect native submissions".into());}
            if (*found || starts) && !*ended {
                writeln!(output,"{v}").map_err(|e|format!("Claude native observation log failed: {e}"))?;
                if v["type"]=="assistant" {
                    if let Some(text)=v["message"]["content"].as_array().and_then(|blocks|blocks.iter().rev().find_map(|b|(b["type"]=="text").then(||b["text"].as_str()).flatten())) {
                        *answer=Some(text.to_owned());
                    }
                }
                *ended=v["type"]=="system" && v["subtype"]=="turn_duration";
            }
            *found|=starts;*count+=1;
            Ok(())
        })?;
        if !complete {return Err("Claude native history scan pending; retry observation without submission".into());}
        if *count<before {return Err("Claude history truncated before submitted input boundary".into());}
        Ok(*found)
    }
}

/// Native intent locates a candidate; only its user-message readback can establish acceptance.
fn claude_job_input(home:&Path,id:&str,session:&str,root:&Path,input:&str,budget:&mut u64)->Result<bool,String> {
    #[derive(serde::Deserialize)]
    struct Input {cwd:String,#[serde(rename="sessionId")] session:String,intent:Option<String>}
    let path=home.join("jobs").join(id).join("state.json");
    let meta=std::fs::metadata(&path).map_err(|_|"Claude native input locator unavailable; inspect original job")?;
    if !meta.is_file() || meta.len()>1024*1024 {return Err("Claude native input locator is nonregular or exceeds 1 MiB".into());}
    let mut reader=File::open(path).map_err(|_|"Claude native input locator unreadable")?.take(budget.saturating_add(1));
    let limit=reader.limit();
    // Deserialize only locator fields; provider environment and other payloads are never retained or logged.
    let value:Result<Input,_>=serde_json::from_reader(&mut reader);
    *budget=budget.checked_sub(limit-reader.limit()).ok_or("Claude recovery scan exceeds 8 MiB; original submission remains unknown")?;
    let value=value.map_err(|_|"Claude native input locator invalid")?;
    if value.session!=session || Path::new(&value.cwd).canonicalize().map_err(|_|"Claude native input locator project unavailable")?!=root {
        return Err("Claude native input locator identity differs from native job list".into());
    }
    Ok(value.intent.as_deref().is_some_and(|text|text.trim()==input))
}

fn claude_stop(root: &Path, id: &str, known: Option<&coreq::process::Identity>) -> Result<(), String> {
    if !native_job_id(id) {return Err("Claude native job identity unavailable; reconcile the original input before stopping".into());}
    let rows = claude_agents(root)?;
    let row = rows.iter().find(|v| v["id"] == id).ok_or("Claude stop identity unavailable")?;
    if let (Some(pid),Some(saved))=(claude_pid(row)?,known) {
        if pid!=saved.pid {return Err("Claude native process changed; preserve the execution for reconciliation".into());}
    }
    // An unknown outcome must not prevent requesting native cancellation of the known job.
    if matches!(claude_stopped(row, known),Ok(true)) { return Ok(()); }
    let (ok, out) = probe("runtime.stop", &which("claude"), &["stop", id]).ok_or("Claude stop unavailable or timed out; state unknown")?;
    if !ok { return Err(format!("claude stop: {out}")); }
    // Native stop acknowledges the request first; the service owns later retries/readback.
    let rows = claude_agents(root)?;
    let row = rows.iter().find(|v| v["id"] == id).ok_or("stopped Claude session disappeared; state unknown")?;
    if claude_stopped(row, known)? { Ok(()) } else { Err("Claude stop still pending; do not launch another turn".into()) }
}

fn claude_pid(row: &serde_json::Value) -> Result<Option<u32>, String> {
    if row["pid"].is_null() { return Ok(None); }
    row["pid"].as_u64().and_then(|n| u32::try_from(n).ok()).filter(|n| *n > 0).map(Some).ok_or_else(|| "Claude process identity invalid".into())
}

fn claude_stopped(row: &serde_json::Value, known: Option<&coreq::process::Identity>) -> Result<bool, String> {
    let state = row["state"].as_str().ok_or("Claude state unavailable; stop unknown")?;
    let pid=claude_pid(row)?;
    if !matches!(state,"working"|"idle"|"done"|"blocked"|"failed"|"crashed"|"stopped") {
        return Err(format!("Claude state {state:?} unsupported; inspect native lifecycle before release"));
    }
    if matches!(state,"working"|"idle") || pid.is_some() { return Ok(false); }
    // Native stop may retain done; work state alone never establishes process exit.
    let known=known.ok_or("Claude process exit evidence unavailable; inspect the original native job before release")?;
    coreq::process::identity(known.pid).map(|current|current.as_ref()!=Some(known)).map_err(|e|format!("Claude process identity unreadable: {e}"))
}

impl ClaudeTurn {
    fn recover_job(&mut self,rows:&[serde_json::Value],home:Option<&Path>)->Result<(),String> {
        if self.accepted || (!self.id.is_empty() && self.launch_error.is_none()) {return Ok(());}
        let home=home.ok_or("Claude native home unavailable; original submission remains unknown")?;
        let mut budget=8*1024*1024;
        let mut matched=None;
        for row in rows {
            if row["kind"]=="interactive" {continue;}
            if row["kind"]!="background" {return Err("Claude recovery job kind unknown".into());}
            let root=row["cwd"].as_str().ok_or("Claude recovery project unknown")?;
            // Native --cwd includes descendants; TaskQ submits the exact canonical project root.
            if Path::new(root)!=self.root {continue;}
            let session=row["sessionId"].as_str().filter(|s|s.len()==36 && native_job_id(s)).ok_or("Claude recovery session unknown")?;
            let id=row["id"].as_str().filter(|id|native_job_id(id)).ok_or("Claude native job identity unavailable during recovery")?;
            if session!=self.session && !claude_job_input(home,id,session,&self.root,&self.input,&mut budget)? {continue;}
            if matched.is_some() {return Err("Claude original input matches multiple native candidates; inspect them before release".into());}
            matched=Some((id.to_owned(),session.to_owned()));
        }
        let (id,session)=matched.ok_or("Claude original input not found; submission remains unknown; retry native observation")?;
        if !self.session.is_empty() && session!=self.session {return Err(format!("Claude original input found in copied job {id}; conversation identity differs; inspect native state"));}
        let before=if session==self.session {self.before} else {0};
        if !self.history.read(&claude_transcript_in(home,&session)?,before,&self.input,&self.log)? {
            return Err("Claude original input not found; submission remains unknown; retry native observation".into());
        }
        self.id=id;
        self.session=session;
        self.accepted=true;
        Ok(())
    }

    pub fn start(env: &Env, l: &Launch, log: &Path, text: &str, access: &crate::policy::Effective) -> Result<Self, StartError> {
        let permission_mode = access.claude.as_deref().unwrap_or(env.permission_mode);
        let root = env.root.canonicalize().map_err(|e| e.to_string())?;
        let mut expected_id = None;
        let mut before = 0;
        if l.resume.is_some() {
            let rows = claude_agents(&root)?;
            let row = rows.iter().find(|v| v["sessionId"] == l.session).ok_or("Claude background session unavailable; no replacement")?;
            // The caller releases the previous execution before continuation; reject contradictory native liveness.
            if claude_pid(row)?.is_some() { return Err("Claude session is still running; no copy started".into()); }
            expected_id = Some(row["id"].as_str().ok_or("Claude background ID missing")?.to_string());
            let path = claude_transcript(&l.session).ok_or("Claude saved permissions unavailable")?;
            let mut mode=None;
            before=claude_history(&path,0,|v|{if let Some(m)=v["permissionMode"].as_str() {mode=Some(m.to_owned());}})?;
            let mode=mode.ok_or("Claude saved permission mode unknown")?;
            if mode != permission_mode { return Err("Claude saved permission mode differs; a new attempt is required".into()); }
        }
        std::fs::create_dir_all(log.parent().ok_or("missing log directory")?).map_err(|e| e.to_string())?;
        let mut c = executable(which("claude"), access, env)?;
        environment(env, l, &mut c);
        let (input, input_file, path) = claude_input(&root, text, Some(l))?;
        bind_input(log,l,&path,&input,serde_json::json!({"session":l.session,"job":expected_id,"before":before,"permission_mode":permission_mode}))?;
        c.current_dir(&root).arg("--bg");
        if l.resume.is_some() { c.args(["--resume", &l.session]); }
        else { c.args(["--settings", &compact_settings(&root, l), "--remote-control", &l.title, "--permission-mode", permission_mode]); }
        if l.role == "operator" { for k in ["TASKQ_TASK", "TASKQ_ATTEMPT", "TASKQ_ROLE"] { c.env_remove(k); } }
        let mut turn=Self { root, log: log.into(), session: if l.resume.is_some() { l.session.clone() } else { String::new() }, id: expected_id.unwrap_or_default(),
            announced: false, accepted:false, history:ClaudeHistory::default(), before, mode: permission_mode.into(), input, _input_file: input_file, pid: 0, native:None, recovery:None, done: false, failure: None, launch_error:None };
        let out=coreq::process::exchange("runtime.claude-launch", &mut c, turn.input.as_bytes(), std::time::Duration::from_secs(30));
        turn.finish_launch(out)?;
        Ok(turn)
    }

    fn finish_launch(&mut self,out:Result<std::process::Output,coreq::process::Failure>)->Result<(),StartError> {
        let out=match out {
            Ok(out)|Err(coreq::process::Failure::Exit(out))=>out,
            Err(e)=>{
                use coreq::process::Failure;
                if matches!(e,Failure::InvalidDeadline|Failure::Capacity|Failure::Owner(_)|Failure::Spawn(_)|Failure::InputLimit) {
                    return Err(StartError {message:format!("Claude background launch: {e}"),reportable:false,no_start:true});
                }
                self.launch_error=Some(format!("Claude background launch outcome unknown: {e}; reconcile the original input"));
                return Ok(());
            }
        };
        let stdout=String::from_utf8_lossy(&out.stdout);
        let stderr=String::from_utf8_lossy(&out.stderr);
        let returned=stdout.lines().find_map(|line|line.strip_prefix("backgrounded · ").and_then(|s|s.split_whitespace().next())).filter(|id|native_job_id(id));
        if let Some(id)=returned {
            if !self.id.is_empty() && self.id!=id {self.launch_error=Some("Claude returned a different background job; reconcile the original input and session".into());}
            self.id=id.into();
        } else {self.launch_error=Some("Claude returned no valid background ID; launch outcome unknown; reconcile the original input".into());}
        if !out.status.success() {self.launch_error.get_or_insert_with(||format!("Claude launch response failed: {stderr}; submission outcome unknown"));}
        let recorded=File::options().append(true).open(&self.log).and_then(|mut file|
            writeln!(file,"{}",serde_json::json!({"type":"claude.launch","stdout":stdout,"stderr":stderr})));
        if let Err(e)=recorded {self.launch_error.get_or_insert_with(||format!("Claude launch observation not recorded: {e}; native handle retained"));}
        Ok(())
    }
}

impl Proc for ClaudeTurn {
    fn checkpoint(&mut self,cfg:&crate::service::Config,l:&Launch,execution:u64)->Result<(),String> {
        self.recovery=Some(Recovery {install:cfg.install.clone(),root:cfg.root.clone(),task:l.task,attempt:l.attempt,execution});
        let rows=claude_agents(&self.root)?;
        self.recover_job(&rows,claude_home().as_deref())?;
        let row=rows.iter().find(|v|v["id"]==self.id).ok_or("Claude checkpoint job unavailable; preserve original input")?;
        self.capture_identity(row)?;
        self.persist()
    }
    fn native_identity(&self)->Result<coreq::process::Identity,String> {self.native.clone().ok_or("Claude original process identity unavailable".into())}
    fn input_offset(&self)->Result<u64,String> {Ok(0)}
    fn accepted(&mut self) -> Result<bool, String> {
        if self.accepted {Ok(true)} else if let Some(e)=&self.launch_error {Err(e.clone())} else {Ok(false)}
    }
    fn exited(&mut self) -> Result<Option<i32>, String> {
        if self.done { return Ok(Some(0)); }
        let rows = claude_agents(&self.root)?;
        self.recover_job(&rows,claude_home().as_deref())?;
        let row = rows.iter().find(|v| v["id"] == self.id).ok_or("Claude session unavailable; state unknown")?;
        let observed=self.observe(row);
        if self.accepted {self.launch_error=None;}
        if matches!(observed,Ok(None)) && self.launch_error.is_some() {return Err(self.launch_error.clone().unwrap());}
        observed
    }
    fn exit_error(&self) -> Option<&'static str> { self.failure }
    fn kill(&mut self) -> Result<(), String> {
        let rows=claude_agents(&self.root)?;
        self.recover_job(&rows,claude_home().as_deref())?;
        let row=rows.iter().find(|v|v["id"]==self.id).ok_or("Claude stop identity unavailable")?;
        self.capture_identity(row)?;
        if let Err(e)=self.persist() {log!("Claude recovery checkpoint failed before stop: {e}");}
        claude_stop(&self.root, &self.id, self.native.as_ref())?;
        self.done = true;
        Ok(())
    }
    fn id(&self) -> u32 { self.native.as_ref().map_or(self.pid,|n|n.pid) }
}

impl ClaudeTurn {
    fn capture_identity(&mut self,row:&serde_json::Value)->Result<(),String> {
        if row["id"]!=self.id || !self.session.is_empty() && row["sessionId"]!=self.session {
            return Err("Claude process locator differs from the original native job".into());
        }
        if let Some(pid)=claude_pid(row)? {
            if self.native.as_ref().is_some_and(|n|n.pid!=pid) {return Err("Claude native process changed; reconcile original execution".into());}
            let current=coreq::process::identity(pid).map_err(|e|format!("Claude process identity unreadable: {e}"))?.ok_or("Claude reported process absent; retry native readback")?;
            if self.native.as_ref().is_some_and(|n|*n!=current) {return Err("Claude PID incarnation changed; original identity retained".into());}
            self.native=Some(current);
            self.pid=pid;
        }
        Ok(())
    }
    fn persist(&mut self)->Result<(),String> {
        let Some(r)=&self.recovery else {return Ok(())};
        let row=crate::data::KidLine {task:r.task,attempt:r.attempt,runtime:"claude".into(),process:self.native_identity()?,execution:r.execution,offset:0,job:None};
        crate::checkpoint_kid(&r.install,&r.root,row).map_err(|e|format!("Claude native recovery checkpoint failed: {e}"))?;
        self.recovery=None;
        Ok(())
    }
    fn observe(&mut self, row: &serde_json::Value) -> Result<Option<i32>, String> {
        // Job outcome and process exit are separate native observations.
        let state = row["state"].as_str().ok_or("Claude job state unavailable; state unknown")?;
        if !matches!(state,"working"|"idle"|"done"|"blocked"|"failed"|"crashed"|"stopped") {
            return Err(format!("Claude state {state:?} unsupported; inspect native lifecycle before release"));
        }
        self.capture_identity(row)?;
        self.persist()?;
        if !matches!(state, "working" | "idle" | "done" | "blocked") {
            if !self.accepted && (self.launch_error.is_some() || self.before>0) {
                return Err(format!("Claude job {state}; current input acceptance unconfirmed, reconcile the original submission"));
            }
            if !claude_stopped(row, self.native.as_ref())? { return Err(format!("Claude job {state}; native process exit unconfirmed; inspect the original job")); }
            self.failure = Some(claude_failure(row, &self.root, &self.id, claude_home().as_deref()));
            return Ok(Some(-1));
        }
        // On Windows a waking session briefly publishes a different UUID before restore.
        if !self.announced {
            let session = row["sessionId"].as_str().ok_or("Claude session ID unknown")?;
            if !self.session.is_empty() && self.session != session { return Err("Claude resumed another conversation; identity unknown".into()); }
            self.session = session.into();
            let mut file = File::options().append(true).open(&self.log).map_err(|e| e.to_string())?;
            writeln!(file, "{}", serde_json::json!({"type":"thread.started", "thread_id":session, "session_id":session, "runtime":"claude-bg", "permission_mode":self.mode})).map_err(|e| e.to_string())?;
            self.announced = true;
        }
        self.pid = claude_pid(row)?.unwrap_or(self.pid);
        let path = claude_transcript(&self.session).ok_or("Claude transcript unavailable; completion unknown")?;
        if !self.history.read(&path,self.before,&self.input,&self.log)? {return Ok(None);}
        self.accepted=true;
        if row["status"]!="idle" && row["state"]!="done" && row["state"]!="blocked" {return Ok(None);}
        if !self.history.ended {return Ok(None);}
        let answer=self.history.answer.as_deref().ok_or("Claude completed without final text; inspect native logs")?;
        let mut log=File::options().append(true).open(&self.log).map_err(|e|e.to_string())?;
        claude_stop(&self.root, &self.id, self.native.as_ref())?;
        writeln!(log, "{}", serde_json::json!({"type":"result", "session_id":self.session, "result":answer})).map_err(|e| e.to_string())?;
        self.done = true;
        Ok(Some(0))
    }
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

/// A `codex-app` attempt stays owned until its exact native input and turn completion are observed.
pub struct AppTurn {
    root: PathBuf,
    history: AppHistory,
    hand: Option<Box<dyn Proc>>,
    log: PathBuf,
    /// The brief, or the resume text, to queue.
    text: String,
    _input: Option<crate::data::InputLock>,
    binding: Option<serde_json::Value>,
    thread: String,
    /// `task_complete` events of the thread before the queued text.
    done: usize,
    rollout: Option<PathBuf>,
    running: fn() -> Result<bool, String>,
    control: fn(&str,&str)->Result<(),String>,
    cancelled: bool,
    queue_error: Option<String>,
    accepted: bool,
}

impl AppTurn {
    fn queue(&mut self) -> Result<(), String> {
        // the id goes through the Windows shell: only a plain thread id
        if self.thread.is_empty() || !self.thread.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return Err(format!("invalid thread id {:?}", self.thread));
        }
        self.rollout = rollout(&self.thread);
        self.done = completes(self.rollout.as_ref().ok_or("Codex rollout unavailable; turn baseline unknown")?)?;
        if let Some(binding)=&mut self.binding {
            binding["native"]=serde_json::json!({"thread":self.thread,"baseline":self.done});
            record_input(&self.log,binding)?;
        }
        let ok = |c: &mut Command| coreq::process::output("runtime.app-submit", c, std::time::Duration::from_secs(10)).is_ok();
        let url = format!("codex://threads/{}", self.thread);
        let opened = if cfg!(windows) { ok(Command::new("cmd").args(["/C", "start", "", &url])) } else { ok(Command::new("open").arg(&url)) };
        if !opened || !ok(codex_direct().args(["queue", "--thread", &self.thread, "--message", &self.text])) {
            return Err(format!("codex queue failed for thread {}", self.thread));
        }
        Ok(())
    }
}

/// Codex without a `cmd` in between: an npm `codex.cmd` shim runs its `codex.js` through `node`, since `cmd` caps a
/// line at 8191 characters and the brief is longer (§ 8).
fn codex_direct() -> Command {
    let bin = which("codex");
    let js = bin.parent().map(|d| d.join("node_modules/@openai/codex/bin/codex.js"));
    match js.filter(|js| bin.extension().is_some_and(|e| e.eq_ignore_ascii_case("cmd")) && js.is_file()) {
        Some(js) => {
            let node = bin.with_file_name("node.exe");
            let mut c = Command::new(if node.is_file() { node } else { which("node") });
            c.arg(js);
            c
        }
        None => Command::new(bin),
    }
}

impl Proc for AppTurn {
    fn accepted(&mut self) -> Result<bool, String> { Ok(self.accepted) }
    fn exited(&mut self) -> Result<Option<i32>, String> {
        if let Some(h) = &mut self.hand {
            let Some(code) = h.exited()? else { return Ok(None) };
            self.hand = None;
            if self.cancelled { return Ok(Some(code)); }
            self.thread = codex_thread(&self.log).unwrap_or_default();
            if self.thread.is_empty() { return Ok(Some(if code == 0 { -1 } else { code })); }
            if let Err(e) = self.queue() {
                self.queue_error = Some(format!("{e}; submission outcome unknown"));
                return Err(self.queue_error.clone().unwrap());
            }
            return Ok(None);
        }
        if !(self.running)()? { return Err("Codex app unavailable; submitted turn termination unconfirmed".into()); }
        if self.rollout.is_none() {
            self.rollout = rollout(&self.thread);
        }
        let (accepted, ended) = self.history.read(self.rollout.as_ref().ok_or("Codex rollout unavailable; completion unknown")?, self.done, &self.text, &self.root, &self.thread)?;
        self.accepted |= accepted;
        if self.accepted {self.queue_error=None;}
        if ended { return Ok(Some(if self.history.aborted {-1} else {0})); }
        if let Some(e) = &self.queue_error { return Err(e.clone()); }
        Ok(None)
    }
    fn kill(&mut self) -> Result<(), String> {
        if let Some(h) = &mut self.hand {
            self.cancelled = true;
            return h.kill();
        }
        if self.exited()?.is_some() { return Ok(()); }
        let turn=self.history.owned.as_deref().ok_or("Codex App input acceptance unknown; cancellation cannot target a native turn")?;
        (self.control)(&self.thread,turn)
    }
    fn id(&self) -> u32 {
        self.hand.as_ref().map(|h| h.id()).unwrap_or(0)
    }
}

/// The Codex app runs in this login session: macOS `lsappinfo`, Windows a `tasklist /APPS` row of its package.
fn app_running() -> Result<bool, String> {
    coreq::telemetry::capture::operation("runtime.app-readback", || {
    let mut c = if cfg!(windows) { Command::new("tasklist") } else { Command::new("lsappinfo") };
    if cfg!(windows) {
        c.args(["/APPS", "/FO", "CSV", "/NH"]);
    } else {
        c.args(["find", "bundleID=com.openai.codex"]);
    }
    let out = coreq::process::output("runtime.app-presence", &mut c, std::time::Duration::from_secs(10)).map_err(|e| format!("Codex app presence unknown: {e}"))?;
    let text = std::str::from_utf8(&out.stdout).map_err(|_| "Codex app presence returned invalid UTF-8")?;
    app_presence(text, cfg!(windows))
    }, |_| "readback")
}

fn app_presence(text: &str, windows: bool) -> Result<bool, String> {
    if text.trim().is_empty() { return Ok(false); }
    if windows {
        let mut found = false;
        for line in text.lines() {
            let row = line.strip_prefix('"').and_then(|s| s.strip_suffix('"')).ok_or("Codex app presence returned an invalid process list")?;
            let columns: Vec<_> = row.split("\",\"").collect();
            if columns.len() < 3 || columns[1].parse::<u32>().ok().filter(|n| *n > 0).is_none()
                || columns.iter().any(|s| s.replace("\"\"", "").contains('"')) {
                return Err("Codex app presence returned an invalid process list".into());
            }
            found |= columns.iter().any(|s| s.starts_with("OpenAI.Codex_"));
        }
        Ok(found)
    } else {
        let valid = |line: &str| {
            let Some((asn, title)) = line.strip_prefix("ASN:0x").and_then(|s| s.split_once("-\"")) else { return false };
            let Some((high, low)) = asn.split_once("-0x") else { return false };
            u32::from_str_radix(high, 16).is_ok() && u32::from_str_radix(low, 16).is_ok() && title.ends_with("\":")
        };
        if !text.lines().all(valid) {
            return Err("Codex app presence returned an invalid application list".into());
        }
        Ok(true)
    }
}

/// The thread's rollout file under `$CODEX_HOME/sessions` (default `~/.codex`; a Windows service has no `HOME`).
pub(crate) fn rollout(thread: &str) -> Option<PathBuf> {
    let home = std::env::var_os("CODEX_HOME").map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(|h| Path::new(&h).join(".codex")))?;
    fn find(dir: &Path, end: &str) -> Option<PathBuf> {
        std::fs::read_dir(dir).ok()?.flatten().find_map(|e| {
            let p = e.path();
            if p.is_dir() { find(&p, end) } else { p.file_name()?.to_str()?.ends_with(end).then_some(p) }
        })
    }
    find(&home.join("sessions"), &format!("{thread}.jsonl"))
}

/// Volatile cursor over the native append-only history; restart rebuilds it without replaying work.
#[derive(Default)]
struct NativeHistory {
    path: Option<PathBuf>,
    metadata: Option<std::fs::Metadata>,
    offset: u64,
    boundary: Vec<u8>,
}

impl NativeHistory {
    fn read(&mut self,path:&Path,mut visit:impl FnMut(serde_json::Value)->Result<(),String>)->Result<bool,String> {
        self.lines(path,|line| {
            let event:serde_json::Value=serde_json::from_str(line).map_err(|_|"native history record invalid")?;
            if event["type"].as_str().is_none() {return Err("native history record shape invalid".into());}
            visit(event)
        })
    }
    fn lines(&mut self,path:&Path,mut visit:impl FnMut(&str)->Result<(),String>)->Result<bool,String> {
        use std::io::{Seek,SeekFrom};
        const LIMIT:u64=8*1024*1024;
        let mut file=File::open(path).map_err(|e|format!("native history unavailable: {e}"))?;
        let meta=file.metadata().map_err(|e|e.to_string())?;
        if !meta.is_file() {return Err("native history is not a regular file".into());}
        if let Some(old)=&self.metadata {
            #[cfg(unix)]
            {use std::os::unix::fs::MetadataExt;
                if (old.dev(),old.ino())!=(meta.dev(),meta.ino()) {return Err("native history file replaced; reconcile original input".into());}
            }
            if self.path.as_deref()!=Some(path) || old.created().ok()!=meta.created().ok() {
                return Err("native history identity changed; reconcile original input".into());
            }
            if meta.len()<self.offset || meta.len()<old.len() {return Err("native history truncated; completion unknown".into());}
            if meta.len()==old.len() && meta.modified().ok()!=old.modified().ok() {return Err("native history rewritten; completion unknown".into());}
        }
        if !self.boundary.is_empty() {
            file.seek(SeekFrom::Start(self.offset-self.boundary.len() as u64)).map_err(|e|e.to_string())?;
            let mut bytes=vec![0;self.boundary.len()];file.read_exact(&mut bytes).map_err(|e|e.to_string())?;
            if bytes!=self.boundary {return Err("native history boundary changed; completion unknown".into());}
        }
        self.path=Some(path.into());self.metadata=Some(meta);
        file.seek(SeekFrom::Start(self.offset)).map_err(|e|e.to_string())?;
        let mut reader=BufReader::new(file.take(LIMIT+1));
        let (mut line,mut consumed)=(String::new(),0u64);
        loop {
            line.clear();
            let size=reader.read_line(&mut line).map_err(|e|format!("native history read failed: {e}"))? as u64;
            if size==0 {break;}
            if size>LIMIT {return Err("native history record exceeds 8 MiB; completion unknown".into());}
            if consumed+size>LIMIT {return Ok(false);}
            if !line.ends_with('\n') {return Err("native history record incomplete; retry observation".into());}
            visit(&line)?;
            self.offset+=size;consumed+=size;
            self.boundary=line.as_bytes()[line.len().saturating_sub(64)..].to_vec();
        }
        let length=reader.get_ref().get_ref().metadata().map_err(|e|e.to_string())?.len();
        if length<self.offset {return Err("native history truncated while reading".into());}
        Ok(length==self.offset)
    }
}

#[derive(Default)]
struct AppHistory {
    reader: NativeHistory,
    count: usize,
    current: Option<String>,
    owned: Option<String>,
    ended: bool,
    aborted: bool,
    identity: bool,
}

/// Match this submission and its native turn id; another user turn cannot release its fence.
impl AppHistory {
    fn read(&mut self,path:&Path,baseline:usize,input:&str,root:&Path,thread:&str)->Result<(bool,bool),String> {
        let Self {reader,count,current,owned,ended,aborted,identity}=self;
        let complete=reader.read(path,|v| {
            if v["type"] == "event_msg" && !v["payload"].is_object() {return Err("Codex rollout record shape invalid".into());}
            if v["type"] == "session_meta" {
                let meta=&v["payload"];
                if *identity || meta["id"]!=thread || meta["cwd"].as_str().and_then(|p|Path::new(p).canonicalize().ok()).as_deref()!=Some(root)
                    || !meta["forked_from_id"].is_null() {
                    return Err("Codex native session identity differs or is copied; inspect the original submission".into());
                }
                *identity=true;
            }
            if v["type"] != "event_msg" {return Ok(());}
            if !*identity {return Err("Codex native session identity missing before turn events".into());}
            let p=&v["payload"];
            if *count>=baseline {
                match p["type"].as_str() {
                    Some("task_started")=>*current=p["turn_id"].as_str().map(String::from),
                    Some(kind) if (kind=="user_message" && p["message"].as_str()==Some(input))
                        || (kind=="item_completed" && p["item"]["type"]=="UserMessage"
                            && p["item"]["content"].as_array().is_some_and(|parts|parts.len()==1 && parts[0]["type"]=="text" && parts[0]["text"].as_str().is_some_and(|text|text==input || text.strip_suffix('\n')==Some(input))))=>{
                        let id=current.clone().ok_or("Codex submitted input has no native turn identity")?;
                        if kind=="item_completed" && (p["thread_id"]!=thread || p["turn_id"].as_str()!=Some(id.as_str())) {
                            return Err("Codex submitted user item has conflicting native thread/turn identity".into());
                        }
                        if p["turn_id"].as_str().is_some_and(|turn|turn!=id) {return Err("Codex submitted input has conflicting native turn identity".into());}
                        if owned.is_some() {return Err("Codex submitted input matches multiple native messages".into());}
                        *owned=Some(id);
                    },
                    Some(kind @ ("task_complete"|"turn_aborted")) if owned.is_some() && p["turn_id"].as_str()==owned.as_deref()=>{
                        let cancelled=kind=="turn_aborted";
                        if *ended && *aborted!=cancelled {return Err("Codex native terminal outcomes conflict; reconcile original turn".into());}
                        *ended=true;*aborted=cancelled;
                    },
                    _=>{},
                }
            }
            if p["type"]=="task_complete" {*count+=1;*current=None;}
            if p["type"]=="turn_aborted" && p["turn_id"].as_str()==current.as_deref() {*current=None;}
            Ok(())
        })?;
        if !complete {return Err("Codex native history scan pending; retry observation without queueing".into());}
        if !*identity {return Err("Codex native session identity unavailable".into());}
        if *count<baseline {return Err("Codex rollout truncated; completion unknown".into());}
        Ok((owned.is_some(),*ended))
    }
}

/// Finished turns in a rollout: its `task_complete` events.
fn completes(rollout: &Path) -> Result<usize, String> {
    coreq::telemetry::capture::operation("runtime.turn-readback", || {
        let mut reader=NativeHistory::default();let mut count=0;
        let until=std::time::Instant::now()+std::time::Duration::from_secs(5);
        loop {
            let complete=reader.read(rollout,|v| {
                if v["type"]=="event_msg" && !v["payload"].is_object() {return Err("Codex rollout record shape invalid".into());}
                count+=usize::from(v["type"]=="event_msg" && v["payload"]["type"]=="task_complete");
                Ok(())
            })?;
            if complete {return Ok(count);}
            if std::time::Instant::now()>=until {return Err("Codex baseline scan exceeded 5 seconds; no input queued".into());}
        }
    }, |_| "readback")
}

// ---------- `taskq agents` (taskq.md § 5): read-only probes, unknown when unreadable (R12) ----------

/// Exit success and bounded stdout + stderr; incomplete execution/output remains unknown.
fn probe(name: &'static str, bin: &Path, args: &[&str]) -> Option<(bool, String)> {
    probe_within(name, bin, args, std::time::Duration::from_secs(10))
}

fn probe_within(name: &'static str, bin: &Path, args: &[&str], budget: std::time::Duration) -> Option<(bool, String)> {
    let out = match coreq::process::output(name, Command::new(bin).args(args), budget) {
        Ok(out) | Err(coreq::process::Failure::Exit(out)) => out,
        Err(_) => return None,
    };
    Some((out.status.success(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))))
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

/// The runtimes taskq can start here (§ 8): `codex-app` is macOS and Windows only and shares Codex's binary and login.
pub fn agent_runtimes() -> &'static [&'static str] {
    if cfg!(any(target_os = "macos", windows)) { &["claude", "codex", "codex-app"] } else { &["claude", "codex"] }
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
            let ver = probe("runtime.version", &p, &["--version"]).and_then(|(_, o)| version(&o));
            let (state, account) = login(bin, probe("runtime.login", &p, if bin == "claude" { &["auth", "status"] } else { &["login", "status"] }));
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
    use std::process::Stdio;
    use super::*;

    // Chain: installed npm entrypoint -> exact native executable -> owned process identity.
    // Risk: tracking a launcher lets its child survive cancellation and falsely releases capacity.
    #[cfg(unix)]
    #[test]
    fn codex_launch_resolves_native_package_without_wrapper_fallback() {
        let root=std::env::temp_dir().join(format!("taskq-codex-package-{}",uuid()));
        let package=root.join("node_modules/@openai/codex");let bin=package.join("bin/codex.js");
        std::fs::create_dir_all(bin.parent().unwrap()).unwrap();std::fs::write(&bin,"#!/usr/bin/env node\n").unwrap();
        std::fs::write(package.join("package.json"),r#"{"name":"@openai/codex"}"#).unwrap();
        assert!(codex_native(bin.clone()).is_err(),"a missing native package must not run the launcher");
        for target in ["aarch64-apple-darwin","x86_64-apple-darwin","aarch64-unknown-linux-musl","x86_64-unknown-linux-musl"] {
            let path=package.join(format!("vendor/{target}/bin/codex"));std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path,b"\x7fELFfixture").unwrap();
        }
        let native=codex_native(bin.clone()).unwrap();
        assert_ne!(native,bin);assert_eq!(codex_native(native.clone()).unwrap(),native);
        std::fs::write(&native,"#!/bin/sh\n").unwrap();assert!(codex_native(bin.clone()).is_err());
        std::fs::write(&native,b"\x7fELFfixture").unwrap();
        std::fs::write(package.join("package.json"),r#"{"name":"foreign"}"#).unwrap();
        assert!(codex_native(bin).unwrap_err().contains("identity differs"));
        std::fs::remove_dir_all(root).unwrap();
    }

    // Chain: actual installed Codex entrypoint -> native PID -> stop -> observed exit.
    // No prompt is sent; EOF cleans up even if an assertion fails before cancellation.
    #[cfg(any(unix,windows))]
    #[test]
    #[ignore="owned installed CLI packaging probe; no model or prompt"]
    fn native_codex_executable_stop_without_model() {
        let entry=which("codex");let native=codex_native(entry.clone()).unwrap();
        let version=coreq::process::output("qualification.codex-version",Command::new(&native).arg("--version"),std::time::Duration::from_secs(5)).unwrap();
        let mut child=Command::new(&native).args(["exec","--json","-"])
            .env_remove("CODEX_THREAD_ID").env_remove("CLAUDE_CODE_SESSION_ID")
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        let identity=coreq::process::identity(child.id()).unwrap().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert_eq!(child.try_wait().unwrap(),None);
        assert_eq!(coreq::process::identity(child.id()).unwrap(),Some(identity.clone()));
        coreq::process::terminate(&identity).unwrap();child.wait().unwrap();
        assert_eq!(coreq::process::identity(identity.pid).unwrap(),None);
        let output=child.wait_with_output().unwrap();assert!(output.stdout.is_empty(),"no native turn should start without input");
        println!("{}entry={} native={} pid={} native exit confirmed; no model dispatched",String::from_utf8_lossy(&version.stdout),entry.display(),native.display(),identity.pid);
    }

    // Chain: actual native Codex -> owner crash -> cold adoption -> cancellation/exit receipt.
    // The controller holds stdin open; no prompt, model, tools or live board are involved.
    #[cfg(any(target_os="macos",windows))]
    #[test]
    #[ignore="owned installed CLI cold-recovery probe; no model or prompt"]
    fn native_codex_cold_owner_restart() {
        let only=std::env::var("TASKQ_NATIVE_OWNER_ONLY").ok();
        assert!(only.as_deref().is_none_or(|phase|matches!(phase,"cancel"|"ended")),"invalid native fixture selector");
        for phase in ["cancel","ended"] {
            if only.as_deref().is_some_and(|selected|selected!=phase) {continue;}

            let root=std::env::temp_dir().join(format!("taskq-native-owner-{phase}-{}",uuid()));
            std::fs::create_dir_all(&root).unwrap();let root=root.canonicalize().unwrap();
            let mut command=Command::new(std::env::current_exe().unwrap());
            command.args(["--exact","runtime::tests::native_codex_owner_child","--ignored","--nocapture"])
                .env("TASKQ_NATIVE_OWNER_ROOT",&root).env("TASKQ_NATIVE_OWNER_CASE",phase);
            let mut old=command.env("TASKQ_NATIVE_OWNER_PHASE","before").stdin(Stdio::piped())
                .stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
            let mut input=Some(old.stdin.take().unwrap());
            let until=std::time::Instant::now()+std::time::Duration::from_secs(5);
            while !root.join("ready").exists() {
                assert!(old.try_wait().unwrap().is_none(),"owner exited before checkpoint");
                assert!(std::time::Instant::now()<until,"native checkpoint timed out");
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            let saved:serde_json::Value=serde_json::from_slice(&std::fs::read(root.join("ready")).unwrap()).unwrap();
            let identity=coreq::process::Identity {pid:saved["pid"].as_u64().unwrap() as u32,started:saved["started"].as_str().unwrap().into()};
            old.kill().unwrap();old.wait().unwrap();
            assert_eq!(coreq::process::identity(identity.pid).unwrap(),Some(identity.clone()),"the native executor must survive its owner for this case");
            if phase=="ended" {
                drop(input.take());
                while !coreq::process::identity(identity.pid).is_ok_and(|p|p.is_none()) {
                    assert!(std::time::Instant::now()<until,"native exit after EOF timed out");
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
            }
            let result=coreq::process::output("qualification.native-cold-owner",command.env("TASKQ_NATIVE_OWNER_PHASE","after")
                .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()),std::time::Duration::from_secs(10));
            drop(input);
            assert!(result.is_ok(),"{phase}: {result:?}");
            let output=result.unwrap();
            for line in String::from_utf8_lossy(&output.stdout).lines().filter(|l|l.starts_with("intermediate native readback")) {println!("{phase}: {line}");}
            assert_eq!(coreq::process::identity(identity.pid).unwrap(),None);
            let expected=if cfg!(windows) && phase=="ended" {"missing native scope; explicit unknown; evidence retained; no release or replay"}
                else {"same native execution; acceptance unknown; exit read back; no replay"};
            assert_eq!(std::fs::read_to_string(root.join("verified")).unwrap(),expected);
            println!("{phase}: native pid={} survived owner crash; {expected}; no model dispatched",identity.pid);
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[cfg(any(target_os="macos",windows))]
    #[test]
    #[ignore="native CLI owner process fixture"]
    fn native_codex_owner_child() {
        use crate::board::Board;
        let root=PathBuf::from(std::env::var_os("TASKQ_NATIVE_OWNER_ROOT").unwrap());
        let b=crate::board::Fake::default();let task=b.create("native fixture","",&[crate::service::TASK_LABEL.into()]).unwrap();
        let attempt=b.comment(task,"taskq: spawn worker codex:fixture mac").unwrap();
        let cfg=crate::service::Config {root:root.clone(),install:root.join("install"),..crate::tests::cfg()};
        let mut service=crate::service::Service::new(&b,&cfg,"native-fixture",Box::new(|_|panic!("native recovery must not dispatch")));
        if std::env::var("TASKQ_NATIVE_OWNER_PHASE").unwrap()=="before" {
            let log=attempt_log(&root,"worker",task,attempt);std::fs::create_dir_all(log.parent().unwrap()).unwrap();
            let output=File::create(&log).unwrap();
            let mut command=Command::new(codex_native(which("codex")).unwrap());
            command.args(["exec","--json","-"]).current_dir(&root).env_remove("CODEX_THREAD_ID").env_remove("CLAUDE_CODE_SESSION_ID")
                .stdin(Stdio::inherit()).stderr(output.try_clone().unwrap()).stdout(output);
            #[cfg(windows)] let child:Box<dyn Proc>=Box::new(windows_process::Execution::spawn(&mut command).unwrap());
            #[cfg(not(windows))] let child:Box<dyn Proc>=Box::new(command.spawn().unwrap());
            let native=child.native_identity().unwrap();
            let launch=Launch {execution:Some(attempt),task,title:String::new(),attempt,role:"worker".into(),rt:"codex".into(),session:"fixture".into(),url:String::new(),resume:None};
            let mut proc=CliTurn {proc:child,log,pos:0,start:0,stopping:false,accepted:false,read_error:None,native:Some(native.clone()),recovery:None};
            proc.checkpoint(&cfg,&launch,attempt).unwrap();
            service.kids.push(crate::service::Kid {task,attempt,execution:attempt,rt:"codex".into(),proc:Box::new(proc)});
            crate::data::kids(&std::fs::read_to_string(crate::kids_path(&cfg.install,&root).unwrap()).unwrap()).unwrap();
            coreq::write_durable(&root.join("ready"),&serde_json::json!({"pid":native.pid,"started":native.started}).to_string()).unwrap();
            let until=std::time::Instant::now()+std::time::Duration::from_secs(10);
            loop {assert!(std::time::Instant::now()<until,"controller did not kill the owner");std::thread::sleep(std::time::Duration::from_millis(10));}
        }
        #[cfg(windows)]
        if std::env::var("TASKQ_NATIVE_OWNER_CASE").unwrap()=="ended" {
            let error=crate::adopt(&mut service,&cfg.install,&root).unwrap_err();
            assert!(error.contains("native reconciliation"),"{error}");
            assert!(service.kids.is_empty());assert_eq!(crate::kids_file(&cfg.install,&root).unwrap().len(),1);
            assert_eq!(crate::core::executions(task,&crate::board::entries(&b,task).unwrap())[0].release,None);
            std::fs::write(root.join("verified"),"missing native scope; explicit unknown; evidence retained; no release or replay").unwrap();
            return;
        }
        crate::adopt(&mut service,&cfg.install,&root).unwrap();
        assert_eq!(service.kids.len(),1);assert_eq!(service.kids[0].execution,attempt);
        if std::env::var("TASKQ_NATIVE_OWNER_CASE").unwrap()=="cancel" {
            service.reap_owned();assert_eq!(service.kids.len(),1,"a live executor cannot release on owner restart");
            assert!(!crate::core::executions(task,&crate::board::entries(&b,task).unwrap())[0].accepted);
            service.kids[0].proc.kill().unwrap();
        }
        let until=std::time::Instant::now()+std::time::Duration::from_secs(5);
        let mut unknown=std::collections::BTreeSet::new();
        loop {
            match service.kids[0].proc.exited() {
                Ok(Some(_))=>break,
                Err(error)=>{unknown.insert(error);},
                Ok(None)=>{},
            }
            assert!(std::time::Instant::now()<until,"native stop did not finish: {unknown:?}");std::thread::sleep(std::time::Duration::from_millis(10));
        }
        println!("intermediate native readback errors (never terminal): {unknown:?}");
        b.state.borrow_mut().fail_comments=1;
        service.reap_owned();assert_eq!(service.kids.len(),1,"lost exit receipt must retain ownership");
        assert_eq!(crate::kids_file(&cfg.install,&root).unwrap().len(),1);
        service.reap_owned();assert!(service.kids.is_empty());
        let x=crate::core::executions(task,&crate::board::entries(&b,task).unwrap()).remove(0);
        assert!(!x.accepted);assert_eq!(x.release,Some(crate::core::Release::Exit));
        assert!(crate::kids_file(&cfg.install,&root).unwrap().is_empty());
        std::fs::write(root.join("verified"),"same native execution; acceptance unknown; exit read back; no replay").unwrap();
    }

    // Chain: prepared brief -> native stdin -> consumer or non-reading child, with no blocking producer.
    #[test]
    fn file_stdin_handoff_preserves_bytes_and_never_waits_for_a_reader() {
        let root = std::env::temp_dir().join(format!("taskq-input-handoff-{}", uuid()));
        std::fs::create_dir_all(&root).unwrap();
        let text = format!("{}EOF-CANARY", "private-input-".repeat(12_000));
        for mode in ["blocked", "read"] {
            let go = root.join(mode);
            let mut command = Command::new(std::env::current_exe().unwrap());
            command.args(["--exact", "runtime::tests::stdin_child", "--ignored"])
                .env("TASKQ_STDIN_MODE", mode).env("TASKQ_STDIN_GO", &go).stdout(Stdio::null()).stderr(Stdio::null());
            let began = std::time::Instant::now();
            let mut child = start_command(&mut command, &root, &text).unwrap_or_else(|e| panic!("{e}"));
            assert!(began.elapsed() < std::time::Duration::from_secs(3), "handoff waited for the child to read");
            for entry in std::fs::read_dir(root.join(".taskq/inputs")).unwrap() {
                let path = entry.unwrap().path();
                #[cfg(unix)]
                { use std::os::unix::fs::PermissionsExt; assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600); }
                std::fs::remove_file(path).unwrap();
            }
            if mode == "blocked" { let _pending = stop(child.as_mut()); }
            else { std::fs::write(&go, "read now").unwrap(); }
            let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
            let status = loop {
                if let Some(status) = child.exited().unwrap() { break status; }
                if std::time::Instant::now() >= until {
                    let cleanup = stop(child.as_mut());
                    panic!("input fixture did not finish; cleanup: {cleanup:?}");
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            };
            if mode == "read" { assert!(status==0, "child could not read the original bytes after cache unlink"); }
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[ignore = "file stdin handoff fixture"]
    fn stdin_child() {
        if std::env::var("TASKQ_STDIN_MODE").unwrap() == "blocked" {
            std::thread::sleep(std::time::Duration::from_secs(30)); return;
        }
        let go = PathBuf::from(std::env::var_os("TASKQ_STDIN_GO").unwrap());
        while !go.exists() { std::thread::sleep(std::time::Duration::from_millis(5)); }
        let mut text = String::new(); std::io::stdin().read_to_string(&mut text).unwrap();
        assert_eq!(text, format!("{}EOF-CANARY", "private-input-".repeat(12_000)));
    }

    // Chain: native owned child / exec-adopted child -> stop request -> OS exit readback.
    #[test]
    fn native_stop_reaps_the_owned_process() {
        let spawn = || Command::new(std::env::current_exe().unwrap()).args(["--exact", "runtime::tests::stop_child", "--ignored"])
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
        let check = |proc: &mut dyn Proc| {
            let requested = stop(proc);
            let until = std::time::Instant::now() + std::time::Duration::from_secs(3);
            loop {
                if proc.exited().unwrap().is_some() { break; }
                assert!(std::time::Instant::now() < until, "owned stop did not finish: {requested:?}");
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        };
        check(&mut spawn());
        #[cfg(unix)]
        {
            let child = spawn(); let pid = child.id(); drop(child);
            check(&mut Adopted::new(coreq::process::identity(pid).unwrap().unwrap()));
            assert_eq!(liveness(pid), Ok(false));
        }
    }

    // Chain: production stdin launch -> launcher exits -> exact known descendant remains live.
    // Risk: root exit releases the execution while an owned tool is still running.
    #[cfg(windows)]
    #[test]
    #[ignore="owned Windows process-tree qualification; no model or board"]
    fn native_cli_owner_exit_does_not_release_live_descendant() {
        use crate::board::Board;
        for cold in [false,true] {
            let root=std::env::temp_dir().join(format!("taskq-native-tree-{}",uuid()));std::fs::create_dir_all(&root).unwrap();
            let board=crate::board::Fake::default();let task=board.create("native scope fixture","",&[crate::service::TASK_LABEL.into()]).unwrap();
            let attempt=board.comment(task,"taskq: spawn worker codex:fixture mac").unwrap();
            let cfg=crate::service::Config {root:root.clone(),install:root.join("install"),..crate::tests::cfg()};
            let log=attempt_log(&root,"worker",task,attempt);std::fs::create_dir_all(log.parent().unwrap()).unwrap();std::fs::write(&log,"").unwrap();
            let mut command=Command::new(std::env::current_exe().unwrap());
            command.args(["--exact","runtime::tests::windows_tree_owner_child","--ignored"])
                .env("TASKQ_TREE_OWNER_ROOT",&root).stdout(Stdio::null()).stderr(Stdio::null());
            let child=start_command(&mut command,&root,"").unwrap();let native=child.native_identity().unwrap();
            let mut turn=CliTurn {proc:child,log,pos:0,start:0,stopping:false,accepted:false,read_error:None,native:Some(native.clone()),recovery:None};
            let launch=Launch {execution:Some(attempt),task,title:String::new(),attempt,role:"worker".into(),rt:"codex".into(),session:"fixture".into(),url:String::new(),resume:None};
            turn.checkpoint(&cfg,&launch,attempt).unwrap();
            let saved=crate::kids_file(&cfg.install,&root).unwrap();assert_eq!(saved.len(),1);assert!(saved[0].job.is_some());
            let mut legacy=saved[0].clone();legacy.job=None;
            assert!(execution_live(&legacy).unwrap_err().contains("legacy"));
            let until=std::time::Instant::now()+std::time::Duration::from_secs(10);
            while !root.join("descendant.json").exists() {
                assert!(std::time::Instant::now()<until,"owned fixture did not announce its descendant");std::thread::sleep(std::time::Duration::from_millis(10));
            }
            let row:serde_json::Value=serde_json::from_slice(&std::fs::read(root.join("descendant.json")).unwrap()).unwrap();
            let descendant=coreq::process::Identity {pid:row["pid"].as_u64().unwrap() as u32,started:row["started"].as_str().unwrap().into()};
            let mut unrelated=Command::new(std::env::current_exe().unwrap());
            unrelated.args(["--exact","runtime::tests::stop_child","--ignored"]).stdout(Stdio::null()).stderr(Stdio::null());
            let mut sentinel=start_command(&mut unrelated,&root,"").unwrap();
            assert!(windows_process::Execution::recover(native.clone(),&sentinel.job().unwrap()).err().unwrap().contains("does not belong"));
            assert_eq!(turn.exited(),Ok(None));std::fs::write(root.join("exit-owner"),"").unwrap();
            while coreq::process::identity(native.pid).unwrap().as_ref()==Some(&native) {
                assert!(std::time::Instant::now()<until,"fixture launcher did not exit");std::thread::sleep(std::time::Duration::from_millis(10));
            }
            assert_eq!(coreq::process::identity(descendant.pid).unwrap(),Some(descendant.clone()));
            assert!(execution_live(&saved[0]).unwrap(),"scope-aware readback cannot retire surviving descendants");
            let mut service=crate::service::Service::new(&board,&cfg,"native-fixture",Box::new(|_|panic!("native reconstruction must not dispatch")));
            let mut proc:Box<dyn Proc>=if cold {
                drop(turn);
                let error=crate::adopt(&mut service,&cfg.install,&root).unwrap_err();
                assert!(error.contains("native reconciliation"),"{error}");
                assert!(service.kids.is_empty());assert_eq!(crate::kids_file(&cfg.install,&root).unwrap().len(),1);
                assert_eq!(crate::core::executions(task,&crate::board::entries(&board,task).unwrap())[0].release,None);
                assert_eq!(coreq::process::identity(descendant.pid).unwrap(),Some(descendant.clone()),"unknown recovery cannot stop or release work");
                // The fixture knows this exact descendant; product recovery has no such surviving observation.
                coreq::process::terminate(&descendant).unwrap();
                while coreq::process::identity(descendant.pid).unwrap().as_ref()==Some(&descendant) {assert!(std::time::Instant::now()<until,"fixture descendant cleanup timed out");std::thread::sleep(std::time::Duration::from_millis(10));}
                assert_eq!(sentinel.exited(),Ok(None));sentinel.kill().unwrap();
                while sentinel.exited().unwrap().is_none() {assert!(std::time::Instant::now()<until,"sentinel cleanup not confirmed");std::thread::sleep(std::time::Duration::from_millis(10));}
                drop(service);drop(sentinel);std::fs::remove_dir_all(root).unwrap();
                println!("cold=true: native scope unavailable; explicit unknown; protected evidence retained; no replay/release; exact fixture cleanup confirmed");
                continue;
            } else {Box::new(turn)};
            assert_eq!(proc.exited(),Ok(None),"launcher exit cannot release a live descendant");
            proc.kill().unwrap();
            while proc.exited().unwrap().is_none() {assert!(std::time::Instant::now()<until,"native job cancellation did not finish");std::thread::sleep(std::time::Duration::from_millis(10));}
            assert_ne!(coreq::process::identity(descendant.pid).unwrap(),Some(descendant.clone()));
            assert!(!execution_live(&saved[0]).unwrap());assert_eq!(sentinel.exited(),Ok(None),"another execution's job must remain live");
            sentinel.kill().unwrap();
            while sentinel.exited().unwrap().is_none() {assert!(std::time::Instant::now()<until,"sentinel cleanup not confirmed");std::thread::sleep(std::time::Duration::from_millis(10));}
            service.kids.push(crate::service::Kid {task,attempt,execution:attempt,rt:"codex".into(),proc});
            board.state.borrow_mut().fail_comments=1;service.reap_owned();assert_eq!(service.kids.len(),1,"failed terminal receipt retains ownership");
            service.reap_owned();assert!(service.kids.is_empty());assert!(crate::kids_file(&cfg.install,&root).unwrap().is_empty());
            assert_eq!(crate::core::executions(task,&crate::board::entries(&board,task).unwrap())[0].release,Some(crate::core::Release::Exit));
            drop(service);drop(sentinel);std::fs::remove_dir_all(root).unwrap();
            println!("cold={cold}: launcher exit held while descendant lived; native job cancelled; unrelated job preserved; terminal receipt retried; protected record retired");
        }
    }

    #[cfg(windows)]
    #[test]
    #[ignore="owned Windows tree launcher fixture"]
    fn windows_tree_owner_child() {
        let root=PathBuf::from(std::env::var_os("TASKQ_TREE_OWNER_ROOT").unwrap());
        let child=Command::new(std::env::current_exe().unwrap()).args(["--exact","runtime::tests::stop_child","--ignored"])
            .env_remove("TASKQ_TREE_OWNER_ROOT").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
        let identity=coreq::process::identity(child.id()).unwrap().unwrap();
        coreq::write_durable(&root.join("descendant.json"),&serde_json::json!({"pid":identity.pid,"started":identity.started}).to_string()).unwrap();
        let until=std::time::Instant::now()+std::time::Duration::from_secs(10);
        while !root.join("exit-owner").exists() {assert!(std::time::Instant::now()<until,"controller did not release fixture launcher");std::thread::sleep(std::time::Duration::from_millis(10));}
        drop(child);
    }

    #[test]
    #[ignore = "owned process stop fixture"]
    fn stop_child() { std::thread::sleep(std::time::Duration::from_secs(30)); }

    // Chain: terminal CLI log / cancelled app handshake -> stop -> observed exit, never a synthetic completion.
    #[test]
    fn stop_retains_terminal_cli_and_cancelled_handshake_until_exit() {
        use std::{cell::Cell, rc::Rc};
        struct Pending { exit: Rc<Cell<bool>>, kills: Rc<Cell<usize>> }
        impl Proc for Pending {
            fn exited(&mut self) -> Result<Option<i32>, String> { Ok(self.exit.get().then_some(-9)) }
            fn kill(&mut self) -> Result<(), String> { self.kills.set(self.kills.get() + 1); Ok(()) }
            fn id(&self) -> u32 { 0 }
        }
        use tracing_subscriber::prelude::*;
        let dir = std::env::temp_dir().join(format!("taskq-stop-{}", uuid()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = std::sync::Arc::new(coreq::telemetry::Store::open(&dir.join("db")).unwrap());
        let capture = coreq::telemetry::capture::Capture::new(store.clone(), "taskq", "fixture").unwrap();
        tracing::subscriber::with_default(tracing_subscriber::registry().with(capture.clone()), || {
            let log = dir.join("turn.log");
            std::fs::write(&log, "{\"type\":\"turn.completed\"}\n").unwrap();
            let exit = Rc::new(Cell::new(false)); let kills = Rc::new(Cell::new(0));
            let make = || Box::new(Pending { exit: exit.clone(), kills: kills.clone() }) as Box<dyn Proc>;
            let mut cli = CliTurn { proc: make(), log: log.clone(), pos: 0, start: 0, stopping: false, accepted: false, read_error: None, native: None, recovery: None };
            assert_eq!(cli.exited(),Ok(None)); assert_eq!(cli.exited(),Ok(None),"acknowledged stop awaiting exit is pending, not a failed observation");
            assert_eq!(cli.accepted(),Ok(false),"terminal alone is not exact-input acceptance");
            assert_eq!(kills.get(), 2, "terminal state must survive a pending stop");
            exit.set(true); assert_eq!(cli.exited(), Ok(Some(-9)));
            exit.set(false);
            let mut app = AppTurn { root: std::env::temp_dir(), history:AppHistory::default(), hand: Some(make()), log, text: String::new(), _input: None, binding:None, thread: String::new(), done: 0,
                rollout: None, running: || Ok(true), control:|_,_|Err("fixture native cancellation unavailable".into()), cancelled: false, queue_error: None, accepted: false };
            assert!(stop(&mut app).is_err());
            assert!(app.hand.is_some());
            exit.set(true); assert_eq!(app.exited(), Ok(Some(-9)));
            assert!(app.thread.is_empty(), "cancelled handshake must not open or queue an app task");
        });
        capture.flush().unwrap();
        let rows = store.export(0, 0, 100).unwrap();
        assert!(rows["events"].as_array().unwrap().iter().any(|r| r["observation"]["operation"] == "taskq.operation.runtime.stop"
            && r["observation"]["error"] == "stop-unconfirmed"));
        drop(capture); drop(store); std::fs::remove_dir_all(dir).unwrap();
    }

    // Chain: § 4 step 1 `gone` reason, #1034. Risk: a worker's last message publishes local paths or many lines.
    #[test]
    fn last_message_is_one_public_line_without_local_paths() {
        for (root, text, want) in [
            (r"C:\w\taskq", "See [service.rs](/C:/w/taskq/.worktrees/taskq-1/src/service.rs#L5).", "See [service.rs](.worktrees/taskq-1/src/service.rs#L5)."),
            (r"C:\w\taskq", r"Log: c:\W\taskq\.taskq\T1-2.log and C:\Users\me\AppData\x.txt", "Log: .taskq/T1-2.log and <path>"),
            ("/home/me/taskq", "`/home/me/taskq/src/a.rs`, file:///tmp/x/y, ~/.codex/auth.json\n\n  next line", "`src/a.rs`, <path>, <path> next line"),
            ("/home/me/taskq", "Kept: https://github.com/o/r/pull/1 and/or /help, root /home/me/taskq.", "Kept: https://github.com/o/r/pull/1 and/or /help, root .."),
        ] {
            assert_eq!(public_line(text, Path::new(root), 240), want);
        }
        let long = public_line(&"word ".repeat(100), Path::new("/r"), 224);
        assert_eq!((long.chars().count(), long.ends_with(" wor…")), (224, true));
    }

    // Chain: § 4 service pass, `agents` rows (R12), #1114. Risk: a runtime CLI whose child keeps the pipes open blocks the pass.
    #[test]
    fn probe_gives_up_on_time_when_a_grandchild_keeps_the_pipes() {
        let dir = std::env::temp_dir().join(format!("taskq-probe-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = |name: &str, body: &str| {
            let (p, text) = if cfg!(windows) { (dir.join(format!("{name}.cmd")), format!("@echo off\r\n{body}\r\n")) }
                else { (dir.join(name), format!("#!/bin/sh\n{body}\n")) };
            std::fs::write(&p, text).unwrap();
            #[cfg(unix)] std::fs::set_permissions(&p, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
            p
        };
        let (hang, fork) = if cfg!(windows) { ("ping -n 30 127.0.0.1", "start /b ping -n 30 127.0.0.1") } else { ("sleep 30", "sleep 30 &") };
        // the child hangs, then a killed child leaves its grandchild holding the pipes; or the child exits and leaves it
        for (name, body, want) in [("ok", "echo 1.2.3".to_string(), Some("1.2.3")), ("hang", format!("echo 1.2.3\n{hang}"), None),
            ("fork", format!("{fork}\necho 1.2.3"), None)] {
            let t = std::time::Instant::now();
            let got = probe_within("runtime.version", &script(name, &body.replace('\n', if cfg!(windows) { "\r\n" } else { "\n" })), &["--version"], std::time::Duration::from_secs(2));
            assert_eq!(got.as_ref().map(|(ok, out)| (*ok, out.trim())), want.map(|v| (true, v)), "{name}");
            assert!(t.elapsed() < std::time::Duration::from_secs(5), "{name}: {:?}", t.elapsed());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

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

    // Chain: CLI input boundary -> native stream -> acceptance. Risk: an old turn or a process exit consumes new input.
    #[test]
    fn cli_acceptance_uses_only_native_events_after_its_input_boundary() {
        struct Exited;
        impl Proc for Exited {
            fn exited(&mut self) -> Result<Option<i32>,String> { Ok(Some(0)) }
            fn kill(&mut self) -> Result<(),String> { Ok(()) }
            fn id(&self) -> u32 { 0 }
        }
        let dir = std::env::temp_dir().join(format!("taskq-cli-accept-{}",uuid()));
        std::fs::create_dir_all(&dir).unwrap();
        let path=dir.join("turn.log");
        let old="{\"type\":\"turn.started\"}\n{\"type\":\"turn.completed\"}\n";
        for native in ["", "{\"type\":\"turn.started\"}\n", "{\"type\":\"assistant\",\"message\":{}}\n"] {
            std::fs::write(&path,format!("{old}{native}{{\"type\":\"turn.completed\"}}\n")).unwrap();
            let mut turn=CliTurn { proc:Box::new(Exited),log:path.clone(),pos:old.len() as u64,start:old.len() as u64,stopping:false,accepted:false,read_error:None,native:None,recovery:None };
            assert_eq!(turn.accepted(),Ok(false));
            assert_eq!(turn.exited(),Ok(Some(0)));
            assert_eq!(turn.accepted(),Ok(!native.is_empty()));
            assert_eq!(turn.exited(),Ok(Some(0)));
            assert_eq!(turn.accepted(),Ok(!native.is_empty()),"proof survives repeated polls");
        }
        let mut truncated=CliTurn {proc:Box::new(Exited),log:path.clone(),pos:1000,start:1000,stopping:false,accepted:false,read_error:None,native:None,recovery:None};
        assert_eq!(truncated.exited(),Ok(Some(0)),"native exit remains independently observable");
        assert!(truncated.accepted().unwrap_err().contains("truncated"));
        assert!(truncated.input_offset().is_err(),"a lost native input boundary cannot be handed over");
        std::fs::remove_dir_all(dir).unwrap();
    }

    // Chain: § 8 codex-app turn end. Risk: a turn ends early or never ends.
    #[test]
    fn app_turn_keeps_unreadable_completion_unknown() {
        let p = std::env::temp_dir().join(format!("taskq-rollout-unknown-{}.jsonl", std::process::id()));
        let mut t = AppTurn { root: p.parent().unwrap().canonicalize().unwrap(), history:AppHistory::default(), hand: None, log: p.clone(), text: String::new(), _input: None, binding:None, thread: "th".into(), done: 1, rollout: Some(p.clone()), running: || Ok(true), control:|_,_|Err("fixture native cancellation unavailable".into()), cancelled: false, queue_error: None, accepted: false };
        let _ = std::fs::remove_file(&p);
        assert!(t.exited().is_err(), "missing history must retain an explicit unknown reason");
        std::fs::write(&p, "{\"type\":\"event_msg\"").unwrap();
        assert!(t.exited().is_err(), "incomplete history cannot prove the turn is still healthy");
        std::fs::write(&p, "").unwrap();
        assert!(t.exited().is_err(), "history shorter than the baseline cannot reset the turn count");
        t.running = || Err("probe timed out".into());
        assert_eq!(t.exited(), Err("probe timed out".into()), "failed app query cannot become an exit");
        for windows in [false, true] {
            assert!(app_presence("unreadable", windows).is_err());
            assert_eq!(app_presence("", windows), Ok(false));
        }
        assert_eq!(app_presence("ASN:0x0-0x12-\"Codex\":\n", false), Ok(true));
        assert_eq!(app_presence("\"Codex.exe\",\"42\",\"OpenAI.Codex_1\"\n", true), Ok(true));
        assert_eq!(app_presence("\"other.exe\",\"43\",\"Other\"\n", true), Ok(false));
        assert!(app_presence("\"other.exe\",\"unknown\",\"Other\"\n", true).is_err());
        assert!(app_presence("ASN:0xbad:", false).is_err());
        std::fs::remove_file(p).unwrap();
    }

    // Chain: § 8 codex-app turn end. Risk: a turn ends early or never ends.
    #[test]
    fn app_turn_requires_its_input_and_matching_native_completion() {
        let p = std::env::temp_dir().join(format!("taskq-rollout-{}.jsonl", std::process::id()));
        let event = |kind: &str, id: &str, message: &str| serde_json::json!({"type":"event_msg", "payload":{"type":kind,"turn_id":id,"message":message}}).to_string() + "\n";
        let meta=serde_json::json!({"type":"session_meta","payload":{"id":"th","cwd":p.parent().unwrap().canonicalize().unwrap()}});
        let mut history = meta.to_string()+"\n"+&event("task_complete", "old", "");
        std::fs::write(&p, &history).unwrap();
        let mut t = AppTurn { root: p.parent().unwrap().canonicalize().unwrap(), history:AppHistory::default(), hand: None, log: p.clone(), text: "owned input".into(), _input: None, binding:None, thread: "th".into(), done: 1,
            rollout: Some(p.clone()), running: || Ok(true), control:|_,_|Err("fixture native cancellation unavailable".into()), cancelled: false, queue_error: None, accepted: false };
        assert_eq!(t.exited(), Ok(None));
        assert_eq!(t.accepted(),Ok(false));
        for (id, input, completion, expected) in [("other", "unrelated input", "other", false), ("owned", "owned input", "wrong", false)] {
            history += &event("task_started", id, ""); history += &event("user_message", id, input); history += &event("task_complete", completion, "");
            std::fs::write(&p, &history).unwrap();
            assert_eq!(t.exited(), Ok(expected.then_some(0)), "unrelated completion cannot release ownership");
            assert_eq!(t.accepted(),Ok(id == "owned"),"acceptance belongs to the exact input and native turn");
        }
        t.done = 0; t.history=AppHistory::default(); // cold observer has no volatile completion baseline
        t.queue_error = Some("submission response lost".into());
        assert_eq!(t.exited(), Ok(None), "native input acceptance resolves the lost submission response");
        assert!(stop(&mut t).unwrap_err().contains("native cancellation unavailable"), "a live App turn needs an explicit unsupported-control reason");
        assert_eq!(t.exited(), Ok(None), "refused cancellation retains the exact execution observer");
        t.control=|thread,turn|{assert_eq!((thread,turn),("th","owned"));Ok(())};
        assert_eq!(t.kill(),Ok(()));
        assert_eq!(t.exited(),Ok(None),"acknowledged interrupt is not native terminal evidence");
        history += &event("task_complete", "owned", "");
        std::fs::write(&p, &history).unwrap();
        assert_eq!(t.exited(), Ok(Some(0)), "later exact proof resolves an uncertain submission without replay");
        assert_eq!(stop(&mut t), Ok(0), "observed native completion resolves stop even when cancellation is unsupported");
        t.running = || Ok(false);
        assert!(t.exited().unwrap_err().contains("termination unconfirmed"));
        t.running = || Ok(true);
        for change in ["root","thread","copied","missing","duplicate"] {
            let mut wrong=meta.clone();
            match change {
                "root"=>wrong["payload"]["cwd"]="/nonexistent/taskq".into(),
                "thread"=>wrong["payload"]["id"]="foreign".into(),
                "copied"=>wrong["payload"]["forked_from_id"]="original".into(),
                _=>{},
            }
            let (_,events)=history.split_once('\n').unwrap();
            let text=match change {"missing"=>events.to_owned(),"duplicate"=>meta.to_string()+"\n"+&history,_=>wrong.to_string()+"\n"+events};
            std::fs::write(&p,text).unwrap();
            t.history=AppHistory::default();
            assert!(t.exited().is_err(),"{change} cannot prove native completion");
        }
        // Installed desktop emits UserMessage items instead of legacy user_message events.
        let native=serde_json::json!({"type":"event_msg","payload":{"type":"item_completed","thread_id":"th","turn_id":"owned",
            "item":{"type":"UserMessage","id":"native-input","content":[{"type":"text","text":"owned input","text_elements":[]}]}}});
        for change in ["valid","transport-lf","extra-lf","space","aborted","foreign-abort","conflicting-terminal","thread","turn","missing-turn","content","duplicate","duplicate-lf"] {
            let mut item=native.clone();
            match change {
                "thread"=>item["payload"]["thread_id"]="foreign".into(),
                "turn"=>item["payload"]["turn_id"]="foreign".into(),
                "missing-turn"=>{item["payload"].as_object_mut().unwrap().remove("turn_id");},
                "content"=>item["payload"]["item"]["content"][0]["text"]="unrelated".into(),
                "transport-lf"|"duplicate-lf"=>item["payload"]["item"]["content"][0]["text"]="owned input\n".into(),
                "extra-lf"=>item["payload"]["item"]["content"][0]["text"]="owned input\n\n".into(),
                "space"=>item["payload"]["item"]["content"][0]["text"]="owned input ".into(),
                _=>{},
            }
            let mut text=meta.to_string()+"\n"+&event("task_started","owned","")+&item.to_string()+"\n";
            if matches!(change,"duplicate"|"duplicate-lf") {text+=&(native.to_string()+"\n");}
            if matches!(change,"aborted"|"foreign-abort"|"conflicting-terminal") {text+=&event("turn_aborted",if change=="foreign-abort" {"foreign"} else {"owned"},"");}
            if change!="aborted" && change!="foreign-abort" {text+=&event("task_complete","owned","");}
            std::fs::write(&p,text).unwrap();t.history=AppHistory::default();t.accepted=false;
            let outcome=t.exited();
            match change {
                "valid"|"transport-lf"=>{assert_eq!(outcome,Ok(Some(0)));assert_eq!(t.accepted(),Ok(true));},
                "aborted"=>{assert_eq!(outcome,Ok(Some(-1)));assert_eq!(t.accepted(),Ok(true));},
                "foreign-abort"=>{assert_eq!(outcome,Ok(None));assert_eq!(t.accepted(),Ok(true));},
                "content"|"extra-lf"|"space"=>{assert_eq!(outcome,Ok(None));assert_eq!(t.accepted(),Ok(false));},
                _=>assert!(outcome.is_err(),"{change} cannot establish input acceptance"),
            }
        }
        t.thread = "x & calc".into();
        assert!(t.queue().is_err());
        std::fs::remove_file(p).unwrap();
    }

    // Chain: native queued input -> lost-response cold observer -> exact native completion.
    // Read-only qualification of an already-completed dedicated probe; no dispatch or control.
    #[test]
    #[ignore = "owned native probe; TASKQ_APP_READBACK selects captured lifecycle evidence"]
    fn native_app_lost_reply_readback() {
        let evidence=PathBuf::from(std::env::var("TASKQ_APP_READBACK").expect("TASKQ_APP_READBACK"));
        let read=|file:&str|serde_json::from_str::<serde_json::Value>(&std::fs::read_to_string(evidence.join(file)).unwrap()).unwrap();
        let manifest=read("manifest.json");
        for (file,code) in [("queue-command.json",0),("interrupt-queue.json",-1)] {
        let queued=read(file);
        let root=PathBuf::from(manifest["root"].as_str().unwrap()).canonicalize().unwrap();
        let thread=manifest["thread"].as_str().unwrap().to_owned();let text=queued["input"].as_str().unwrap().to_owned();
        let mut turn=AppTurn {root,history:AppHistory::default(),hand:None,log:evidence.join("handshake.jsonl"),text,_input:None,binding:None,
            rollout:rollout(&thread),thread,done:0,running:app_running,control:app_cancel,cancelled:false,queue_error:Some("fixture: submission response lost".into()),accepted:false};
        assert_eq!(turn.exited(),Ok(Some(code)));assert_eq!(turn.accepted(),Ok(true));assert!(turn.queue_error.is_none());
        assert_eq!(turn.exited(),Ok(Some(code)),"repeated observation cannot replay the submitted input");
        println!("native App exact input and terminal {code} recovered without submission: {}",turn.thread);
        }
    }

    // Chain: owned native input -> production IPC cancellation -> matching native abort.
    #[cfg(any(target_os="macos",windows))]
    #[test]
    #[ignore = "dedicated active native probe; TASKQ_APP_READBACK selects its evidence"]
    fn native_app_cancels_exact_owned_turn() {
        let evidence=PathBuf::from(std::env::var("TASKQ_APP_READBACK").expect("TASKQ_APP_READBACK"));
        let read=|file:&str|serde_json::from_str::<serde_json::Value>(&std::fs::read_to_string(evidence.join(file)).unwrap()).unwrap();
        let manifest=read("manifest.json");let queued=read("source-cancel-queue.json");
        let root=PathBuf::from(manifest["root"].as_str().unwrap()).canonicalize().unwrap();
        let thread=manifest["thread"].as_str().unwrap().to_owned();let text=queued["input"].as_str().unwrap().to_owned();
        let mut turn=AppTurn {root,history:AppHistory::default(),hand:None,log:evidence.join("handshake.jsonl"),text,_input:None,binding:None,
            rollout:rollout(&thread),thread,done:0,running:app_running,control:app_cancel,cancelled:false,queue_error:Some("fixture: submission response lost".into()),accepted:false};
        let until=std::time::Instant::now()+std::time::Duration::from_secs(30);
        while !turn.accepted {
            let observed=turn.exited();assert!(!matches!(observed,Ok(Some(_))),"native probe must still be active before cancellation");
            assert!(std::time::Instant::now()<until,"input acceptance unavailable: {observed:?}");std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let owned=turn.history.owned.clone().unwrap();turn.kill().expect("exact owner/turn interrupt request");
        std::fs::write(evidence.join("source-cancel-receipt.json"),serde_json::json!({"thread":turn.thread,"turn":owned,"request_acknowledged":true}).to_string()).unwrap();
        loop {
            let observed=turn.exited();
            if let Ok(Some(code))=observed {assert_eq!(code,-1);break;}
            assert!(std::time::Instant::now()<until,"interrupt request is not terminal: {observed:?}");std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert_eq!(turn.history.owned,Some(owned.clone()));assert_eq!(turn.accepted(),Ok(true));
        println!("production App IPC cancelled exact native turn {owned}; native abort observed; no resubmission");
    }

    // Chain: long native App history -> bounded cold scan -> exact completion -> appended observations.
    // Risk: a total-size ceiling stalls long sessions, or a partial scan/rewritten history releases work early.
    #[test]
    fn app_history_progresses_in_bounded_passes_without_early_completion() {
        let root=std::env::temp_dir().join(format!("taskq-app-history-{}",uuid()));std::fs::create_dir_all(&root).unwrap();
        let root=root.canonicalize().unwrap();let path=root.join("rollout.jsonl");
        let event=|kind:&str,message:&str|serde_json::json!({"type":"event_msg","payload":{"type":kind,"turn_id":"owned","message":message}}).to_string()+"\n";
        let header=serde_json::json!({"type":"session_meta","payload":{"id":"thread","cwd":root}}).to_string()+"\n";
        let prefix=header+&event("task_started","")+&event("user_message","exact input")+&event("task_complete","");
        for duplicate in [false,true] {
            let mut file=File::create(&path).unwrap();file.write_all(prefix.as_bytes()).unwrap();
            let padding=serde_json::json!({"type":"response_item","payload":"x".repeat(1024*1024)}).to_string()+"\n";
            for _ in 0..18 {file.write_all(padding.as_bytes()).unwrap();}
            if duplicate {file.write_all(event("task_started","").as_bytes()).unwrap();file.write_all(event("user_message","exact input").as_bytes()).unwrap();}
            file.sync_all().unwrap();drop(file);
            let baseline_began=std::time::Instant::now();
            assert_eq!(completes(&path).unwrap(),1);
            let baseline_time=baseline_began.elapsed();
            let mut history=AppHistory::default();let mut passes=0;
            loop {
                passes+=1;let before=history.reader.offset;
                let result=history.read(&path,0,"exact input",&root,"thread");
                assert!(history.reader.offset-before<=8*1024*1024);
                match result {
                    Err(e) if e.contains("scan pending")=>{assert!(history.reader.offset>before);assert!(passes<5);},
                    Err(e)=>{assert!(duplicate && e.contains("multiple native messages"),"{e}");break;},
                    Ok(outcome)=>{assert!(!duplicate);assert_eq!(outcome,(true,true));assert!(passes>=3);break;},
                }
            }
            if duplicate {continue;}
            let end=history.reader.offset;
            assert_eq!(history.read(&path,0,"exact input",&root,"thread"),Ok((true,true)));
            assert_eq!(history.reader.offset,end);
            let idle=std::time::Instant::now();
            for _ in 0..100 {assert_eq!(history.read(&path,0,"exact input",&root,"thread"),Ok((true,true)));}
            eprintln!("App native history: {} bytes, {passes} bounded passes; baseline {:?}; 100 unchanged reads {:?}",end,baseline_time,idle.elapsed());
            let extra=event("agent_message","later native diagnostic");
            let split=extra.len()/2;
            File::options().append(true).open(&path).unwrap().write_all(&extra.as_bytes()[..split]).unwrap();
            assert!(history.read(&path,0,"exact input",&root,"thread").unwrap_err().contains("incomplete"));
            assert_eq!(history.reader.offset,end,"a partial record is not consumed");
            File::options().append(true).open(&path).unwrap().write_all(&extra.as_bytes()[split..]).unwrap();
            assert_eq!(history.read(&path,0,"exact input",&root,"thread"),Ok((true,true)));
            assert_eq!(history.reader.offset,end+extra.len() as u64);
            // A replacement with the same bytes must not inherit the old cursor.
            let replacement=root.join("replacement.jsonl");std::fs::copy(&path,&replacement).unwrap();
            std::fs::remove_file(&path).unwrap();std::fs::rename(replacement,&path).unwrap();
            assert!(history.read(&path,0,"exact input",&root,"thread").is_err());
        }
        std::fs::write(&path,&prefix).unwrap();let mut history=AppHistory::default();
        assert_eq!(history.read(&path,0,"exact input",&root,"thread"),Ok((true,true)));
        std::fs::write(&path,"{}\n").unwrap();
        assert!(history.read(&path,0,"exact input",&root,"thread").unwrap_err().contains("truncated"));
        std::fs::write(&path,&prefix).unwrap();let mut history=AppHistory::default();history.read(&path,0,"exact input",&root,"thread").unwrap();
        let changed=prefix.replace("task_complete","task_conflict")+&event("agent_message","growth cannot hide a rewritten boundary");
        std::fs::write(&path,changed).unwrap();
        assert!(history.read(&path,0,"exact input",&root,"thread").unwrap_err().contains("boundary changed"));
        std::fs::write(&path,"x".repeat(8*1024*1024+1)).unwrap();
        assert!(AppHistory::default().read(&path,0,"exact input",&root,"thread").unwrap_err().contains("exceeds 8 MiB"));
        std::fs::remove_dir_all(root).unwrap();
    }

    // Chain: board intent -> native input -> cold observer. Risk: restart substitutes input, replays work or drops its lock.
    #[test]
    fn cold_input_recovery_preserves_the_exact_execution() {
        let root=std::env::temp_dir().join(format!("taskq-cold-input-{}",uuid()));
        std::fs::create_dir_all(&root).unwrap();
        for rt in ["claude","codex-app"] {
            let l=Launch {execution:Some(44),task:7,title:String::new(),attempt:42,role:"worker".into(),rt:rt.into(),session:uuid(),url:String::new(),resume:Some("continue".into())};
            let x=crate::core::Execution {id:44,attempt:42,through:43,host:"mac".into(),runtime:rt.into(),accepted:false,release:None};
            let log=log_path(&root,&l);
            let (path,lock)=turn_input(&root,"original task",Some(&l)).unwrap();
            assert_eq!(path,root.canonicalize().unwrap().join(".taskq/inputs/execution-7-42-44.txt"));
            assert!(turn_input(&root,"replacement task",Some(&l)).is_err());
            assert_eq!(std::fs::read_to_string(&path).unwrap(),"original task");
            let input=if rt=="claude" {claude_reference(&path)} else {app_reference(&path)}.unwrap();
            let binding=bind_input(&log,&l,&path,&input,serde_json::json!({"thread":"native-thread","session":l.session})).unwrap();
            drop(lock);
            let mut large_log=File::options().append(true).open(&log).unwrap();
            let row=serde_json::json!({"type":"fixture","padding":"x".repeat(1024*1024)}).to_string()+"\n";
            for _ in 0..10 {large_log.write_all(row.as_bytes()).unwrap();}
            drop(large_log);
            let mut observer=recover(&root,&root.join("install"),7,&x).unwrap();
            assert_eq!(observer.id(),0);
            assert!(!matches!(observer.accepted(),Ok(true)),"binding is not native acceptance");
            let file=File::options().read(true).write(true).open(&path).unwrap();
            assert!(matches!(file.try_lock(),Err(std::fs::TryLockError::WouldBlock)));
            drop(observer);
            file.try_lock().unwrap();file.unlock().unwrap();
            for case in ["foreign-task","foreign-execution","foreign-runtime","input","path","legacy","missing","partial","oversized","ambiguous","released"] {
                let mut b=binding.clone();
                let mut execution=x.clone();
                let reviewer=attempt_log(&root,"reviewer",7,42);
                match case {
                    "foreign-task"=>b["task"]=8.into(),
                    "foreign-execution"=>b["execution"]=45.into(),
                    "foreign-runtime"=>b["runtime"]="other".into(),
                    "input"=>b["input"]="unrelated input".into(),
                    "path"=>b["path"]="/unrelated/input.txt".into(),
                    "legacy"=>{std::fs::rename(&path,root.join("legacy.txt")).unwrap();},
                    "ambiguous"=>std::fs::write(&reviewer,"{}").unwrap(),
                    "released"=>execution.release=Some(crate::core::Release::Exit),
                    _=>{},
                }
                std::fs::write(&log,match case {"missing"=>String::new(),"partial"=>"{incomplete".into(),"oversized"=>"x".repeat(8*1024*1024+1),_=>b.to_string()+"\n"}).unwrap();
                assert!(recover(&root,&root.join("install"),7,&execution).is_err(),"{rt}: {case}");
                if case=="legacy" {std::fs::rename(root.join("legacy.txt"),&path).unwrap();}
                if case=="ambiguous" {std::fs::remove_file(reviewer).unwrap();}
            }
            std::fs::remove_file(path).unwrap();std::fs::remove_file(log).unwrap();
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    // Chain: exact execution input binding -> uncertain native launch -> retained observation and input lock.
    // Risk: a timeout or unreadable launch response drops the only recovery token and invites resubmission.
    #[test]
    fn uncertain_background_launch_retains_its_bound_input() {
        #[cfg(unix)] use std::os::unix::process::ExitStatusExt;
        #[cfg(windows)] use std::os::windows::process::ExitStatusExt;
        let root=std::env::temp_dir().join(format!("taskq-uncertain-input-{}",uuid()));
        std::fs::create_dir_all(&root).unwrap();
        let candidate="c37eb75a";
        for case in ["timeout","response","exit","log","copy","no-start"] {
            let log=root.join(format!("{case}.log"));
            let mut l=Launch {execution:Some(44),task:7,title:String::new(),attempt:42,role:"worker".into(),rt:"claude".into(),session:uuid(),url:String::new(),resume:Some("continue".into())};
            let (input,lock,path)=claude_input(&root,"private task body",None).unwrap();
            for execution in [None,Some(41)] {
                l.execution=execution;
                assert!(bind_input(&log,&l,&path,&input,serde_json::json!({})).is_err());
                assert!(!log.exists(),"invalid intent must refuse before native submission");
            }
            l.execution=Some(44);
            assert!(bind_input(&root,&l,&path,&input,serde_json::json!({})).is_err(),"unwritable binding must refuse before native submission");
            bind_input(&log,&l,&path,&input,serde_json::json!({"before":10,"session":l.session})).unwrap();
            let written=std::fs::read_to_string(&log).unwrap();
            let record:serde_json::Value=serde_json::from_str(written.trim()).unwrap();
            assert_eq!((record["task"].as_u64(),record["attempt"].as_u64(),record["execution"].as_u64()),(Some(7),Some(42),Some(44)));
            assert_eq!(record["input"],input);assert!(!written.contains("private task body"));
            let mut turn=ClaudeTurn {root:root.clone(),log:log.clone(),session:l.session.clone(),id:if case=="copy" {uuid()} else {String::new()},
                announced:false,accepted:false,history:ClaudeHistory::default(),before:10,mode:"dontAsk".into(),input:input.clone(),_input_file:lock,pid:0,native:None,recovery:None,done:false,failure:None,launch_error:None};
            if case=="log" {std::fs::remove_file(&log).unwrap();std::fs::create_dir(&log).unwrap();}
            let output=match case {
                "timeout"=>Err(coreq::process::Failure::Timeout),
                "no-start"=>Err(coreq::process::Failure::Spawn(std::io::Error::new(std::io::ErrorKind::NotFound,"fixture missing launcher"))),
                "exit"=>Err(coreq::process::Failure::Exit(std::process::Output {status:std::process::ExitStatus::from_raw(256),stdout:vec![],stderr:b"native launch failed".to_vec()})),
                _=>Ok(std::process::Output {status:std::process::ExitStatus::from_raw(0),stdout:if case=="response" {b"lost response".to_vec()} else {format!("backgrounded · {candidate}\n").into_bytes()},stderr:vec![]}),
            };
            let result=turn.finish_launch(output);
            if case=="no-start" {assert!(result.unwrap_err().no_start);}
            else {
                assert!(result.is_ok(),"{case}: uncertain native side effect must retain its handle");
                assert!(turn.launch_error.is_some());
                assert_eq!(turn.input,input);
                assert!(turn.observe(&serde_json::json!({"id":turn.id,"sessionId":turn.session,"state":"failed","pid":null})).unwrap_err().contains("current input acceptance unconfirmed"),"old job status cannot settle the current submission");
                if matches!(case,"log"|"copy") {assert_eq!(turn.id,candidate,"returned native identity survives a subsequent failure");}
                let probe=File::options().read(true).write(true).open(&path).unwrap();
                assert!(matches!(probe.try_lock(),Err(std::fs::TryLockError::WouldBlock)),"uncertain turn keeps its original input");
            }
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    // Chain: lost launch reply -> exact native input lookup -> resumed observation without resubmission.
    // Risk: a missing identity stays orphaned, or unrelated/ambiguous history is adopted as this execution.
    #[test]
    fn claude_lost_reply_recovers_only_its_unique_input() {
        let root=std::env::temp_dir().join(format!("taskq-claude-lookup-{}",uuid()));
        std::fs::create_dir_all(&root).unwrap();
        let root=root.canonicalize().unwrap();
        let session=uuid();let other=uuid();
        std::fs::create_dir_all(root.join("projects/a")).unwrap();
        #[cfg(unix)] std::os::unix::fs::symlink(root.join("missing-native-directory"),root.join("projects/missing")).unwrap();
        for id in ["c37eb75a","eaff90e7"] {std::fs::create_dir_all(root.join("jobs").join(id)).unwrap();}
        for case in ["match","known","blocks","absent","two-jobs","two-inputs","partial","invalid","oversize","missing","foreign","resume","old","copied","large-unrelated","locator-only","bad-locator","foreign-locator","large-locator","long-history"] {
            let (input,lock,path)=claude_input(&root,"fixture task body",None).unwrap();
            let event=serde_json::json!({"type":"user","message":{"content":if case=="blocks" {serde_json::json!([{"type":"text","text":input}])} else {serde_json::json!(input)}}}).to_string()+"\n";
            let a=root.join("projects/a").join(format!("{session}.jsonl"));let b=root.join("projects/a").join(format!("{other}.jsonl"));
            let content=match case {
                "absent"=>serde_json::json!({"type":"user","message":{"content":"unrelated input"}}).to_string()+"\n",
                "two-inputs"=>event.repeat(2),
                "partial"=>event.clone()+"{",
                "invalid"=>event.clone()+"{}\n",
                "oversize"=>"x".repeat(8*1024*1024+1),
                "long-history"=>(serde_json::json!({"type":"system","padding":"x".repeat(1024*1024)}).to_string()+"\n").repeat(18)+&event
                    +&serde_json::json!({"type":"assistant","message":{"content":[{"type":"text","text":"owned final"}]}}).to_string()+"\n"
                    +&serde_json::json!({"type":"system","subtype":"turn_duration"}).to_string()+"\n"
                    +&serde_json::json!({"type":"user","message":{"content":"another turn"}}).to_string()+"\n"
                    +&serde_json::json!({"type":"assistant","message":{"content":[{"type":"text","text":"foreign final"}]}}).to_string()+"\n",
                "resume"=>serde_json::json!({"type":"system"}).to_string()+"\n"+&event,
                _=>event.clone(),
            };
            std::fs::write(&a,content).unwrap();std::fs::write(&b,&event).unwrap();
            for (id,sid) in [("c37eb75a",&session),("eaff90e7",&other)] {
                let intent=if case=="locator-only" || case=="resume" || (case=="large-unrelated" && id=="eaff90e7") {"unrelated original input"} else {&input};
                let locator=serde_json::json!({"cwd":root,"sessionId":sid,"intent":intent,"providerEnv":{"FIXTURE":"not retained or printed"}});
                std::fs::write(root.join("jobs").join(id).join("state.json"),locator.to_string()).unwrap();
            }
            let locator=root.join("jobs/c37eb75a/state.json");
            if case=="bad-locator" {std::fs::write(&locator,"{").unwrap();}
            if case=="foreign-locator" {std::fs::write(&locator,serde_json::json!({"cwd":root,"sessionId":other,"intent":input}).to_string()).unwrap();}
            if case=="large-locator" {std::fs::write(&locator,"x".repeat(1024*1024+1)).unwrap();}
            if case=="large-unrelated" {std::fs::write(&b,"x".repeat(8*1024*1024+1)).unwrap();}
            let mut turn=ClaudeTurn {root:root.clone(),log:root.join("attempt.log"),session:if matches!(case,"resume"|"old") {session.clone()} else if case=="copied" {other.clone()} else {String::new()},id:if matches!(case,"known"|"two-jobs"|"resume") {"c37eb75a".into()} else {String::new()},
                announced:false,accepted:false,history:ClaudeHistory::default(),before:usize::from(matches!(case,"resume"|"old")),mode:"dontAsk".into(),input:input.clone(),_input_file:lock,pid:0,native:None,recovery:None,done:false,failure:None,launch_error:Some("launch response lost".into())};
            let mut rows=vec![serde_json::json!({"id":"c37eb75a","kind":"background","sessionId":session,"cwd":if case=="foreign" {root.parent().unwrap()} else {&root},"state":"working"})];
            if matches!(case,"two-jobs"|"large-unrelated") {rows.push(serde_json::json!({"id":"eaff90e7","kind":"background","sessionId":other,"cwd":root,"state":"working"}));}
            rows.push(serde_json::json!({"id":"abcdef12","kind":"background","cwd":root.join("deleted-worktree"),"state":"done"}));
            if case=="missing" {std::fs::remove_file(&a).unwrap();}
            let original_id=turn.id.clone();
            let mut result=turn.recover_job(&rows,Some(&root));
            if case=="long-history" {
                let mut passes=1;
                while result.as_ref().is_err_and(|e|e.contains("scan pending")) {
                    assert!(!turn.accepted);assert_eq!(turn.id,original_id);assert!(passes<5);
                    result=turn.recover_job(&rows,Some(&root));passes+=1;
                }
                assert!(passes>=3);
                assert!(turn.history.ended);assert_eq!(turn.history.answer.as_deref(),Some("owned final"));
                assert!(!std::fs::read_to_string(&turn.log).unwrap().contains("foreign final"));
                assert_eq!(claude_history(&a,0,|_|{}).unwrap(),23,"pre-submission scan supports the same large history");
            }
            if matches!(case,"match"|"known"|"blocks"|"resume"|"large-unrelated"|"long-history") {
                result.unwrap();assert_eq!(turn.id,"c37eb75a");assert_eq!(turn.session,session);
                assert_eq!(turn.accepted(),Ok(true));assert!(!turn.done,"input acceptance is not native exit");
                turn.recover_job(&rows,None).unwrap();
            } else {
                assert!(result.is_err(),"{case} must remain unknown");assert_eq!(turn.id,original_id);assert!(!turn.accepted);
            }
            assert_eq!(turn.input,input);assert!(path.exists());
            let file=File::options().read(true).write(true).open(path).unwrap();
            assert!(matches!(file.try_lock(),Err(std::fs::TryLockError::WouldBlock)),"recovery retains the original input lock");
        }
        let projects=root.join("projects");
        std::fs::create_dir_all(projects.join("a")).unwrap();
        std::fs::create_dir_all(projects.join("b")).unwrap();
        std::fs::write(projects.join("unrelated-file"),"").unwrap();
        let native=projects.join("a").join(format!("{session}.jsonl"));
        std::fs::write(&native,"{}\n").unwrap();
        assert_eq!(claude_transcript_in(&root,&session).unwrap(),native);
        std::fs::write(projects.join("b").join(format!("{session}.jsonl")),"{}\n").unwrap();
        assert!(claude_transcript_in(&root,&session).unwrap_err().contains("multiple transcript paths"));
        assert!(claude_transcript_in(&root,"../other").is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    // Chain: an owned native probe loses its launch reply -> candidate recovers input and observes exit.
    // Reuses an already-stopped probe; never dispatches another model turn.
    #[test]
    #[ignore = "owned native probe; TASKQ_CLAUDE_READBACK selects its captured lifecycle evidence"]
    fn native_claude_lost_reply_readback() {
        let evidence=PathBuf::from(std::env::var("TASKQ_CLAUDE_READBACK").expect("TASKQ_CLAUDE_READBACK"));
        let read=|file:&str|serde_json::from_str::<serde_json::Value>(&std::fs::read_to_string(evidence.join(file)).unwrap()).unwrap();
        let manifest=read("manifest.json");let result=read("stop-result.json");let identity=read("identity-before.json");
        let pid=u32::try_from(identity["pid"].as_u64().unwrap()).unwrap();
        assert_eq!(coreq::process::identity(pid).unwrap(),None,"probe must already have exited");
        assert_eq!(result["stop"]["exit_code"],0);
        let root=PathBuf::from(manifest["native_root"].as_str().unwrap()).canonicalize().unwrap();
        let input_path=evidence.join("input.txt");let input=std::fs::read_to_string(&input_path).unwrap();
        let log=evidence.join("candidate-readback.log");File::create(&log).unwrap();
        let mut turn=ClaudeTurn {root,log,session:String::new(),id:String::new(),announced:false,accepted:false,history:ClaudeHistory::default(),before:0,
            mode:"inherited-probe".into(),input,_input_file:crate::data::InputLock::new(File::open(input_path).unwrap()).unwrap(),pid,native:Some(coreq::process::Identity {pid,started:identity["started"].as_str().unwrap().into()}),recovery:None,done:false,failure:None,launch_error:Some("fixture: launch reply lost".into())};
        assert_eq!(turn.exited(),Ok(Some(0)));
        assert_eq!(turn.accepted(),Ok(true));assert_eq!(turn.id,result["job"].as_str().unwrap());
        assert_eq!(turn.session,result["session"].as_str().unwrap());
        assert_eq!(coreq::process::identity(pid).unwrap(),None);
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
        let mut l = Launch { execution: None, task: 7, title: "T".into(), attempt: 42, role: "worker".into(), rt: "claude".into(), session: "s".into(), url: "https://b/7".into(), resume: None };
        crate::policy::grant(&install, "claude", "full", None, "op", 1).unwrap();
        crate::policy::grant(&install, "codex", "full", None, "op", 1).unwrap();
        for resume in [None, Some("answer".to_string())] {
            l.resume = resume;
            l.rt = "claude".into();
            assert!(args(&command(&env, &l, &access(&env, "claude").unwrap()).unwrap()).contains("--permission-mode bypassPermissions"));
            l.rt = "codex".into();
            assert!(args(&command(&env, &l, &access(&env, "codex").unwrap()).unwrap()).starts_with("exec --json -s danger-full-access -m gpt-5 -c model_reasoning_effort=high "));
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

    // Chain: native identity -> failed/retried protected checkpoint -> cold observer -> exact process exit.
    // Risk: cold Claude recovery recaptures a reused PID, or failed persistence drops the only original identity.
    #[test]
    fn claude_cold_identity_survives_checkpoint_failure_and_process_exit() {
        let root=std::env::temp_dir().join(format!("taskq-claude-cold-{}",uuid()));
        let install=root.join("install");std::fs::create_dir_all(&install).unwrap();
        let l=Launch {execution:Some(42),task:7,title:String::new(),attempt:42,role:"worker".into(),rt:"claude".into(),session:uuid(),url:String::new(),resume:None};
        let log=log_path(&root,&l);
        let (input,lock,path)=claude_input(&root,"controlled process fixture",Some(&l)).unwrap();
        bind_input(&log,&l,&path,&input,serde_json::json!({"session":l.session})).unwrap();
        let mut child=Command::new(std::env::current_exe().unwrap()).args(["--exact","runtime::tests::stop_child","--ignored"])
            .stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
        let mut t=ClaudeTurn {root:root.clone(),log,session:l.session.clone(),id:"c37eb75a".into(),announced:false,accepted:true,history:ClaudeHistory::default(),before:0,mode:"fixture".into(),input,
            _input_file:lock,pid:0,native:None,recovery:Some(Recovery {root:root.clone(),install:install.clone(),task:7,attempt:42,execution:42}),done:false,failure:None,launch_error:None};
        let row=serde_json::json!({"id":t.id,"sessionId":t.session,"pid":child.id(),"state":"working"});
        t.capture_identity(&row).unwrap();let original=t.native_identity().unwrap();
        std::fs::write(install.join("native"),"fixture: unavailable recovery directory").unwrap();
        assert!(t.persist().unwrap_err().contains("checkpoint failed"));
        assert_eq!(t.native_identity(),Ok(original.clone()));assert!(t.recovery.is_some());
        std::fs::remove_file(install.join("native")).unwrap();t.persist().unwrap();
        assert!(t.recovery.is_none());
        let saved=crate::kids_file(&install,&root).unwrap();assert_eq!(saved.len(),1);
        assert_eq!(saved[0].process,original);assert_eq!(saved[0].runtime,"claude");assert_eq!(saved[0].offset,0);
        let mut conflicting=row.clone();conflicting["pid"]=std::process::id().into();
        assert!(t.capture_identity(&conflicting).is_err());assert_eq!(t.native_identity(),Ok(original.clone()));
        let x=crate::core::Execution {id:42,attempt:42,through:42,host:"mac".into(),runtime:"claude".into(),accepted:true,release:None};
        drop(t);
        let restored=recover(&root,&install,7,&x).unwrap();
        assert_eq!(restored.native_identity(),Ok(original.clone()));
        assert_eq!(claude_stopped(&serde_json::json!({"state":"done"}),Some(&original)),Ok(false));
        child.kill().unwrap();child.wait().unwrap();
        assert_eq!(claude_stopped(&serde_json::json!({"state":"done"}),Some(&original)),Ok(true));
        drop(restored);
        let after_exit=recover(&root,&install,7,&x).unwrap();
        assert_eq!(after_exit.native_identity(),Ok(original),"cold recovery does not recapture an absent/reused PID");
        let mut other=x.clone();other.id=43;
        assert!(recover(&root,&install,7,&other).is_err());
        let protected=crate::kids_path(&install,&root).unwrap();std::fs::write(&protected,"corrupt").unwrap();
        assert!(recover(&root,&install,7,&x).is_err());assert_eq!(std::fs::read_to_string(protected).unwrap(),"corrupt");
        drop(after_exit);std::fs::remove_dir_all(root).unwrap();
    }

    // Chain: native job schema -> observation -> release fence.
    // Risk: missing optional fields or a new state fabricate exit or hide observation failures.
    #[test]
    fn claude_native_state_requires_exit_evidence() {
        let root=std::env::temp_dir().join(format!("taskq-claude-state-{}",uuid()));
        std::fs::create_dir_all(&root).unwrap();
        let log=root.join("attempt.log");
        File::create(&log).unwrap();
        let mut t = ClaudeTurn { root:root.clone(), log, session:uuid(),
            id: "c37eb75a".into(), announced: false, accepted:false, history:ClaudeHistory::default(), before: 0, mode: "dontAsk".into(),
            input: "invented input".into(), _input_file: crate::data::InputLock::new(File::open(std::env::current_exe().unwrap()).unwrap()).unwrap(), pid: 0, native:None, recovery:None, done: false, failure: None, launch_error:None };
        assert!(native_job_id(&t.id));
        assert!(native_job_id(&uuid()));
        for id in ["--all","../c37eb75a","c37eb75a --all","invented-job",""] {assert!(!native_job_id(id));}
        for state in ["failed", "stopped", "crashed", "unexpected"] {
            assert!(t.observe(&serde_json::json!({"state":state})).is_err(),"{state} without process proof must not release");
        }
        for state in ["done","blocked","failed","crashed","unexpected"] {
            assert_ne!(claude_stopped(&serde_json::json!({"state":state}),None),Ok(true),"{state} is not a process exit receipt");
        }
        assert!(claude_stopped(&serde_json::json!({"state":"stopped"}),None).is_err());
        assert_eq!(claude_stopped(&serde_json::json!({"state":"stopped"}),coreq::process::identity(std::process::id()).unwrap().as_ref()),Ok(false));
        assert_eq!(claude_stopped(&serde_json::json!({"state":"done"}),coreq::process::identity(std::process::id()).unwrap().as_ref()),Ok(false));
        let mut child=Command::new(std::env::current_exe().unwrap()).args(["--exact","runtime::tests::stop_child","--ignored"]).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().unwrap();
        let identity=coreq::process::identity(child.id()).unwrap().unwrap();child.kill().unwrap();child.wait().unwrap();
        assert_eq!(claude_stopped(&serde_json::json!({"state":"done"}),Some(&identity)),Ok(true),"native stop can preserve done after observed process exit");
        for pid in [serde_json::json!(0), serde_json::json!(u64::MAX), serde_json::json!("unknown")] {
            assert!(claude_stopped(&serde_json::json!({"state":"stopped","pid":pid}),None).is_err());
        }
        for row in [serde_json::json!({}),serde_json::json!({"state":null})] {assert!(t.observe(&row).is_err());}
        let row=serde_json::json!({"id":t.id,"state":"working","sessionId":t.session});
        assert!(t.observe(&row).unwrap_err().contains("transcript unavailable"),"no status field must not suppress the real observation error");
        assert!(t.announced);
        std::fs::remove_dir_all(root).unwrap();
    }

    // Chain: § 8 codex-app handshake -> open thread -> queue -> one new `task_complete` on this host's app.
    // Paid/client-dependent qualification: deliberately excluded from the normal CI gate.
    #[test]
    #[ignore = "live runtime matrix; set TASKQ_APP_ROOT=<git dir> and run explicitly"]
    fn live_app_turn_completes_once() {
        let root = PathBuf::from(std::env::var("TASKQ_APP_ROOT").expect("TASKQ_APP_ROOT"));
        let install = std::env::temp_dir().join(format!("taskq-app-{}", std::process::id()));
        std::fs::create_dir_all(&install).unwrap();
        crate::policy::grant(&install, "codex-app", "full", None, "op", 1).unwrap();
        let env = Env { root: &root, exe: Path::new("taskq"), install: &install, permission_mode: "dontAsk", codex: &[], publish: "pr", board_env: &[] };
        let l = Launch { execution: None, task: 0, title: "live".into(), attempt: 0, role: "worker".into(), rt: "codex-app".into(), session: "-".into(), url: String::new(), resume: None };
        let log = root.join("app-turn.log");
        let (text, input, _path) = app_input(&root, &format!("{}Reply with the single word QUEUED and do nothing else.", "A taskq test line; ignore it.\n".repeat(700)),None).unwrap();
        let mut t = AppTurn { root: root.canonicalize().unwrap(), history:AppHistory::default(), hand: None, log: log.clone(), text, _input: Some(input), binding:None, thread: String::new(), done: 0, rollout: None, running: app_running, control:app_cancel, cancelled: false, queue_error: None, accepted: false };
        t.hand = Some(start(&env, &l, &log, HANDSHAKE, &access(&env, "codex-app").unwrap()).unwrap());
        let code = (0..600).find_map(|_| { std::thread::sleep(std::time::Duration::from_secs(1)); t.exited().unwrap() });
        eprintln!("thread {} rollout {:?} completes {} exit {code:?}", t.thread, t.rollout, t.rollout.as_ref().map_or(0, |p| completes(p).unwrap()));
        let _ = std::fs::remove_dir_all(&install);
        assert_eq!(code, Some(0));
        assert_eq!(completes(t.rollout.as_ref().unwrap()).unwrap(), t.done + 1, "exactly one queued turn completed");
    }

    // Chain: § 8 Windows app brief -> PowerShell -> taskq. Risk: quoted paths break parsing or execute input.
    #[cfg(windows)]
    #[test]
    fn app_brief_powershell_values_round_trip() {
        let root = Path::new("C:\\project's $cash; folder");
        let exe = Path::new("C:\\tool's $cash; folder\\taskq.exe");
        let env = Env { root, exe, install: Path::new("C:\\install"), permission_mode: "dontAsk", codex: &[], publish: "pr", board_env: &[] };
        let l = Launch { execution: None, task: 7, title: "T".into(), attempt: 42, role: "worker's $cash; role".into(), rt: "codex-app".into(), session: "s".into(), url: "https://b/7".into(), resume: None };
        let text = brief(&env, &l);
        let command = text.split("your inbox: `").nth(1).unwrap().split('`').next().unwrap();
        let prefix = command.split("; & ").next().unwrap();
        let executable = command.split("; & ").nth(1).unwrap().strip_suffix(" inbox 7").unwrap();
        let script = format!("{prefix}; ConvertTo-Json -Compress @($env:TASKQ_TASK, $env:TASKQ_ATTEMPT, $env:TASKQ_ROLE, $env:TASKQ_ROOT, {executable})");
        let out = Command::new("powershell.exe").args(["-NoProfile", "-NonInteractive", "-Command", &script]).output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let values: Vec<String> = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(values, vec!["7", "42", &l.role, &root.display().to_string(), &exe.display().to_string()]);
    }

    fn rules_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("taskq-rules-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        std::fs::create_dir_all(root.join("docs/sub")).unwrap();
        std::fs::write(root.join("docs/guide.md"), "Intro line\n## Guide title\nBody.").unwrap();
        std::fs::write(root.join("docs/sub/plain.md"), "No heading here.").unwrap();
        std::fs::write(root.join("docs/shot.png"), [0u8, 159, 146]).unwrap();
        root
    }

    // Chain: section 8 rule delivery. Risk: native runtime argv contains a full brief or its file is pruned mid-turn.
    #[test]
    fn claude_large_input_stays_out_of_argv() {
        let root = rules_root("background input ' quoted");
        let text = format!("{}\nUTF-8: \u{2605}\n", "invented padding ".repeat(6400));
        assert!(text.len() > 100 * 1024);
        let mut inputs = Vec::new();
        for (turn, app) in [("start",false),("resume",false),("concurrent operator",false),("app start",true),("app resume",true),("app concurrent",true)] {
            let brief = format!("{text}{turn}");
            let (input, _file, _path) = if app {app_input(&root,&brief,None)} else {claude_input(&root, &brief,None)}.unwrap();
            assert!(input.len() < 8192 && !input.contains("invented padding"));
            assert!(input.contains("in full"));
            if app { assert!(input.contains("do not proceed")); } else { assert!(input.contains("use Read")); }
            assert!(!inputs.contains(&input), "each turn has an immutable input file");
            let prefix=if app {"Before any work, read "} else {"Before any work, use Read to read "};
            let quoted = input.strip_prefix(prefix).unwrap().split(" in full").next().unwrap();
            let path: String = serde_json::from_str(quoted).unwrap();
            assert_eq!(std::fs::read_to_string(&path).unwrap(), brief);
            let probe = File::options().read(true).write(true).open(&path).unwrap();
            assert!(matches!(probe.try_lock(), Err(std::fs::TryLockError::WouldBlock)));
            drop(_file);
            probe.try_lock().unwrap();
            inputs.push(input);
        }
        assert_eq!(std::fs::read_dir(root.join(".taskq/inputs")).unwrap().count(), 6);
        std::fs::remove_dir_all(root).unwrap();
    }

    fn rule(url: &str) -> Result<String, String> {
        if url==SPECQ_REFS {return Ok(advertisement(SPECQ_REVISION));}
        if let Some(revision)=url.strip_prefix(SPECQ_TREES).and_then(|s|s.strip_suffix("?recursive=1")) {return Ok(tree_fixture(revision).to_string());}
        Ok(format!("# Rule from {url}\nApply this rule."))
    }

    fn pinned(root: &Path, revision: &str, file: &str, fetch: &mut impl FnMut(&str)->Result<String,String>) -> Result<String,StartError> {
        let body=rule(&format!("https://raw.githubusercontent.com/taskqorg/specq/{revision}/{file}")).unwrap();
        super::pinned(root,revision,file,&git_blob_hash(&body),fetch)
    }

    fn tree_fixture(revision: &str) -> serde_json::Value {
        let entries=SPECQ_CORE.iter().chain(SPECQ_INDEX).map(|file| {
            let body=format!("# Rule from https://raw.githubusercontent.com/taskqorg/specq/{revision}/{file}\nApply this rule.");
            serde_json::json!({"path":file,"type":"blob","mode":"100644","sha":git_blob_hash(&body)})
        }).collect::<Vec<_>>();
        serde_json::json!({"sha":revision,"truncated":false,"tree":entries})
    }

    #[test]
    fn newly_published_shared_file_is_indexed_and_cached_without_a_loader_change() {
        let root=rules_root("new-shared-file");let path="engineering/new-shared-rule.md";
        let url=format!("https://raw.githubusercontent.com/taskqorg/specq/{SPECQ_REVISION}/{path}");
        let body="# Newly published shared rule\nRead this rule on demand.\n";let mut fetched=Vec::new();
        let text=rules(&root,true,|request| {
            fetched.push(request.to_string());
            if request.starts_with(SPECQ_TREES) {
                let mut tree=tree_fixture(SPECQ_REVISION);
                for name in [path,"docs/excluded.md","vendor/excluded.md","README.md"] {
                    tree["tree"].as_array_mut().unwrap().push(serde_json::json!({"type":"blob","mode":"100644","path":name,"sha":git_blob_hash(body)}));
                }
                Ok(tree.to_string())
            } else if request==url {Ok(body.into())} else {rule(request)}
        }).unwrap();
        assert!(text.contains(&format!("- {path}: Newly published shared rule\n")));
        assert!(!text.contains("Read this rule on demand."),"noncore body remains demand-loaded");
        assert!(!fetched.iter().any(|url|url.contains("excluded.md")));
        assert_eq!(std::fs::read_to_string(specq_dir(&root,SPECQ_REVISION).join(path)).unwrap(),body);
        assert_eq!(rules(&root,true,|_|Err("must use fresh source/tree/body cache".into())).unwrap(),text);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn incomplete_unsafe_duplicate_or_nonregular_shared_trees_refuse_delivery() {
        let valid=tree_fixture(SPECQ_REVISION);assert_eq!(parse_shared_tree(&valid.to_string()).unwrap().len(),SPECQ_CORE.len()+SPECQ_INDEX.len());
        let mut broken=Vec::new();
        let mut tree=valid.clone();tree["truncated"]=true.into();broken.push(tree);
        let mut tree=valid.clone();tree.as_object_mut().unwrap().remove("truncated");broken.push(tree);
        let mut tree=valid.clone();tree["tree"].as_array_mut().unwrap().remove(0);broken.push(tree);
        let mut tree=valid.clone();let duplicate=tree["tree"][0].clone();tree["tree"].as_array_mut().unwrap().push(duplicate);broken.push(tree);
        let mut tree=valid.clone();tree["tree"][0]["mode"]="120000".into();broken.push(tree);
        let mut tree=valid.clone();tree["tree"][0]["sha"]="invalid".into();broken.push(tree);
        let mut tree=valid.clone();tree["tree"][SPECQ_CORE.len()]["type"]="unknown".into();broken.push(tree);
        let mut tree=valid.clone();tree["tree"]=serde_json::json!(vec![serde_json::json!({});1025]);broken.push(tree);
        for path in ["../escape.md","/absolute.md","C:/escape.md","engineering/../escape.md","engineering//empty.md","engineering\\escape.md","engineering/query?.md","engineering/NUL.md","engineering/COM1.md","engineering./trailing.md","ENGINEERING/PRINCIPLES.md"] {
            let mut tree=valid.clone();let mut entry=tree["tree"][0].clone();entry["path"]=path.into();tree["tree"].as_array_mut().unwrap().push(entry);broken.push(tree);
        }
        for tree in broken {assert!(parse_shared_tree(&tree.to_string()).is_err());}
    }

    #[test]
    fn source_validation_repairs_coherent_bad_caches_and_never_publishes_bad_fetches() {
        assert_eq!(git_blob_hash("abc"),"f2ba8f84ab5c1bce84a7b441cb1959cfc7093b7f");
        let root=rules_root("source-validation");let revision=SPECQ_REVISION;let file=SPECQ_CORE[0];
        let expected=rule(&url(file)).unwrap();let hash=git_blob_hash(&expected);
        let path=specq_dir(&root,revision).join(file);
        assert!(super::pinned(&root,revision,file,&hash,&mut |_|Ok("wrong source body".into())).is_err());assert!(!path.exists());
        assert_eq!(super::pinned(&root,revision,file,&hash,&mut rule).unwrap(),expected);
        let bad="# Corrupted coherent cache\n";std::fs::write(&path,bad).unwrap();
        std::fs::write(path.with_extension("cache.json"),rule_cache(revision,file,bad).to_string()).unwrap();
        assert!(super::pinned(&root,revision,file,&hash,&mut |_|Err("offline".into())).is_err());
        assert_eq!(super::pinned(&root,revision,file,&hash,&mut rule).unwrap(),expected);
        shared_files(&root,revision,&mut rule).unwrap();let tree=specq_dir(&root,revision).join("tree.json");
        std::fs::write(&tree,"{}").unwrap();std::fs::write(tree.with_extension("cache.json"),rule_cache(revision,"tree.json","{}").to_string()).unwrap();
        assert!(shared_files(&root,revision,&mut |_|Ok("{}".into())).is_err());assert_eq!(std::fs::read_to_string(&tree).unwrap(),"{}");
        assert_eq!(shared_files(&root,revision,&mut rule).unwrap().len(),SPECQ_CORE.len()+SPECQ_INDEX.len());
        std::fs::remove_dir_all(root).unwrap();
    }

    fn advertisement(revision: &str) -> String {
        let line=format!("{revision} refs/heads/main\n");
        format!("001e# service=git-upload-pack\n0000{:04x}{line}0000",line.len()+4)
    }

    #[test]
    fn git_advertisement_requires_complete_unambiguous_main_reference() {
        let valid=advertisement(SPECQ_REVISION);assert_eq!(advertised_revision(&valid).unwrap(),SPECQ_REVISION);
        for cut in 0..valid.len() {assert!(advertised_revision(&valid[..cut]).is_err(),"truncation {cut}");}
        for bad in [valid.replace("refs/heads/main","refs/heads/main-other"),valid.replace(SPECQ_REVISION,&"z".repeat(40)),
            valid.clone()+"0001",valid.clone()+&valid,valid.replace("003d","ffff"),"0000".into()] {
            assert!(advertised_revision(&bad).is_err(),"{bad:?}");
        }
        let head=format!("{SPECQ_REVISION} HEAD\0multi_ack thin-pack\n");
        let with_head=format!("001e# service=git-upload-pack\n0000{:04x}{head}{}",head.len()+4,&valid[34..]);
        assert_eq!(advertised_revision(&with_head).unwrap(),SPECQ_REVISION);
        let other=format!("{SPECQ_REVISION} refs/heads/main-other\n");
        assert!(advertised_revision(&format!("001e# service=git-upload-pack\n0000{:04x}{other}0000",other.len()+4)).is_err());
    }

    #[test]
    fn shared_revision_refreshes_at_bound_and_never_falls_back_after_failure() {
        let root=rules_root("source-freshness");let first="a".repeat(40);let second="b".repeat(40);
        assert_eq!(shared_revision_at(&root,100,&mut |url|{assert_eq!(url,SPECQ_REFS);Ok(advertisement(&first))}).unwrap(),first);
        assert_eq!(shared_revision_at(&root,159,&mut |_|Err("offline".into())).unwrap(),first);
        assert!(shared_revision_at(&root,160,&mut |_|Err("offline".into())).unwrap_err().to_string().contains("freshness"));
        assert_eq!(shared_revision_at(&root,160,&mut |_|Ok(advertisement(&second))).unwrap(),second);
        assert!(shared_revision_at(&root,159,&mut |_|Err("future cache".into())).is_err());
        assert!(shared_revision_at(&root,220,&mut |_|Ok("invalid".into())).is_err());
        assert!(shared_revision_at(&root,220,&mut |_|Ok("x".repeat(RULES_CAP+1))).is_err());
        let path=root.join(".taskq/specq/source.json");
        for bad in ["{".to_string(),"{}".into(),serde_json::json!({"revision":"../escape","checked_at":100}).to_string(),"x".repeat(1025)] {
            std::fs::write(&path,bad).unwrap();assert!(shared_revision_at(&root,101,&mut |_|Err("offline".into())).is_err());
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_moving_shared_head_cannot_mix_bodies_and_resume_gets_the_new_snapshot() {
        let root=rules_root("snapshot-consistency");let first="a".repeat(40);let second="b".repeat(40);let mut files=Vec::new();
        let initial=rules(&root,false,|url| {
            if url==SPECQ_REFS {return Ok(advertisement(&first));}
            files.push(url.to_string());
            let now=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
            std::fs::create_dir_all(root.join(".taskq/specq")).unwrap();
            std::fs::write(root.join(".taskq/specq/source.json"),serde_json::json!({"revision":second,"checked_at":now}).to_string()).unwrap();
            rule(url)
        }).unwrap();
        assert!(initial.starts_with(&format!("TaskQ rules: specq@{first} ")));
        assert_eq!(files.len(),1+SPECQ_CORE.len()+SPECQ_INDEX.len());
        assert!(files.iter().all(|url|url.contains(&format!("/{first}/")) || url==&format!("{SPECQ_TREES}{first}?recursive=1")));
        let resumed=rules(&root,true,rule).unwrap();
        assert!(resumed.starts_with(&format!("TaskQ rules: specq@{second} ")));
        assert!(!resumed.contains(&format!("/{first}/")));
        assert_eq!(resumed.matches("\n--- specq/").count(),SPECQ_CORE.len());
        std::fs::remove_dir_all(root).unwrap();
    }

    fn url(file: &str) -> String {
        format!("https://raw.githubusercontent.com/taskqorg/specq/{SPECQ_REVISION}/{file}")
    }

    // Chain: § 8 rule delivery, #1156. Risk: a new/resumed worker or reviewer misses the contract, a core rule or the index,
    // or a continuation retains obsolete mandatory bodies.
    #[test]
    fn rules_reach_every_runtime_role_and_resume() {
        let root = rules_root("roles");
        let env = Env { root: &root, exe: Path::new("/i/taskq"), install: Path::new("/i"), permission_mode: "dontAsk", codex: &[], publish: "pr", board_env: &[] };
        let dir = specq_dir(&root,SPECQ_REVISION).display().to_string();
        for rt in ["claude", "codex", "codex-app"] {
            for role in ["worker", "reviewer"] {
                for resume in [None, Some("Owner answer: preserve current work.\nLiteral $HOME stays text.".into())] {
                    let l = Launch { execution: None, task: 7, title: "T".into(), attempt: 42, role: role.into(), rt: rt.into(), session: "same-session".into(), url: "https://b/7".into(), resume };
                    let text = turn_text(&env, &l, rule).unwrap();
                    let (rules, task) = text.split_once("\nTaskQ task\n").unwrap();
                    // header: revision, cache path, contract path (this release's URL: the root has no taskq.md), precedence
                    let header = rules.split("\n\n").next().unwrap();
                    assert!(header.contains(SPECQ_REVISION) && header.contains(&dir) && header.contains("https://github.com/taskqorg/taskq/blob/"));
                    assert!(header.contains("Precedence: the project contract overrides shared rules where they conflict."));
                    assert!(header.contains("\nEvidence: cite the applied rule as R<n> or § n; a fix records the failure before and the pass after."));
                    // one index line per specq index file and per docs Markdown file, at start and at resume
                    for file in SPECQ_INDEX { assert_eq!(rules.matches(&format!("\n- {file}: Rule from {}\n", url(file))).count(), 1, "{file}"); }
                    assert!(rules.contains("\n- docs/guide.md: Guide title\n- docs/sub/plain.md: (no heading)\n") && !rules.contains("shot.png"));
                    let bodies = rules.matches("\n--- ").count();
                    assert_eq!(bodies, 1 + SPECQ_CORE.len());
                    assert!(rules.contains(&format!("\n{CONTRACT}\n")), "{rt} {role}: whole current contract");
                    for file in SPECQ_CORE { assert_eq!(rules.matches(&format!("\n--- specq/{file} ---\n{}\n", rule(&url(file)).unwrap())).count(), 1); }
                    if let Some(answer) = &l.resume { assert!(task.ends_with(answer)); }
                    else { assert_eq!(task, brief(&env, &l)); }
                    assert_eq!(l.session, "same-session");
                    // Native sessions do not inherit the child env; without the prefix `result` is refused as a non-worker.
                    let prefix = if cfg!(windows) && rt == "codex-app" { "$env:TASKQ_TASK='7'; $env:TASKQ_ATTEMPT='42'; " } else { "env TASKQ_TASK=7 TASKQ_ATTEMPT=42 " };
                    assert_eq!(brief(&env, &l).contains(prefix), rt != "codex", "{rt} {role}: attempt identity");
                    assert!(task.contains(FILES) && task.contains(CI_WAIT));
                    if role == "worker" { assert!(brief(&env, &l).contains(SANDBOX_CHECKS)); }
                    assert!(brief(&env, &l).contains("later 7 --text") && brief(&env, &l).contains("one unblock condition")
                        && brief(&env, &l).contains("unchanged external blocker; never requeue it"), "{rt} {role}: external blockers must not loop");
                    // the attempt log's evidence line
                    let log = root.join(format!(".taskq/{rt}-{role}.log"));
                    std::fs::remove_file(&log).ok();
                    rules_line(&log, "start", &text).unwrap();
                    let v: serde_json::Value = serde_json::from_str(std::fs::read_to_string(&log).unwrap().trim()).unwrap();
                    assert_eq!(v["shared_revision"],SPECQ_REVISION);
                    assert_eq!((v["type"].as_str(), v["header"].as_str(), v["bodies"].as_u64()), (Some("taskq.rules"), Some(header), Some(bodies as u64)));
                    assert_eq!(v["index"].as_u64(), Some(SPECQ_INDEX.len() as u64 + 2));
                }
            }
        }
        std::fs::remove_dir_all(&root).ok();
    }

    // Chain: section 7 step 4 test repair guidance (#1245). Risk: repairs rejected or weakened assertions accepted without a rule change.
    #[test]
    fn briefs_distinguish_test_repairs_from_weakened_checks() {
        let env = Env { root: Path::new("/project"), exe: Path::new("taskq"), install: Path::new("/i"), permission_mode: "dontAsk", codex: &[], publish: "pr", board_env: &[] };
        let rule = "Test changes follow specq engineering/testing.md: name the defect and retained risk protection in --text; change a rule only when accepted behavior or protection changes (section 7 step 4).";
        for rt in ["claude", "codex", "codex-app"] {
            for role in ["worker", "reviewer"] {
                let l = Launch { execution: None, task: 7, title: "Repair".into(), attempt: 42, role: role.into(), rt: rt.into(), session: "s".into(), url: "https://b/7".into(), resume: None };
                let text = brief(&env, &l);
                assert!(text.contains(rule), "{rt}: {role}");
                assert!(!text.contains("a changed or deleted existing test is a gap unless"), "{rt}: {role}: obsolete ban");
            }
        }
    }

    // Chain: § 7 research decision handoff (#1234). Risk: a worker bypasses owner choice or a reviewer accepts that bypass.
    #[test]
    fn research_decision_requires_ask_before_result() {
        let env = Env { root: Path::new("/project"), exe: Path::new("taskq"), install: Path::new("/i"), permission_mode: "dontAsk", codex: &[], publish: "pr", board_env: &[] };
        for rt in ["claude", "codex", "codex-app"] {
            let mut l = Launch { execution: None, task: 7, title: "Proposal".into(), attempt: 42, role: "worker".into(), rt: rt.into(), session: "s".into(), url: "https://b/7".into(), resume: None };
            assert!(brief(&env, &l).contains("Research or proposal tasks (type research, docs proposals) end with ask: options plus a recommendation; result only after the owner's answer."), "{rt}: worker");
            l.role = "reviewer".into();
            assert!(brief(&env, &l).contains("A research or proposal task without an ask and owner's answer before result/close is a task-fit gap"), "{rt}: reviewer");
        }
    }

    // Chain: § 8 rule delivery after compaction, #1211. Risk: a compacted Claude worker loses mandatory rule bodies
    // or task text again, or another runtime gets the hook.
    #[test]
    fn compaction_resends_header_and_index_only_to_claude() {
        let root = rules_root("compact");
        // fixture grant in a scoped install: hosts without enforcement refuse a missing grant (§ 8 Access)
        let install = root.join("install");
        std::fs::create_dir_all(&install).unwrap();
        for rt in ["claude", "codex"] { crate::policy::grant(&install, rt, "full", None, "op", 1).unwrap(); }
        let env = Env { root: &root, exe: Path::new("/i/taskq"), install: &install, permission_mode: "dontAsk", codex: &[], publish: "pr", board_env: &[] };
        let launch = |rt: &str| Launch { execution: None, task: 7, title: "T".into(), attempt: 42, role: "worker".into(), rt: rt.into(), session: "s".into(), url: "https://b/7".into(), resume: None };
        let args = |rt: &str| command(&env, &launch(rt), &access(&env, rt).unwrap()).unwrap().get_args().map(|a| a.to_string_lossy().into_owned()).collect::<Vec<_>>();
        let l = launch("claude");
        let claude = args("claude");
        let settings: serde_json::Value = serde_json::from_str(&claude[claude.iter().position(|a| a == "--settings").unwrap() + 1]).unwrap();
        let hook = &settings["hooks"]["SessionStart"][0];
        assert_eq!(hook["matcher"], "compact");
        assert!(hook["hooks"][0]["command"].as_str().unwrap().ends_with(&format!(" rules compact '{}' 'worker' 7 42", root.display())));
        assert!(!args("codex").iter().any(|a| a == "--settings"), "codex gets no hook");
        // The compact hook restores current mandatory bodies and index, without inventing a task.
        let start = turn_text(&env, &l, rule).unwrap();
        let text = compact(&root, "worker", 7, 42).unwrap();
        assert!(text.starts_with("TaskQ rules: specq@") && text.contains("\nEvidence: cite the applied rule"));
        assert!(text.contains(INDEX) && text.contains("\n- docs/guide.md: Guide title\n"));
        assert!(!text.contains("TaskQ task") && text.contains("Apply this rule."), "current mandatory bodies, no task");
        assert_eq!(text.matches("\n--- ").count(),1+SPECQ_CORE.len());
        assert_eq!(text, rules(&root, true, rule).unwrap());
        assert!(start.len() > text.len());
        let log = std::fs::read_to_string(attempt_log(&root, "worker", 7, 42)).unwrap();
        let v: serde_json::Value = serde_json::from_str(log.lines().last().unwrap()).unwrap();
        assert_eq!((v["phase"].as_str(), v["bodies"].as_u64(), v["bytes"].as_u64()), (Some("compact"), Some((1+SPECQ_CORE.len()) as u64), Some(text.len() as u64)));
        std::fs::remove_dir_all(&root).ok();
    }

    // Chain: § 8 rule delivery to a PM session, #1219. Risk: the PM hook gets other rules than a worker at the same pin,
    // compaction loses mandatory bodies, or the committed hook names a local path or misses start/compact.
    #[test]
    fn pm_hook_gets_the_worker_start_rules() {
        let root = rules_root("pm");
        let env = Env { root: &root, exe: Path::new("/i/taskq"), install: Path::new("/i"), permission_mode: "dontAsk", codex: &[], publish: "pr", board_env: &[] };
        let l = Launch { execution: None, task: 7, title: "T".into(), attempt: 42, role: "worker".into(), rt: "claude".into(), session: "s".into(), url: "https://b/7".into(), resume: None };
        let start = turn_text(&env, &l, rule).unwrap();
        assert_eq!(pm_rules(&root, "start", rule).unwrap(), start.split("\nTaskQ task\n").next().unwrap());
        assert_eq!(pm_rules(&root, "compact", rule).unwrap(), rules(&root, true, rule).unwrap());
        // the committed project hook: start on startup and clear, compact on compaction, no local path or secret
        let settings: serde_json::Value = serde_json::from_str(include_str!("../.claude/settings.json")).unwrap();
        let hooks = settings["hooks"]["SessionStart"].as_array().unwrap();
        for (matcher, phase) in [("startup|clear", "start"), ("compact", "compact")] {
            let cmd = hooks.iter().find(|h| h["matcher"] == matcher).unwrap()["hooks"][0]["command"].as_str().unwrap();
            assert!(cmd.ends_with(&format!(" rules pm {phase}")) && cmd.contains("$CLAUDE_PROJECT_DIR"), "{cmd}");
            assert!(!cmd.contains("/Users/") && !cmd.contains("token"), "{cmd}");
        }
        std::fs::remove_dir_all(&root).ok();
    }

    // Chain: § 8 rule delivery cache and refusals, #1156. Risk: a missing or unreadable pinned file, an unreadable doc or an
    // oversized payload starts a worker without its rules; or every start refetches the pin.
    #[test]
    fn registered_project_contract_overrides_taskq_and_missing_contract_refuses() {
        let root = rules_root("project-contract");
        std::fs::write(root.join("taskq.json"),r#"{"repo":"group/nested/serverq"}"#).unwrap();
        std::fs::write(root.join("taskq.md"),"# Wrong primary contract\n").unwrap();
        std::fs::write(root.join("serverq.md"),"# ServerQ contract\nProject rule overrides shared rule.\n").unwrap();
        let text = rule_text(&root,"task",false,rule).unwrap();
        assert!(text.contains(&format!("--- contract {} ---\n# ServerQ contract",root.join("serverq.md").display())));
        assert!(!text.contains("Wrong primary contract"));
        for resume in [false,true] {
            let text=rule_text(&root,"task",resume,rule).unwrap();
            assert!(text.contains(&format!("contract `{}`",root.join("serverq.md").display())));
        }
        std::fs::write(root.join("serverq.md"),"# Updated ServerQ contract\nNew current rule.\n").unwrap();
        let resumed=rule_text(&root,"task",true,rule).unwrap();
        assert!(resumed.contains("New current rule.") && !resumed.contains("Project rule overrides shared rule."));
        std::fs::remove_file(root.join("serverq.md")).unwrap();
        for resume in [false,true] {assert!(rule_text(&root,"task",resume,rule).is_err());}
        for config in ["{",r#"{"repo":"group/.."}"#,r#"{"repo":"group/"}"#,r#"{}"#] {
            std::fs::write(root.join("taskq.json"),config).unwrap();
            assert!(rule_text(&root,"task",false,rule).is_err());
        }
        std::fs::write(root.join("taskq.json"),r#"{"repo":"taskqorg/taskq"}"#).unwrap();
        assert!(rule_text(&root,"task",false,rule).unwrap().contains("Wrong primary contract"));
        std::fs::remove_file(root.join("taskq.md")).unwrap();
        assert!(rule_text(&root,"task",false,rule).unwrap().contains(CONTRACT));
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn pinned_cache_rejects_changed_utf8_wrong_identity_and_partial_pairs() {
        assert_eq!(rule_cache(SPECQ_REVISION,"fixture","abc")["sha256"],"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        let root=rules_root("cache-integrity");let file=SPECQ_CORE[0];let path=specq_dir(&root,SPECQ_REVISION).join(file);
        let expected=pinned(&root,SPECQ_REVISION,file,&mut rule).unwrap();
        std::fs::write(&path,"# Changed valid UTF-8 rule\n").unwrap();
        assert!(pinned(&root,SPECQ_REVISION,file,&mut |_|Err("offline".into())).is_err(),"altered nonempty cache must not be accepted");
        assert_eq!(pinned(&root,SPECQ_REVISION,file,&mut rule).unwrap(),expected);
        let metadata=path.with_extension("cache.json");
        for key in ["revision","file","sha256"] {
            let mut state=rule_cache(SPECQ_REVISION,file,&expected);state[key]=serde_json::json!("wrong");
            std::fs::write(&metadata,state.to_string()).unwrap();
            assert!(pinned(&root,SPECQ_REVISION,file,&mut |_|Err("offline".into())).is_err(),"{key}");
            assert_eq!(pinned(&root,SPECQ_REVISION,file,&mut rule).unwrap(),expected);
        }
        for bad in [None,Some("{"),Some("")] {
            if let Some(text)=bad {std::fs::write(&metadata,text).unwrap();} else {std::fs::remove_file(&metadata).unwrap();}
            assert!(pinned(&root,SPECQ_REVISION,file,&mut |_|Err("offline".into())).is_err());
            assert_eq!(pinned(&root,SPECQ_REVISION,file,&mut rule).unwrap(),expected);
        }
        std::fs::write(&path,"x".repeat(RULES_CAP+1)).unwrap();
        assert!(pinned(&root,SPECQ_REVISION,file,&mut |_|Err("offline".into())).is_err());
        assert!(pinned(&root,SPECQ_REVISION,file,&mut |_|Ok("x".repeat(RULES_CAP+1))).is_err());
        assert_eq!(pinned(&root,SPECQ_REVISION,file,&mut rule).unwrap(),expected);
        assert_eq!(pinned(&root,SPECQ_REVISION,file,&mut |_|Err("must use verified cache".into())).unwrap(),expected);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn pinned_cache_parallel_repairs_publish_complete_files_without_collisions() {
        let root=rules_root("cache-concurrent");let barrier=std::sync::Arc::new(std::sync::Barrier::new(8));let file=SPECQ_CORE[0];
        std::thread::scope(|scope| {
            let threads=(0..8).map(|_| {let root=&root;let barrier=barrier.clone();scope.spawn(move||pinned(root,SPECQ_REVISION,file,&mut |url|{barrier.wait();rule(url)}))}).collect::<Vec<_>>();
            for thread in threads {assert_eq!(thread.join().unwrap().unwrap(),rule(&url(file)).unwrap());}
        });
        assert_eq!(pinned(&root,SPECQ_REVISION,file,&mut |_|Err("must use verified cache".into())).unwrap(),rule(&url(file)).unwrap());
        assert_eq!(std::fs::read_dir(specq_dir(&root,SPECQ_REVISION).join("engineering")).unwrap().count(),2,"no staging files left");
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn rules_come_from_the_pin_cache_and_refuse_when_unreadable() {
        let root = rules_root("cache");
        std::fs::write(root.join("taskq.md"), "# Project contract\nProject rule.\n").unwrap();
        let mut fetched = Vec::new();
        let text = rule_text(&root, "task", false, |u| { fetched.push(u.to_string()); rule(u) }).unwrap();
        assert_eq!(fetched.len(), 2 + SPECQ_CORE.len() + SPECQ_INDEX.len(), "source reference, tree and each pinned file once");
        assert!(text.contains(&format!("--- contract {} ---\n# Project contract\nProject rule.\n", root.join("taskq.md").display())));
        for file in SPECQ_CORE.iter().chain(SPECQ_INDEX) {
            assert!(fetched.contains(&url(file)));
            assert_eq!(std::fs::read_to_string(specq_dir(&root,SPECQ_REVISION).join(file)).unwrap(), rule(&url(file)).unwrap());
        }
        for resume in [false, true] {
            assert!(rule_text(&root, "task", resume, |u| Err(format!("refetched {u}"))).is_ok(), "the cache serves the pin");
        }
        // a cached file that went bad is fetched again; a failed fetch refuses
        std::fs::write(specq_dir(&root,SPECQ_REVISION).join(SPECQ_CORE[3]), [0xffu8, 0xfe]).unwrap();
        std::fs::write(specq_dir(&root,SPECQ_REVISION).join(SPECQ_INDEX[0]), "").unwrap();
        for resume in [false, true] {
            for bad in [Err("offline".to_string()), Ok(" \n".to_string())] {
                let e = rule_text(&root, "task", resume, |_| bad.clone()).unwrap_err().to_string();
                assert!(e.contains(SPECQ_CORE[3]), "{e}");
            }
        }
        let mut again = Vec::new();
        rule_text(&root, "task", true, |u| { again.push(u.to_string()); rule(u) }).unwrap();
        assert_eq!(again, [url(SPECQ_CORE[3]), url(SPECQ_INDEX[0])]);
        // an unreadable doc, contract or an over-cap payload refuses
        std::fs::write(root.join("docs/sub/bad.md"), [0xffu8]).unwrap();
        assert!(rule_text(&root, "task", true, rule).unwrap_err().to_string().contains("docs/sub/bad.md"));
        std::fs::remove_file(root.join("docs/sub/bad.md")).unwrap();
        std::fs::write(root.join("taskq.md"), [0xffu8]).unwrap();
        assert!(rule_text(&root, "task", false, rule).is_err());
        std::fs::write(root.join("taskq.md"), "x".repeat(RULES_CAP)).unwrap();
        assert!(rule_text(&root, "task", false, rule).unwrap_err().to_string().contains("cap"));
        assert!(rule_text(&root, "task", true, rule).unwrap_err().to_string().contains("cap"), "resume also enforces the mandatory-body cap");
        std::fs::remove_dir_all(&root).ok();
    }

}
