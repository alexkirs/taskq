//! AI requests (taskq.md R3 operator, § 4): the tray writes `<root>/.taskq/requests/<id>.json`, the service runs one
//! operator child per request and writes `<id>.answer.json`; a proposal is applied only after `<id>.confirm` names
//! its `rev`. One writer per file: the tray writes the request and the confirm, the service the answer. A request
//! with `onboard` (the tray's Add project) sets up that folder (§ 6): its only op is `setup`.

use crate::board::{BOARDS, Kind};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Requests kept per project (§ 4).
pub const KEEP: usize = 20;
const TIMEOUT: u64 = 600;
const KINDS: [&str; 4] = ["result", "clarify", "proposal", "refuse"];

pub fn dir(root: &Path) -> PathBuf {
    crate::dot(root).join("requests")
}

pub fn read(p: &Path) -> Value {
    fs::read_to_string(p).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null)
}

pub fn answer_path(root: &Path, id: &str) -> PathBuf {
    dir(root).join(format!("{id}.answer.json"))
}

/// Write the answer of `id` with `rev` one above the last one.
fn answer(root: &Path, id: &str, mut v: Value) {
    v["id"] = json!(id);
    v["rev"] = json!(read(&answer_path(root, id))["rev"].as_u64().unwrap_or(0) + 1);
    let _ = crate::platform::write_atomic(&answer_path(root, id), &v.to_string());
    crate::poke(root);
}

/// Request ids, oldest first: the tray names them by epoch milliseconds, so only digits.
pub fn ids(root: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir(root)).into_iter().flatten().flatten()
        .filter_map(|e| e.file_name().to_str()?.strip_suffix(".json").filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())).map(String::from))
        .collect();
    v.sort_by_key(|s| (s.len(), s.clone()));
    v
}

/// Registered projects (`<install>/projects`) by the name the tray shows.
pub fn projects(install: &Path) -> Vec<(String, PathBuf)> {
    fs::read_to_string(install.join("projects")).unwrap_or_default().lines().filter(|l| !l.is_empty()).map(|r| {
        let root = PathBuf::from(r);
        let name = read(&crate::dot(&root).join("status.json"))["project"].as_str().map(String::from).unwrap_or_else(|| crate::project_name(&root));
        (name, root)
    }).collect()
}

/// Owner comments (§ 4): one request per new comment id; `requests/comments` lists every id ever made a request, so a
/// confirmed, refused or pruned request is never made again. `ponytail:` the list only grows, a line per comment.
pub fn owner_comments(root: &Path, project: &str, issues: &str, found: &[(u64, u64, String, String)]) {
    let list = dir(root).join("comments");
    let done = fs::read_to_string(&list).unwrap_or_default();
    let mut id = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    for (n, c, text, card) in found {
        if done.lines().any(|l| l == c.to_string()) {
            continue;
        }
        // above every existing id: the newest request is the one the tray shows
        id = id.max(ids(root).last().and_then(|x| x.parse::<u128>().ok()).unwrap_or(0) + 1);
        let req = json!({"text": text, "task": n, "comment": c, "project": project, "card": card, "url": format!("{issues}/{n}")});
        let path = dir(root).join(format!("{id}.json"));
        if fs::create_dir_all(dir(root)).is_err() || crate::platform::write_atomic(&path, &req.to_string()).is_err() {
            continue;
        }
        let mut f = fs::OpenOptions::new().create(true).append(true).open(&list);
        if !f.as_mut().is_ok_and(|f| std::io::Write::write_all(f, format!("{c}\n").as_bytes()).is_ok()) {
            let _ = fs::remove_file(&path); // unrecorded: made again next pass rather than twice
        }
        id += 1;
    }
}

/// A request on an owner comment proposes one `answer`, `later` or `requeue` of its own task (§ 4).
fn scoped(v: Value, req: &Value) -> Value {
    if v["state"] != "proposal" || req["task"].is_null() {
        return v;
    }
    match v["ops"].as_array().map(Vec::as_slice) {
        Some([o]) if ["answer", "later", "requeue"].contains(&o["op"].as_str().unwrap_or("")) && o["task"] == req["task"] && o["project"] == req["project"] => v,
        _ => json!({"state": "failed", "reason": format!("refused proposal: only one answer, later or requeue of #{}", req["task"])}),
    }
}

/// The owner comment request a request answers: itself or the start of its clarify chain; else null.
fn comment_of(root: &Path, req: &Value) -> Value {
    let mut r = req.clone();
    for _ in 0..KEEP {
        if r["task"].is_u64() {
            return r;
        }
        let Some(a) = r["after"].as_str().filter(|a| a.bytes().all(|b| b.is_ascii_digit())).map(String::from) else { break };
        r = read(&dir(root).join(format!("{a}.json")));
    }
    Value::Null
}

/// After a restart no thread waits for a `running` operator: say so (§ 4).
pub fn recover(root: &Path) {
    for id in ids(root) {
        if read(&answer_path(root, &id))["state"] == "running" {
            answer(root, &id, json!({"state": "failed", "reason": "service restarted; send the request again"}));
        }
    }
}

