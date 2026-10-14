//! GitLab issues (taskq.md § 1, § 2, § 7): REST v4 on gitlab.com or a self-managed host, `glab` login.

use super::{Api, Board, Comment, Issue, Kind, Publish, Result, hex40, into, iso, names, outcome, s, stamp, verify_assignment};
use serde_json::{Value, json};
use std::cell::RefCell;

pub const KIND: Kind = Kind {
    name: "gitlab",
    cli: "glab",
    host_env: "GITLAB_HOST",
    user_key: "username",
    default_host: "gitlab.com",
    repo_view: None,
    token: |tool, host| tool.token(true, host),
    token_env: &["GITLAB_TOKEN"],
    api,
    board: |api| Box::new(GitLab { api, members: RefCell::new(vec![]) }),
};

fn api(host: &str, repo: &str, token: &str) -> Api {
    let root = format!("https://{host}/api/v4");
    Api::new(root.clone(), format!("{root}/projects/{}", repo.replace('/', "%2F")), token)
}

pub struct GitLab {
    pub(super) api: Api,
    members: RefCell<Vec<(u64, bool)>>,
}

fn csv(v: &[String]) -> String {
    v.join(",")
}

impl GitLab {
    fn parse_issue(&self, v: &Value) -> Issue {
        Issue {
            number: v["iid"].as_u64().unwrap_or(0),
            title: s(v, "title"),
            body: s(v, "description"),
            labels: names(&v["labels"], "name"),
            assignees: names(&v["assignees"], "username"),
            updated: iso(&s(v, "updated_at")).unwrap_or(0),
            open: v["state"] == "opened",
            trusted: self.member(v["author"]["id"].as_u64().unwrap_or(0)),
            // GitLab records no close reason: a closed issue is done (§ 1)
            reason: None,
        }
    }

    /// Reporter+ (access level 20), cached per author id.
    fn member(&self, user: u64) -> bool {
        if let Some((_, ok)) = self.members.borrow().iter().find(|(u, _)| *u == user) {
            return *ok;
        }
        let ok = self.api.call("GET", &format!("/members/all/{user}"), None).ok().and_then(|r| r.json().ok())
            .is_some_and(|v| v["access_level"].as_u64().unwrap_or(0) >= 20);
        self.members.borrow_mut().push((user, ok));
        ok
    }

    /// CI on `sha`: Ok(None) green, Ok(Some(msg)) failed, Err pending or missing.
    fn ci(&self, sha: &str) -> Result<Option<String>> {
        let v = self.api.call("GET", &format!("/repository/commits/{sha}"), None)?.json()?;
        match v["last_pipeline"]["status"].as_str() {
            Some("success") => Ok(None),
            Some(st @ ("failed" | "canceled")) => Ok(Some(format!("CI {st} on {sha}"))),
            Some(st) => Err(format!("CI {st} on {sha}")),
            None => Err(format!("no CI on {sha}")),
        }
    }

    fn publication_ancestry(&self, sha: &str, mode: &str) {
        // Only the immediate read in this close qualifies; later main changes are not incidents.
        if !hex40(sha) { return; }
        let on_main = self.api.call("GET", &format!("/repository/merge_base?refs[]=main&refs[]={sha}"), None)
            .and_then(|r| r.json()).and_then(|v| {
                let id = v["id"].as_str().filter(|id| hex40(id)).ok_or("unknown merge base")?;
                Ok(id == sha)
            });
        if on_main == Ok(false) {
            crate::incident::detect("publication.not_on_main", concat!(file!(), ":", line!()), "none", &[mode], &["ancestry:missing"]);
        }
    }

    fn try_publish(&self, n: u64, sha: &str, mode: &str, title: &str, task_line: &str) -> Result<Publish> {
        let branch = format!("taskq-{n}");
        // No commit, or the SHA is already on main: verify ancestry, close.
        if self.api.call("GET", &format!("/repository/merge_base?refs[]=main&refs[]={sha}"), None)?.json()?["id"] == sha {
            return Ok(Publish::Done(format!("{sha} already on main")));
        }
        if let Some(fail) = self.ci(sha)? {
            return Ok(Publish::Failed(fail));
        }
        // no fast-forward ref API: `direct` merges a fast-forward MR (§ 7)
        let mrs = self.api.call("GET", &format!("/merge_requests?source_branch={branch}&target_branch=main&state=opened"), None)?.json()?;
        let mrs = mrs.as_array().cloned().unwrap_or_default();
        if mrs.is_empty() {
            // § 7 merged, not closed: a merged MR at the result SHA into any target (a release branch too) is the publication; no second merge
            let merged = self.api.call("GET", &format!("/merge_requests?source_branch={branch}&state=merged"), None)?.json()?;
            if let Some(mr) = merged.as_array().into_iter().flatten().find(|m| m["sha"] == sha) {
                return Ok(Publish::Done(format!("merged !{}{}", mr["iid"], into(&s(mr, "target_branch")))));
            }
        }
        let mr = match (mrs.len(), mode) {
            (1, _) => mrs[0].clone(),
            (0, "direct") => self.api.call("POST", "/merge_requests", Some(json!({ "source_branch": branch, "target_branch": "main", "title": title })))?.json()?,
            (k, _) => return Ok(Publish::Failed(format!("{k} open MRs of {branch}, want exactly one"))),
        };
        if mr["sha"] != sha {
            return Ok(Publish::Failed(format!("MR head {} is not the result {sha}", s(&mr, "sha"))));
        }
        let iid = mr["iid"].as_u64().unwrap_or(0);
        let body = if mode == "pr" { json!({ "sha": sha, "squash": true, "squash_commit_message": format!("{title}\n\n{task_line}") }) } else { json!({ "sha": sha }) };
        let merged = self.api.call("PUT", &format!("/merge_requests/{iid}/merge"), Some(body))?;
        let back = self.api.call("GET", &format!("/merge_requests/{iid}"), None)?.json()?;
        if back["state"] != "merged" {
            if let Ok(second) = self.api.call("GET", &format!("/merge_requests/{iid}"), None).and_then(|r| r.json())
                && second["state"] != "merged"
            {
                crate::incident::detect("publication.readback", concat!(file!(), ":", line!()), "none", &[mode], &["merge:unconfirmed"]);
            }
        }
        if back["state"] == "merged" {
            let merged = merged.json().ok();
            let target = if mode == "direct" { sha } else { merged.as_ref().and_then(|v| v["squash_commit_sha"].as_str()).unwrap_or("") };
            self.publication_ancestry(target, mode);
        }
        if back["state"] == "merged" { Ok(Publish::Done(format!("merged !{iid}"))) } else { Err(format!("!{iid} not merged yet")) }
    }
}

