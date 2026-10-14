//! GitHub issues (taskq.md § 1, § 2, § 7): REST v3 on github.com or a GitHub Enterprise host, owner-installed token.

use super::{Api, Board, Comment, Issue, Kind, Publish, Result, encode, hex40, into, iso, names, outcome, s, stamp, verify_assignment};
use serde_json::{Value, json};

pub const KIND: Kind = Kind {
    name: "github",
    cli: "gh",
    host_env: "GH_HOST",
    user_key: "login",
    default_host: "github.com",
    repo_view: Some(&["repo", "view", "--json", "nameWithOwner,visibility"]),
    token: |tool, _host| token(&tool.install),
    token_env: &["GH_TOKEN", "GITHUB_TOKEN"],
    api,
    board: |api| Box::new(GitHub { api }),
};

pub const TOKEN_ERROR: &str = "GitHub board token missing, empty or unreadable; owner must install it in <install>/token; never run gh auth login";

pub fn token(install: &std::path::Path) -> Result<String> {
    std::fs::read_to_string(install.join("token")).ok()
        .map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).ok_or_else(|| TOKEN_ERROR.into())
}

fn api(host: &str, repo: &str, token: &str) -> Api {
    let root = if host == "github.com" || host.is_empty() { "https://api.github.com".to_string() } else { format!("https://{host}/api/v3") };
    Api::new(root.clone(), format!("{root}/repos/{repo}"), token)
}

pub struct GitHub {
    pub(super) api: Api,
}

fn trusted(v: &Value) -> bool {
    matches!(v["author_association"].as_str(), Some("OWNER" | "MEMBER" | "COLLABORATOR"))
}

fn parse_issue(v: &Value) -> Issue {
    Issue {
        number: v["number"].as_u64().unwrap_or(0),
        title: s(v, "title"),
        body: s(v, "body"),
        labels: names(&v["labels"], "name"),
        assignees: names(&v["assignees"], "login"),
        updated: iso(&s(v, "updated_at")).unwrap_or(0),
        open: v["state"] == "open",
        trusted: trusted(v),
    }
}

/// The open-issue list of a pass in one GraphQL query per 100 issues (§ 4 budget): it costs the separate GraphQL
/// budget about 2 points instead of one core request, and returns no pull requests.
const OPEN_ISSUES: &str = "query($owner: String!, $name: String!, $after: String) { repository(owner: $owner, name: $name) { \
    issues(states: OPEN, first: 100, after: $after, orderBy: {field: CREATED_AT, direction: DESC}) { pageInfo { hasNextPage endCursor } \
    nodes { number title body updatedAt authorAssociation labels(first: 50) { nodes { name } } assignees(first: 20) { nodes { login } } } } } }";

impl GitHub {
    fn open_issues(&self) -> Result<Vec<Issue>> {
        let repo = self.api.base.rsplit_once("/repos/").map(|x| x.1).unwrap_or_default();
        let (owner, name) = repo.split_once('/').ok_or("no repository")?;
        let (mut out, mut after) = (vec![], Value::Null);
        loop {
            let v = self.api.call("POST", &format!("{}/graphql", self.api.root), Some(json!({
                "query": OPEN_ISSUES, "variables": { "owner": owner, "name": name, "after": after },
            })))?.json()?;
            let page = &v["data"]["repository"]["issues"];
            if !page["nodes"].is_array() {
                return Err(format!("GraphQL open issues: {}", v["errors"]));
            }
            out.extend(page["nodes"].as_array().into_iter().flatten().map(|n| parse_issue(&json!({
                "number": n["number"], "title": n["title"], "body": n["body"], "updated_at": n["updatedAt"], "state": "open",
                "author_association": n["authorAssociation"], "labels": n["labels"]["nodes"], "assignees": n["assignees"]["nodes"],
            }))));
            if page["pageInfo"]["hasNextPage"] != true { return Ok(out); }
            after = page["pageInfo"]["endCursor"].clone();
        }
    }

    /// CI on `sha`: Ok(None) green, Ok(Some(msg)) failed, Err pending or missing.
    fn ci(&self, sha: &str) -> Result<Option<String>> {
        let runs = self.api.call("GET", &format!("/commits/{sha}/check-runs?per_page=100"), None)?.json()?;
        let runs = runs["check_runs"].as_array().cloned().unwrap_or_default();
        let status = self.api.call("GET", &format!("/commits/{sha}/status"), None)?.json()?;
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
        if !hex40(sha) { return; }
        let on_main = self.api.call("GET", &format!("/compare/main...{sha}"), None).and_then(|r| r.json())
            .and_then(|v| match v["status"].as_str() {
                Some("identical" | "behind") => Ok(true), Some("ahead" | "diverged") => Ok(false), _ => Err("unknown ancestry".into()),
            });
        if on_main == Ok(false) {
            crate::incident::detect("publication.not_on_main", concat!(file!(), ":", line!()), "none", &[mode], &["ancestry:missing"]);
        }
    }

