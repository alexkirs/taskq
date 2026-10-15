//! Board contract (taskq.md § 1, § 2, § 9): the `Board` trait, shared HTTP through the OS `curl`, the registry of
//! board modules (`github`, `gitlab`) and an in-memory board for tests. Every write is a native board object; nothing else is state.

use crate::core::Entry;
use serde_json::{Value, json};
use std::cell::RefCell;
use std::io::Write;
use std::process::{Command, Stdio};

pub type Result<T> = std::result::Result<T, String>;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Issue {
    pub number: u64,
    pub title: String,
    pub body: String,
    pub labels: Vec<String>,
    pub assignees: Vec<String>,
    pub updated: i64,
    pub open: bool,
    pub trusted: bool,
    /// Why a closed issue closed, when the board records it (GitHub `state_reason`): `completed`, `not_planned`, ...
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Comment {
    pub id: u64,
    pub created: i64,
    pub body: String,
    pub trusted: bool,
    pub edited: bool,
}

pub trait Board {
    fn canonical_labels(&self) -> Result<bool> { Ok(true) }
    fn state_label(&self, state: &str) -> Result<String> {
        Ok(format!("q-{}", if state == "waiting" && self.canonical_labels()? { "blocked" } else { state }))
    }
    fn ensure_labels(&self, _names: &[String]) -> Result<()> { Ok(()) }
    /// Open issues; with `since` (board seconds) every issue, closed too, updated at or after it.
    fn issues(&self, since: Option<i64>) -> Result<Vec<Issue>>;
    fn issue(&self, n: u64) -> Result<Issue>;
    fn comments(&self, n: u64) -> Result<Vec<Comment>>;
    fn comment(&self, n: u64, body: &str) -> Result<u64>;
    /// Edits comment `id` of issue `n`: an edited entry is ignored by `fold` (§ 2).
    fn edit_comment(&self, n: u64, id: u64, body: &str) -> Result<()>;
    fn labels(&self, n: u64, add: &[String], remove: &[String]) -> Result<()>;
    fn create(&self, title: &str, body: &str, labels: &[String]) -> Result<u64>;
    fn edit(&self, n: u64, body: &str) -> Result<()>;
    fn close(&self, n: u64) -> Result<()>;
    /// The board's clock in seconds; leases never use local clocks (§ 4).
    fn now(&self) -> Result<i64>;
    fn me(&self) -> Result<String>;
    /// § 7 publication of task `n` at `sha` into `main`; `mode` is `direct` or `pr`.
    fn publish(&self, n: u64, sha: &str, mode: &str, title: &str, task_line: &str) -> Publish;
    fn url(&self, n: u64) -> String;
    /// Issue `n` of another repository on this board's host, for `Depends on <repo>#N` (§ 1); a bare name is under
    /// this board's owner.
    fn foreign(&self, repo: &str, _n: u64) -> Result<Issue> {
        Err(format!("{repo}: other repositories unsupported on this board"))
    }
    fn foreign_url(&self, repo: &str, n: u64) -> String {
        format!("{repo}#{n}")
    }
    /// Request accounting and rate-limit pause of this process (§ 4); none for boards without HTTP.
    fn requests(&self) -> crate::requests::Budget { Default::default() }
}

#[derive(Debug, PartialEq)]
pub enum Publish {
    /// Merged or already on `main`: close.
    Done(String),
    /// CI pending or missing: refuse, state unchanged.
    Pending(String),
    /// CI failed or conflict: requeue with this message.
    Failed(String),
}

/// The trusted log of issue `n` (§ 2).
pub fn entries(board: &dyn Board, n: u64) -> Result<Vec<Entry>> {
    Ok(board
        .comments(n)?
        .into_iter()
        .filter(|c| c.trusted)
        .map(|c| Entry { id: c.id, body: c.body, edited: c.edited })
        .collect())
}

/// § 4 Profiling board operations: each call of the inner board is one `taskq-board` record; cached clock, user
/// and label-schema reads are not operations and pass through; `ensure_labels` is a `label` operation.
struct Profiled(Box<dyn Board>);

impl Profiled {
    fn op<T>(&self, kind: &'static str, f: impl FnOnce(&dyn Board) -> Result<T>) -> Result<T> {
        let began = std::time::Instant::now();
        let r = f(&*self.0);
        crate::profile::board(kind, began.elapsed(), r.as_ref().err().map(String::as_str), self.0.requests().remaining);
        r
    }
}

impl Board for Profiled {
    fn canonical_labels(&self) -> Result<bool> { self.0.canonical_labels() }
    fn state_label(&self, state: &str) -> Result<String> { self.0.state_label(state) }
    fn ensure_labels(&self, names: &[String]) -> Result<()> { self.op("label", |b| b.ensure_labels(names)) }
    fn issues(&self, since: Option<i64>) -> Result<Vec<Issue>> { self.op("list", |b| b.issues(since)) }
    fn issue(&self, n: u64) -> Result<Issue> { self.op("read", |b| b.issue(n)) }
    fn comments(&self, n: u64) -> Result<Vec<Comment>> { self.op("inbox", |b| b.comments(n)) }
    fn comment(&self, n: u64, body: &str) -> Result<u64> {
        self.op(if body.starts_with("taskq: spawn ") { "claim" } else { "comment" }, |b| b.comment(n, body))
    }
    fn edit_comment(&self, n: u64, id: u64, body: &str) -> Result<()> { self.op("comment", |b| b.edit_comment(n, id, body)) }
    fn labels(&self, n: u64, add: &[String], remove: &[String]) -> Result<()> { self.op("label", |b| b.labels(n, add, remove)) }
    fn create(&self, title: &str, body: &str, labels: &[String]) -> Result<u64> { self.op("write", |b| b.create(title, body, labels)) }
    fn edit(&self, n: u64, body: &str) -> Result<()> { self.op("write", |b| b.edit(n, body)) }
    fn close(&self, n: u64) -> Result<()> { self.op("close", |b| b.close(n)) }
    fn now(&self) -> Result<i64> { self.0.now() }
    fn me(&self) -> Result<String> { self.0.me() }
    fn publish(&self, n: u64, sha: &str, mode: &str, title: &str, task_line: &str) -> Publish {
        let began = std::time::Instant::now();
        let p = self.0.publish(n, sha, mode, title, task_line);
        let failed = match &p { Publish::Done(_) => None, Publish::Pending(e) | Publish::Failed(e) => Some(e.as_str()) };
        crate::profile::board("publish", began.elapsed(), failed, self.0.requests().remaining);
        p
    }
    fn url(&self, n: u64) -> String { self.0.url(n) }
    fn foreign(&self, repo: &str, n: u64) -> Result<Issue> { self.op("read", |b| b.foreign(repo, n)) }
    fn foreign_url(&self, repo: &str, n: u64) -> String { self.0.foreign_url(repo, n) }
    fn requests(&self) -> crate::requests::Budget { self.0.requests() }
}

// ---------- time ----------

fn days(y: i64, m: i64, d: i64) -> i64 {
    // days from 1970-01-01 to a civil date (Howard Hinnant's algorithm)
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    era * 146097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719468
}

/// `2026-10-12T10:00:00Z`, with optional fraction or `+00:00`; UTC only, as both boards send.
pub fn iso(s: &str) -> Option<i64> {
    let n = |a: usize, b: usize| s.get(a..b).and_then(|x| x.parse::<i64>().ok());
    Some(days(n(0, 4)?, n(5, 7)?, n(8, 10)?) * 86400 + n(11, 13)? * 3600 + n(14, 16)? * 60 + n(17, 19)?)
}

/// HTTP `Date`: `Mon, 12 Oct 2026 10:00:00 GMT`.
pub fn http_date(s: &str) -> Option<i64> {
    let w: Vec<&str> = s.split_whitespace().collect();
    let months = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let mon = *w.get(2)?;
    let m = months.iter().position(|x| *x == mon)? as i64 + 1;
    let t: Vec<i64> = w.get(4)?.split(':').map(|x| x.parse().ok()).collect::<Option<_>>()?;
    Some(days(w.get(3)?.parse().ok()?, m, w.get(1)?.parse().ok()?) * 86400 + t.first()? * 3600 + t.get(1)? * 60 + t.get(2)?)
}

// ---------- HTTP through curl ----------

struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
    operation: &'static str,
}

