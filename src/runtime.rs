//! Runtimes: stdin briefs or file-backed Claude background input (taskq.md section 8).

use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
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
    fn exit_error(&self) -> Option<&'static str> { None }
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
    if pid == 0 { return Err("invalid process id".into()); }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut std::ffi::c_void;
        fn GetExitCodeProcess(h: *mut std::ffi::c_void, code: *mut u32) -> i32;
        fn CloseHandle(h: *mut std::ffi::c_void) -> i32;
    }
    unsafe {
        let h = OpenProcess(0x1000, 0, pid); // PROCESS_QUERY_LIMITED_INFORMATION
        if h.is_null() {
            let e = std::io::Error::last_os_error();
            return if e.raw_os_error() == Some(87) { Ok(false) } else { Err(format!("process liveness unknown: {e}")) };
        }
        let mut code = 0;
        let read = GetExitCodeProcess(h, &mut code) != 0;
        let error = std::io::Error::last_os_error();
        CloseHandle(h);
        if read { Ok(code == 259) } else { Err(format!("process liveness unknown: {error}")) }
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
}

impl CliTurn {
    fn ended(&mut self) -> bool {
        use std::io::{Read, Seek, SeekFrom};
        let mut rest = vec![];
        let Ok(mut f) = File::open(&self.log) else { return false };
        if f.seek(SeekFrom::Start(self.pos)).and_then(|_| f.read_to_end(&mut rest)).is_err() { return false; }
        let Some(end) = rest.iter().rposition(|b| *b == b'\n') else { return false };
        self.pos += end as u64 + 1;
        String::from_utf8_lossy(&rest[..end]).lines().filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .any(|v| matches!(v["type"].as_str(), Some("turn.completed" | "turn.failed" | "result")))
    }
}

impl Proc for CliTurn {
    fn exited(&mut self) -> Result<Option<i32>, String> {
        if let Some(code) = self.proc.exited()? { return Ok(Some(code)); }
        if !self.ended() { return Ok(None); }
        log!("{}: turn ended; stopping its lingering process", self.log.display());
        self.proc.kill();
        Ok(Some(self.proc.exited()?.unwrap_or(-1)))
    }
    fn kill(&mut self) { self.proc.kill() }
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
    let mut c = executable(which(if l.rt == "claude" { "claude" } else { "codex" }), access, env)?;
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
    format!("{head}Changing an existing test or golden trace while preserving every assertion is a repair, named in --text with the defect it fixes; weakening or deleting an assertion needs a changed rule in the same diff and is named in --text (section 7 step 4).\n{body}Then stop. Everything written through taskq is public: no secrets or paths outside the repository.\n")
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
    StartError { message: format!("rules {what}: {e}"), reportable: false }
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
fn claude_input(root: &Path, text: &str) -> Result<(String, crate::data::InputLock), String> {
    let (path, file) = turn_input(root, text)?;
    Ok((format!("Before any work, use Read to read {} in full. It contains this turn's TaskQ rules and task. Apply those rules and carry out that task. If Read truncates the file, read the remaining parts before proceeding.", serde_json::to_string(&path.to_string_lossy()).map_err(|e| e.to_string())?), file))
}

fn app_input(root: &Path, text: &str) -> Result<(String, crate::data::InputLock), String> {
    let (path, file) = turn_input(root, text)?;
    Ok((format!("Before any work, read {} in full using your file tools. It contains this turn's TaskQ rules and task. Apply those rules and carry out that task. Read all remaining parts if a tool truncates the file. If the file cannot be read completely, report the failure and do not proceed with the task.", serde_json::to_string(&path.to_string_lossy()).map_err(|e| e.to_string())?), file))
}

fn turn_input(root: &Path, text: &str) -> Result<(PathBuf, crate::data::InputLock), String> {
    let dir = root.join(".taskq/inputs");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("{}.txt", uuid()));
    let mut file = File::options().read(true).write(true).create_new(true).open(&path).map_err(|e| e.to_string())?;
    file.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
    let file = crate::data::InputLock::new(file).map_err(|e| e.to_string())?;
    Ok((path, file))
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
        let (text, input_file) = app_input(env.root, &text).map_err(|message| StartError { message, reportable: false })?;
        let mut turn = AppTurn { hand: None, log, text, _input: Some(input_file), thread: l.session.clone(), done: 0, rollout: None, running: app_running };
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
    let pos = std::fs::metadata(&log).map_or(0, |m| m.len());
    let child = start(env, l, &log, &text, &access)?;
    Ok(Box::new(CliTurn { proc: Box::new(child), log, pos }))
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

fn start(env: &Env, l: &Launch, log: &Path, text: &str, access: &crate::policy::Effective) -> Result<Child, StartError> {
    std::fs::create_dir_all(log.parent().unwrap()).map_err(|e| StartError::io(e, None))?;
    let out = File::options().create(true).append(true).open(log).map_err(|e| StartError::io(e, None))?;
    let err = out.try_clone().map_err(|e| StartError::io(e, None))?;
    let mut c = command(env, l, access).map_err(|message| StartError { message, reportable: false })?;
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
    _input_file: crate::data::InputLock,
    pid: u32,
    done: bool,
    failure: Option<&'static str>,
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
    if session.is_empty() || !session.chars().all(|c| c.is_ascii_hexdigit() || c == '-') { return None; }
    let home = claude_home()?;
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
    let (ok, out) = probe("runtime.stop", &which("claude"), &["stop", id]).ok_or("Claude stop unavailable or timed out; state unknown")?;
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
        let mut c = executable(which("claude"), access, env)?;
        environment(env, l, &mut c);
        let (input, input_file) = claude_input(&root, text)?;
        c.current_dir(&root).arg("--bg");
        if l.resume.is_some() { c.args(["--resume", &l.session]); }
        else { c.args(["--settings", &compact_settings(&root, l), "--remote-control", &l.title, "--permission-mode", permission_mode]); }
        if l.role == "operator" { for k in ["TASKQ_TASK", "TASKQ_ATTEMPT", "TASKQ_ROLE"] { c.env_remove(k); } }
        let mut child = crate::platform::quiet(&mut c).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| e.to_string())?;
        if let Err(e) = child.stdin.take().unwrap().write_all(input.as_bytes()) {
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
            announced: false, copied: 0, before, mode: permission_mode.into(), input, _input_file: input_file, pid: 0, done: false, failure: None })
    }
}