    fn try_publish(&self, n: u64, sha: &str, mode: &str, title: &str, task_line: &str) -> Result<Publish> {
        let branch = format!("taskq-{n}");
        // No commit, or the SHA is already on main: verify ancestry, close.
        if matches!(self.api.call("GET", &format!("/compare/main...{sha}"), None)?.json()?["status"].as_str(), Some("identical" | "behind")) {
            return Ok(Publish::Done(format!("{sha} already on main")));
        }
        if let Some(fail) = self.ci(sha)? {
            return Ok(Publish::Failed(fail));
        }
        if mode == "direct" {
            let head = self.api.call("GET", &format!("/git/ref/heads/{branch}"), None)?.json()?;
            if head["object"]["sha"] != sha {
                return Ok(Publish::Failed(format!("{branch} is not at the result {sha}")));
            }
            self.api.call("PATCH", "/git/refs/heads/main", Some(json!({ "sha": sha, "force": false })))?;
            self.publication_ancestry(sha, mode);
            return Ok(Publish::Done(format!("main fast-forwarded to {sha}")));
        }
        let owner = self.api.base.rsplit_once("/repos/").map(|x| x.1).and_then(|r| r.split('/').next()).unwrap_or_default().to_string();
        let prs = self.api.call("GET", &format!("/pulls?head={owner}:{branch}&base=main&state=open"), None)?.json()?;
        let prs = prs.as_array().cloned().unwrap_or_default();
        if prs.is_empty() {
            // § 7 merged, not closed: a merged PR at the result SHA into any base (a release branch too) is the publication; no second merge
            let closed = self.api.call("GET", &format!("/pulls?head={owner}:{branch}&state=closed"), None)?.json()?;
            if let Some(pr) = closed.as_array().into_iter().flatten().find(|p| p["head"]["sha"] == sha && !p["merged_at"].is_null()) {
                return Ok(Publish::Done(format!("merged #{}{} as {}", pr["number"], into(&s(&pr["base"], "ref")), s(pr, "merge_commit_sha"))));
            }
        }
        if prs.len() != 1 {
            return Ok(Publish::Failed(format!("{} open PRs of {branch}, want exactly one", prs.len())));
        }
        if prs[0]["head"]["sha"] != sha {
            return Ok(Publish::Failed(format!("PR head is not the result {sha}")));
        }
        let p = prs[0]["number"].as_u64().unwrap_or(0);
        if prs[0]["draft"] == true {
            // GitHub refuses to merge a draft (HTTP 405) and REST cannot mark it ready
            let ready = self.api.call("POST", &format!("{}/graphql", self.api.root), Some(json!({
                "query": "mutation($id: ID!) { markPullRequestReadyForReview(input: {pullRequestId: $id}) { pullRequest { isDraft } } }",
                "variables": { "id": prs[0]["node_id"] },
            })))?.json()?;
            if ready["data"]["markPullRequestReadyForReview"]["pullRequest"]["isDraft"] != false {
                return Err(format!("#{p} is still a draft: {}", ready["errors"]));
            }
        }
        let merged = self.api.call("PUT", &format!("/pulls/{p}/merge"), Some(json!({ "merge_method": "squash", "sha": sha, "commit_title": format!("{title} (#{p})"), "commit_message": task_line })))?;
        let back = self.api.call("GET", &format!("/pulls/{p}"), None)?.json()?;
        if back["merged"] != true {
            if let Ok(second) = self.api.call("GET", &format!("/pulls/{p}"), None).and_then(|r| r.json())
                && second["merged"] != true
            {
                crate::incident::detect("publication.readback", concat!(file!(), ":", line!()), "none", &[mode], &["merge:unconfirmed"]);
            }
        }
        if back["merged"] == true {
            if let Ok(merged) = merged.json() { self.publication_ancestry(&s(&merged, "sha"), mode); }
        }
        if back["merged"] == true { Ok(Publish::Done(format!("merged #{p} as {}", s(&back, "merge_commit_sha")))) } else { Err(format!("#{p} not merged yet")) }
    }
}

impl Board for GitHub {
    fn canonical_labels(&self) -> Result<bool> {
        self.api.canonical_labels()
    }

    fn ensure_labels(&self, names: &[String]) -> Result<()> {
        self.api.ensure_labels(names, |c| c.into(), |current, payload| {
            let (method, path) = match current {
                Some(_) => ("PATCH", format!("/labels/{}", encode(payload["name"].as_str().unwrap_or_default()))),
                None => ("POST", "/labels".into()),
            };
            self.api.call(method, &path, Some(payload)).map(|_| ())
        })
    }

    fn issues(&self, since: Option<i64>) -> Result<Vec<Issue>> {
        let url = match since {
            Some(t) => format!("/issues?state=all&per_page=100&since={}", stamp(t)),
            None if self.api.root == "https://api.github.com" => return self.open_issues(),
            None => "/issues?state=open&per_page=100".into(),
        };
        Ok(self.api.all(&url)?.iter().filter(|v| v.get("pull_request").is_none()).map(parse_issue).collect())
    }

