//! Board adapters (taskq.md § 1, § 2): GitHub and GitLab issues through the OS `curl`, and an in-memory board for
//! tests. Every write is a native board object; nothing else is state.

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
    /// Open issues; with `since` (board seconds) every issue, closed too, updated at or after it.
    fn issues(&self, since: Option<i64>) -> Result<Vec<Issue>>;
    fn issue(&self, n: u64) -> Result<Issue>;
    fn comments(&self, n: u64) -> Result<Vec<Comment>>;
    fn comment(&self, n: u64, body: &str) -> Result<u64>;
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
    headers: Vec<(String, String)>,
    body: String,
}

impl Reply {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
    fn json(&self) -> Result<Value> {
        serde_json::from_str(&self.body).map_err(|e| format!("bad JSON from board: {e}"))
    }
    fn next(&self) -> Option<String> {
        // Link: <url>; rel="next", <url>; rel="last"
        self.header("link")?.split(',').find(|p| p.contains("rel=\"next\""))
            .and_then(|p| Some(p.split_once('<')?.1.split_once('>')?.0.to_string()))
    }
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

/// One request. The token and body go through curl's config on stdin, never argv (R12: no secrets in `ps`).
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
        let reply = Reply { headers, body: body.to_string() };
        if !(200..300).contains(&status) {
            return Err(format!("{method} {url}: HTTP {status} {}", reply.body.chars().take(300).collect::<String>()));
        }
        return Ok(reply);
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    GitHub,
    GitLab,
}

pub struct Http {
    kind: Kind,
    api: String,
    auth: String,
    me: RefCell<Option<String>>,
    clock: RefCell<i64>,
    members: RefCell<Vec<(u64, bool)>>,
}

fn s(v: &Value, k: &str) -> String {
    v[k].as_str().unwrap_or_default().to_string()
}

fn names(v: &Value, k: &str) -> Vec<String> {
    v.as_array().into_iter().flatten().map(|x| x.as_str().map(String::from).unwrap_or_else(|| s(x, k))).collect()
}

fn trusted(v: &Value) -> bool {
    matches!(v["author_association"].as_str(), Some("OWNER" | "MEMBER" | "COLLABORATOR"))
}

fn csv(v: &[String]) -> String {
    v.join(",")
}

impl Http {
    /// `repo` is `owner/name`; `host` is the web host (`github.com` or a GitLab host).
    pub fn new(kind: Kind, host: &str, repo: &str, token: &str) -> Http {
        let (api, auth) = match kind {
            Kind::GitHub if host == "github.com" || host.is_empty() => (format!("https://api.github.com/repos/{repo}"), format!("Authorization: Bearer {token}")),
            Kind::GitHub => (format!("https://{host}/api/v3/repos/{repo}"), format!("Authorization: Bearer {token}")),
            Kind::GitLab => (format!("https://{host}/api/v4/projects/{}", repo.replace('/', "%2F")), format!("PRIVATE-TOKEN: {token}")),
        };
        Http { kind, api, auth, me: RefCell::new(None), clock: RefCell::new(0), members: RefCell::new(vec![]) }
    }

    fn call(&self, method: &str, url: &str, body: Option<Value>) -> Result<Reply> {
        // absolute URLs come from pagination headers: the token goes only to the configured API
        let url = if url.starts_with("https://") { url.to_string() } else { format!("{}{url}", self.api) };
        if !url.starts_with(&format!("{}/", self.root())) {
            return Err(format!("refused: {url} is outside the board API"));
        }
        let r = curl(method, &url, &self.auth, body.as_ref())?;
        if let Some(t) = r.header("date").and_then(http_date) {
            *self.clock.borrow_mut() = t;
        }
        Ok(r)
    }

    fn all(&self, url: &str) -> Result<Vec<Value>> {
        let mut out = vec![];
        let mut next = Some(url.to_string());
        while let Some(u) = next {
            let r = self.call("GET", &u, None)?;
            out.extend(r.json()?.as_array().cloned().unwrap_or_default());
            next = r.next();
        }
        Ok(out)
    }

    fn root(&self) -> &str {
        // https://host/api/v4/projects/x -> https://host/api/v4
        self.api.split_once("/projects/").map(|(a, _)| a).or_else(|| self.api.split_once("/repos/").map(|(a, _)| a)).unwrap_or(&self.api)
    }