/// One service pass: start an operator for each new request, apply a confirmed proposal, drop all but the newest 20.
pub fn step(root: &Path, install: &Path, exe: &Path, rt: Option<&str>) {
    let ids = ids(root);
    for id in ids.iter().rev().skip(KEEP) {
        for f in [format!("{id}.json"), format!("{id}.answer.json"), format!("{id}.confirm"), format!("{id}.out")] {
            let _ = fs::remove_file(dir(root).join(f));
        }
    }
    for id in ids.iter().rev().take(KEEP) {
        let a = read(&answer_path(root, id));
        if a.is_null() {
            start(root, install, id, rt);
            continue;
        }
        let confirm = dir(root).join(format!("{id}.confirm"));
        let Ok(rev) = fs::read_to_string(&confirm) else { continue };
        let _ = fs::remove_file(&confirm);
        // only the proposal the owner saw, once: a stale or repeated confirm does nothing (M-A7)
        // ponytail: files are not signed; whoever can write them can run the same allowlisted `taskq` commands anyway
        if a["state"] == "proposal" && rev.trim().parse::<u64>().ok() == a["rev"].as_u64() {
            answer(root, id, json!({"state": "applying", "text": a["text"], "ops": a["ops"], "effects": a["effects"]}));
            let ops = a["ops"].as_array().map_or(&[][..], |v| v);
            let (ok, lines) = match onboard(root, &read(&dir(root).join(format!("{id}.json")))) {
                Some(r) => apply_setup(install, exe, &r, ops, &a["effects"]),
                None => apply(install, exe, ops),
            };
            answer(root, id, json!({"state": if ok { "applied" } else { "apply failed" }, "text": a["text"], "ops": a["ops"], "effects": a["effects"], "readback": lines}));
        }
    }
}

fn start(root: &Path, install: &Path, id: &str, rt: Option<&str>) {
    let Some(rt) = rt.map(String::from) else {
        return answer(root, id, json!({"state": "failed", "reason": "no runtime with a nonzero limit on this host"}));
    };
    answer(root, id, json!({"state": "running", "rt": rt}));
    let (root, install, id) = (root.to_path_buf(), install.to_path_buf(), id.to_string());
    std::thread::spawn(move || {
        let v = match run(&root, &install, &id, &rt) {
            Ok(text) => {
                let req = read(&dir(&root).join(format!("{id}.json")));
                scoped(parse(&text, &projects(&install), onboard(&root, &req).as_deref()), &comment_of(&root, &req))
            }
            Err(e) => json!({"state": "failed", "reason": e}),
        };
        answer(&root, &id, v);
    });
}

/// The operator child: no tools (R9), brief on stdin, killed after 10 minutes.
fn run(root: &Path, install: &Path, id: &str, rt: &str) -> Result<String, String> {
    let req = read(&dir(root).join(format!("{id}.json")));
    let out = dir(root).join(format!("{id}.out"));
    let last = dir(root).join(format!("{id}.last"));
    let mut c = std::process::Command::new(crate::runtime::which(rt));
    crate::platform::quiet(&mut c);
    if rt == "claude" {
        c.args(["-p", "--output-format", "json", "--tools", ""]);
    } else {
        c.args(["exec", "--skip-git-repo-check", "-s", "read-only", "-o"]).arg(&last).arg("-");
    }
    for k in ["CLAUDE_CODE_SESSION_ID", "CODEX_THREAD_ID", "TASKQ_TASK", "TASKQ_ATTEMPT", "TASKQ_ROLE"] {
        c.env_remove(k);
    }
    let f = fs::File::create(&out).map_err(|e| e.to_string())?;
    // the requests folder: no project files at hand even for a runtime that could read them
    let mut child = c.current_dir(dir(root)).stdin(std::process::Stdio::piped()).stdout(f).stderr(std::process::Stdio::null()).spawn().map_err(|e| format!("{rt}: {e}"))?;
    let _ = std::io::Write::write_all(&mut child.stdin.take().unwrap(), brief(root, install, &req).as_bytes());
    let mut waited = 0;
    let status = loop {
        if let Ok(Some(s)) = child.try_wait() {
            break s;
        }
        if waited >= TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("{rt} gave no answer in {TIMEOUT} s"));
        }
        std::thread::sleep(Duration::from_secs(1));
        waited += 1;
    };
    let text = if rt == "claude" {
        let v: Value = serde_json::from_str(&fs::read_to_string(&out).unwrap_or_default()).unwrap_or_default();
        if v["is_error"] == true {
            return Err(format!("claude: {}", v["result"].as_str().unwrap_or("error")));
        }
        v["result"].as_str().unwrap_or_default().to_string()
    } else {
        let t = fs::read_to_string(&last).unwrap_or_default();
        let _ = fs::remove_file(&last);
        t
    };
    if !status.success() && text.is_empty() {
        return Err(format!("{rt} exited with {}", status.code().unwrap_or(-1)));
    }
    Ok(text)
}

