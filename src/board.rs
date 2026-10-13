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
    operation: &'static str,
}

impl Reply {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
    fn json(&self) -> Result<Value> {
        serde_json::from_str(&self.body).map_err(|e| {
            crate::incident::detect("adapter.bad_json", concat!("src/board.rs:", line!()), "none", &[self.operation], &["http:2xx"]);
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
        let reply = Reply { headers, body: body.to_string(), operation: operation(method) };
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
    label_schema: RefCell<Option<bool>>,
    #[cfg(test)]
    transport: Option<Box<dyn Fn(&str, &str, Option<Value>) -> Result<Reply>>>,
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
        Http { kind, api, auth, me: RefCell::new(None), clock: RefCell::new(0), members: RefCell::new(vec![]), label_schema: RefCell::new(None), #[cfg(test)] transport: None }
    }

    pub fn set_label_schema(&mut self, schema: Option<&str>) -> Result<()> {
        *self.label_schema.borrow_mut() = match schema {
            None => None, Some("canonical") => Some(true), Some("legacy") => Some(false),
            Some(s) => return Err(format!("unknown label_schema {s}; expected canonical|legacy")),
        };
        Ok(())
    }

    fn call(&self, method: &str, url: &str, body: Option<Value>) -> Result<Reply> {
        // absolute URLs come from pagination headers: the token goes only to the configured API
        let url = if url.starts_with("https://") { url.to_string() } else { format!("{}{url}", self.api) };
        if !url.starts_with(&format!("{}/", self.root())) {
            return Err(format!("refused: {url} is outside the board API"));
        }
        #[cfg(test)]
        if let Some(transport) = &self.transport { return transport(method, &url, body); }
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

    fn publication_ancestry(&self, sha: &str, mode: &str) {
        // Only the immediate read in this close qualifies; later main changes are not incidents.
        if sha.len() != 40 || !sha.bytes().all(|b| b.is_ascii_hexdigit()) { return; }
        let on_main = if self.kind == Kind::GitLab {
            self.call("GET", &format!("/repository/merge_base?refs[]=main&refs[]={sha}"), None)
                .and_then(|r| r.json()).and_then(|v| {
                    let id = v["id"].as_str().filter(|id| id.len() == 40 && id.bytes().all(|b| b.is_ascii_hexdigit())).ok_or("unknown merge base")?;
                    Ok(id == sha)
                })
        } else {
            self.call("GET", &format!("/compare/main...{sha}"), None).and_then(|r| r.json())
                .and_then(|v| match v["status"].as_str() {
                    Some("identical" | "behind") => Ok(true), Some("ahead" | "diverged") => Ok(false), _ => Err("unknown ancestry".into()),
                })
        };
        if on_main == Ok(false) {
            crate::incident::detect("publication.not_on_main", concat!("src/board.rs:", line!()), "none", &[mode], &["ancestry:missing"]);
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
            let merged = self.call("PUT", &format!("/merge_requests/{iid}/merge"), Some(body))?;
            let back = self.call("GET", &format!("/merge_requests/{iid}"), None)?.json()?;
            if back["state"] != "merged" {
                if let Ok(second) = self.call("GET", &format!("/merge_requests/{iid}"), None).and_then(|r| r.json())
                    && second["state"] != "merged"
                {
                    crate::incident::detect("publication.readback", concat!("src/board.rs:", line!()), "none", &[mode], &["merge:unconfirmed"]);
                }
            }
            if back["state"] == "merged" {
                let merged = merged.json().ok();
                let target = if mode == "direct" { sha } else { merged.as_ref().and_then(|v| v["squash_commit_sha"].as_str()).unwrap_or("") };
                self.publication_ancestry(target, mode);
            }
            return if back["state"] == "merged" { Ok(Publish::Done(format!("merged !{iid}"))) } else { Err(format!("!{iid} not merged yet")) };
        }
        if mode == "direct" {
            let head = self.call("GET", &format!("/git/ref/heads/{branch}"), None)?.json()?;
            if head["object"]["sha"] != sha {
                return Ok(Publish::Failed(format!("{branch} is not at the result {sha}")));
            }
            self.call("PATCH", "/git/refs/heads/main", Some(json!({ "sha": sha, "force": false })))?;
            self.publication_ancestry(sha, mode);
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
        let merged = self.call("PUT", &format!("/pulls/{p}/merge"), Some(json!({ "merge_method": "squash", "sha": sha, "commit_title": format!("{title} (#{p})"), "commit_message": task_line })))?;
        let back = self.call("GET", &format!("/pulls/{p}"), None)?.json()?;
        if back["merged"] != true {
            if let Ok(second) = self.call("GET", &format!("/pulls/{p}"), None).and_then(|r| r.json())
                && second["merged"] != true
            {
                crate::incident::detect("publication.readback", concat!("src/board.rs:", line!()), "none", &[mode], &["merge:unconfirmed"]);
            }
        }
        if back["merged"] == true {
            if let Ok(merged) = merged.json() { self.publication_ancestry(&s(&merged, "sha"), mode); }
        }
        if back["merged"] == true { Ok(Publish::Done(format!("merged #{p} as {}", s(&back, "merge_commit_sha")))) } else { Err(format!("#{p} not merged yet")) }
    }
}

impl Board for Http {
    fn canonical_labels(&self) -> Result<bool> {
        if let Some(c) = *self.label_schema.borrow() { return Ok(c); }
        let all = self.all("/labels?per_page=100")?;
        let canonical = !all.iter().any(|v| matches!(v["name"].as_str(), Some("q-waiting" | "priority-1" | "priority-2")));
        *self.label_schema.borrow_mut() = Some(canonical);
        Ok(canonical)
    }

    fn ensure_labels(&self, names: &[String]) -> Result<()> {
        if !self.canonical_labels()? { return Err("legacy board: catalog adoption requires all-host gate and label_schema=canonical".into()); }
        let all = self.all("/labels?per_page=100")?;
        let gl = self.kind == Kind::GitLab;
        let mut seen = std::collections::HashSet::new();
        for name in names {
            if !seen.insert(name) { continue; }
            let Some((color, description)) = crate::labels::definition(name) else { continue };
            let current = all.iter().find(|v| v["name"] == name.as_str());
            if current.is_some_and(|v| label_matches(v, name, color, &description)) { continue; }
            let payload = json!({"name": name, "color": if gl { format!("#{color}") } else { color.into() }, "description": description});
            let path = if let Some(v) = current {
                if gl { format!("/labels/{}", v["id"].as_u64().ok_or("label has no id")?) }
                else { format!("/labels/{}", encode(name)) }
            } else { "/labels".into() };
            let method = if current.is_none() { "POST" } else if gl { "PUT" } else { "PATCH" };
            self.call(method, &path, Some(payload)).map_err(|e| format!("label {name} not verified; retry catalog: {e}"))?;
        }
        let readback = self.all("/labels?per_page=100")?;
        for name in names {
            if let Some((color, desc)) = crate::labels::definition(name) {
                if !readback.iter().any(|v| label_matches(v, name, color, &desc)) {
                    return Err(format!("label {name}: readback differs; retry catalog"));
                }
            }
        }
        Ok(())
    }

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
        if !add.is_empty() && self.canonical_labels()? { self.ensure_labels(add)?; }
        if self.kind == Kind::GitLab {
            self.call("PUT", &format!("/issues/{n}"), Some(json!({ "add_labels": csv(add), "remove_labels": csv(remove) })))?;
            return verify_assignment(self, n, add, remove);
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
        verify_assignment(self, n, add, remove)
    }

    fn create(&self, title: &str, body: &str, labels: &[String]) -> Result<u64> {
        if self.canonical_labels()? {
            let mut names = crate::labels::FIXED.iter().map(|s| s.to_string()).collect::<Vec<_>>();
            names.extend_from_slice(labels);
            self.ensure_labels(&names)?;
        }
        let gl = self.kind == Kind::GitLab;
        let v = if gl { json!({ "title": title, "description": body, "labels": csv(labels) }) } else { json!({ "title": title, "body": body, "labels": labels }) };
        let n = self.call("POST", "/issues", Some(v))?.json()?[if gl { "iid" } else { "number" }].as_u64().ok_or("no issue number")?;
        verify_assignment(self, n, labels, &[])?;
        Ok(n)
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
    pub hide_latest_comment: bool,
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
        let st = self.state.borrow();
        Ok(st.comments.iter().filter(|(i, c)| *i == n && !(st.hide_latest_comment && c.id == st.next_id)).map(|(_, c)| c.clone()).collect())
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

    fn publication_board(kind: Kind, mode: &str, first_merged: bool, second_merged: Option<bool>, ancestry: Option<bool>) -> (Http, std::rc::Rc<RefCell<Vec<(String, String)>>>) {
        let calls = std::rc::Rc::new(RefCell::new(vec![]));
        let captured = calls.clone();
        let reads = RefCell::new(0);
        let comparisons = RefCell::new(0);
        let direct = mode == "direct";
        let mut h = Http::new(kind, if kind == Kind::GitHub { "github.com" } else { "gitlab.example" }, "o/r", "fake");
        h.transport = Some(Box::new(move |method, url, _| {
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
                if kind == Kind::GitHub { json!({"status": if on_main { "identical" } else { "ahead" }}) }
                else { json!({"id": if on_main { if direct { result } else { merged } } else { "c".repeat(40) }}) }
            } else if url.contains("/check-runs") { json!({"check_runs": [{"status": "completed", "conclusion": "success"}]}) }
            else if url.ends_with("/status") { json!({"total_count": 0}) }
            else if url.contains("/repository/commits/") { json!({"last_pipeline": {"status": "success"}}) }
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
            Ok(Reply { headers: vec![], body: value.to_string(), operation: operation(method) })
        }));
        (h, calls)
    }

    #[test]
    fn incident_publication_uses_merge_sha_and_keeps_outcomes() {
        for kind in [Kind::GitHub, Kind::GitLab] {
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

    #[test]
    fn incident_publication_readback_requires_two_negative_reads() {
        for kind in [Kind::GitHub, Kind::GitLab] {
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

    #[test]
    fn incident_bad_json_is_only_a_successful_response() {
        for fail in [false, true] {
            crate::incident::take_detected();
            let mut h = Http::new(Kind::GitHub, "github.com", "o/r", "fake");
            h.transport = Some(Box::new(move |_, _, _| if fail { Err("HTTP 503 malformed response".into()) }
                else { Ok(Reply { headers: vec![], body: "private invalid JSON".into(), operation: "read" }) }));
            assert!(h.call("GET", "/issues", None).and_then(|r| r.json()).is_err());
            assert_eq!(crate::incident::take_detected(), if fail { vec![] } else { vec!["adapter.bad_json"] });
        }
    }

    fn label_board(kind: Kind, fail: bool, mismatch: bool) -> (Http, std::rc::Rc<RefCell<Vec<Value>>>, std::rc::Rc<RefCell<usize>>) {
        let catalog = std::rc::Rc::new(RefCell::new(vec![json!({"id": 99, "name": "bug", "color": "FF0000", "description": "user owned"})]));
        let writes = std::rc::Rc::new(RefCell::new(0));
        let (data, count) = (catalog.clone(), writes.clone());
        let mut h = Http::new(kind, if kind == Kind::GitHub { "github.com" } else { "gitlab.example" }, "o/r", "test");
        h.transport = Some(Box::new(move |method, url, body| {
            assert!(url.contains("/labels"));
            if method == "GET" { return Ok(Reply { headers: vec![], operation: "read", body: json!(*data.borrow()).to_string() }); }
            assert!(matches!(method, "POST" | "PATCH" | "PUT"));
            if method == "POST" { assert!(url.ends_with("/labels")); }
            else if kind == Kind::GitLab { assert_eq!(method, "PUT"); assert!(url.rsplit('/').next().unwrap().parse::<u64>().is_ok()); }
            else { assert_eq!(method, "PATCH"); }
            assert_eq!(body.as_ref().unwrap()["color"].as_str().unwrap().starts_with('#'), kind == Kind::GitLab);
            *count.borrow_mut() += 1;
            if fail && *count.borrow() == 2 { return Err("HTTP 403 denied".into()); }
            let mut v = body.unwrap();
            if mismatch { v["color"] = json!("000000"); }
            let mut all = data.borrow_mut();
            if let Some(old) = all.iter_mut().find(|old| old["name"] == v["name"]) {
                v["id"] = old["id"].clone(); *old = v;
            } else { v["id"] = json!(all.len() + 100); all.push(v); }
            Ok(Reply { headers: vec![], operation: "read", body: "{}".into() })
        }));
        (h, catalog, writes)
    }

    #[test]
    fn label_adapters_readback_idempotence_and_preservation() {
        for kind in [Kind::GitHub, Kind::GitLab] {
            let (h, catalog, writes) = label_board(kind, false, false);
            let mut names = crate::labels::FIXED.iter().chain(["host-win", "needs-gpu", "area-ui"].iter()).map(|s| s.to_string()).collect::<Vec<_>>();
            names.push("taskq".into());
            h.ensure_labels(&names).unwrap();
            assert_eq!(*writes.borrow(), names.len() - 1);
            h.ensure_labels(&names).unwrap();
            assert_eq!(*writes.borrow(), names.len() - 1, "repeat makes no writes");
            assert_eq!(catalog.borrow()[0]["description"], "user owned");
            let i = catalog.borrow().iter().position(|v| v["name"] == "q-ask").unwrap();
            catalog.borrow_mut()[i]["color"] = json!("000000");
            h.ensure_labels(&names).unwrap();
            assert_eq!(*writes.borrow(), names.len(), "repairs existing metadata");
        }
    }

    #[test]
    fn label_adapter_failures_and_legacy_gate() {
        for kind in [Kind::GitHub, Kind::GitLab] {
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
            let (mut h, _, _) = label_board(kind, false, false);
            h.set_label_schema(Some("canonical")).unwrap();
            assert_eq!(h.state_label("waiting").unwrap(), "q-blocked");
            assert!(h.set_label_schema(Some("typo")).is_err());
        }
    }

    #[test]
    fn issue_label_adapters_preserve_unrelated_assignments() {
        for kind in [Kind::GitHub, Kind::GitLab] {
            let labels = std::rc::Rc::new(RefCell::new(vec!["bug".to_string(), "q-ready".to_string()]));
            let data = labels.clone();
            let mut h = Http::new(kind, if kind == Kind::GitHub { "github.com" } else { "gitlab.example" }, "o/r", "test");
            h.set_label_schema(Some("legacy")).unwrap();
            h.transport = Some(Box::new(move |method, url, body| {
                let reply = |body: Value| Ok(Reply { headers: vec![], operation: "read", body: body.to_string() });
                if url.contains("/members/") { return reply(json!({"access_level": 20})); }
                if method == "GET" {
                    assert!(url.ends_with("/issues/7"));
                    return reply(json!({"number": 7, "iid": 7, "state": "open", "author_association": "OWNER", "labels": *data.borrow()}));
                }
                if method == "POST" && url.ends_with("/issues") {
                    let body = body.unwrap();
                    *data.borrow_mut() = if kind == Kind::GitLab { body["labels"].as_str().unwrap().split(',').map(String::from).collect() }
                        else { names(&body["labels"], "name") };
                    return reply(json!({"number": 7, "iid": 7}));
                }
                if kind == Kind::GitLab {
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

    #[test]
    fn assignment_readback_rejects_partial_application() {
        let b = Fake::default();
        let n = b.create("t", "", &["bug".into(), "q-ready".into()]).unwrap();
        assert!(verify_assignment(&b, n, &["q-doing".into()], &["q-ready".into()]).is_err());
        b.labels(n, &["q-doing".into()], &["q-ready".into()]).unwrap();
        verify_assignment(&b, n, &["q-doing".into()], &["q-ready".into()]).unwrap();
        assert!(b.issue(n).unwrap().labels.contains(&"bug".into()));
    }

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
        b.raw(n, "1", false, false);
        b.raw(n, "taskq: later", true, true);
        let e = entries(&b, n).unwrap();
        assert_eq!(e.len(), 1);
        assert_eq!(crate::core::fold(0, &e, false).state, crate::core::State::Ready);
    }

    #[test]
    fn token_goes_only_to_the_board_api() {
        let h = Http::new(Kind::GitHub, "github.com", "o/r", "t");
        for url in ["https://evil.example/x", "https://api.github.com.evil.example/x"] {
            assert!(h.call("GET", url, None).err().is_some_and(|e| e.starts_with("refused")), "{url}");
        }
    }
}