    fn parse_issue(&self, v: &Value) -> Issue {
        let gl = self.kind == Kind::GitLab;
        Issue {
            number: v[if gl { "iid" } else { "number" }].as_u64().unwrap_or(0),
            title: s(v, "title"),
            body: s(v, if gl { "description" } else { "body" }),
            labels: names(&v["labels"], "name"),
            assignees: names(&v["assignees"], if gl { "username" } else { "login" }),
            updated: iso(&s(v, "updated_at")).unwrap_or(0),
            open: matches!(v["state"].as_str(), Some("open" | "opened")),
            trusted: if gl { self.member(v["author"]["id"].as_u64().unwrap_or(0)) } else { trusted(&v) },
        }
    }

    /// GitLab Reporter+ (access level 20), cached per author id.
    fn member(&self, user: u64) -> bool {
        if let Some((_, ok)) = self.members.borrow().iter().find(|(u, _)| *u == user) {
            return *ok;
        }
        let ok = self.call("GET", &format!("/members/all/{user}"), None).ok().and_then(|r| r.json().ok())
            .is_some_and(|v| v["access_level"].as_u64().unwrap_or(0) >= 20);
        self.members.borrow_mut().push((user, ok));
        ok
    }

    /// CI on `sha`: Ok(None) green, Ok(Some(msg)) failed, Err pending or missing.
    fn ci(&self, sha: &str) -> Result<Option<String>> {
        if self.kind == Kind::GitLab {
            let v = self.call("GET", &format!("/repository/commits/{sha}"), None)?.json()?;
            return match v["last_pipeline"]["status"].as_str() {
                Some("success") => Ok(None),
                Some(st @ ("failed" | "canceled")) => Ok(Some(format!("CI {st} on {sha}"))),
                Some(st) => Err(format!("CI {st} on {sha}")),
                None => Err(format!("no CI on {sha}")),
            };
        }
        let runs = self.call("GET", &format!("/commits/{sha}/check-runs?per_page=100"), None)?.json()?;
        let runs = runs["check_runs"].as_array().cloned().unwrap_or_default();
        let status = self.call("GET", &format!("/commits/{sha}/status"), None)?.json()?;
        let statuses = status["total_count"].as_u64().unwrap_or(0);
        if runs.is_empty() && statuses == 0 {
            return Err(format!("no CI on {sha}"));
        }
        if let Some(r) = runs.iter().find(|r| r["status"] != "completed") {
            return Err(format!("CI {} pending on {sha}", s(r, "name")));
        }
        if let Some(r) = runs.iter().find(|r| !matches!(r["conclusion"].as_str(), Some("success" | "neutral" | "skipped"))) {
            return Ok(Some(format!("CI {} {} on {sha}", s(r, "name"), s(r, "conclusion"))));
        }
        match status["state"].as_str() {
            _ if statuses == 0 => Ok(None),
            Some("success") => Ok(None),
            Some("pending") => Err(format!("CI pending on {sha}")),
            st => Ok(Some(format!("CI {} on {sha}", st.unwrap_or("unknown")))),
        }
    }

    fn try_publish(&self, n: u64, sha: &str, mode: &str, title: &str, task_line: &str) -> Result<Publish> {
        let gl = self.kind == Kind::GitLab;
        let branch = format!("taskq-{n}");
        // No commit, or the SHA is already on main: verify ancestry, close.
        let on_main = if gl {
            self.call("GET", &format!("/repository/merge_base?refs[]=main&refs[]={sha}"), None)?.json()?["id"] == sha
        } else {
            matches!(self.call("GET", &format!("/compare/main...{sha}"), None)?.json()?["status"].as_str(), Some("identical" | "behind"))
        };
        if on_main {
            return Ok(Publish::Done(format!("{sha} already on main")));
        }
        if let Some(fail) = self.ci(sha)? {
            return Ok(Publish::Failed(fail));
        }
        if gl {
            let mrs = self.call("GET", &format!("/merge_requests?source_branch={branch}&target_branch=main&state=opened"), None)?.json()?;
            let mrs = mrs.as_array().cloned().unwrap_or_default();
            let mr = match (mrs.len(), mode) {
                (1, _) => mrs[0].clone(),
                (0, "direct") => self.call("POST", "/merge_requests", Some(json!({ "source_branch": branch, "target_branch": "main", "title": title })))?.json()?,
                (k, _) => return Ok(Publish::Failed(format!("{k} open MRs of {branch}, want exactly one"))),
            };
            if mr["sha"] != sha {
                return Ok(Publish::Failed(format!("MR head {} is not the result {sha}", s(&mr, "sha"))));
            }
            let iid = mr["iid"].as_u64().unwrap_or(0);
            let body = if mode == "pr" { json!({ "sha": sha, "squash": true, "squash_commit_message": format!("{title}\n\n{task_line}") }) } else { json!({ "sha": sha }) };
            self.call("PUT", &format!("/merge_requests/{iid}/merge"), Some(body))?;
            let back = self.call("GET", &format!("/merge_requests/{iid}"), None)?.json()?;
            return if back["state"] == "merged" { Ok(Publish::Done(format!("merged !{iid}"))) } else { Err(format!("!{iid} not merged yet")) };
        }
        if mode == "direct" {
            let head = self.call("GET", &format!("/git/ref/heads/{branch}"), None)?.json()?;
            if head["object"]["sha"] != sha {
                return Ok(Publish::Failed(format!("{branch} is not at the result {sha}")));
            }
            self.call("PATCH", "/git/refs/heads/main", Some(json!({ "sha": sha, "force": false })))?;
            return Ok(Publish::Done(format!("main fast-forwarded to {sha}")));
        }
        let owner = self.api.rsplit_once("/repos/").map(|x| x.1).and_then(|r| r.split('/').next()).unwrap_or_default().to_string();
        let prs = self.call("GET", &format!("/pulls?head={owner}:{branch}&base=main&state=open"), None)?.json()?;
        let prs = prs.as_array().cloned().unwrap_or_default();
        if prs.len() != 1 {
            return Ok(Publish::Failed(format!("{} open PRs of {branch}, want exactly one", prs.len())));
        }
        if prs[0]["head"]["sha"] != sha {
            return Ok(Publish::Failed(format!("PR head is not the result {sha}")));
        }
        let p = prs[0]["number"].as_u64().unwrap_or(0);
        self.call("PUT", &format!("/pulls/{p}/merge"), Some(json!({ "merge_method": "squash", "sha": sha, "commit_title": format!("{title} (#{p})"), "commit_message": task_line })))?;
        let back = self.call("GET", &format!("/pulls/{p}"), None)?.json()?;
        if back["merged"] == true { Ok(Publish::Done(format!("merged #{p} as {}", s(&back, "merge_commit_sha")))) } else { Err(format!("#{p} not merged yet")) }
    }
}