fn brief(root: &Path, install: &Path, req: &Value) -> String {
    let ps: Vec<Value> = projects(install).iter().map(|(name, r)| {
        let s = read(&crate::dot(r).join("status.json"));
        let paused = fs::read_to_string(crate::dot(r).join("focus.paused")).unwrap_or_default();
        let tasks: Vec<Value> = s["tasks"].as_array().into_iter().flatten().map(|t| json!([t["n"], t["title"], t["state"], t["labels"]])).collect();
        json!({"project": name, "focus": s["focus"], "saved_focus": paused.trim(), "request": s["request"], "status_age_s": crate::file_age(&crate::dot(r).join("status.json")), "error": s["error"], "tasks [n, title, state, labels]": tasks})
    }).collect();
    // the whole clarify chain, oldest first: a setup conversation takes several turns
    let (mut before, mut r) = (String::new(), req.clone());
    for _ in 0..KEEP {
        let Some(a) = r["after"].as_str().filter(|a| a.bytes().all(|b| b.is_ascii_digit())).map(String::from) else { break };
        r = read(&dir(root).join(format!("{a}.json")));
        before = format!("Earlier request: {}\nYour answer then: {}\n{before}", r["text"], read(&answer_path(root, &a))["text"]);
    }
    // an owner comment, also through its clarify chain: the task and its open card; the reply stays on that task
    let c = comment_of(root, req);
    let scope = match c["task"].as_u64() {
        Some(n) => format!("The owner commented on task #{n} of project {} while it waits for an answer to this card:\n{}\n\
            A proposal holds exactly one op on #{n}: answer (option or text), later or requeue; close only by answering an option that starts with close. Unclear: clarify. Nothing to change: result.\n",
            c["project"].as_str().unwrap_or(""), c["card"].as_str().unwrap_or("")),
        None => String::new(),
    };
    if let Some(f) = onboard(root, req) {
        return format!(
            "You are the taskq operator (taskq.md R3) in a project setup (taskq.md § 6): help the owner set up the folder below as a taskq project. You have no tools: use only this data.\n\
             Reply with exactly one JSON object and nothing else: {{\"kind\": \"clarify|proposal|refuse\", \"text\": \"...\", \"ops\": [...]}}.\n\
             - clarify: ask only for missing information, never repository visibility already read in the inspection; one question at a time until you know: GitHub or GitLab and the account below, an existing repository or a new one, owner/name, visibility (a new one is private unless the owner says public), publish pr (default) or direct.\n\
             - proposal: exactly one op {{\"op\":\"setup\",\"board\":\"github|gitlab\",\"host\":\"github.com\",\"repo\":\"owner/name\",\"create\":true|false,\"visibility\":\"private|public\",\"publish\":\"pr|direct\",\"incident_reports\":true|false}}; incident_reports defaults to true; use false if the owner opts out. Text names account, repository and visibility. The service lists every effect, including public incident reporting, for the owner to confirm; files, uncommitted work and remotes stay.\n\
             - refuse: it cannot be set up so (for example no account); text is the reason.\n\
             Use English, short plain lines (taskq.md R7).\n\
             Folder: {}\n{before}Request: {}\n",
            serde_json::to_string_pretty(&inspect(&f)).unwrap_or_default(),
            req["text"].as_str().unwrap_or("")
        );
    }
    format!(
        "You are the taskq operator (taskq.md R3): answer the owner's request about the taskq projects below. You have no tools: use only this data.\n\
         Reply with exactly one JSON object and nothing else: {{\"kind\": \"result|clarify|proposal|refuse\", \"text\": \"...\", \"ops\": [...]}}.\n\
         - result: a read request; text is the answer.\n\
         - clarify: the request is unclear or its scope ambiguous; text is one question. Never widen a scope to guess.\n\
         - proposal: a change; text says in 1-3 short lines what changes and its effect; ops lists it. Nothing changes until the owner confirms.\n\
         - refuse: not possible with these ops or data; text is the reason.\n\
         Ops (only these; project = a name below; no value starts with '-'):\n\
         {{\"op\":\"focus\",\"project\":P,\"labels\":[\"area-x\"],\"request\":\"<owner's words>\"}} admit tasks with one of the labels (\"all\": any),\n\
         {{\"op\":\"pause\",\"project\":P}}, {{\"op\":\"resume\",\"project\":P}} (restores the saved focus),\n\
         {{\"op\":\"add\",\"project\":P,\"title\":T,\"goal\":G,\"acceptance\":[\"A1 check\"]}} a new task,\n\
         {{\"op\":\"answer\",\"project\":P,\"task\":N,\"option\":K}} or with \"text\" instead of \"option\" (only a task in state ask),\n\
         {{\"op\":\"later\",\"project\":P,\"task\":N,\"text\":R}}, {{\"op\":\"requeue\",\"project\":P,\"task\":N,\"text\":R}}.\n\
         Use English, short plain lines (taskq.md R7). Name tasks as #N with their project.\n\
         Projects: {}\n{before}{scope}Request: {}\n",
        serde_json::to_string_pretty(&ps).unwrap_or_default(),
        req["text"].as_str().unwrap_or("")
    )
}

/// The operator's reply as an answer: one JSON object of a known kind; a proposal only of valid ops (§ 4).
pub fn parse(text: &str, projects: &[(String, PathBuf)], onboard: Option<&Path>) -> Value {
    let v: Value = match (text.find('{'), text.rfind('}')) {
        (Some(a), Some(b)) if a < b => serde_json::from_str(&text[a..=b]).unwrap_or_default(),
        _ => Value::Null,
    };
    let kind = v["kind"].as_str().filter(|k| KINDS.contains(k));
    let Some(kind) = kind else {
        return json!({"state": "failed", "reason": "the operator gave no answer JSON"});
    };
    let body = v["text"].as_str().unwrap_or("").trim();
    if kind != "proposal" {
        return json!({"state": kind, "text": body});
    }
    let ops = v["ops"].as_array().cloned().unwrap_or_default();
    if ops.is_empty() {
        return json!({"state": "failed", "reason": "a proposal without ops"});
    }
    if let Some(r) = onboard {
        return match (ops.len(), setup(&ops[0], r)) {
            (1, Ok(s)) => json!({"state": "proposal", "text": body, "ops": ops, "effects": s.effects()}),
            (1, Err(e)) => json!({"state": "failed", "reason": format!("refused proposal: {e}")}),
            _ => json!({"state": "failed", "reason": "refused proposal: a setup is one setup op"}),
        };
    }
    if let Some(e) = ops.iter().find_map(|o| validate(o, projects).err()) {
        return json!({"state": "failed", "reason": format!("refused proposal: {e}")});
    }
    json!({"state": "proposal", "text": body, "ops": ops})
}