impl Reply {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
    fn json(&self) -> Result<Value> {
        serde_json::from_str(&self.body).map_err(|e| {
            crate::incident::detect("adapter.bad_json", concat!(file!(), ":", line!()), "none", &[self.operation], &["http:2xx"]);
            format!("bad JSON from board: {e}")
        })
    }
    fn next(&self) -> Option<String> {
        // Link: <url>; rel="next", <url>; rel="last"
        self.header("link")?.split(',').find(|p| p.contains("rel=\"next\""))
            .and_then(|p| Some(p.split_once('<')?.1.split_once('>')?.0.to_string()))
    }
}

fn operation(method: &str) -> &'static str {
    match method { "GET" => "read", "POST" => "create", "PUT" | "PATCH" => "update", "DELETE" => "delete", _ => "other" }
}

fn quote(s: &str) -> String {
    let mut q = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => q.push_str("\\\""),
            '\\' => q.push_str("\\\\"),
            '\n' => q.push_str("\\n"),
            '\r' => q.push_str("\\r"),
            '\t' => q.push_str("\\t"),
            c => q.push(c),
        }
    }
    q.push('"');
    q
}

/// One request, any HTTP status. The token and body go through curl's config on stdin, never argv (R12: no secrets in `ps`).
fn curl(method: &str, url: &str, auth: &str, body: Option<&Value>) -> Result<Reply> {
    let mut cfg = format!(
        "silent\nshow-error\ndump-header = \"-\"\nrequest = {}\nurl = {}\nheader = {}\nheader = \"Accept: application/json\"\nheader = \"Expect:\"\nmax-time = 60\n",
        quote(method), quote(url), quote(auth)
    );
    if let Some(b) = body {
        cfg += &format!("header = \"Content-Type: application/json\"\ndata-binary = {}\n", quote(&b.to_string()));
    }
    let mut child = Command::new("curl")
        .args(["-K", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("curl: {e}"))?;
    child.stdin.take().unwrap().write_all(cfg.as_bytes()).map_err(|e| format!("curl: {e}"))?;
    let out = child.wait_with_output().map_err(|e| format!("curl: {e}"))?;
    if !out.status.success() {
        return Err(format!("curl {method} {url}: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let mut rest = text.as_str();
    loop {
        let (head, body) = rest.split_once("\r\n\r\n").ok_or("curl: no HTTP header")?;
        let mut lines = head.lines();
        let status: u16 = lines.next().and_then(|l| l.split_whitespace().nth(1)).and_then(|s| s.parse().ok()).unwrap_or(0);
        if status == 100 && body.starts_with("HTTP/") {
            rest = body;
            continue;
        }
        let headers = lines.filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_string(), v.trim().to_string())).collect();
        return Ok(Reply { status, headers, body: body.to_string(), operation: operation(method) });
    }
}

pub mod github;
pub mod gitlab;

/// ` into <base>` of a § 7 receipt whose PR/MR merged into a branch other than `main`.
fn into(base: &str) -> String {
    if base.is_empty() || base == "main" { String::new() } else { format!(" into {base}") }
}

fn s(v: &Value, k: &str) -> String {
    v[k].as_str().unwrap_or_default().to_string()
}

/// A `Depends on <repo>#N` reply (§ 1, R12): issue `n` (key `num`) whose `state` is `open` or `closed`, else unknown.
fn dep_reply(v: Value, num: &str, n: u64, open: &str) -> Result<Value> {
    match (v[num].as_u64(), v["state"].as_str()) {
        (Some(m), Some(st)) if m == n && (st == open || st == "closed") => Ok(v),
        _ => Err(format!("malformed reply for issue {n}")),
    }
}

fn names(v: &Value, k: &str) -> Vec<String> {
    v.as_array().into_iter().flatten().map(|x| x.as_str().map(String::from).unwrap_or_else(|| s(x, k))).collect()
}

fn verify_assignment(board: &dyn Board, n: u64, add: &[String], remove: &[String]) -> Result<()> {
    let issue = board.issue(n)?;
    if add.iter().any(|l| !issue.labels.contains(l)) || remove.iter().any(|l| issue.labels.contains(l)) {
        return Err(format!("#{n}: label assignment readback differs; issue may already exist/changes may be partial; inspect before retry"));
    }
    Ok(())
}

fn encode(s: &str) -> String {
    s.bytes().map(|c| if c.is_ascii_alphanumeric() || b"-_.~".contains(&c) { (c as char).to_string() } else { format!("%{c:02X}") }).collect()
}

fn label_matches(v: &Value, name: &str, color: &str, description: &str) -> bool {
    v["name"] == name && s(v, "color").trim_start_matches('#').eq_ignore_ascii_case(color) && v["description"] == description
}


// ---------- boards ----------

/// One board: its setup facts and constructors. Each lives in its own module (taskq.md § 9); nothing outside a
/// board module branches on which board it is.
pub struct Kind {
    /// `board` in `taskq.json`.
    pub name: &'static str,
    /// The login CLI and the env that names its host.
    pub cli: &'static str,
    pub host_env: &'static str,
    /// The user name field of the CLI's `api user`.
    pub user_key: &'static str,
    pub default_host: &'static str,
    /// CLI arguments that print the current folder's repository, if the CLI has them.
    pub repo_view: Option<&'static [&'static str]>,
    token: fn(&coreq::Tool, &str) -> Result<String>,
    token_env: &'static [&'static str],
    api: fn(&str, &str, &str) -> Api,
    board: fn(Api) -> Box<dyn Board>,
}

pub const BOARDS: [&Kind; 2] = [&github::KIND, &gitlab::KIND];

pub fn kind(name: &str) -> Result<&'static Kind> {
    BOARDS.into_iter().find(|k| k.name == name).ok_or(format!("unknown board {name}; expected {}", BOARDS.map(|k| k.name).join("|")))
}

fn label_schema(schema: Option<&str>) -> Result<Option<bool>> {
    match schema {
        None => Ok(None), Some("canonical") => Ok(Some(true)), Some("legacy") => Ok(Some(false)),
        Some(s) => Err(format!("unknown label_schema {s}; expected canonical|legacy")),
    }
}

impl Kind {
    pub fn worker_env(&self, tool: &coreq::Tool, host: &str) -> Result<Vec<(String, String)>> {
        let token = (self.token)(tool, host)?;
        Ok(self.token_env.iter().map(|name| (name.to_string(), token.clone())).collect())
    }

    /// The board of `repo` on the web `host`, with the token this board's lookup finds (§ 9).
    pub fn open(&self, host: &str, repo: &str, schema: Option<&str>, tool: &coreq::Tool) -> Result<Box<dyn Board>> {
        let schema = label_schema(schema)?;
        let api = (self.api)(host, repo, &(self.token)(tool, host)?);
        *api.label_schema.borrow_mut() = schema;
        Ok(Box::new(Profiled((self.board)(api))))
    }
}

#[cfg(test)]
type Transport = Box<dyn Fn(&str, &str, Option<Value>) -> Result<Reply>>;

/// The HTTP side every board shares: one API root, its token, the board clock and cached reads.
pub struct Api {
    /// `https://api.github.com/repos/o/r`, `https://host/api/v4/projects/o%2Fr`
    base: String,
    /// `https://api.github.com`, `https://host/api/v4`: the token goes nowhere else.
    root: String,
    auth: String,
    me: RefCell<Option<String>>,
    clock: RefCell<i64>,
    label_schema: RefCell<Option<bool>>,
    /// Request accounting and rate-limit pause of this process (§ 4).
    budget: crate::requests::Shared,
    /// Labels whose catalog definition this process has read back: verified once per process (§ 4 budget).
    labels_ok: RefCell<std::collections::HashSet<String>>,
    #[cfg(test)]
    transport: Option<Transport>,
}