impl Board for Http {
    fn issues(&self, since: Option<i64>) -> Result<Vec<Issue>> {
        let gl = self.kind == Kind::GitLab;
        let state = match (since, gl) { (Some(_), true) => "all", (Some(_), false) => "all", (None, true) => "opened", (None, false) => "open" };
        let mut url = format!("/issues?state={state}&per_page=100");
        if let Some(t) = since {
            url += &format!("&{}={}", if gl { "updated_after" } else { "since" }, stamp(t));
        }
        Ok(self.all(&url)?.iter().filter(|v| v.get("pull_request").is_none()).map(|v| self.parse_issue(v)).collect())
    }

    fn issue(&self, n: u64) -> Result<Issue> {
        Ok(self.parse_issue(&self.call("GET", &format!("/issues/{n}"), None)?.json()?))
    }

    fn comments(&self, n: u64) -> Result<Vec<Comment>> {
        let gl = self.kind == Kind::GitLab;
        let url = if gl { format!("/issues/{n}/notes?sort=asc&order_by=created_at&per_page=100") } else { format!("/issues/{n}/comments?per_page=100") };
        let mut out = vec![];
        for v in self.all(&url)? {
            if gl && v["system"].as_bool() == Some(true) {
                continue;
            }
            let trusted = if gl {
                self.member(v["author"]["id"].as_u64().unwrap_or(0))
            } else {
                trusted(&v)
            };
            let created = iso(&s(&v, "created_at")).unwrap_or(0);
            out.push(Comment { id: v["id"].as_u64().unwrap_or(0), created, body: s(&v, "body"), trusted, edited: s(&v, "updated_at") != s(&v, "created_at") });
        }
        Ok(out)
    }

    fn comment(&self, n: u64, body: &str) -> Result<u64> {
        let path = if self.kind == Kind::GitLab { "notes" } else { "comments" };
        self.call("POST", &format!("/issues/{n}/{path}"), Some(json!({ "body": body })))?.json()?["id"].as_u64().ok_or("no comment id".into())
    }

    fn labels(&self, n: u64, add: &[String], remove: &[String]) -> Result<()> {
        if self.kind == Kind::GitLab {
            self.call("PUT", &format!("/issues/{n}"), Some(json!({ "add_labels": csv(add), "remove_labels": csv(remove) })))?;
            return Ok(());
        }
        if !add.is_empty() {
            self.call("POST", &format!("/issues/{n}/labels"), Some(json!({ "labels": add })))?;
        }
        for l in remove {
            // percent-encode: a label like `q-/../..` must not walk the API path
            let enc: String = l.bytes().map(|c| if c.is_ascii_alphanumeric() || b"-_.~".contains(&c) { (c as char).to_string() } else { format!("%{c:02X}") }).collect();
            match self.call("DELETE", &format!("/issues/{n}/labels/{enc}"), None) {
                Err(e) if e.contains("HTTP 404") => {}
                r => {
                    r?;
                }
            }
        }
        Ok(())
    }