/// The folder an onboarding request sets up: its `onboard`, else that of the request it clarifies; only an absolute,
/// existing folder.
fn onboard(root: &Path, req: &Value) -> Option<PathBuf> {
    let mut r = req.clone();
    for _ in 0..KEEP {
        if let Some(p) = r["onboard"].as_str() {
            return Some(PathBuf::from(p)).filter(|p| p.is_absolute() && p.is_dir());
        }
        let a = r["after"].as_str().filter(|a| a.bytes().all(|b| b.is_ascii_digit()))?;
        r = read(&dir(root).join(format!("{a}.json")));
    }
    None
}

/// Run a command in `dir`: its stdout, else its first error line.
fn cmd(prog: impl AsRef<std::ffi::OsStr>, args: &[&str], dir: &Path, env: &[(&str, &str)]) -> Result<String, String> {
    let mut c = std::process::Command::new(prog);
    c.args(args).current_dir(dir).envs(env.iter().copied()).env_remove("TASKQ_TASK").env_remove("TASKQ_ATTEMPT").stdin(std::process::Stdio::null());
    let o = crate::platform::quiet(&mut c).output().map_err(|e| format!("{}: {e}", args.first().unwrap_or(&"")))?;
    let (out, err) = (String::from_utf8_lossy(&o.stdout).trim().to_string(), String::from_utf8_lossy(&o.stderr).to_string());
    if o.status.success() { Ok(out) } else { Err(err.lines().chain(out.lines()).find(|l| !l.trim().is_empty()).unwrap_or("failed").trim().to_string()) }
}

/// The board user of the board CLI's login on `host`.
fn account(k: &Kind, host: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(&cmd(k.cli, &["api", "user", "--hostname", host], &std::env::temp_dir(), &[])?).unwrap_or_default();
    v[k.user_key].as_str().map(String::from).ok_or(format!("{}: no user", k.cli))
}

/// What § 6 inspects first, for the operator: entries, Git root, remotes, uncommitted files, logins.
fn inspect(root: &Path) -> Value {
    let mut names: Vec<String> = fs::read_dir(root).into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    let git = root.join(".git").exists();
    let g = |a: &[&str]| if git { cmd("git", a, root, &[]).unwrap_or_default() } else { String::new() };
    let mut v = json!({"path": root, "entries_total": names.len(), "entries (first 40)": names.iter().take(40).collect::<Vec<_>>(), "git_root": git,
        "remotes": g(&["remote", "-v"]), "uncommitted_files": g(&["status", "--porcelain"]).lines().count(), "taskq_json": root.join("taskq.json").exists(), "accounts": {}});
    for k in BOARDS {
        if let Some(view) = k.repo_view {
            v[format!("{}_repository", k.name)] = if git {
                cmd(k.cli, view, root, &[])
                    .and_then(|s| serde_json::from_str::<Value>(&s).map_err(|e| e.to_string()))
                    .unwrap_or_else(|e| json!({"unknown": e}))
            } else { Value::Null };
        }
        v["accounts"][format!("{} ({})", k.default_host, k.cli)] = json!(account(k, k.default_host).unwrap_or_else(|e| format!("unknown: {e}")));
    }
    v
}

/// A checked `setup` op (§ 4 Add project).
#[derive(Debug, PartialEq)]
pub struct Setup {
    root: PathBuf,
    board: String,
    host: String,
    repo: String,
    create: bool,
    private: bool,
    publish: String,
    incident_reports: bool,
}

/// The setup allowlist: known words, a plain host and `owner/name`; never over an existing `taskq.json`.
pub fn setup(o: &Value, root: &Path) -> Result<Setup, String> {
    if o["op"] != "setup" {
        return Err(format!("op {} is not allowed in a setup", o["op"]));
    }
    if root.join("taskq.json").exists() {
        return Err("taskq.json exists: a setup never overwrites it".into());
    }
    let word = |k: &str, ok: &[&str]| match o[k].as_str().unwrap_or(ok[0]) {
        v if ok.contains(&v) => Ok(v.to_string()),
        _ => Err(format!("{k}: one of {}", ok.join(", "))),
    };
    let plain = |v: &str, more: &str| !v.is_empty() && !v.starts_with(['-', '.', '/']) && v.chars().all(|c| c.is_ascii_alphanumeric() || "-._".contains(c) || more.contains(c));
    let board = word("board", &BOARDS.map(|k| k.name))?;
    let host = o["host"].as_str().unwrap_or(crate::board::kind(&board)?.default_host).to_string();
    let repo = o["repo"].as_str().unwrap_or("").to_string();
    if !plain(&host, "") {
        return Err("host: a plain host name".into());
    }
    if !plain(&repo, "/") || !repo.contains('/') || repo.split('/').any(|p| p.is_empty() || p == "..") {
        return Err("repo: owner/name".into());
    }
    let private = word("visibility", &["private", "public"])? == "private";
    let incident_reports = match o.get("incident_reports") {
        None => true,
        Some(Value::Bool(value)) => *value,
        _ => return Err("incident_reports: true or false".into()),
    };
    Ok(Setup { root: root.into(), board, host, repo, create: o["create"] == true, private, publish: word("publish", &["pr", "direct"])?, incident_reports })
}