impl Api {
    fn new(root: String, base: String, token: &str) -> Api {
        let budget = crate::requests::Shared::for_api(&root, token);
        Api { base, root, auth: format!("Authorization: Bearer {token}"), me: RefCell::new(None), clock: RefCell::new(0), label_schema: RefCell::new(None), budget, labels_ok: RefCell::default(), #[cfg(test)] transport: None }
    }

    fn call(&self, method: &str, url: &str, body: Option<Value>) -> Result<Reply> {
        self.send(method, url, body, &self.op(method, url))
    }

    /// The accounting key of `url`, relative to this board's API (§ 4).
    fn op(&self, method: &str, url: &str) -> String {
        crate::requests::op(method, url.strip_prefix(&self.base).or_else(|| url.strip_prefix(&self.root)).unwrap_or(url))
    }

    /// One request counted under `op`. While the board budget is exhausted nothing is sent (§ 4).
    fn send(&self, method: &str, url: &str, body: Option<Value>, op: &str) -> Result<Reply> {
        // absolute URLs come from pagination headers: the token goes only to the configured API
        let url = if url.starts_with("https://") { url.to_string() } else { format!("{}{url}", self.base) };
        if !url.starts_with(&format!("{}/", self.root)) {
            return Err(format!("refused: {url} is outside the board API"));
        }
        if let Some(reason) = &self.budget.borrow().unknown { return Err(format!("board requests held: {reason}")); }
        if let Some(reset) = self.budget.borrow_mut().paused(std::time::Instant::now()) {
            return Err(format!("{method} {url}: {}", crate::requests::paused_text(reset)));
        }
        self.budget.borrow_mut().count(op.into());
        crate::requests::tally(op);
        crate::profile::measure("forge.call", || {
            #[cfg(test)]
            let r = match &self.transport { Some(transport) => transport(method, &url, body)?, None => curl(method, &url, &self.auth, body.as_ref())? };
            #[cfg(not(test))]
            let r = curl(method, &url, &self.auth, body.as_ref())?;
            if let Some(t) = r.header("date").and_then(http_date) {
                *self.clock.borrow_mut() = t;
            }
            // GitHub GraphQL refuses an exhausted budget with HTTP 200 and a RATE_LIMITED error
            let status = if url.ends_with("/graphql") && r.status == 200 && r.body.contains("\"RATE_LIMITED\"") { 403 } else { r.status };
            let paused = self.budget.borrow_mut().observe(status, |n| r.header(n).map(String::from), *self.clock.borrow(), std::time::Instant::now());
            if let Some(reset) = paused {
                return Err(format!("{method} {url}: HTTP {status} {}", crate::requests::paused_text(reset)));
            }
            if !(200..300).contains(&r.status) {
                // one log line: a multi-line error body folds into it (§ 4)
                let body = r.body.split_whitespace().collect::<Vec<_>>().join(" ");
                return Err(format!("{method} {url}: HTTP {} {}", r.status, body.chars().take(300).collect::<String>()));
            }
            Ok(r)
        })
    }

    /// The full path of `repo` in a `Depends on <repo>#N` (§ 1); a bare name is under this board's owner. `sep` ends the
    /// API part of `base`.
    fn sibling(&self, sep: &str, repo: &str) -> String {
        let own = self.base.rsplit_once(sep).map(|x| x.1.replace("%2F", "/")).unwrap_or_default();
        match own.rsplit_once('/') {
            Some((owner, _)) if !repo.contains('/') => format!("{owner}/{repo}"),
            _ => repo.to_string(),
        }
    }

    fn all(&self, url: &str) -> Result<Vec<Value>> {
        let mut out = vec![];
        let op = self.op("GET", url);
        let mut next = Some(url.to_string());
        while let Some(u) = next {
            // later pages count under their first page's operation
            let r = self.send("GET", &u, None, &op)?;
            out.extend(r.json()?.as_array().cloned().unwrap_or_default());
            next = r.next();
        }
        Ok(out)
    }

    /// The board user, `key` its name field.
    fn me(&self, key: &str) -> Result<String> {
        if let Some(m) = self.me.borrow().clone() {
            return Ok(m);
        }
        let v = self.call("GET", &format!("{}/user", self.root), None)?.json()?;
        let m = s(&v, key);
        *self.me.borrow_mut() = Some(m.clone());
        Ok(m)
    }

    fn now(&self, key: &str) -> Result<i64> {
        if *self.clock.borrow() == 0 {
            self.me(key)?;
        }
        Ok(*self.clock.borrow())
    }

    fn canonical_labels(&self) -> Result<bool> {
        if let Some(c) = *self.label_schema.borrow() { return Ok(c); }
        let all = self.all("/labels?per_page=100")?;
        let canonical = !all.iter().any(|v| matches!(v["name"].as_str(), Some("q-waiting" | "priority-1" | "priority-2")));
        *self.label_schema.borrow_mut() = Some(canonical);
        Ok(canonical)
    }

    /// Catalog sync: `color` renders a catalog color for the board; `write(current, payload)` creates or updates one label.
    fn ensure_labels(&self, names: &[String], color: fn(&str) -> String, write: impl Fn(Option<&Value>, Value) -> Result<()>) -> Result<()> {
        if !self.canonical_labels()? { return Err("legacy board: catalog adoption requires all-host gate and label_schema=canonical".into()); }
        let fresh: Vec<String> = names.iter().filter(|n| !self.labels_ok.borrow().contains(*n)).cloned().collect();
        if fresh.is_empty() { return Ok(()); }
        let names = &fresh[..];
        let all = self.all("/labels?per_page=100")?;
        let mut seen = std::collections::HashSet::new();
        for name in names {
            if !seen.insert(name) { continue; }
            let Some((c, description)) = crate::labels::definition(name) else { continue };
            let current = all.iter().find(|v| v["name"] == name.as_str());
            if current.is_some_and(|v| label_matches(v, name, c, &description)) { continue; }
            write(current, json!({"name": name, "color": color(c), "description": description}))
                .map_err(|e| format!("label {name} not verified; retry catalog: {e}"))?;
        }
        let readback = self.all("/labels?per_page=100")?;
        for name in names {
            if let Some((color, desc)) = crate::labels::definition(name) {
                if !readback.iter().any(|v| label_matches(v, name, color, &desc)) {
                    return Err(format!("label {name}: readback differs; retry catalog"));
                }
            }
        }
        self.labels_ok.borrow_mut().extend(fresh);
        Ok(())
    }

    /// The labels a new issue needs on a canonical board: the fixed catalog and its own.
    fn creation_labels(&self, labels: &[String]) -> Result<Option<Vec<String>>> {
        if !self.canonical_labels()? { return Ok(None); }
        let mut names = crate::labels::FIXED.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        names.extend_from_slice(labels);
        Ok(Some(names))
    }
}

/// `publish` outcome of a board's attempt: a refusal by the board fails, anything else is pending (§ 7).
fn outcome(r: Result<Publish>) -> Publish {
    match r {
        Ok(p) => p,
        Err(e) if e.contains("HTTP 405") || e.contains("HTTP 409") || e.contains("HTTP 422") => Publish::Failed(e),
        Err(e) => Publish::Pending(e),
    }
}

fn hex40(sha: &str) -> bool {
    sha.len() == 40 && sha.bytes().all(|b| b.is_ascii_hexdigit())
}


pub fn stamp(t: i64) -> String {
    let (d, r) = (t.div_euclid(86400), t.rem_euclid(86400));
    // civil from days (Hinnant)
    let z = d + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{day:02}T{:02}:{:02}:{:02}Z", r / 3600, r % 3600 / 60, r % 60)
}

// ---------- in-memory board ----------

/// A board in memory for tests: one clock tick per write, comment ids shared across issues like real boards.
#[cfg(test)]
#[derive(Default)]
pub struct Fake {
    pub state: RefCell<FakeState>,
}

#[cfg(test)]
#[derive(Default)]
pub struct FakeState {
    pub issues: Vec<Issue>,
    pub comments: Vec<(u64, Comment)>,
    pub clock: i64,
    pub next_id: u64,
    /// `pending` or `failed` makes `publish` refuse; anything else merges.
    pub publish: String,
    pub hide_latest_comment: bool,
    pub fail_issue_read: Option<(u64, usize)>,
    pub fail_labels: bool,
    /// The next this many `comment` calls fail like a board rate-limit pause.
    pub fail_comments: usize,
    /// The next this many `comments` reads fail.
    pub fail_comment_reads: usize,
    /// A concurrent writer: this body lands just before the next `comment`.
    pub race: Option<String>,
    /// Issues of other repositories, `(repo, issue)`.
    pub foreign: Vec<(String, Issue)>,
    /// Issue reads, `foreign` included, for budget checks.
    pub reads: usize,
}

#[cfg(test)]
impl FakeState {
    fn tick(&mut self) -> i64 {
        self.clock += 1;
        self.clock
    }
    fn get(&mut self, n: u64) -> Result<&mut Issue> {
        self.issues.iter_mut().find(|i| i.number == n).ok_or(format!("no issue #{n}"))
    }
}

#[cfg(test)]
impl Fake {
    pub fn advance(&self, secs: i64) {
        self.state.borrow_mut().clock += secs;
    }
    /// A comment by an outsider or an edited one, for trust tests.
    pub fn raw(&self, n: u64, body: &str, trusted: bool, edited: bool) -> u64 {
        let mut st = self.state.borrow_mut();
        st.next_id += 1;
        let id = st.next_id;
        let t = st.tick();
        if let Ok(i) = st.get(n) {
            i.updated = t;
        }
        st.comments.push((n, Comment { id, created: t, body: body.into(), trusted, edited }));
        id
    }
}

#[cfg(test)]
impl Board for Fake {
    fn issues(&self, since: Option<i64>) -> Result<Vec<Issue>> {
        Ok(self.state.borrow().issues.iter().filter(|i| since.map_or(i.open, |t| i.updated >= t)).cloned().collect())
    }
    fn issue(&self, n: u64) -> Result<Issue> {
        let mut st = self.state.borrow_mut();
        if let Some((task, remaining)) = &mut st.fail_issue_read {
            if *task == n {
                *remaining -= 1;
                if *remaining == 0 {
                    st.fail_issue_read = None;
                    return Err("issue read failed".into());
                }
            }
        }
        st.reads += 1;
        drop(st);
        self.state.borrow_mut().get(n).cloned()
    }
    fn foreign(&self, repo: &str, n: u64) -> Result<Issue> {
        let mut st = self.state.borrow_mut();
        st.reads += 1;
        st.foreign.iter().find(|(r, i)| r == repo && i.number == n).map(|x| x.1.clone()).ok_or(format!("HTTP 404 {repo}#{n}"))
    }
    fn foreign_url(&self, repo: &str, n: u64) -> String {
        format!("https://board/{repo}/{n}")
    }
    fn comments(&self, n: u64) -> Result<Vec<Comment>> {
        if self.state.borrow().fail_comment_reads > 0 {
            self.state.borrow_mut().fail_comment_reads -= 1;
            return Err("comment read failed".into());
        }
        let st = self.state.borrow();
        Ok(st.comments.iter().filter(|(i, c)| *i == n && !(st.hide_latest_comment && c.id == st.next_id)).map(|(_, c)| c.clone()).collect())
    }
    fn comment(&self, n: u64, body: &str) -> Result<u64> {
        self.state.borrow_mut().get(n)?;
        if self.state.borrow().fail_comments > 0 {
            self.state.borrow_mut().fail_comments -= 1;
            return Err("rate limit: board requests paused".into());
        }
        let race = self.state.borrow_mut().race.take();
        if let Some(other) = race {
            self.raw(n, &other, true, false);
        }
        Ok(self.raw(n, body, true, false))
    }
    fn edit_comment(&self, n: u64, id: u64, body: &str) -> Result<()> {
        let mut st = self.state.borrow_mut();
        let c = st.comments.iter_mut().find(|(i, c)| *i == n && c.id == id).ok_or(format!("no comment {id}"))?;
        if c.1.body != body {
            c.1.body = body.into();
            c.1.edited = true;
        }
        Ok(())
    }
    fn labels(&self, n: u64, add: &[String], remove: &[String]) -> Result<()> {
        let mut st = self.state.borrow_mut();
        if std::mem::take(&mut st.fail_labels) { return Err("label write failed".into()); }
        let t = st.tick();
        let i = st.get(n)?;
        i.labels.retain(|l| !remove.contains(l));
        for l in add {
            if !i.labels.contains(l) {
                i.labels.push(l.clone());
            }
        }
        i.updated = t;
        Ok(())
    }
    fn create(&self, title: &str, body: &str, labels: &[String]) -> Result<u64> {
        let mut st = self.state.borrow_mut();
        let t = st.tick();
        st.next_id += 1;
        let number = st.issues.len() as u64 + 1;
        st.issues.push(Issue { number, title: title.into(), body: body.into(), labels: labels.to_vec(), assignees: vec![], updated: t, open: true, trusted: true, reason: None });
        Ok(number)
    }
    fn edit(&self, n: u64, body: &str) -> Result<()> {
        let mut st = self.state.borrow_mut();
        let t = st.tick();
        let i = st.get(n)?;
        // like GitHub: an unchanged body keeps `updated_at`
        if i.body != body {
            i.body = body.into();
            i.updated = t;
        }
        Ok(())
    }
    fn close(&self, n: u64) -> Result<()> {
        let mut st = self.state.borrow_mut();
        let t = st.tick();
        let i = st.get(n)?;
        i.open = false;
        i.reason = Some("completed".into());
        i.updated = t;
        Ok(())
    }
    fn now(&self) -> Result<i64> {
        Ok(self.state.borrow().clock)
    }
    fn me(&self) -> Result<String> {
        Ok("owner".into())
    }
    fn publish(&self, _n: u64, sha: &str, _mode: &str, _title: &str, _task_line: &str) -> Publish {
        match self.state.borrow().publish.as_str() {
            "pending" => Publish::Pending("CI pending".into()),
            "failed" => Publish::Failed("CI failed".into()),
            _ => Publish::Done(format!("merged {sha}")),
        }
    }
    fn url(&self, n: u64) -> String {
        format!("https://board/{n}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;


    fn http(kind: &Kind, schema: Option<bool>, transport: Transport) -> Box<dyn Board> {
        let mut api = (kind.api)(if kind.name == "github" { "github.com" } else { "gitlab.example" }, "o/r", "test");
        api.transport = Some(transport);
        *api.label_schema.borrow_mut() = schema;
        (kind.board)(api)
    }
    fn publication_board(kind: &'static Kind, mode: &str, first_merged: bool, second_merged: Option<bool>, ancestry: Option<bool>) -> (Box<dyn Board>, std::rc::Rc<RefCell<Vec<(String, String)>>>) {
        let calls = std::rc::Rc::new(RefCell::new(vec![]));
        let captured = calls.clone();
        let reads = RefCell::new(0);
        let comparisons = RefCell::new(0);
        let direct = mode == "direct";
        let h = http(kind, None, Box::new(move |method, url, _| {
            captured.borrow_mut().push((method.into(), url.into()));
            let result = "a".repeat(40);
            let merged = "b".repeat(40);
            let value = if url.contains("/compare/") || url.contains("/merge_base?") {
                *comparisons.borrow_mut() += 1;
                let initial = *comparisons.borrow() == 1;
                if !initial {
                    assert!(url.ends_with(if direct { &result } else { &merged }), "ancestry must use the published SHA");
                    if ancestry.is_none() { return Err("HTTP 503".into()); }
                }
                let on_main = !initial && ancestry == Some(true);
                if kind.name == "github" { json!({"status": if on_main { "identical" } else { "ahead" }}) }
                else { json!({"id": if on_main { if direct { result } else { merged } } else { "c".repeat(40) }}) }
            } else if url.contains("/check-runs") { json!({"check_runs": [{"status": "completed", "conclusion": "success"}]}) }
            else if url.ends_with("/status") { json!({"total_count": 0}) }
            else if url.contains("/repository/commits/") { json!({"last_pipeline": {"status": "success"}}) }
            else if url.ends_with("/repository/branches/main") { json!({"commit": {"id": "c".repeat(40)}}) }
            else if url.ends_with("/rules/branches/main") { json!([{"type": "required_status_checks", "ruleset_id": 1, "parameters": {"strict_required_status_checks_policy": true}}]) }
            else if url.ends_with("/rulesets/1") { json!({"current_user_can_bypass": "never"}) }
            else if url.ends_with("/projects/o%2Fr") { json!({"merge_method": "ff"}) }
            else if url.contains("/git/ref/heads/taskq-") { json!({"object": {"sha": result}}) }
            else if url.ends_with("/git/refs/heads/main") { json!({"object": {"sha": result}}) }
            else if url.contains("/pulls?") { json!([{"number": 7, "head": {"sha": result}}]) }
            else if url.contains("/merge_requests?") { json!([{"iid": 7, "sha": result}]) }
            else if url.ends_with("/merge") { json!({"sha": merged, "squash_commit_sha": merged}) }
            else if url.ends_with("/pulls/7") || url.ends_with("/merge_requests/7") {
                *reads.borrow_mut() += 1;
                let yes = if *reads.borrow() == 1 { first_merged } else { second_merged.ok_or("HTTP 503")? };
                json!({"merged": yes, "state": if yes { "merged" } else { "opened" }, "merge_commit_sha": merged})
            } else { panic!("unexpected {method} {url}") };
            Ok(Reply { status: 200, headers: vec![], body: value.to_string(), operation: operation(method) })
        }));
        (h, calls)
    }

    // Chain: § 7 publication on both boards. Risk: publication uses the wrong SHA or writes twice.
    #[test]
    fn incident_publication_uses_merge_sha_and_keeps_outcomes() {
        for kind in BOARDS {
            for mode in ["direct", "pr"] {
                for ancestry in [Some(true), Some(false), None] {
                    crate::incident::take_detected();
                    let (h, calls) = publication_board(kind, mode, true, Some(true), ancestry);
                    assert!(matches!(h.publish(1, &"a".repeat(40), mode, "title", "Task: fake"), Publish::Done(_)));
                    assert_eq!(crate::incident::take_detected(), if ancestry == Some(false) { vec!["publication.not_on_main"] } else { vec![] });
                    assert_eq!(calls.borrow().iter().filter(|(method, _)| method != "GET").count(), 1);
                }
            }
        }
    }

    // Chain: § 7 merged, not closed, on both boards. Risk: a close retried after a merge requeues or merges twice;
    // a PR/MR merged into a release branch (#1039) never closes its task.
    #[test]
    fn a_merged_pr_at_the_result_sha_reconciles_to_one_publication() {
        for (kind, base) in BOARDS.iter().flat_map(|k| [(k, "main"), (k, "release/4.1")]) {
            let calls = std::rc::Rc::new(RefCell::new(vec![]));
            let captured = calls.clone();
            let h = http(kind, None, Box::new(move |method, url, _| {
                captured.borrow_mut().push(method.to_string());
                assert!(!url.contains("main&state=closed") && !url.contains("main&state=merged"), "merged lookup must not filter the base: {url}");
                let value = if url.contains("/compare/") { json!({"status": "diverged"}) }
                    else if url.contains("/merge_base?") { json!({"id": "c".repeat(40)}) }
                    else if url.contains("/check-runs") { json!({"check_runs": [{"status": "completed", "conclusion": "success"}]}) }
                    else if url.ends_with("/status") { json!({"total_count": 0}) }
                    else if url.contains("/repository/commits/") { json!({"last_pipeline": {"status": "success"}}) }
                    else if url.contains("state=open") { json!([]) }
                    else if url.contains("state=closed") || url.contains("state=merged") {
                        json!([{"number": 6, "iid": 6, "head": {"sha": "d".repeat(40)}, "sha": "d".repeat(40), "merged_at": "x"},
                               {"number": 7, "iid": 7, "head": {"sha": "a".repeat(40)}, "sha": "a".repeat(40), "merged_at": "x", "merge_commit_sha": "b".repeat(40),
                                "base": {"ref": base}, "target_branch": base}])
                    } else { panic!("unexpected {method} {url}") };
                Ok(Reply { status: 200, headers: vec![], body: value.to_string(), operation: operation(method) })
            }));
            let Publish::Done(m) = h.publish(1, &"a".repeat(40), "pr", "title", "Task: fake") else { panic!("not reconciled") };
            let into = if base == "main" { "" } else { " into release/4.1" };
            assert!(m.starts_with(&if kind.name == "github" { format!("merged #7{into} as bbbb") } else { format!("merged !7{into}") }), "{m}");
            assert!(calls.borrow().iter().all(|m| m == "GET"), "no second merge");
        }
    }

    // Chain: § 7 `pr` publication on both boards (#1139). Risk: CI green on a head that lacks current main merges a
    // combination nobody built, and main stops compiling: refused before the merge, or by the board when main moves
    // between that check and the merge; a main that cannot refuse at merge time, or whose refusal this token can
    // bypass (ruleset bypass actor, classic protection exempting admins), is never merged into.
    #[test]
    fn a_pr_merges_only_onto_the_main_its_ci_built() {
        // (rebased, how main refuses stale merges, main moves before the merge) -> outcome
        let cases = [
            (false, "ruleset:never", false, "Failed"),
            (true, "none", false, "Pending"),
            (true, "ruleset:always", false, "Pending"),
            (true, "ruleset:pull_requests_only", false, "Pending"),
            (true, "classic:admins exempt", false, "Pending"),
            (true, "ruleset:never", true, "Failed"),
            (true, "classic:enforced", true, "Failed"),
        ];
        for (rebased, guard, moved, want) in cases {
            for kind in BOARDS {
                let calls = std::rc::Rc::new(RefCell::new(vec![]));
                let captured = calls.clone();
                let enforced = matches!(guard, "ruleset:never" | "classic:enforced");
                let h = http(kind, None, Box::new(move |method, url, _| {
                    captured.borrow_mut().push(method.to_string());
                    let result = "a".repeat(40);
                    let value = if url.contains("/compare/") { json!({"status": if rebased { "ahead" } else { "diverged" }}) }
                        else if url.contains("/merge_base?") { json!({"id": "c".repeat(40)}) }
                        else if url.ends_with("/repository/branches/main") { json!({"commit": {"id": if rebased { "c" } else { "d" }.repeat(40)}}) }
                        else if url.ends_with("/rules/branches/main") {
                            if guard.starts_with("ruleset:") { json!([{"type": "required_status_checks", "ruleset_id": 1, "parameters": {"strict_required_status_checks_policy": true}}]) } else { json!([]) }
                        }
                        else if url.ends_with("/rulesets/1") { json!({"current_user_can_bypass": guard.trim_start_matches("ruleset:")}) }
                        else if url.ends_with("/branches/main/protection") {
                            if !guard.starts_with("classic:") { return Err("HTTP 403 Resource not accessible by personal access token".into()) }
                            json!({"required_status_checks": {"strict": true}, "enforce_admins": {"enabled": guard == "classic:enforced"}})
                        }
                        else if url.ends_with("/projects/o%2Fr") { json!({"merge_method": if enforced { "ff" } else { "merge" }}) }
                        else if url.contains("/check-runs") { json!({"check_runs": [{"status": "completed", "conclusion": "success"}]}) }
                        else if url.ends_with("/status") { json!({"total_count": 0}) }
                        else if url.contains("/repository/commits/") { json!({"last_pipeline": {"status": "success"}}) }
                        else if url.contains("/pulls?") { json!([{"number": 7, "head": {"sha": result}}]) }
                        else if url.contains("/merge_requests?") { json!([{"iid": 7, "sha": result}]) }
                        else if url.ends_with("/merge") && moved { return Err("HTTP 405 Head branch is out of date".into()) }
                        else { panic!("unexpected {method} {url}") };
                    Ok(Reply { status: 200, headers: vec![], body: value.to_string(), operation: operation(method) })
                }));
                let got = match h.publish(1, &"a".repeat(40), "pr", "title", "Task: fake") {
                    Publish::Failed(m) => { assert!(m.contains(if moved { "HTTP 405" } else { "not rebased on current main" }), "{m}"); "Failed" }
                    Publish::Pending(_) => "Pending",
                    Publish::Done(m) => panic!("merged: {m}"),
                };
                assert_eq!(got, want, "{} rebased={rebased} guard={guard} moved={moved}", kind.name);
                assert_eq!(calls.borrow().iter().filter(|m| *m != "GET").count(), usize::from(moved), "merge only behind the fence");
            }
        }
    }

    // Chain: § 7 `pr` publication of a draft PR. Risk: GitHub refuses to merge a draft (HTTP 405), so close never publishes.
    #[test]
    fn a_draft_pr_is_marked_ready_then_merged_at_the_result_sha() {
        for undrafted in [true, false] {
            let writes = std::rc::Rc::new(RefCell::new(vec![]));
            let captured = writes.clone();
            let h = http(&github::KIND, None, Box::new(move |method, url, body| {
                let (result, merged) = ("a".repeat(40), "b".repeat(40));
                if method != "GET" { captured.borrow_mut().push(url.rsplit_once(".com").map_or(url, |x| x.1).to_string()); }
                let value = if url.contains("/compare/") { json!({"status": "ahead"}) }
                    else if url.contains("/check-runs") { json!({"check_runs": [{"status": "completed", "conclusion": "success"}]}) }
                    else if url.ends_with("/status") { json!({"total_count": 0}) }
                    else if url.ends_with("/rules/branches/main") { json!([{"type": "required_status_checks", "ruleset_id": 1, "parameters": {"strict_required_status_checks_policy": true}}]) }
                    else if url.ends_with("/rulesets/1") { json!({"current_user_can_bypass": "never"}) }
                    else if url.contains("/pulls?") { json!([{"number": 7, "node_id": "PR_7", "draft": true, "head": {"sha": result}}]) }
                    else if url == "https://api.github.com/graphql" {
                        assert_eq!(body.unwrap()["variables"]["id"], "PR_7");
                        if undrafted { json!({"data": {"markPullRequestReadyForReview": {"pullRequest": {"isDraft": false}}}}) }
                        else { json!({"data": null, "errors": [{"message": "forbidden"}]}) }
                    }
                    else if url.ends_with("/pulls/7/merge") { assert_eq!(body.unwrap()["sha"], result); json!({"sha": merged}) }
                    else if url.ends_with("/pulls/7") { json!({"merged": true, "merge_commit_sha": merged}) }
                    else { panic!("unexpected {method} {url}") };
                Ok(Reply { status: 200, headers: vec![], body: value.to_string(), operation: operation(method) })
            }));
            let out = h.publish(1, &"a".repeat(40), "pr", "title", "Task: fake");
            if undrafted {
                assert!(matches!(&out, Publish::Done(m) if m.starts_with("merged #7 as bbbb")));
                assert_eq!(*writes.borrow(), ["/graphql", "/repos/o/r/pulls/7/merge"], "ready before merge");
            } else {
                assert!(matches!(&out, Publish::Pending(m) if m.contains("still a draft")));
                assert_eq!(*writes.borrow(), ["/graphql"], "no merge of a draft");
            }
        }
    }

    // Chain: § 7 publication readback. Risk: one flaky read reports a lost publication.
    #[test]
    fn incident_publication_readback_requires_two_negative_reads() {
        for kind in BOARDS {
            for second in [Some(false), Some(true), None] {
                crate::incident::take_detected();
                let (h, calls) = publication_board(kind, "pr", false, second, Some(true));
                assert!(matches!(h.publish(1, &"a".repeat(40), "pr", "title", "Task: fake"), Publish::Pending(_)), "reporting preserves first read outcome");
                assert_eq!(crate::incident::take_detected(), if second == Some(false) { vec!["publication.readback"] } else { vec![] });
                assert_eq!(calls.borrow().iter().filter(|(m, u)| m == "GET" && (u.ends_with("/pulls/7") || u.ends_with("/merge_requests/7"))).count(), 2);
                assert_eq!(calls.borrow().iter().filter(|(method, _)| method != "GET").count(), 1);
            }
        }
    }

    // Chain: incident adapter.bad_json. Risk: HTTP failures reported as bad JSON.
    #[test]
    fn incident_bad_json_is_only_a_successful_response() {
        for fail in [false, true] {
            crate::incident::take_detected();
            let mut h = (github::KIND.api)("github.com", "o/r", "fake");
            h.transport = Some(Box::new(move |_, _, _| if fail { Err("HTTP 503 malformed response".into()) }
                else { Ok(Reply { status: 200, headers: vec![], body: "private invalid JSON".into(), operation: "read" }) }));
            assert!(h.call("GET", "/issues", None).and_then(|r| r.json()).is_err());
            assert_eq!(crate::incident::take_detected(), if fail { vec![] } else { vec!["adapter.bad_json"] });
        }
    }

    // Chain: § 1 `Depends on <repo>#N` adapters (#1126). Risk: a wrong repository path or a lost close reason makes a
    // dependency of another repository never or wrongly done.
    #[test]
    fn foreign_dependency_reads_sibling_repositories() {
        for (kind, body, api, web, reason) in [
            (&github::KIND, r#"{"number":36,"state":"closed","state_reason":"not_planned"}"#, "https://api.github.com/repos/o/dashq/issues/36",
                "https://github.com/o/dashq/issues/36", Some("not_planned")),
            (&gitlab::KIND, r#"{"iid":36,"state":"closed"}"#, "https://gitlab.example/api/v4/projects/o%2Fdashq/issues/36",
                "https://gitlab.example/o/dashq/-/issues/36", None),
        ] {
            let urls = std::rc::Rc::new(RefCell::new(vec![]));
            let seen = urls.clone();
            let b = http(kind, Some(true), Box::new(move |_, url, _| {
                seen.borrow_mut().push(url.to_string());
                Ok(Reply { status: 200, headers: vec![], body: body.into(), operation: "read" })
            }));
            let i = b.foreign("dashq", 36).unwrap();
            assert_eq!((i.number, i.open, i.reason.as_deref()), (36, false, reason));
            assert_eq!((urls.borrow()[0].as_str(), b.foreign_url("dashq", 36)), (api, web.to_string()));
            assert_eq!(b.foreign_url("x/y", 1).replace("x/y", "o/r"), b.url(1), "a full path is used as written");
        }
        // R12: a reply that is not that issue in a known state stays unknown, never done
        for (kind, body) in [(&github::KIND, "{}"), (&github::KIND, r#"{"number":37,"state":"closed"}"#), (&github::KIND, r#"{"number":36,"state":"gone"}"#),
            (&gitlab::KIND, "{}"), (&gitlab::KIND, r#"{"iid":36}"#), (&gitlab::KIND, r#"{"iid":36,"state":"open"}"#)] {
            let b = http(kind, Some(true), Box::new(move |_, _, _| Ok(Reply { status: 200, headers: vec![], body: body.into(), operation: "read" })));
            assert_eq!(b.foreign("dashq", 36).unwrap_err(), "malformed reply for issue 36", "{body}");
        }
    }

    // Chain: A3 forge response -> profile report. Risk: HTTP/GraphQL failures count as successes or pauses send again.
    #[test]
    fn forge_outcomes_reach_profile_report() {
        crate::profile::take_report();
        for (status, body, limited) in [(200, "{}", false), (204, "", false), (404, "private response", false),
            (503, "private response", false), (200, "{\"errors\":[{\"type\":\"RATE_LIMITED\"}]}", true)] {
            let mut api = (github::KIND.api)("github.com", "o/r", "private token");
            api.transport = Some(Box::new(move |_, _, _| Ok(Reply { status, body: body.into(), operation: "read",
                headers: if limited { vec![("retry-after".into(), "60".into())] } else { vec![] } })));
            assert_eq!(api.call("POST", "https://api.github.com/graphql", None).is_ok(), status < 300 && !limited);
            if limited {
                assert!(api.call("POST", "https://api.github.com/graphql", None).err().unwrap().contains("paused until"));
                assert_eq!(api.budget.borrow().total(), 1);
            }
        }
        let report = crate::profile::take_report();
        assert!(report.contains("| forge.call | 5 |"), "{report}");
        assert!(report.contains("| 2/3 | 60.00 |"), "{report}");
        assert!(report.contains("http4xx:2, http5xx:1"), "{report}");
        assert!(!report.contains("private"));
    }

    fn label_board(kind: &'static Kind, fail: bool, mismatch: bool) -> (Box<dyn Board>, std::rc::Rc<RefCell<Vec<Value>>>, std::rc::Rc<RefCell<usize>>) {
        let catalog = std::rc::Rc::new(RefCell::new(vec![json!({"id": 99, "name": "bug", "color": "FF0000", "description": "user owned"})]));
        let writes = std::rc::Rc::new(RefCell::new(0));
        let (data, count) = (catalog.clone(), writes.clone());
        let h = http(kind, None, Box::new(move |method, url, body| {
            assert!(url.contains("/labels"));
            if method == "GET" { return Ok(Reply { status: 200, headers: vec![], operation: "read", body: json!(*data.borrow()).to_string() }); }
            assert!(matches!(method, "POST" | "PATCH" | "PUT"));
            if method == "POST" { assert!(url.ends_with("/labels")); }
            else if kind.name == "gitlab" { assert_eq!(method, "PUT"); assert!(url.rsplit('/').next().unwrap().parse::<u64>().is_ok()); }
            else { assert_eq!(method, "PATCH"); }
            assert_eq!(body.as_ref().unwrap()["color"].as_str().unwrap().starts_with('#'), kind.name == "gitlab");
            *count.borrow_mut() += 1;
            if fail && *count.borrow() == 2 { return Err("HTTP 403 denied".into()); }
            let mut v = body.unwrap();
            if mismatch { v["color"] = json!("000000"); }
            let mut all = data.borrow_mut();
            if let Some(old) = all.iter_mut().find(|old| old["name"] == v["name"]) {
                v["id"] = old["id"].clone(); *old = v;
            } else { v["id"] = json!(all.len() + 100); all.push(v); }
            Ok(Reply { status: 200, headers: vec![], operation: "read", body: "{}".into() })
        }));
        (h, catalog, writes)
    }

    // Chain: § 6 label setup on both boards. Risk: repeat writes or user label metadata overwritten.
    #[test]
    fn label_adapters_readback_idempotence_and_preservation() {
        for kind in BOARDS {
            let (h, catalog, writes) = label_board(kind, false, false);
            let mut names = crate::labels::FIXED.iter().chain(["host-win", "needs-gpu", "area-ui"].iter()).map(|s| s.to_string()).collect::<Vec<_>>();
            names.push("taskq".into());
            h.ensure_labels(&names).unwrap();
            assert_eq!(*writes.borrow(), names.len() - 1);
            let reads = h.requests().total();
            h.ensure_labels(&names).unwrap();
            assert_eq!(*writes.borrow(), names.len() - 1, "repeat makes no writes");
            assert_eq!(h.requests().total(), reads, "repeat sends no catalog read");
            assert_eq!(catalog.borrow()[0]["description"], "user owned");
            let i = catalog.borrow().iter().position(|v| v["name"] == "q-ask").unwrap();
            catalog.borrow_mut()[i]["color"] = json!("000000");
            h.ensure_labels(&names).unwrap();
            assert_eq!(*writes.borrow(), names.len() - 1, "verified once per process (§ 4 budget)");
            let (fresh, fresh_catalog, fresh_writes) = label_board(kind, false, false);
            *fresh_catalog.borrow_mut() = catalog.borrow().clone();
            fresh.ensure_labels(&names).unwrap();
            assert_eq!(*fresh_writes.borrow(), 1, "a new process repairs existing metadata");
        }
    }

    // Chain: § 9 label cutover gate. Risk: partial label writes or legacy board relabelled early.
    #[test]
    fn label_adapter_failures_and_legacy_gate() {
        for kind in BOARDS {
            let names = vec!["q-doing".into(), "q-ask".into()];
            let (h, _, writes) = label_board(kind, true, false);
            assert!(h.ensure_labels(&names).unwrap_err().contains("403"));
            h.ensure_labels(&names).unwrap();
            assert_eq!(*writes.borrow(), 3, "safe retry only writes missing label");
            let (h, _, _) = label_board(kind, false, true);
            assert!(h.ensure_labels(&names).unwrap_err().contains("readback differs"));
            let (h, catalog, writes) = label_board(kind, false, false);
            catalog.borrow_mut().push(json!({"name":"q-waiting"}));
            assert!(!h.canonical_labels().unwrap());
            assert_eq!(h.state_label("waiting").unwrap(), "q-waiting");
            assert!(h.ensure_labels(&names).unwrap_err().contains("all-host gate"));
            assert_eq!(*writes.borrow(), 0);
            let h = http(kind, Some(true), Box::new(|_, url, _| Err(format!("unexpected {url}"))));
            assert_eq!(h.state_label("waiting").unwrap(), "q-blocked");
            assert!(label_schema(Some("typo")).is_err());
        }
    }

    // Chain: § 4 board budget on both boards. Risk: an exhausted budget keeps sending the heartbeat every pass, or a
    // multi-line error body splits a log line.
    #[test]
    fn exhausted_budget_sends_nothing_until_reset() {
        for kind in BOARDS {
            let sent = std::rc::Rc::new(RefCell::new(0));
            let count = sent.clone();
            let reset = iso("2026-10-14T04:41:00Z").unwrap().to_string();
            let h = http(kind, Some(true), Box::new(move |_, url, _| {
                *count.borrow_mut() += 1;
                let date = ("Date".to_string(), "Wed, 14 Oct 2026 04:27:00 GMT".to_string());
                if url.ends_with("/issues/6") {
                    return Ok(Reply { status: 500, headers: vec![date], body: "<html>\n  <p>down</p>\n</html>".into(), operation: "update" });
                }
                let headers = vec![date, ("X-RateLimit-Remaining".into(), "0".into()), ("X-RateLimit-Reset".into(), reset.clone())];
                Ok(Reply { status: 403, headers, body: "{\"message\": \"API rate limit exceeded\"}".into(), operation: "update" })
            }));
            assert!(h.edit(6, "beat").unwrap_err().ends_with("HTTP 500 <html> <p>down</p> </html>"));
            let first = h.edit(5, "beat").unwrap_err();
            assert!(first.contains("HTTP 403 rate limit: board requests paused until 2026-10-14T04:41:00Z"), "{first}");
            let again = h.edit(5, "beat").unwrap_err();
            assert!(again.ends_with("rate limit: board requests paused until 2026-10-14T04:41:00Z"), "{again}");
            assert_eq!(*sent.borrow(), 2, "nothing is sent while paused");
            let b = h.requests();
            assert_eq!((b.total(), b.remaining, b.paused_until()), (2, Some(0), iso("2026-10-14T04:41:00Z")));
        }
    }

    // Chain: § 4 two project adapters -> shared host/token pause. Risk: another project keeps sending after 429,
    // or a different credential inherits that pause. Fake transport proves it without board writes.
    #[test]
    fn host_project_adapters_share_budget_and_pause_by_credential() {
        std::thread::spawn(|| {
            crate::host::enter(std::path::Path::new("invented-root"), std::path::Path::new("invented-install"));
            let sent = std::rc::Rc::new(RefCell::new(0));
            let make = |token: &str, status: u16| {
                let mut api = (github::KIND.api)("github.com", "invented/project", token);
                let sent = sent.clone();
                api.transport = Some(Box::new(move |_, _, _| {
                    *sent.borrow_mut() += 1;
                    Ok(Reply { status, headers: vec![("retry-after".into(), "60".into())], body: "{}".into(), operation: "read" })
                })); api
            };
            let a = make("invented-host-budget-token", 429);
            let b = make("invented-host-budget-token", 200);
            let different = make("invented-other-budget-token", 200);
            assert!(a.call("GET", "/issues", None).err().unwrap().contains("paused"));
            assert!(b.call("GET", "/issues", None).err().unwrap().contains("paused"));
            assert_eq!(*sent.borrow(), 1); assert_eq!(b.budget.borrow().total(), 1);
            assert!(different.call("GET", "/issues", None).is_ok()); assert_eq!(*sent.borrow(), 2);
        }).join().unwrap();
    }

    // Chain: § 4 board budget, GitHub open list over GraphQL. Risk: the list loses a page, a field or trust, falls back
    // to core REST, or keeps sending after a GraphQL RATE_LIMITED reply.
    #[test]
    fn github_open_list_is_one_graphql_request_per_page() {
        let kind = BOARDS.iter().find(|k| k.name == "github").unwrap();
        let limited = std::rc::Rc::new(RefCell::new(false));
        let flag = limited.clone();
        let h = http(kind, Some(true), Box::new(move |method, url, body| {
            assert_eq!((method, url), ("POST", "https://api.github.com/graphql"));
            let body = body.unwrap();
            // labels are the only nested connection: assignees would double the points (#1043)
            assert!(!body["query"].as_str().unwrap().contains("assignees"));
            assert_eq!((&body["variables"]["owner"], &body["variables"]["name"]), (&json!("o"), &json!("r")));
            let left = if *flag.borrow() { "0" } else { "4990" };
            let headers = vec![("X-RateLimit-Resource".into(), "graphql".into()), ("X-RateLimit-Remaining".into(), left.into()),
                ("X-RateLimit-Limit".into(), "5000".into()), ("X-RateLimit-Reset".into(), "1791964800".into())];
            let value = if *flag.borrow() { json!({"errors": [{"type": "RATE_LIMITED", "message": "API rate limit exceeded"}]}) }
                else if body["variables"]["after"].is_null() { json!({"data": {"repository": {"issues": {"pageInfo": {"hasNextPage": true, "endCursor": "c1"},
                    "nodes": [{"number": 5, "title": "t", "body": "b", "updatedAt": "2026-10-14T04:27:00Z", "authorAssociation": "OWNER",
                        "labels": {"nodes": [{"name": "q-ready"}]}}]}}}}) }
                else { json!({"data": {"repository": {"issues": {"pageInfo": {"hasNextPage": false, "endCursor": null},
                    "nodes": [{"number": 6, "title": "u", "body": null, "updatedAt": "2026-10-14T04:28:00Z", "authorAssociation": "NONE",
                        "labels": {"nodes": []}}]}}}}) };
            Ok(Reply { status: 200, headers, body: value.to_string(), operation: operation(method) })
        }));
        let list = h.issues(None).unwrap();
        assert_eq!(list.iter().map(|i| (i.number, i.open, i.trusted)).collect::<Vec<_>>(), [(5, true, true), (6, true, false)]);
        assert_eq!((list[0].labels.clone(), list[0].assignees.clone(), list[0].updated), (vec!["q-ready".to_string()], vec![], iso("2026-10-14T04:27:00Z").unwrap()));
        let b = h.requests();
        assert_eq!((b.ops.clone(), b.resource.clone(), b.remaining), (std::collections::BTreeMap::from([("POST graphql".to_string(), 2)]), Some("graphql".into()), Some(4990)));
        *limited.borrow_mut() = true;
        assert!(h.issues(None).unwrap_err().contains("HTTP 403 rate limit: board requests paused until"));
        assert!(h.issues(None).unwrap_err().contains("paused"));
        assert_eq!(h.requests().total(), 3, "nothing is sent while paused");
    }

    // Chain: § 2 state labels on both boards. Risk: a state change removes user labels.
    #[test]
    fn issue_label_adapters_preserve_unrelated_assignments() {
        for kind in BOARDS {
            let labels = std::rc::Rc::new(RefCell::new(vec!["bug".to_string(), "q-ready".to_string()]));
            let data = labels.clone();
            let h = http(kind, Some(false), Box::new(move |method, url, body| {
                let reply = |body: Value| Ok(Reply { status: 200, headers: vec![], operation: "read", body: body.to_string() });
                if url.contains("/members/") { return reply(json!({"access_level": 20})); }
                if method == "GET" {
                    assert!(url.ends_with("/issues/7"));
                    return reply(json!({"number": 7, "iid": 7, "state": "open", "author_association": "OWNER", "labels": *data.borrow()}));
                }
                if method == "POST" && url.ends_with("/issues") {
                    let body = body.unwrap();
                    *data.borrow_mut() = if kind.name == "gitlab" { body["labels"].as_str().unwrap().split(',').map(String::from).collect() }
                        else { names(&body["labels"], "name") };
                    return reply(json!({"number": 7, "iid": 7}));
                }
                if kind.name == "gitlab" {
                    assert_eq!(method, "PUT"); assert!(url.ends_with("/issues/7"));
                    let body = body.unwrap();
                    let remove = body["remove_labels"].as_str().unwrap();
                    data.borrow_mut().retain(|l| !remove.split(',').any(|r| r == l));
                    for label in body["add_labels"].as_str().unwrap().split(',').filter(|s| !s.is_empty()) { data.borrow_mut().push(label.into()); }
                } else if method == "POST" {
                    assert!(url.ends_with("/issues/7/labels"));
                    data.borrow_mut().extend(names(&body.unwrap()["labels"], "name"));
                } else {
                    assert_eq!(method, "DELETE"); assert!(url.ends_with("/issues/7/labels/q-ready"));
                    data.borrow_mut().retain(|l| l != "q-ready");
                }
                reply(json!({}))
            }));
            h.labels(7, &["q-doing".into()], &["q-ready".into()]).unwrap();
            assert_eq!(*labels.borrow(), vec!["bug", "q-doing"]);
            h.create("new", "", &["taskq".into(), "q-ready".into()]).unwrap();
            assert_eq!(*labels.borrow(), vec!["taskq", "q-ready"]);
        }
    }

    // Chain: R12 label readback. Risk: a partial label change reported as applied.
    #[test]
    fn assignment_readback_rejects_partial_application() {
        let b = Fake::default();
        let n = b.create("t", "", &["bug".into(), "q-ready".into()]).unwrap();
        assert!(verify_assignment(&b, n, &["q-doing".into()], &["q-ready".into()]).is_err());
        b.labels(n, &["q-doing".into()], &["q-ready".into()]).unwrap();
        verify_assignment(&b, n, &["q-doing".into()], &["q-ready".into()]).unwrap();
        assert!(b.issue(n).unwrap().labels.contains(&"bug".into()));
    }

    // Chain: § 3 lease time from board stamps. Risk: board dates parsed wrong, leases expire wrong.
    #[test]
    fn clocks() {
        assert_eq!(iso("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(iso("2026-10-12T10:20:30.123Z"), Some(1791800430));
        assert_eq!(http_date("Mon, 12 Oct 2026 10:20:30 GMT"), Some(1791800430));
        assert_eq!(stamp(1791800430), "2026-10-12T10:20:30Z");
    }

    // Chain: R1 board entries to fold. Risk: untrusted or edited comments move the queue.
    #[test]
    fn untrusted_and_edited_entries_never_reach_fold() {
        let b = Fake::default();
        let n = b.create("t", "", &[]).unwrap();
        b.raw(n, "taskq: spawn worker claude:x evil", false, false);
        b.raw(n, "1", false, false);
        b.raw(n, "taskq: later", true, true);
        let e = entries(&b, n).unwrap();
        assert_eq!(e.len(), 1);
        assert_eq!(crate::core::fold(0, &e, false).state, crate::core::State::Ready);
    }

    // Chain: board token boundary. Risk: the token is sent to another host.
    #[test]
    fn token_goes_only_to_the_board_api() {
        let h = (github::KIND.api)("github.com", "o/r", "t");
        for url in ["https://evil.example/x", "https://api.github.com.evil.example/x"] {
            assert!(h.call("GET", url, None).err().is_some_and(|e| e.starts_with("refused")), "{url}");
        }
    }
}
