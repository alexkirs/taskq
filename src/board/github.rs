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
        reason: v["state_reason"].as_str().map(String::from),
    }
}

/// The open-issue list of a pass in one GraphQL query per 100 issues (§ 4 budget): it costs the separate GraphQL
/// budget 1 point instead of one core request, and returns no pull requests. Labels are its only nested connection:
/// assignees would make it 2 points (#1043), so listed issues carry none and callers read `issue(n)` when they need them.
const OPEN_ISSUES: &str = "query($owner: String!, $name: String!, $after: String) { repository(owner: $owner, name: $name) { \
    issues(states: OPEN, first: 100, after: $after, orderBy: {field: CREATED_AT, direction: DESC}) { pageInfo { hasNextPage endCursor } \
    nodes { number title body updatedAt authorAssociation labels(first: 50) { nodes { name } } } } } }";

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
                "author_association": n["authorAssociation"], "labels": n["labels"]["nodes"],
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

    /// main refuses a merge of a head that lacks current main and this token cannot bypass it (§ 7):
    /// a ruleset it can never bypass, or classic protection the token can read that binds admins too.
    fn strict_main(&self) -> bool {
        let read = |url: &str| self.api.call("GET", url, None).and_then(|r| r.json());
        read("/rules/branches/main").is_ok_and(|v| v.as_array().into_iter().flatten()
            .filter(|r| r["type"] == "required_status_checks" && r["parameters"]["strict_required_status_checks_policy"] == true)
            .any(|r| r["ruleset_id"].as_u64().is_some_and(|id| read(&format!("/rulesets/{id}")).is_ok_and(|s| s["current_user_can_bypass"] == "never"))))
            || read("/branches/main/protection").is_ok_and(|v| v["required_status_checks"]["strict"] == true && v["enforce_admins"]["enabled"] == true)
    }

    fn graphql(&self, query: &str, variables: Value) -> Result<Value> {
        let v = self.api.call("POST", &format!("{}/graphql", self.api.root), Some(json!({"query": query, "variables": variables})))?.json()?;
        if v["errors"].as_array().is_some_and(|e| !e.is_empty()) { return Err(format!("publication GraphQL: {}", v["errors"])); }
        if !v["data"].is_object() { return Err("publication GraphQL data unknown".into()); }
        Ok(v["data"].clone())
    }

    /// § 7: qualify the merge result, never substitute green checks on the original head.
    fn merge_ci(&self, sha: &str) -> Result<()> {
        if !hex40(sha) { return Err("merge result SHA unknown".into()); }
        let v = self.api.call("GET", &format!("/actions/runs?event=merge_group&head_sha={sha}&per_page=100"), None)?.json()?;
        let runs = v["workflow_runs"].as_array().ok_or("merge_group CI unknown")?;
        if !runs.iter().any(|r| r["head_sha"] == sha && r["event"] == "merge_group" && r["status"] == "completed" && r["conclusion"] == "success") {
            return Err(format!("merge_group CI not green on merge result {sha}"));
        }
        if let Some(fail) = self.ci(sha)? { return Err(fail); }
        Ok(())
    }

    fn queue(&self, pr: &Value, sha: &str, enqueued: bool) -> Result<Publish> {
        let id = pr["node_id"].as_str().ok_or("PR node id unknown")?;
        let v = self.graphql("query($id: ID!) { node(id: $id) { ... on PullRequest { headRefOid merged mergeCommit { oid } mergeQueueEntry { id } timelineItems(last: 1, itemTypes: [REMOVED_FROM_MERGE_QUEUE_EVENT]) { nodes { ... on RemovedFromMergeQueueEvent { reason } } } } } }", json!({"id": id}))?;
        let p = &v["node"];
        if p["headRefOid"].as_str().ok_or("queued PR head unknown")? != sha { return Err("queued PR head changed; publication needs review".into()); }
        if p.get("mergeQueueEntry").is_none() { return Err("queue entry state unknown".into()); }
        if p["merged"] == true {
            let merged = p["mergeCommit"]["oid"].as_str().ok_or("merge result unknown")?;
            self.merge_ci(merged)?;
            self.publication_ancestry(merged, "pr");
            return Ok(Publish::Done(format!("merged #{} as {merged}; merge-result CI green", pr["number"])));
        }
        if p["merged"] != false { return Err("queue merge state unknown".into()); }
        if let Some(id) = p["mergeQueueEntry"]["id"].as_str() { return Ok(Publish::Enqueued(id.into())); }
        if enqueued {
            let reason = p["timelineItems"]["nodes"][0]["reason"].as_str().ok_or("queue entry missing; removal reason unknown")?;
            let lower = reason.to_lowercase();
            return Ok(if ["conflict", "base branch", "out of date"].iter().any(|s| lower.contains(s)) {
                Publish::MainMoved(format!("merge queue removed PR: {reason}"))
            } else if lower.contains("failed") || lower.contains("failure") {
                Publish::Failed(format!("merge queue removed PR: {reason}"))
            } else { Publish::Pending(format!("merge queue removed PR; owner action needed: {reason}")) });
        }
        if pr["draft"] == true {
            let v = self.graphql("mutation($id: ID!) { markPullRequestReadyForReview(input: {pullRequestId: $id}) { pullRequest { isDraft } } }", json!({"id": id}))?;
            if v["markPullRequestReadyForReview"]["pullRequest"]["isDraft"] != false { return Err("PR is still a draft".into()); }
        }
        self.graphql("mutation($id: ID!, $sha: GitObjectID!) { enqueuePullRequest(input: {pullRequestId: $id, expectedHeadOid: $sha}) { mergeQueueEntry { id } } }", json!({"id": id, "sha": sha}))?;
        // Read back admission: a mutation response alone is not proof (R12); a retry recovers it after a lost response.
        let v = self.graphql("query($id: ID!) { node(id: $id) { ... on PullRequest { mergeQueueEntry { id } } } }", json!({"id": id}))?;
        let id = v["node"]["mergeQueueEntry"]["id"].as_str().ok_or("queue admission not yet read back")?;
        Ok(Publish::Enqueued(id.into()))
    }

    fn rebase(&self, root: &std::path::Path, n: u64, sha: &str) -> Result<Publish> {
        use std::process::Command;
        let path = root.join(".worktrees").join(format!("taskq-{n}"));
        let path = path.canonicalize().map_err(|_| "private fallback needs its owned task worktree on this host")?;
        if !path.starts_with(root.canonicalize().map_err(|_| "private fallback root unknown")?) {
            return Err("private fallback worktree escapes this project".into());
        }
        let host = self.api.root.strip_prefix("https://").unwrap_or_default().split('/').next().unwrap_or_default();
        let host = if host == "api.github.com" { "github.com" } else { host };
        let git = |args: &[&str]| -> Result<std::process::Output> {
            Command::new("git").current_dir(&path).args(args).env("GIT_TERMINAL_PROMPT", "0")
                // Git's credential protocol receives the host board token through the environment, never argv/logs.
                .env("GIT_CONFIG_COUNT", "2").env("GIT_CONFIG_KEY_0", "credential.helper").env("GIT_CONFIG_VALUE_0", "")
                .env("GIT_CONFIG_KEY_1", format!("credential.https://{host}.helper"))
                .env("GIT_CONFIG_VALUE_1", "!f() { printf 'username=x-access-token\\npassword=%s\\n' \"$TASKQ_GIT_TOKEN\"; }; f")
                .env("TASKQ_GIT_TOKEN", self.api.auth.strip_prefix("Authorization: Bearer ").unwrap_or_default())
                .output().map_err(|_| "private fallback cannot run Git".into())
        };
        let read = |args: &[&str]| -> Result<String> {
            let out = git(args)?;
            if !out.status.success() { return Err("private fallback Git read failed; worktree kept".into()); }
            String::from_utf8(out.stdout).map(|s| s.trim().to_string()).map_err(|_| "private fallback Git output unknown".into())
        };
        let common = read(&["rev-parse", "--git-common-dir"])?;
        let common = path.join(common).canonicalize().map_err(|_| "private fallback ownership unknown")?;
        let project = Command::new("git").current_dir(root).args(["rev-parse", "--git-common-dir"]).output().map_err(|_| "private fallback project unknown")?;
        if !project.status.success() || root.join(String::from_utf8_lossy(&project.stdout).trim()).canonicalize().ok().as_ref() != Some(&common) {
            return Err("private fallback worktree belongs to another repository".into());
        }
        let expected = format!("taskq-{n}");
        if read(&["symbolic-ref", "--short", "HEAD"])? != expected || read(&["rev-parse", "HEAD"])? != sha || !read(&["status", "--porcelain"])?.is_empty() {
            return Err("private fallback refuses a dirty, changed or unowned task worktree".into());
        }
        let repo = self.api.base.rsplit_once("/repos/").map(|x| x.1).ok_or("repository unknown")?;
        let origin = read(&["config", "--get", "remote.origin.url"])?;
        let origin = origin.trim_end_matches('/').trim_end_matches(".git");
        if origin != format!("https://{host}/{repo}") && origin != format!("git@{host}:{repo}") {
            return Err("private fallback origin is not this board; worktree kept".into());
        }
        if !git(&["fetch", "origin", "main"])?.status.success() { return Err("private fallback fetch failed; worktree kept".into()); }
        if !git(&["rebase", "origin/main"])?.status.success() {
            let conflict = !read(&["ls-files", "-u"])?.is_empty();
            if !git(&["rebase", "--abort"])?.status.success() { return Err("private fallback rebase abort unknown; worktree kept".into()); }
            return if conflict { Ok(Publish::MainMoved("private fallback rebase conflict; worker must resolve against main".into())) }
                else { Err("private fallback rebase failed; worktree restored".into()) };
        }
        let head = read(&["rev-parse", "HEAD"])?;
        if !hex40(&head) { return Err("private fallback head unknown".into()); }
        if !git(&["push", "origin", &format!("HEAD:refs/heads/{expected}"), &format!("--force-with-lease=refs/heads/{expected}:{sha}")])?.status.success() {
            return Err("private fallback push failed; rebased worktree kept, publication unknown".into());
        }
        let back = self.api.call("GET", &format!("/git/ref/heads/{expected}"), None)?.json()?;
        if back["object"]["sha"] != head { return Err("private fallback pushed head not read back".into()); }
        Ok(Publish::Rebased(head))
    }

    fn try_publish(&self, n: u64, sha: &str, mode: &str, title: &str, task_line: &str, root: Option<&std::path::Path>, enqueued: bool) -> Result<Publish> {
        let branch = format!("taskq-{n}");
        // No commit, or the SHA is already on main: verify ancestry, close.
        let status = self.api.call("GET", &format!("/compare/main...{sha}"), None)?.json()?["status"].as_str().unwrap_or_default().to_string();
        if matches!(status.as_str(), "identical" | "behind") {
            return Ok(Publish::Done(format!("{sha} already on main")));
        }
        if mode == "direct" {
            if let Some(fail) = self.ci(sha)? { return Ok(Publish::Failed(fail)); }
            let head = self.api.call("GET", &format!("/git/ref/heads/{branch}"), None)?.json()?;
            if head["object"]["sha"] != sha {
                return Ok(Publish::Failed(format!("{branch} is not at the result {sha}")));
            }
            self.api.call("PATCH", "/git/refs/heads/main", Some(json!({ "sha": sha, "force": false })))?;
            self.publication_ancestry(sha, mode);
            return Ok(Publish::Done(format!("main fast-forwarded to {sha}")));
        }
        let rules = self.api.call("GET", "/rules/branches/main", None)?.json()?;
        let rules = rules.as_array().ok_or("unknown main rules")?;
        let queue = rules.iter().any(|r| r["type"] == "merge_queue");
        let owner = self.api.base.rsplit_once("/repos/").map(|x| x.1).and_then(|r| r.split('/').next()).unwrap_or_default().to_string();
        let prs = self.api.call("GET", &format!("/pulls?head={owner}:{branch}&base=main&state=open"), None)?.json()?;
        let prs = prs.as_array().cloned().ok_or("PR list unknown")?;
        if prs.is_empty() {
            // § 7 merged, not closed: a merged PR at the result SHA into any base (a release branch too) is the publication; no second merge
            let closed = self.api.call("GET", &format!("/pulls?head={owner}:{branch}&state=closed"), None)?.json()?;
            if let Some(pr) = closed.as_array().into_iter().flatten().find(|p| p["head"]["sha"] == sha && !p["merged_at"].is_null()) {
                let merged = s(pr, "merge_commit_sha");
                if queue || enqueued { self.merge_ci(&merged)?; }
                else if let Some(fail) = self.ci(sha)? { return Ok(Publish::Failed(fail)); }
                let ci = if queue || enqueued { "merge-result CI green".to_string() } else { format!("exact-SHA CI green on {sha}") };
                return Ok(Publish::Done(format!("merged #{}{} as {}; {ci}", pr["number"], into(&s(&pr["base"], "ref")), merged)));
            }
        }
        if prs.len() != 1 {
            return Ok(Publish::Failed(format!("{} open PRs of {branch}, want exactly one", prs.len())));
        }
        if prs[0]["head"]["sha"].as_str().ok_or("PR head SHA unknown")? != sha {
            // shortcut: an unrecorded/foreign head stays unknown; use a board-backed rebase intent if automatic recovery is needed.
            return Err(format!("PR head differs from result {sha}; publication unknown; verify the branch before retrying"));
        }
        if queue { return self.queue(&prs[0], sha, enqueued); }
        match self.api.call("GET", "", None)?.json()?["private"].as_bool() {
            Some(true) => {},
            Some(false) => return Err("public board needs a main merge queue with required merge_group CI; Git fallback is private-only".into()),
            None => return Err("repository visibility unknown; no fallback".into()),
        }
        let root = root.ok_or("private fallback requires the service")?;
        if !self.strict_main() { return Err("private fallback needs strict main protection without token bypass".into()); }
        if status == "diverged" {
            return self.rebase(root, n, sha);
        }
        if status != "ahead" { return Err("unknown result ancestry; private fallback left unchanged".into()); }
        if let Some(fail) = self.ci(sha)? { return Ok(Publish::Failed(fail)); }
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
        let merged = match self.api.call("PUT", &format!("/pulls/{p}/merge"), Some(json!({ "merge_method": "squash", "sha": sha, "commit_title": format!("{title} (#{p})"), "commit_message": task_line }))) {
            Err(e) if e.contains("HTTP 405") || e.contains("HTTP 409") => return Err("main moved or merge unavailable; service will reread the private fallback".into()),
            r => r?,
        };
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
        if back["merged"] == true { Ok(Publish::Done(format!("merged #{p} as {}; exact-SHA CI green on {sha}", s(&back, "merge_commit_sha")))) } else { Err(format!("#{p} not merged yet")) }
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

    fn defer_publication(&self, mode: &str) -> bool { mode == "pr" }

    fn cancel_publication(&self, n: u64, sha: &str) -> Result<()> {
        let owner = self.api.base.rsplit_once("/repos/").and_then(|r| r.1.split('/').next()).ok_or("repository owner unknown")?;
        let prs = self.api.call("GET", &format!("/pulls?head={owner}:taskq-{n}&base=main&state=open"), None)?.json()?;
        let prs = prs.as_array().ok_or("cancellation PR list unknown")?;
        if prs.len() != 1 { return Err("publication PR absent or ambiguous; verify merge before cancelling".into()); }
        if prs[0]["head"]["sha"].as_str().ok_or("cancellation head unknown")? != sha { return Ok(()); }
        let id = prs[0]["node_id"].as_str().ok_or("cancellation PR id unknown")?;
        let query = "query($id: ID!) { node(id: $id) { ... on PullRequest { merged mergeQueueEntry { id } } } }";
        let read = || self.graphql(query, json!({"id": id}));
        let v = read()?;
        if v["node"]["merged"] != false { return Err("publication already merged or unknown; verify close before cancelling".into()); }
        if v["node"]["mergeQueueEntry"].is_object() {
            self.graphql("mutation($id: ID!) { dequeuePullRequest(input: {pullRequestId: $id}) { clientMutationId } }", json!({"id": id}))?;
            let back = read()?;
            if back["node"]["merged"] != false || back["node"].get("mergeQueueEntry") != Some(&Value::Null) { return Err("queue cancellation not read back; task unchanged".into()); }
        } else if v["node"].get("mergeQueueEntry") != Some(&Value::Null) { return Err("queue cancellation state unknown".into()); }
        Ok(())
    }


    fn resume_publication(&self, n: u64, sha: &str, title: &str, task_line: &str, root: &std::path::Path, enqueued: bool) -> Publish {
        outcome(self.try_publish(n, sha, "pr", title, task_line, Some(root), enqueued))
    }

    fn publish(&self, n: u64, sha: &str, mode: &str, title: &str, task_line: &str) -> Publish {
        outcome(self.try_publish(n, sha, mode, title, task_line, None, false))
    }

    fn requests(&self) -> crate::requests::Budget {
        self.api.budget.borrow().clone()
    }

    fn url(&self, n: u64) -> String {
        self.foreign_url(self.api.base.rsplit_once("/repos/").map(|x| x.1).unwrap_or_default(), n)
    }

    fn foreign(&self, repo: &str, n: u64) -> Result<Issue> {
        let v = self.api.call("GET", &format!("{}/repos/{}/issues/{n}", self.api.root, self.api.sibling("/repos/", repo)), None)?.json()?;
        Ok(parse_issue(&super::dep_reply(v, "number", n, "open")?))
    }

    fn foreign_url(&self, repo: &str, n: u64) -> String {
        let web = self.api.root.replace("api.github.com", "github.com").replace("/api/v3", "");
        format!("{web}/{}/issues/{n}", self.api.sibling("/repos/", repo))
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