impl Setup {
    fn kind(&self) -> &'static Kind {
        crate::board::kind(&self.board).expect("setup() checked the board")
    }

    fn git(&self) -> bool {
        self.root.join(".git").exists()
    }

    fn origin(&self) -> bool {
        self.git() && cmd("git", &["remote", "get-url", "origin"], &self.root, &[]).is_ok()
    }

    /// Every effect in the order `apply_setup` makes it: the owner confirms this exact list (§ 6).
    pub fn effects(&self) -> Vec<String> {
        let mut v = vec![];
        if !self.git() {
            v.push(format!("Run git init in {}", self.root.display()));
        }
        v.push(if self.create {
            let who = account(self.kind(), &self.host).unwrap_or_else(|e| format!("unknown account ({e})"));
            format!("Create {} repository {}/{} as {who}", if self.private { "PRIVATE" } else { "PUBLIC" }, self.host, self.repo)
        } else {
            format!("Use existing repository {}/{}", self.host, self.repo)
        });
        if !self.origin() {
            v.push(format!("Add git remote origin https://{}/{}.git", self.host, self.repo));
        }
        v.push(format!("Write taskq.json: board {}, repo {}, publish {}", self.board, self.repo, self.publish));
        let who = self.incident_reports.then(|| incident_account(self.host == "github.com")).flatten();
        v.extend(incident_disclosure(self.incident_reports, who.as_deref()));
        v.push(format!("Create the taskq labels on {}", self.repo));
        v.push(format!("Register {} on this host and install its service (starts at logon)", crate::project_name(&self.root)));
        v
    }
}

fn incident_account(board_github_com: bool) -> Option<String> {
    use coreq::incident::{GitHub, Transport, github_token};
    let tool = crate::platform::tool(&crate::platform::default_install());
    let token = github_token(&tool, board_github_com)?;
    GitHub::new(&tool, token).login().ok()
}

fn incident_disclosure(enabled: bool, account: Option<&str>) -> Vec<String> {
    if !enabled {
        return vec!["Incident reports: disabled (incident_reports: false); nothing sent or saved".into()];
    }
    vec![
        format!("Incident reports: enabled (incident_reports: true); public destination https://github.com/{}", crate::platform::RELEASE_REPO),
        format!("Posting account: {}", account.unwrap_or("unavailable; reports stay local without a verified github.com login")),
        "Payload keys: schema, fault, fingerprint, report_id, release, revision, contract, board, runtime, os, site, evidence, observed, inferred".into(),
        "Only sanitized codes and release metadata; no project, task, user, session, token, absolute path or raw error text".into(),
        "Offline or unsupported login: local JSON fallback. Opt out: incident_reports: false or TASKQ_INCIDENT_REPORTS=off".into(),
    ]
}

/// Apply the confirmed setup step by step, stopping at the first failure; `applied` only when `selftest` passes there
/// and `<install>/projects` lists the folder (R12).
fn apply_setup(install: &Path, exe: &Path, root: &Path, ops: &[Value], shown: &Value) -> (bool, Vec<String>) {
    let mut lines = vec![];
    let r = (|| -> Result<(), String> {
        let s = setup(ops.first().unwrap_or(&Value::Null), root)?;
        // only the effects the owner confirmed: a folder changed since needs a new proposal
        if json!(s.effects()) != *shown {
            return Err("the effects changed since the proposal; send the request again".into());
        }
        if !s.git() {
            cmd("git", &["init", "-q"], root, &[])?;
            lines.push("git init: ok".to_string());
        }
        if s.create {
            let k = s.kind();
            cmd(k.cli, &["repo", "create", &s.repo, if s.private { "--private" } else { "--public" }], root, &[(k.host_env, &s.host)])?;
            lines.push(format!("created {}/{}", s.host, s.repo));
        }
        if !s.origin() {
            cmd("git", &["remote", "add", "origin", &format!("https://{}/{}.git", s.host, s.repo)], root, &[])?;
            lines.push("origin: added".into());
        }
        let c = json!({"board": s.board, "host": s.host, "repo": s.repo, "publish": s.publish, "incident_reports": s.incident_reports});
        if root.join("taskq.json").exists() {
            return Err("taskq.json appeared meanwhile: nothing overwritten".into());
        }
        crate::platform::write_atomic(&root.join("taskq.json"), &(serde_json::to_string_pretty(&c).unwrap_or_default() + "\n"))?;
        lines.push("taskq.json: written".into());
        let r = root.to_string_lossy();
        for a in [&["labels"][..], &["service", "install"], &["selftest"]] {
            cmd(exe, a, root, &[("TASKQ_ROOT", &r)]).map_err(|e| format!("taskq {}: {e}", a.join(" ")))?;
            lines.push(format!("taskq {}: ok", a.join(" ")));
        }
        if !projects(install).iter().any(|(_, p)| p == root) {
            return Err("this host's projects list does not show it".into());
        }
        lines.push("registered: read back".into());
        Ok(())
    })();
    if let Err(e) = &r {
        lines.push(format!("failed: {e}"));
    }
    (r.is_ok(), lines)
}

/// What an op does and how its readback is checked.
#[derive(Debug, PartialEq)]
pub enum Op {
    Focus(PathBuf, String, Option<String>),
    Pause(PathBuf),
    Resume(PathBuf),
    /// A `taskq` command in the project and the states its board re-read may show.
    Board(PathBuf, Vec<String>, &'static [&'static str]),
}

const ANY_BUT_ASK: &[&str] = &["ready", "waiting", "doing", "review", "later", "failed", "closed"];