    fn issue(&self, n: u64) -> Result<Issue> {
        Ok(parse_issue(&self.api.call("GET", &format!("/issues/{n}"), None)?.json()?))
    }

    fn comments(&self, n: u64) -> Result<Vec<Comment>> {
        Ok(self.api.all(&format!("/issues/{n}/comments?per_page=100"))?.iter().map(|v| Comment {
            id: v["id"].as_u64().unwrap_or(0),
            created: iso(&s(v, "created_at")).unwrap_or(0),
            body: s(v, "body"),
            trusted: trusted(v),
            edited: s(v, "updated_at") != s(v, "created_at"),
        }).collect())
    }

    fn comment(&self, n: u64, body: &str) -> Result<u64> {
        self.api.call("POST", &format!("/issues/{n}/comments"), Some(json!({ "body": body })))?.json()?["id"].as_u64().ok_or("no comment id".into())
    }

    fn edit_comment(&self, _n: u64, id: u64, body: &str) -> Result<()> {
        self.api.call("PATCH", &format!("/issues/comments/{id}"), Some(json!({ "body": body }))).map(|_| ())
    }

    fn labels(&self, n: u64, add: &[String], remove: &[String]) -> Result<()> {
        if !add.is_empty() && self.canonical_labels()? { self.ensure_labels(add)?; }
        if !add.is_empty() {
            self.api.call("POST", &format!("/issues/{n}/labels"), Some(json!({ "labels": add })))?;
        }
        for l in remove {
            // percent-encode: a label like `q-/../..` must not walk the API path
            match self.api.call("DELETE", &format!("/issues/{n}/labels/{}", encode(l)), None) {
                Err(e) if e.contains("HTTP 404") => {}
                r => {
                    r?;
                }
            }
        }
        verify_assignment(self, n, add, remove)
    }

    fn create(&self, title: &str, body: &str, labels: &[String]) -> Result<u64> {
        if let Some(names) = self.api.creation_labels(labels)? { self.ensure_labels(&names)?; }
        let n = self.api.call("POST", "/issues", Some(json!({ "title": title, "body": body, "labels": labels })))?.json()?["number"].as_u64().ok_or("no issue number")?;
        verify_assignment(self, n, labels, &[])?;
        Ok(n)
    }

    fn edit(&self, n: u64, body: &str) -> Result<()> {
        self.api.call("PATCH", &format!("/issues/{n}"), Some(json!({ "body": body }))).map(|_| ())
    }

    fn close(&self, n: u64) -> Result<()> {
        self.api.call("PATCH", &format!("/issues/{n}"), Some(json!({ "state": "closed" }))).map(|_| ())
    }

    fn now(&self) -> Result<i64> {
        self.api.now(KIND.user_key)
    }

    fn me(&self) -> Result<String> {
        self.api.me(KIND.user_key)
    }

    fn publish(&self, n: u64, sha: &str, mode: &str, title: &str, task_line: &str) -> Publish {
        outcome(self.try_publish(n, sha, mode, title, task_line))
    }

    fn requests(&self) -> crate::requests::Budget {
        self.api.budget.borrow().clone()
    }

    fn url(&self, n: u64) -> String {
        let web = self.api.root.replace("api.github.com", "github.com").replace("/api/v3", "");
        let repo = self.api.base.rsplit_once("/repos/").map(|x| x.1).unwrap_or_default();
        format!("{web}/{repo}/issues/{n}")
    }
}

#[cfg(test)]
mod token_tests {
    use super::*;

    // Chain: host credential -> service board and worker environment. Risk: login fallback or mismatched errors.
    #[test]
    fn service_and_workers_share_the_host_credential_and_missing_token_error() {
        let dir = std::env::temp_dir().join(format!("taskq-token-{}", crate::runtime::uuid()));
        std::fs::create_dir_all(&dir).unwrap();
        let tool = crate::platform::tool(&dir);
        for contents in [None, Some(" \n")] {
            if let Some(contents) = contents { std::fs::write(dir.join("token"), contents).unwrap(); }
            let service = KIND.open("github.com", "o/r", None, &tool).err().unwrap();
            assert_eq!(service, TOKEN_ERROR);
            assert_eq!(KIND.worker_env(&tool, "github.com").unwrap_err(), service);
        }
        std::fs::write(dir.join("token"), "existing-owner-token\n").unwrap();
        let env = KIND.worker_env(&tool, "github.com").unwrap();
        assert_eq!(env, vec![("GH_TOKEN".into(), "existing-owner-token".into()), ("GITHUB_TOKEN".into(), "existing-owner-token".into())]);
        assert!(KIND.open("github.com", "o/r", None, &tool).is_ok());
        std::fs::remove_file(dir.join("token")).unwrap();
        std::fs::create_dir(dir.join("token")).unwrap();
        assert_eq!(KIND.worker_env(&tool, "github.com").unwrap_err(), TOKEN_ERROR);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