impl Board for GitLab {
    fn canonical_labels(&self) -> Result<bool> {
        self.api.canonical_labels()
    }

    fn ensure_labels(&self, names: &[String]) -> Result<()> {
        self.api.ensure_labels(names, |c| format!("#{c}"), |current, payload| {
            let (method, path) = match current {
                Some(v) => ("PUT", format!("/labels/{}", v["id"].as_u64().ok_or("label has no id")?)),
                None => ("POST", "/labels".into()),
            };
            self.api.call(method, &path, Some(payload)).map(|_| ())
        })
    }

    fn issues(&self, since: Option<i64>) -> Result<Vec<Issue>> {
        let url = match since {
            Some(t) => format!("/issues?state=all&per_page=100&updated_after={}", stamp(t)),
            None => "/issues?state=opened&per_page=100".into(),
        };
        Ok(self.api.all(&url)?.iter().map(|v| self.parse_issue(v)).collect())
    }

    fn issue(&self, n: u64) -> Result<Issue> {
        Ok(self.parse_issue(&self.api.call("GET", &format!("/issues/{n}"), None)?.json()?))
    }

    fn comments(&self, n: u64) -> Result<Vec<Comment>> {
        Ok(self.api.all(&format!("/issues/{n}/notes?sort=asc&order_by=created_at&per_page=100"))?.iter()
            .filter(|v| v["system"].as_bool() != Some(true))
            .map(|v| Comment {
                id: v["id"].as_u64().unwrap_or(0),
                created: iso(&s(v, "created_at")).unwrap_or(0),
                body: s(v, "body"),
                trusted: self.member(v["author"]["id"].as_u64().unwrap_or(0)),
                edited: s(v, "updated_at") != s(v, "created_at"),
            }).collect())
    }

    fn comment(&self, n: u64, body: &str) -> Result<u64> {
        self.api.call("POST", &format!("/issues/{n}/notes"), Some(json!({ "body": body })))?.json()?["id"].as_u64().ok_or("no comment id".into())
    }

    fn edit_comment(&self, n: u64, id: u64, body: &str) -> Result<()> {
        self.api.call("PUT", &format!("/issues/{n}/notes/{id}"), Some(json!({ "body": body }))).map(|_| ())
    }

    fn labels(&self, n: u64, add: &[String], remove: &[String]) -> Result<()> {
        if !add.is_empty() && self.canonical_labels()? { self.ensure_labels(add)?; }
        self.api.call("PUT", &format!("/issues/{n}"), Some(json!({ "add_labels": csv(add), "remove_labels": csv(remove) })))?;
        verify_assignment(self, n, add, remove)
    }

    fn create(&self, title: &str, body: &str, labels: &[String]) -> Result<u64> {
        if let Some(names) = self.api.creation_labels(labels)? { self.ensure_labels(&names)?; }
        let n = self.api.call("POST", "/issues", Some(json!({ "title": title, "description": body, "labels": csv(labels) })))?.json()?["iid"].as_u64().ok_or("no issue number")?;
        verify_assignment(self, n, labels, &[])?;
        Ok(n)
    }

    fn edit(&self, n: u64, body: &str) -> Result<()> {
        self.api.call("PUT", &format!("/issues/{n}"), Some(json!({ "description": body }))).map(|_| ())
    }

    fn close(&self, n: u64) -> Result<()> {
        self.api.call("PUT", &format!("/issues/{n}"), Some(json!({ "state_event": "close" }))).map(|_| ())
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
        self.foreign_url(&self.api.base.rsplit_once("/projects/").map(|x| x.1.replace("%2F", "/")).unwrap_or_default(), n)
    }

    fn foreign(&self, repo: &str, n: u64) -> Result<Issue> {
        let path = self.api.sibling("/projects/", repo).replace('/', "%2F");
        let v = self.api.call("GET", &format!("{}/projects/{path}/issues/{n}", self.api.root), None)?.json()?;
        // trust is for log entries; a dependency is judged only by its state
        Ok(Issue { number: v["iid"].as_u64().unwrap_or(0), open: v["state"] == "opened", ..Default::default() })
    }

    fn foreign_url(&self, repo: &str, n: u64) -> String {
        format!("{}/{}/-/issues/{n}", self.api.root.trim_end_matches("/api/v4"), self.api.sibling("/projects/", repo))
    }
}