/// The allowlist (§ 4): a registered project, plain values, a task number.
pub fn validate(o: &Value, projects: &[(String, PathBuf)]) -> Result<Op, String> {
    let name = o["project"].as_str().unwrap_or("");
    let roots: Vec<&PathBuf> = projects.iter().filter(|(n, _)| n == name).map(|(_, r)| r).collect();
    let [root] = roots[..] else { return Err(format!("project {name:?} is not one registered project")) };
    let root = root.clone();
    let s = |k: &str| -> Result<String, String> {
        let v = o[k].as_str().map(str::trim).unwrap_or("");
        if v.is_empty() || v.starts_with('-') { Err(format!("{k}: empty or starts with '-'")) } else { Ok(v.to_string()) }
    };
    let task = || o["task"].as_u64().filter(|n| *n > 0).map(|n| n.to_string()).ok_or("task: not a number".to_string());
    let list = |k: &str| -> Result<Vec<String>, String> {
        let v: Vec<String> = o[k].as_array().into_iter().flatten().map(|x| x.as_str().unwrap_or("").trim().to_string()).collect();
        if v.is_empty() || v.iter().any(|x| x.is_empty() || x.starts_with('-')) { Err(format!("{k}: empty or starts with '-'")) } else { Ok(v) }
    };
    match o["op"].as_str().unwrap_or("") {
        "focus" => {
            let labels = list("labels")?;
            if labels.iter().any(|l| l.contains(char::is_whitespace)) {
                return Err("labels: a label has a space".into());
            }
            Ok(Op::Focus(root, labels.join(" "), s("request").ok()))
        }
        "pause" => Ok(Op::Pause(root)),
        "resume" => Ok(Op::Resume(root)),
        "add" => {
            let mut a = vec!["add".to_string(), s("title")?, "--goal".into(), s("goal")?];
            for x in list("acceptance")? {
                a.extend(["--acceptance".into(), x]);
            }
            Ok(Op::Board(root, a, &["ready", "waiting"]))
        }
        "answer" => match (o["option"].as_u64(), s("text")) {
            (Some(k), _) => Ok(Op::Board(root, vec!["answer".into(), format!("{}.{k}", task()?)], ANY_BUT_ASK)),
            (None, Ok(t)) => Ok(Op::Board(root, vec!["answer".into(), task()?, "--text".into(), t], ANY_BUT_ASK)),
            _ => Err("answer: option or text needed".into()),
        },
        k @ ("later" | "requeue") => Ok(Op::Board(root, vec![k.into(), task()?, "--text".into(), s("text")?], if k == "later" { &["later"] } else { &["ready", "waiting"] })),
        k => Err(format!("op {k:?} is not allowed")),
    }
}