impl Proc for ClaudeTurn {
    fn exited(&mut self) -> Result<Option<i32>, String> {
        if self.done { return Ok(Some(0)); }
        let rows = claude_agents(&self.root)?;
        let row = rows.iter().find(|v| v["id"] == self.id).ok_or("Claude session unavailable; state unknown")?;
        self.observe(row)
    }
    fn exit_error(&self) -> Option<&'static str> { self.failure }
    fn kill(&mut self) { if let Err(e) = claude_stop(&self.root, &self.id, self.pid) { log!("{e}"); } }
    fn id(&self) -> u32 { self.pid }
}

impl ClaudeTurn {
    fn observe(&mut self, row: &serde_json::Value) -> Result<Option<i32>, String> {
        // Failed background launches can have neither status nor a transcript (#1208).
        let state = row["state"].as_str().ok_or("Claude job state unavailable; state unknown")?;
        if !matches!(state, "working" | "idle" | "done" | "blocked") {
            self.failure = Some(claude_failure(row, &self.root, &self.id, claude_home().as_deref()));
            return Ok(Some(-1));
        }
        // On Windows a waking session briefly publishes a different UUID before restore.
        if !self.announced {
            if row["status"].as_str().is_none() { return Ok(None); }
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
            return Ok(None);
        }
        if !turn.iter().any(|v| v["type"] == "system" && v["subtype"] == "turn_duration") { return Ok(None); }
        let answer = turn.iter().rev().filter(|v| v["type"] == "assistant").find_map(|v|
            v["message"]["content"].as_array()?.iter().rev().find_map(|b| (b["type"] == "text").then(|| b["text"].as_str()).flatten())).ok_or("Claude completed without final text; inspect native logs")?;
        if self.pid != 0 { claude_stop(&self.root, &self.id, self.pid)?; }
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

/// A `codex-app` attempt (§ 3 app lease): the handshake child, then the turn queued into the Codex app, live until
/// the thread's rollout has a new `task_complete` or the app stops running.
pub struct AppTurn {
    hand: Option<Box<dyn Proc>>,
    log: PathBuf,
    /// The brief, or the resume text, to queue.
    text: String,
    _input: Option<crate::data::InputLock>,
    thread: String,
    /// `task_complete` events of the thread before the queued text.
    done: usize,
    rollout: Option<PathBuf>,
    running: fn() -> Result<bool, String>,
}

impl AppTurn {
    fn queue(&mut self) -> Result<(), String> {
        // the id goes through the Windows shell: only a plain thread id
        if self.thread.is_empty() || !self.thread.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return Err(format!("invalid thread id {:?}", self.thread));
        }
        self.rollout = rollout(&self.thread);
        self.done = completes(self.rollout.as_ref().ok_or("Codex rollout unavailable; turn baseline unknown")?)?;
        let ok = |c: &mut Command| crate::platform::quiet(c).status().is_ok_and(|s| s.success());
        let url = format!("codex://threads/{}", self.thread);
        // ponytail: `open` and `queue` block the pass for a moment; a thread per turn if they ever hang
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
        if !(self.running)()? {
            return Ok(Some(-1));
        }
        if self.rollout.is_none() {
            self.rollout = rollout(&self.thread);
        }
        let count = completes(self.rollout.as_ref().ok_or("Codex rollout unavailable; completion unknown")?)?;
        if count < self.done { return Err("Codex rollout truncated; completion unknown".into()); }
        Ok((count > self.done).then_some(0))
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

/// Finished turns in a rollout: its `task_complete` events.
fn completes(rollout: &Path) -> Result<usize, String> {
    coreq::telemetry::capture::operation("runtime.turn-readback", || {
    let f = File::open(rollout).map_err(|_| "Codex rollout unreadable; completion unknown")?;
    let mut count = 0;
    for line in BufReader::new(f).lines() {
        let line = line.map_err(|_| "Codex rollout read failed; completion unknown")?;
        let v: serde_json::Value = serde_json::from_str(&line).map_err(|_| "Codex rollout record incomplete or invalid; completion unknown")?;
        if v["type"].as_str().is_none() || v["type"] == "event_msg" && !v["payload"].is_object() {
            return Err("Codex rollout record shape invalid; completion unknown".into());
        }
        count += usize::from(v["type"] == "event_msg" && v["payload"]["type"] == "task_complete");
    }
    Ok(count)
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
    use super::*;

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

    // Chain: § 8 codex-app turn end. Risk: a turn ends early or never ends.
    #[test]
    fn app_turn_keeps_unreadable_completion_unknown() {
        let p = std::env::temp_dir().join(format!("taskq-rollout-unknown-{}.jsonl", std::process::id()));
        let mut t = AppTurn { hand: None, log: p.clone(), text: String::new(), _input: None, thread: "th".into(), done: 1, rollout: Some(p.clone()), running: || Ok(true) };
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
    fn app_turn_ends_on_a_new_task_complete_or_a_closed_app() {
        let p = std::env::temp_dir().join(format!("taskq-rollout-{}.jsonl", std::process::id()));
        let done = "{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_complete\"}}\n";
        std::fs::write(&p, format!("{done}{{\"type\":\"event_msg\",\"payload\":{{\"type\":\"user_message\"}}}}\n")).unwrap();
        let mut t = AppTurn { hand: None, log: p.clone(), text: String::new(), _input: None, thread: "th".into(), done: completes(&p).unwrap(), rollout: Some(p.clone()), running: || Ok(true) };
        assert_eq!((t.done, t.exited()), (1, Ok(None)), "the handshake's task_complete does not end the turn");
        std::fs::OpenOptions::new().append(true).open(&p).unwrap().write_all(done.as_bytes()).unwrap();
        assert_eq!(t.exited(), Ok(Some(0)));
        t.done = 2;
        t.running = || Ok(false);
        assert_eq!(t.exited(), Ok(Some(-1)), "a closed app is a child exit");
        t.thread = "x & calc".into();
        assert!(t.queue().is_err(), "a thread id that is not plain never reaches the shell");
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

    // Chain: service failed reviewer launch -> exit -> gone (the service reviewer test covers the pass).
    // Risk: missing status/transcript masks a terminal native job and renews its lease forever.
    #[test]
    fn claude_failed_launch_exits_without_status_or_transcript() {
        let mut t = ClaudeTurn { root: PathBuf::new(), log: PathBuf::new(), session: String::new(),
            id: "invented-job".into(), announced: false, copied: 0, before: 0, mode: "dontAsk".into(),
            input: "invented input".into(), _input_file: crate::data::InputLock::new(File::open(std::env::current_exe().unwrap()).unwrap()).unwrap(), pid: 0, done: false, failure: None };
        for state in ["failed", "stopped", "crashed", "unexpected"] {
            assert_eq!(t.observe(&serde_json::json!({"state":state})), Ok(Some(-1)));
            t.announced = true;
            assert_eq!(t.observe(&serde_json::json!({"state":state,"status":"working"})), Ok(Some(-1)));
            t.announced = false;
        }
        assert!(t.observe(&serde_json::json!({})).is_err());
        assert!(t.observe(&serde_json::json!({"state":null})).is_err());
        assert_eq!(t.observe(&serde_json::json!({"state":"working"})), Ok(None));
        assert_eq!(t.observe(&serde_json::json!({"state":"failed", "detail":"ENAMETOOLONG: name too long, uv_spawn"})), Ok(Some(-1)));
        assert_eq!(t.exit_error(), Some("argument-too-long"));
        assert_eq!(t.observe(&serde_json::json!({"state":"failed", "detail":"permission denied"})), Ok(Some(-1)));
        assert_eq!(t.exit_error(), Some("permission"));
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
        let l = Launch { task: 0, title: "live".into(), attempt: 0, role: "worker".into(), rt: "codex-app".into(), session: "-".into(), url: String::new(), resume: None };
        let log = root.join("app-turn.log");
        let (text, input) = app_input(&root, &format!("{}Reply with the single word QUEUED and do nothing else.", "A taskq test line; ignore it.\n".repeat(700))).unwrap();
        let mut t = AppTurn { hand: None, log: log.clone(), text, _input: Some(input), thread: String::new(), done: 0, rollout: None, running: app_running };
        t.hand = Some(Box::new(start(&env, &l, &log, HANDSHAKE, &access(&env, "codex-app").unwrap()).unwrap()));
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
        let l = Launch { task: 7, title: "T".into(), attempt: 42, role: "worker's $cash; role".into(), rt: "codex-app".into(), session: "s".into(), url: "https://b/7".into(), resume: None };
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

    // Chain: § 7 sandbox-blocked host checks. Risk: a worker requeues a sandbox limit or claims a local PASS.
    #[test]
    fn worker_brief_keeps_sandbox_blocked_checks_at_exact_sha_ci() {
        assert!(SANDBOX_CHECKS.contains("sandbox-blocked") && SANDBOX_CHECKS.contains("Checks"));
        assert!(SANDBOX_CHECKS.contains("never as PASS") && SANDBOX_CHECKS.contains("do not requeue"));
        assert!(SANDBOX_CHECKS.contains("green CI on the exact result SHA"));
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
            let (input, _file) = if app {app_input(&root,&brief)} else {claude_input(&root, &brief)}.unwrap();
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
                    let l = Launch { task: 7, title: "T".into(), attempt: 42, role: role.into(), rt: rt.into(), session: "same-session".into(), url: "https://b/7".into(), resume };
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
        let rule = "Changing an existing test or golden trace while preserving every assertion is a repair, named in --text with the defect it fixes; weakening or deleting an assertion needs a changed rule in the same diff and is named in --text (section 7 step 4).";
        for rt in ["claude", "codex", "codex-app"] {
            for role in ["worker", "reviewer"] {
                let l = Launch { task: 7, title: "Repair".into(), attempt: 42, role: role.into(), rt: rt.into(), session: "s".into(), url: "https://b/7".into(), resume: None };
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
            let mut l = Launch { task: 7, title: "Proposal".into(), attempt: 42, role: "worker".into(), rt: rt.into(), session: "s".into(), url: "https://b/7".into(), resume: None };
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
        let launch = |rt: &str| Launch { task: 7, title: "T".into(), attempt: 42, role: "worker".into(), rt: rt.into(), session: "s".into(), url: "https://b/7".into(), resume: None };
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
        let l = Launch { task: 7, title: "T".into(), attempt: 42, role: "worker".into(), rt: "claude".into(), session: "s".into(), url: "https://b/7".into(), resume: None };
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