    fn create(&self, title: &str, body: &str, labels: &[String]) -> Result<u64> {
        let gl = self.kind == Kind::GitLab;
        let v = if gl { json!({ "title": title, "description": body, "labels": csv(labels) }) } else { json!({ "title": title, "body": body, "labels": labels }) };
        self.call("POST", "/issues", Some(v))?.json()?[if gl { "iid" } else { "number" }].as_u64().ok_or("no issue number".into())
    }

    fn edit(&self, n: u64, body: &str) -> Result<()> {
        let (m, v) = if self.kind == Kind::GitLab { ("PUT", json!({ "description": body })) } else { ("PATCH", json!({ "body": body })) };
        self.call(m, &format!("/issues/{n}"), Some(v)).map(|_| ())
    }

    fn close(&self, n: u64) -> Result<()> {
        let (m, v) = if self.kind == Kind::GitLab { ("PUT", json!({ "state_event": "close" })) } else { ("PATCH", json!({ "state": "closed" })) };
        self.call(m, &format!("/issues/{n}"), Some(v)).map(|_| ())
    }

    fn now(&self) -> Result<i64> {
        if *self.clock.borrow() == 0 {
            self.me()?;
        }
        Ok(*self.clock.borrow())
    }

    fn publish(&self, n: u64, sha: &str, mode: &str, title: &str, task_line: &str) -> Publish {
        match self.try_publish(n, sha, mode, title, task_line) {
            Ok(p) => p,
            Err(e) if e.contains("HTTP 405") || e.contains("HTTP 409") || e.contains("HTTP 422") => Publish::Failed(e),
            Err(e) => Publish::Pending(e),
        }
    }

    fn url(&self, n: u64) -> String {
        let web = self.root().replace("api.github.com", "github.com").replace("/api/v4", "").replace("/api/v3", "");
        let repo = self.api.rsplit_once("/repos/").map(|x| x.1.to_string()).or_else(|| self.api.rsplit_once("/projects/").map(|x| x.1.replace("%2F", "/"))).unwrap_or_default();
        if self.kind == Kind::GitLab { format!("{web}/{repo}/-/issues/{n}") } else { format!("{web}/{repo}/issues/{n}") }
    }

    fn me(&self) -> Result<String> {
        if let Some(m) = self.me.borrow().clone() {
            return Ok(m);
        }
        let v = self.call("GET", &format!("{}/user", self.root()), None)?.json()?;
        let m = s(&v, if self.kind == Kind::GitLab { "username" } else { "login" });
        *self.me.borrow_mut() = Some(m.clone());
        Ok(m)
    }
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
        self.state.borrow_mut().get(n).cloned()
    }
    fn comments(&self, n: u64) -> Result<Vec<Comment>> {
        Ok(self.state.borrow().comments.iter().filter(|(i, _)| *i == n).map(|(_, c)| c.clone()).collect())
    }
    fn comment(&self, n: u64, body: &str) -> Result<u64> {
        self.state.borrow_mut().get(n)?;
        Ok(self.raw(n, body, true, false))
    }
    fn labels(&self, n: u64, add: &[String], remove: &[String]) -> Result<()> {
        let mut st = self.state.borrow_mut();
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
        st.issues.push(Issue { number, title: title.into(), body: body.into(), labels: labels.to_vec(), assignees: vec![], updated: t, open: true, trusted: true });
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

    #[test]
    fn clocks() {
        assert_eq!(iso("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(iso("2026-10-12T10:20:30.123Z"), Some(1791800430));
        assert_eq!(http_date("Mon, 12 Oct 2026 10:20:30 GMT"), Some(1791800430));
        assert_eq!(stamp(1791800430), "2026-10-12T10:20:30Z");
    }

    #[test]
    fn untrusted_and_edited_entries_never_reach_fold() {
        let b = Fake::default();
        let n = b.create("t", "", &[]).unwrap();
        b.raw(n, "taskq: spawn worker claude:x evil", false, false);
        b.raw(n, "taskq: later", true, true);
        let e = entries(&b, n).unwrap();
        assert_eq!(e.len(), 1);
        assert_eq!(crate::core::fold(&e, false).state, crate::core::State::Ready);
    }

    #[test]
    fn token_goes_only_to_the_board_api() {
        let h = Http::new(Kind::GitHub, "github.com", "o/r", "t");
        for url in ["https://evil.example/x", "https://api.github.com.evil.example/x"] {
            assert!(h.call("GET", url, None).err().is_some_and(|e| e.starts_with("refused")), "{url}");
        }
    }
}