/// Apply ops in order; each line says what the readback showed. Local ops read their focus file back; board ops run
/// the `taskq` command, which re-reads the board, and its `#N <state>` line must show the op (R12).
fn apply(install: &Path, exe: &Path, ops: &[Value]) -> (bool, Vec<String>) {
    let ps = projects(install);
    let mut ok = true;
    let mut lines = vec![];
    for o in ops {
        let name = o["project"].as_str().unwrap_or("?");
        let focus = |r: &Path| fs::read_to_string(crate::dot(r).join("focus")).unwrap_or_default().trim().to_string();
        let r: Result<String, String> = validate(o, &ps).and_then(|op| match op {
            Op::Focus(r, f, req) => {
                crate::set_focus(&r, &f, req.as_deref())?;
                if focus(&r) == f { Ok(format!("focus {f}")) } else { Err(format!("focus reads {}", focus(&r))) }
            }
            Op::Pause(r) => {
                crate::pause(&r);
                if focus(&r) == "off" { Ok("paused".into()) } else { Err(format!("focus reads {}", focus(&r))) }
            }
            Op::Resume(r) => {
                crate::resume(&r)?;
                if !matches!(focus(&r).as_str(), "" | "off") { Ok(format!("focus {}", focus(&r))) } else { Err("focus reads off".into()) }
            }
            Op::Board(r, args, want) => {
                let out = std::process::Command::new(exe).args(&args).current_dir(&r).env("TASKQ_ROOT", &r)
                    .env_remove("TASKQ_TASK").env_remove("TASKQ_ATTEMPT").output().map_err(|e| e.to_string())?;
                let text = String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
                let seen = text.lines().find(|l| l.starts_with('#')).map(str::trim).unwrap_or("");
                match seen.split_once(' ') {
                    Some((_, st)) if out.status.success() && want.contains(&st) => Ok(seen.to_string()),
                    _ => Err(text.lines().next().unwrap_or("no output").to_string()),
                }
            }
        });
        ok &= r.is_ok();
        lines.push(match r {
            Ok(x) => format!("{name}: {} ok: {x}", o["op"].as_str().unwrap_or("")),
            Err(e) => format!("{name}: {} failed: {e}", o["op"].as_str().unwrap_or("")),
        });
    }
    (ok, lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("taskq-op-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn only_allowlisted_ops_on_registered_projects_pass() {
        let ps = vec![("a".to_string(), PathBuf::from("/a")), ("b".to_string(), PathBuf::from("/b"))];
        let ok = |o: Value| validate(&o, &ps);
        assert_eq!(ok(json!({"op": "pause", "project": "a"})), Ok(Op::Pause("/a".into())));
        assert_eq!(ok(json!({"op": "focus", "project": "b", "labels": ["area-x", "p1"], "request": "maps"})), Ok(Op::Focus("/b".into(), "area-x p1".into(), Some("maps".into()))));
        assert_eq!(ok(json!({"op": "answer", "project": "a", "task": 7, "option": 2})), Ok(Op::Board("/a".into(), vec!["answer".into(), "7.2".into()], ANY_BUT_ASK)));
        assert!(matches!(ok(json!({"op": "add", "project": "a", "title": "T", "goal": "G", "acceptance": ["A1 x"]})), Ok(Op::Board(_, a, _)) if a == ["add", "T", "--goal", "G", "--acceptance", "A1 x"]));
        assert!(matches!(ok(json!({"op": "requeue", "project": "a", "task": 3, "text": "again"})), Ok(Op::Board(_, _, &["ready", "waiting"]))));
        for bad in [json!({"op": "shell", "project": "a", "cmd": "rm -rf /"}), json!({"op": "pause", "project": "c"}), json!({"op": "later", "project": "a", "task": "7", "text": "x"}),
            json!({"op": "add", "project": "a", "title": "--force", "goal": "G", "acceptance": ["x"]}), json!({"op": "focus", "project": "a", "labels": []}), json!({"op": "answer", "project": "a", "task": 1})] {
            assert!(ok(bad.clone()).is_err(), "{bad}");
        }
        let twice = vec![("a".to_string(), PathBuf::from("/a")), ("a".to_string(), PathBuf::from("/a2"))];
        assert!(validate(&json!({"op": "pause", "project": "a"}), &twice).is_err(), "an ambiguous name is refused");
    }

    #[test]
    fn replies_become_answers_and_bad_proposals_fail() {
        let ps = vec![("a".to_string(), PathBuf::from("/a"))];
        assert_eq!(parse("Sure:\n{\"kind\": \"result\", \"text\": \"2 in work\"}", &ps, None), json!({"state": "result", "text": "2 in work"}));
        assert_eq!(parse("no json", &ps, None)["state"], "failed");
        assert_eq!(parse("{\"kind\": \"done\"}", &ps, None)["state"], "failed");
        assert_eq!(parse("{\"kind\": \"proposal\", \"text\": \"x\", \"ops\": []}", &ps, None)["state"], "failed");
        assert_eq!(parse("{\"kind\": \"proposal\", \"text\": \"x\", \"ops\": [{\"op\": \"close\", \"project\": \"a\", \"task\": 1}]}", &ps, None)["state"], "failed");
        assert_eq!(parse("{\"kind\": \"proposal\", \"text\": \"Pause a\", \"ops\": [{\"op\": \"pause\", \"project\": \"a\"}]}", &ps, None)["state"], "proposal");
    }

    #[test]
    fn incident_setup_discloses_account_payload_fallback_and_opt_out() {
        let enabled = incident_disclosure(true, Some("reporter"));
        assert!(enabled[0].contains(crate::platform::RELEASE_REPO) && enabled[0].contains("public"));
        assert_eq!(enabled[1], "Posting account: reporter");
        for key in ["schema", "fault", "fingerprint", "report_id", "release", "revision", "contract", "board", "runtime", "os", "site", "evidence", "observed", "inferred"] {
            assert!(enabled[2].contains(key), "{key}");
        }
        assert!(enabled[4].contains("local JSON") && enabled[4].contains("false") && enabled[4].contains("TASKQ_INCIDENT_REPORTS=off"));
        assert!(incident_disclosure(true, None)[1].contains("unavailable; reports stay local"));
        assert_eq!(incident_disclosure(false, Some("reporter")), ["Incident reports: disabled (incident_reports: false); nothing sent or saved"]);
    }

    #[test]
    fn a_setup_lists_every_effect_and_never_overwrites() {
        let root = temp("setup");
        let ok = |o: Value| setup(&o, &root);
        let s = ok(json!({"op": "setup", "repo": "o/n"})).unwrap();
        assert!(s.private && !s.create && s.incident_reports && (s.board.as_str(), s.host.as_str(), s.publish.as_str()) == ("github", "github.com", "pr"), "new repositories default to private, reporting disclosed before confirmation");
        assert!(!ok(json!({"op": "setup", "repo": "o/n", "incident_reports": false})).unwrap().incident_reports);
        for bad in [json!({"op": "pause", "project": "a"}), json!({"op": "setup", "repo": "n"}), json!({"op": "setup", "repo": "-o/n"}), json!({"op": "setup", "repo": "o/../n"}),
            json!({"op": "setup", "repo": "o/n", "visibility": "open"}), json!({"op": "setup", "repo": "o/n", "host": "a b"}), json!({"op": "setup", "repo": "o/n", "board": "jira"}),
            json!({"op": "setup", "repo": "o/n", "incident_reports": "false"}), json!({"op": "setup", "repo": "o/n", "incident_reports": null})] {
            assert!(ok(bad.clone()).is_err(), "{bad}");
        }
        let p = parse(r#"{"kind": "proposal", "text": "x", "ops": [{"op": "setup", "repo": "o/n", "publish": "direct", "incident_reports": false}]}"#, &[], Some(&root));
        assert_eq!(p["state"], "proposal");
        let e: Vec<&str> = p["effects"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
        assert!(e[0].starts_with("Run git init in ") && e[1] == "Use existing repository github.com/o/n" && e[2] == "Add git remote origin https://github.com/o/n.git", "{e:?}");
        assert_eq!(e[3], "Write taskq.json: board github, repo o/n, publish direct");
        assert_eq!(e[4], "Incident reports: disabled (incident_reports: false); nothing sent or saved");
        assert_eq!(e[5], "Create the taskq labels on o/n");
        assert!(e[6].starts_with("Register ") && e.len() == 7);
        let two = r#"{"kind": "proposal", "text": "x", "ops": [{"op": "setup", "repo": "o/n"}, {"op": "setup", "repo": "o/m"}]}"#;
        assert_eq!(parse(two, &[], Some(&root))["state"], "failed", "one setup op only");
        assert_eq!(parse(r#"{"kind": "proposal", "text": "x", "ops": [{"op": "pause", "project": "a"}]}"#, &[("a".into(), "/a".into())], Some(&root))["state"], "failed");
        // a clarify reply inherits the folder; an existing taskq.json is never overwritten
        fs::create_dir_all(dir(&root)).unwrap();
        fs::write(dir(&root).join("1.json"), json!({"text": "set up", "onboard": root}).to_string()).unwrap();
        assert_eq!(onboard(&root, &json!({"text": "GitHub", "after": "1"})), Some(root.clone()));
        assert_eq!(onboard(&root, &json!({"text": "x"})), None);
        assert_eq!(onboard(&root, &json!({"onboard": "relative/dir"})), None, "only an absolute, existing folder");
        let shown = json!(setup(&json!({"op": "setup", "repo": "o/n", "incident_reports": false}), &root).unwrap().effects());
        fs::create_dir(root.join(".git")).unwrap();
        let (applied, lines) = apply_setup(&root, Path::new("/nonexistent"), &root, &[json!({"op": "setup", "repo": "o/n", "incident_reports": false})], &shown);
        assert!(!applied && lines[0].contains("effects changed"), "{lines:?}");
        fs::remove_dir(root.join(".git")).unwrap();
        fs::write(root.join("taskq.json"), "{}").unwrap();
        assert!(ok(json!({"op": "setup", "repo": "o/n"})).is_err());
        let (applied, lines) = apply_setup(&root, Path::new("/nonexistent"), &root, &[json!({"op": "setup", "repo": "o/n"})], &Value::Null);
        assert!(!applied && lines.len() == 1 && !root.join(".git").exists(), "{lines:?}");
        assert_eq!(fs::read_to_string(root.join("taskq.json")).unwrap(), "{}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn an_owner_comment_is_one_request_and_proposes_only_on_its_task() {
        let root = temp("comments");
        let found = vec![(7, 101, "use z instead".to_string(), "1. x\n2. y ★".to_string())];
        owner_comments(&root, "p", "https://h/o/r/issues", &found);
        let first = ids(&root);
        assert_eq!(first.len(), 1);
        let req = read(&dir(&root).join(format!("{}.json", first[0])));
        assert_eq!((req["task"].as_u64(), req["comment"].as_u64(), req["url"].as_str()), (Some(7), Some(101), Some("https://h/o/r/issues/7")));
        // confirmed, refused or pruned: the same comment is never a request again
        answer(&root, &first[0], json!({"state": "refuse", "text": "no"}));
        fs::remove_file(dir(&root).join(format!("{}.json", first[0]))).unwrap();
        owner_comments(&root, "p", "https://h/o/r/issues", &found);
        assert!(ids(&root).is_empty());
        let reply = format!(r#"{{"text": "y", "after": "{}"}}"#, first[0]);
        fs::write(dir(&root).join(format!("{}.json", first[0])), req.to_string()).unwrap();
        let p = |ops: Value| json!({"state": "proposal", "text": "t", "ops": ops});
        let c = comment_of(&root, &serde_json::from_str(&reply).unwrap());
        assert_eq!(c["task"], 7, "a clarify reply keeps the comment's task");
        assert_eq!(scoped(p(json!([{"op": "answer", "project": "p", "task": 7, "text": "use z"}])), &c)["state"], "proposal");
        for bad in [json!([{"op": "answer", "project": "p", "task": 8, "option": 1}]), json!([{"op": "pause", "project": "p"}]),
            json!([{"op": "later", "project": "q", "task": 7, "text": "x"}]), json!([{"op": "later", "project": "p", "task": 7, "text": "x"}, {"op": "pause", "project": "p"}])] {
            assert_eq!(scoped(p(bad.clone()), &c)["state"], "failed", "{bad}");
        }
        assert_eq!(scoped(p(json!([{"op": "pause", "project": "p"}])), &Value::Null)["state"], "proposal", "a tray request keeps every op");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_proposal_applies_once_at_its_rev_with_readback() {
        let (install, root) = (temp("install"), temp("root"));
        fs::write(install.join("projects"), format!("{}\n", root.display())).unwrap();
        fs::create_dir_all(dir(&root)).unwrap();
        fs::write(crate::dot(&root).join("focus"), "area-x").unwrap();
        let name = crate::project_name(&root);
        fs::write(dir(&root).join("5.json"), r#"{"text": "pause it"}"#).unwrap();
        answer(&root, "5", json!({"state": "running"}));
        answer(&root, "5", json!({"state": "proposal", "text": "Pause", "ops": [{"op": "pause", "project": name}]}));
        let a = || read(&answer_path(&root, "5"));
        let focus = || fs::read_to_string(crate::dot(&root).join("focus")).unwrap();
        let confirm = |rev: &str| {
            fs::write(dir(&root).join("5.confirm"), rev).unwrap();
            step(&root, &install, Path::new("/nonexistent"), None);
        };
        assert_eq!(a()["rev"], 2);
        confirm("1");
        assert_eq!((a()["state"].as_str(), focus().as_str()), (Some("proposal"), "area-x"), "a stale rev does nothing");
        confirm("2");
        assert_eq!((a()["state"].as_str(), focus().as_str()), (Some("applied"), "off"));
        assert!(a()["readback"][0].as_str().unwrap().ends_with("pause ok: paused"));
        let rev = a()["rev"].clone();
        fs::write(crate::dot(&root).join("focus"), "area-y").unwrap();
        confirm("2");
        confirm(&rev.to_string());
        assert_eq!((a()["rev"].clone(), focus().as_str()), (rev, "area-y"), "a repeated confirm applies nothing");
        // a request without a runtime fails as an AI failure, not a task; a restart ends a running one
        fs::write(dir(&root).join("6.json"), r#"{"text": "x"}"#).unwrap();
        step(&root, &install, Path::new("/nonexistent"), None);
        assert_eq!(read(&answer_path(&root, "6"))["state"], "failed");
        answer(&root, "6", json!({"state": "running"}));
        recover(&root);
        assert_eq!(read(&answer_path(&root, "6"))["state"], "failed");
        for i in 10..40 {
            fs::write(dir(&root).join(format!("{i}.json")), "{}").unwrap();
        }
        step(&root, &install, Path::new("/nonexistent"), None);
        assert_eq!(ids(&root).len(), KEEP);
        assert_eq!(ids(&root).first().map(String::as_str), Some("20"), "the newest 20 stay");
        let _ = (fs::remove_dir_all(&install), fs::remove_dir_all(&root));
    }
}
