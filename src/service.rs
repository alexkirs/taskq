//! The host service (taskq.md § 3, § 4): one loop around `fold`. It owns this host's children and its heartbeat issue;
//! everything it knows about other hosts comes from the board.

use crate::board::{Board, Comment, Issue, entries};
use crate::core::{self, State, Task};
use crate::runtime::{Launch, Proc};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;

#[derive(Clone)]
pub struct Config {
    pub root: PathBuf,
    pub install: PathBuf,
    pub board: crate::board::Kind,
    pub web_host: String,
    pub repo: String,
    pub publish: String,
    pub me: String,
    pub limits: BTreeMap<String, u32>,
    /// What this host offers for `needs-<x>` labels.
    pub has: Vec<String>,
    pub permission_mode: String,
    pub codex: Vec<String>,
    pub ttl: i64,
    pub poll: u64,
    pub update_every: u64,
    pub rollback_after: u64,
}

pub const HOST_LABEL: &str = "taskq-host";
pub const TASK_LABEL: &str = "taskq";

/// Only issues labelled `taskq` are tasks (§ 1): a user issue never starts by itself.
pub fn is_task(i: &Issue) -> bool {
    i.trusted && i.labels.iter().any(|l| l == TASK_LABEL)
}

pub fn is_host(i: &Issue) -> bool {
    i.labels.iter().any(|l| l == HOST_LABEL) && i.title.starts_with("taskq host ")
}

fn label(i: &Issue, prefix: &str) -> Option<String> {
    i.labels.iter().find_map(|l| l.strip_prefix(prefix).map(String::from))
}

pub fn priority(i: &Issue) -> u8 {
    if crate::labels::matches(&i.labels, "p0") { 0 } else if crate::labels::matches(&i.labels, "p1") { 1 } else { 2 }
}

/// The host named in a `spawn` session (`<rt>:<session> <host>`).
fn host_of(t: &Task) -> Option<&str> {
    t.session.as_deref()?.rsplit(' ').next()
}

/// `rt` and the session id of a `spawn` session.
fn rt_of(t: &Task) -> Option<(&str, &str)> {
    t.session.as_deref()?.split(' ').next()?.split_once(':')
}

/// Set the `q-<state>` label from the fold; a closed task loses its `q-` labels and its issue is closed (§ 2).
pub fn project(board: &dyn Board, issue: &Issue, t: &Task) -> Result<(), String> {
    let want = board.state_label(t.state.name())?;
    let stale: Vec<String> = issue.labels.iter().filter(|l| l.starts_with("q-") && **l != want).cloned().collect();
    if t.state == State::Closed {
        board.labels(issue.number, &[], &stale)?;
        board.close(issue.number)
    } else if !stale.is_empty() || !issue.labels.contains(&want) {
        board.labels(issue.number, &[want], &stale)
    } else {
        Ok(())
    }
}

pub struct View {
    pub issue: Issue,
    pub task: Task,
    pub comments: Vec<Comment>,
    pub log: Vec<core::Entry>,
}

/// Fold every open task (heartbeat issues and untrusted authors excluded).
/// Remove `.worktrees/taskq-<N>` of closed tasks and their merged local branch (§ 7, R11). Without `force` git refuses
/// dirty worktrees, so those stay; with it the uncommitted work goes to `.taskq/trash/taskq-<N>.patch` first. Commits
/// stay in the branch: `git branch -d` keeps an unmerged one. `ponytail:` a squash-merged branch stays until a manual
/// `git branch -D`.
pub fn cleanup(board: &dyn Board, root: &std::path::Path, mut skip: impl FnMut(u64) -> bool, force: bool) -> Vec<String> {
    let mut log = vec![];
    let Ok(dir) = std::fs::read_dir(root.join(".worktrees")) else { return log };
    let git = |dir: &std::path::Path, args: &[&str]| std::process::Command::new("git").arg("-C").arg(dir).args(args).output().ok().filter(|o| o.status.success()).map(|o| o.stdout);
    for e in dir.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(n) = name.strip_prefix("taskq-").and_then(|n| n.parse::<u64>().ok()) else { continue };
        if skip(n) || !board.issue(n).is_ok_and(|i| !i.open) {
            continue; // a fresh read must show the issue closed (R12)
        }
        let wt = format!(".worktrees/{name}");
        let removed = if force {
            let trash = root.join(".taskq/trash");
            let saved = git(&e.path(), &["add", "-A"]).and_then(|_| git(&e.path(), &["diff", "--cached", "--binary"])).is_some_and(|patch| {
                patch.is_empty() || std::fs::create_dir_all(&trash).and_then(|_| std::fs::write(trash.join(format!("{name}.patch")), &patch)).is_ok()
            });
            saved && git(root, &["worktree", "remove", "--force", &wt]).is_some()
        } else {
            git(root, &["worktree", "remove", &wt]).is_some()
        };
        if removed {
            git(root, &["branch", "-d", &name]);
            log.push(format!("#{n} worktree removed"));
        } else {
            log.push(format!("#{n} worktree kept: {}", if force { "could not save its patch" } else { "uncommitted work, see `taskq cleanup --force`" }));
        }
    }
    log
}

pub fn views(board: &dyn Board, cache: &mut HashMap<u64, (i64, Vec<Comment>)>) -> Result<(Vec<View>, Vec<Issue>), String> {
    let issues = board.issues(None)?;
    let open: HashSet<u64> = issues.iter().map(|i| i.number).collect();
    let (hosts, tasks): (Vec<Issue>, Vec<Issue>) = issues.into_iter().filter(|i| i.trusted).partition(is_host);
    let mut out = vec![];
    for issue in tasks.into_iter().filter(is_task) {
        let comments = match cache.get(&issue.number) {
            Some((u, c)) if *u == issue.updated => c.clone(),
            _ => board.comments(issue.number)?,
        };
        cache.insert(issue.number, (issue.updated, comments.clone()));
        let log: Vec<_> = comments.iter().filter(|c| c.trusted).map(|c| core::Entry { id: c.id, body: c.body.clone(), edited: c.edited }).collect();
        let deps_open = core::depends(&issue.body).iter().any(|d| open.contains(d));
        out.push(View { task: core::fold(issue.number, &log, deps_open), issue, comments, log });
    }
    Ok((out, hosts))
}

pub struct Kid {
    pub task: u64,
    pub attempt: u64,
    pub rt: String,
    pub proc: Box<dyn Proc>,
}

pub struct Service<'a> {
    pub board: &'a dyn Board,
    pub cfg: &'a Config,
    pub version: String,
    pub kids: Vec<Kid>,
    /// Labels from `<root>/.taskq/focus`; empty: admit nothing, `all`: any task; `p0` passes any non-empty focus (§ 4).
    pub focus: Vec<String>,
    /// The last inbox id handed to each attempt's child.
    delivered: HashMap<u64, u64>,
    cache: HashMap<u64, (i64, Vec<Comment>)>,
    /// Worktrees of closed tasks already tried once by `prune`.
    pruned: HashSet<u64>,
    start: Box<dyn FnMut(&Launch) -> Result<Box<dyn Proc>, String> + 'a>,
    pub log: Vec<String>,
    /// Open tasks of the last pass: number, title, state, labels (for `status.json` and the tray).
    pub tasks: Vec<(u64, String, &'static str, Vec<String>)>,
    /// State of each open task in the previous pass, for `events`.
    seen: HashMap<u64, State>,
    /// First time covered by `<root>/.taskq/events` (§ 4).
    pub events_since: Option<i64>,
}

const MONTH: i64 = 30 * 86400;

/// Append `(task, kind, attempt)` moves to the events file at board time `now` (§ 4): one line per attempt and kind,
/// lines older than 30 days dropped. Returns the first time covered.
pub fn record(path: &std::path::Path, now: i64, moves: &[(u64, &str, u64)]) -> Result<i64, String> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let mut lines = text.lines();
    let since = lines.next().and_then(|l| l.strip_prefix("since ")?.parse().ok()).unwrap_or(now).max(now - MONTH);
    let mut keep: Vec<String> = lines.filter(|l| l.split(' ').next().and_then(|t| t.parse::<i64>().ok()).is_some_and(|t| t >= since)).map(String::from).collect();
    for (n, kind, a) in moves {
        if !keep.iter().any(|l| l.ends_with(&format!(" {kind} {a}"))) {
            keep.push(format!("{now} #{n} {kind} {a}"));
        }
    }
    let out: String = std::iter::once(format!("since {since}")).chain(keep).map(|l| l + "\n").collect();
    if out != text {
        crate::platform::write_atomic(path, &out)?;
    }
    Ok(since)
}

impl<'a> Service<'a> {
    pub fn new(board: &'a dyn Board, cfg: &'a Config, version: &str, start: Box<dyn FnMut(&Launch) -> Result<Box<dyn Proc>, String> + 'a>) -> Self {
        Service { board, cfg, version: version.into(), kids: vec![], focus: vec![], delivered: HashMap::new(), cache: HashMap::new(), pruned: HashSet::new(), start, log: vec![], tasks: vec![], seen: HashMap::new(), events_since: None }
    }

    /// Children of the previous process image after an update re-exec; their inbox counts as delivered.
    pub fn adopt(&mut self, kid: Kid, delivered: u64) {
        self.delivered.insert(kid.attempt, delivered);
        self.kids.push(kid);
    }

    pub fn delivered(&self, attempt: u64) -> u64 {
        self.delivered.get(&attempt).copied().unwrap_or(0)
    }

    fn post(&mut self, n: u64, body: String) -> Result<u64, String> {
        self.log.push(format!("#{n} {}", body.lines().next().unwrap_or_default()));
        self.cache.remove(&n);
        self.board.comment(n, &body)
    }

    fn heartbeat_body(&self) -> String {
        let mut lines = vec![format!("version {}", self.version)];
        let mut kids: Vec<_> = self.kids.iter().map(|k| (k.task, k.attempt)).collect();
        kids.sort();
        lines.extend(kids.iter().map(|(t, a)| format!("#{t} {a}")));
        lines.join("\n")
    }

    /// One pass of § 4. Tasks written to in a step are left alone by later steps until the next pass re-reads them.
    pub fn pass(&mut self) -> Result<(), String> {
        let me = self.cfg.me.clone();
        let ttl = self.cfg.ttl;
        let (views, hosts) = views(self.board, &mut self.cache)?;
        self.tasks = views.iter().filter(|v| v.task.state != State::Closed).map(|v| (v.issue.number, v.issue.title.clone(), v.task.state.name(), v.issue.labels.clone())).collect();
        let now = self.board.now()?;
        self.events(&views, now)?;
        let by_n: HashMap<u64, &View> = views.iter().map(|v| (v.issue.number, v)).collect();
        let mut touched = HashSet::new();

        // 1. reap
        let mut exited = vec![];
        self.kids.retain_mut(|k| match k.proc.exited() {
            Some(code) => {
                exited.push((k.task, k.attempt, code));
                false
            }
            None => true,
        });
        for (n, a, code) in exited {
            if let Some(v) = by_n.get(&n)
                && v.task.attempt == Some(a)
                && matches!(v.task.state, State::Doing | State::Review)
            {
                self.post(n, core::entry("gone", &[], Some(a), &format!("exit {code}")))?;
                touched.insert(n);
            }
        }

        // 2. stop children of replaced attempts (R11)
        self.kids.retain_mut(|k| {
            let current = by_n.get(&k.task).is_some_and(|v| v.task.attempt == Some(k.attempt));
            if !current {
                k.proc.kill();
            }
            current
        });
        self.prune(&by_n);

        // 3. heartbeat
        let body = self.heartbeat_body();
        let title = format!("taskq host {me}");
        match hosts.iter().find(|h| h.title == title) {
            Some(h) if h.body.lines().filter(|l| !l.starts_with("at ")).eq(body.lines()) && now - h.updated < ttl / 5 => {}
            // a same-body edit leaves `updated_at` as is (GitHub), so the stamp keeps the heartbeat fresh
            Some(h) => self.board.edit(h.number, &format!("{body}\nat {now}"))?,
            None => {
                self.board.create(&title, &body, &[HOST_LABEL.to_string()])?;
            }
        }

        // 4. gone for attempts of dead hosts (§ 3)
        for v in &views {
            let (n, t) = (v.issue.number, &v.task);
            let (Some(a), Some(h)) = (t.attempt, host_of(t)) else { continue };
            if h == me || !matches!(t.state, State::Doing | State::Review) || touched.contains(&n) {
                continue;
            }
            let young = v.comments.iter().find(|c| c.id == a).is_some_and(|c| now - c.created <= ttl);
            let dead = match hosts.iter().find(|i| i.title == format!("taskq host {h}")) {
                None => !young,
                Some(hb) => now - hb.updated > ttl || (!young && !hb.body.lines().any(|l| l == format!("#{n} {a}"))),
            };
            if dead {
                self.post(n, core::entry("gone", &[], Some(a), &format!("host {h} lease expired")))?;
                touched.insert(n);
            }
        }

        // 5. projected labels
        for v in &views {
            let n = v.issue.number;
            if touched.contains(&n) {
                continue;
            }
            project(self.board, &v.issue, &v.task)?;
            if v.task.state == State::Closed {
                touched.insert(n);
            }
        }

        // 6. route to own attempts, then admit within capacity
        for v in &views {
            let (n, t) = (v.issue.number, &v.task);
            let (Some(a), Some(h)) = (t.attempt, host_of(t)) else { continue };
            if h != me || !matches!(t.state, State::Doing | State::Review) || touched.contains(&n) || self.kids.iter().any(|k| k.attempt == a) {
                continue;
            }
            let seen = self.delivered.get(&a).copied().unwrap_or(0);
            let new: Vec<&Comment> = v.comments.iter().filter(|c| t.inbox.contains(&c.id) && c.id > seen).collect();
            let Some(last) = new.last().map(|c| c.id) else {
                self.post(n, core::entry("gone", &[], Some(a), "no child on its host"))?;
                touched.insert(n);
                continue;
            };
            let (rt, session) = rt_of(t).unwrap_or(("", ""));
            let mut session = session.to_string();
            let mut l = Launch { task: n, title: v.issue.title.clone(), attempt: a, role: t.role.clone().unwrap_or_default(), rt: rt.into(), session: session.clone(), url: self.board.url(n), resume: None };
            if session == "-" {
                session = crate::runtime::codex_thread(&crate::runtime::log_path(&self.cfg.root, &l)).unwrap_or_default();
            }
            let text: Vec<String> = new.iter().map(|c| core::delivery(n, &v.log, c.id)).collect();
            l.session = session;
            l.resume = Some(text.join("\n\n"));
            if l.session.is_empty() {
                self.post(n, core::entry("gone", &[], Some(a), "session unknown"))?;
                touched.insert(n);
                continue;
            }
            self.delivered.insert(a, last);
            self.launch(l)?;
        }
        if self.focus.is_empty() {
            return Ok(());
        }
        let user = self.board.me().unwrap_or_default();
        let mut queue: Vec<&View> = views
            .iter()
            .filter(|v| !touched.contains(&v.issue.number))
            .filter(|v| v.task.state == State::Ready || (v.task.state == State::Review && v.task.attempt.is_none()))
            .filter(|v| {
                if let Err(e) = crate::labels::validate(&v.issue.labels) { eprintln!("#{}: {e}; not admitted", v.issue.number); return false; }
                true
            })
            .filter(|v| label(&v.issue, "host-").is_none_or(|h| h == me))
            .filter(|v| !v.issue.labels.iter().any(|l| l == "assignee-only") || v.issue.assignees.contains(&user))
            .filter(|v| v.issue.labels.iter().filter_map(|l| l.strip_prefix("needs-")).all(|x| self.cfg.has.iter().any(|h| h == x)))
            .filter(|v| priority(&v.issue) == 0 || self.focus.iter().any(|f| f == "all" || crate::labels::matches(&v.issue.labels, f)))
            .collect();
        queue.sort_by_key(|v| (priority(&v.issue), v.issue.number));
        for v in queue {
            let Some(rt) = self.free(label(&v.issue, "run-").as_deref()) else { continue };
            let role = if v.task.state == State::Review { "reviewer" } else { "worker" };
            self.claim(v, role, &rt)?;
        }
        Ok(())
    }

    /// Moves to closed, `failed`, `ask` or `waiting` since the previous pass go to `<root>/.taskq/events` (§ 4); a task
    /// new since then counts from its first state. A task gone from the open list counts only when a fresh read shows it
    /// closed (R12).
    fn events(&mut self, views: &[View], now: i64) -> Result<(), String> {
        // the attempt current at the move; a `close` or `failed` that ended it leaves the last spawn
        let attempt = |t: &Task, c: &[Comment]| t.attempt.or_else(|| c.iter().rev().find(|c| c.trusted && c.body.starts_with("taskq: spawn ")).map(|c| c.id));
        let now_seen: HashMap<u64, State> = views.iter().map(|v| (v.issue.number, v.task.state)).collect();
        let first = self.seen.is_empty() && self.events_since.is_none();
        let mut ns: Vec<u64> = self.seen.keys().copied().collect();
        if !first {
            ns.extend(views.iter().map(|v| v.issue.number).filter(|n| !self.seen.contains_key(n)));
        }
        let mut moves = vec![];
        for n in ns {
            let (to, a) = match views.iter().find(|v| v.issue.number == n) {
                // `ask` and `waiting` without an attempt: the last comment, else the issue number
                Some(v) if matches!(v.task.state, State::Ask | State::Waiting) => (v.task.state, attempt(&v.task, &v.comments).or(v.comments.last().map(|c| c.id)).or(Some(n))),
                Some(v) => (v.task.state, attempt(&v.task, &v.comments)),
                None if self.board.issue(n).is_ok_and(|i| !i.open) => (State::Closed, attempt(&Task::default(), &self.board.comments(n)?)),
                None => continue,
            };
            let kind = match to {
                State::Closed => "close",
                State::Failed => "failed",
                State::Ask => "ask",
                State::Waiting => "blocked",
                _ => continue,
            };
            if let Some(a) = a.filter(|_| self.seen.get(&n) != Some(&to)) {
                moves.push((n, kind, a));
            }
        }
        moves.sort();
        self.seen = now_seen;
        if first || !moves.is_empty() || self.events_since.is_none_or(|s| s < now - MONTH) {
            let path = crate::dot(&self.cfg.root).join("events");
            match record(&path, now, &moves) {
                Ok(s) => self.events_since = Some(s),
                Err(e) => self.log.push(format!("events: {e}")),
            }
        }
        for (n, kind, a) in moves {
            self.log.push(format!("#{n} event {kind} {a}"));
        }
        Ok(())
    }

    fn prune(&mut self, open: &HashMap<u64, &View>) {
        let tried = &mut self.pruned;
        let kids: Vec<u64> = self.kids.iter().map(|k| k.task).collect();
        let skip = |n: u64| open.contains_key(&n) || kids.contains(&n) || !tried.insert(n);
        self.log.extend(cleanup(self.board, &self.cfg.root, skip, false));
    }

    /// A runtime with free capacity: `limits.<rt>` minus live children of it (§ 3).
    fn free(&self, want: Option<&str>) -> Option<String> {
        self.cfg.limits.iter().filter(|(rt, _)| want.is_none_or(|w| w == rt.as_str())).find(|(rt, n)| self.kids.iter().filter(|k| &k.rt == *rt).count() < **n as usize).map(|(rt, _)| rt.clone())
    }

    /// Post `spawn`, re-read, start the child only if `fold` accepted this spawn (§ 3 claim).
    fn claim(&mut self, v: &View, role: &str, rt: &str) -> Result<(), String> {
        let n = v.issue.number;
        let session = crate::runtime::new_session(rt);
        let id = self.post(n, core::entry("spawn", &[role, &format!("{rt}:{session}"), &self.cfg.me], None, ""))?;
        let t = core::fold(n, &entries(self.board, n)?, false); // only ready or review tasks are claimed
        if t.attempt != Some(id) {
            self.log.push(format!("#{n} spawn {id} lost the claim"));
            return Ok(());
        }
        let l = Launch { task: n, title: v.issue.title.clone(), attempt: id, role: role.into(), rt: rt.into(), session, url: self.board.url(n), resume: None };
        self.delivered.insert(id, t.inbox.last().copied().unwrap_or(0));
        self.launch(l)
    }

    fn launch(&mut self, l: Launch) -> Result<(), String> {
        match (self.start)(&l) {
            Ok(proc) => {
                self.kids.push(Kid { task: l.task, attempt: l.attempt, rt: l.rt.clone(), proc });
                // the heartbeat lists the new child before the next pass judges anything (§ 3 lease)
                let body = self.heartbeat_body();
                if let Some(h) = self.board.issues(None)?.into_iter().find(|i| is_host(i) && i.title == format!("taskq host {}", self.cfg.me)) {
                    self.board.edit(h.number, &body)?;
                }
                Ok(())
            }
            Err(e) => {
                self.post(l.task, core::entry("gone", &[], Some(l.attempt), &format!("start failed: {e}"))).map(|_| ())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::{Board, Fake};
    use std::cell::RefCell;
    use std::rc::Rc;

    struct FakeProc(Rc<RefCell<Option<i32>>>);
    impl Proc for FakeProc {
        fn exited(&mut self) -> Option<i32> {
            *self.0.borrow()
        }
        fn kill(&mut self) {
            *self.0.borrow_mut() = Some(-9);
        }
        fn id(&self) -> u32 {
            0
        }
    }

    fn cfg(me: &str) -> Config {
        Config {
            root: PathBuf::from("/nonexistent"),
            install: PathBuf::from("/nonexistent"),
            board: crate::board::Kind::GitHub,
            web_host: String::new(),
            repo: "o/r".into(),
            publish: "pr".into(),
            me: me.into(),
            limits: BTreeMap::from([("claude".to_string(), 1)]),
            has: vec![],
            permission_mode: "dontAsk".into(),
            codex: vec![],
            ttl: 300,
            poll: 60,
            update_every: 900,
            rollback_after: 600,
        }
    }

    type Exits = Rc<RefCell<Vec<(u64, Rc<RefCell<Option<i32>>>)>>>;

    fn service<'a>(b: &'a Fake, c: &'a Config, exits: Exits) -> Service<'a> {
        let mut s = Service::new(b, c, "4.0.0", Box::new(move |l: &Launch| {
            let flag = Rc::new(RefCell::new(None));
            exits.borrow_mut().push((l.task, flag.clone()));
            Ok(Box::new(FakeProc(flag)) as Box<dyn Proc>)
        }));
        s.focus = vec!["all".into()];
        s
    }

    fn task(b: &Fake, n: u64) -> Task {
        core::fold(n, &entries(b, n).unwrap(), false)
    }

    #[test]
    fn two_hosts_admit_one_attempt_and_the_survivor_recovers_a_dead_host() {
        let b = Fake::default();
        let n = b.create("T", "## Goal\ng", &["taskq".into()]).unwrap();
        let (ca, cb) = (cfg("mac"), cfg("win"));
        let (ea, eb): (Exits, Exits) = Default::default();
        let mut a = service(&b, &ca, ea.clone());
        let mut w = service(&b, &cb, eb.clone());
        a.pass().unwrap();
        w.pass().unwrap();
        let t = task(&b, n);
        assert_eq!((t.state, t.role.as_deref()), (State::Doing, Some("worker")));
        assert_eq!((a.kids.len(), w.kids.len()), (1, 0));
        assert!(b.issue(n).unwrap().labels.contains(&"q-doing".to_string()));

        // a live mac outlasts the ttl: its unchanged heartbeat stays fresh, win leaves the task alone
        for _ in 0..3 {
            b.advance(200);
            a.pass().unwrap();
            w.pass().unwrap();
        }
        assert_eq!(host_of(&task(&b, n)), Some("mac"));
        // the mac host dies: win waits out the ttl, then posts gone and takes the task
        b.advance(301);
        w.pass().unwrap();
        assert_eq!(task(&b, n).state, State::Ready);
        w.pass().unwrap();
        let t = task(&b, n);
        assert_eq!((t.state, host_of(&t)), (State::Doing, Some("win")));
        assert_eq!(t.retries, 1);
    }

    #[test]
    fn exit_without_result_is_gone_and_result_brings_a_reviewer() {
        let b = Fake::default();
        let n = b.create("T", "", &["taskq".into()]).unwrap();
        let c = cfg("mac");
        let e: Exits = Default::default();
        let mut s = service(&b, &c, e.clone());
        s.pass().unwrap();
        let a = task(&b, n).attempt.unwrap();
        b.comment(n, &core::entry("result", &["0123456789abcdef0123456789abcdef01234567"], Some(a), "ok")).unwrap();
        *e.borrow()[0].1.borrow_mut() = Some(0);
        s.pass().unwrap();
        let t = task(&b, n);
        assert_eq!((t.state, t.role.as_deref(), t.retries), (State::Review, Some("reviewer"), 0));
        let r = t.attempt.unwrap();
        *e.borrow()[1].1.borrow_mut() = Some(1);
        s.pass().unwrap(); // reviewer exits without a verdict: gone keeps the result
        let t = task(&b, n);
        assert_eq!((t.state, t.retries, t.result.is_some()), (State::Review, 1, true));
        assert_ne!(t.attempt, Some(r));
    }

    #[test]
    fn answer_resumes_the_same_session_and_requeue_stops_the_child() {
        let b = Fake::default();
        let n = b.create("T", "", &["taskq".into()]).unwrap();
        let c = cfg("mac");
        let e: Exits = Default::default();
        let mut s = service(&b, &c, e.clone());
        s.pass().unwrap();
        let a = task(&b, n).attempt.unwrap();
        b.comment(n, &core::entry("ask", &[], Some(a), "Which?\n1. x\n2. y ★")).unwrap();
        *e.borrow()[0].1.borrow_mut() = Some(0);
        s.pass().unwrap();
        assert_eq!(task(&b, n).state, State::Ask);
        b.comment(n, &core::entry("answer", &["2"], None, "y")).unwrap();
        s.pass().unwrap();
        assert_eq!(e.borrow().len(), 2, "resumed once");
        let t = task(&b, n);
        assert_eq!((t.state, t.attempt), (State::Doing, Some(a)));
        s.pass().unwrap();
        assert_eq!(e.borrow().len(), 2, "an answer is delivered once");
        b.comment(n, &core::entry("requeue", &[], None, "redo")).unwrap();
        s.focus = vec![];
        s.pass().unwrap();
        assert_eq!(*e.borrow()[1].1.borrow(), Some(-9));
        assert!(s.kids.is_empty());
    }

    #[test]
    fn waiting_and_filters_hold_admission() {
        let b = Fake::default();
        let dep = b.create("dep", "", &["later".into()]).unwrap();
        b.comment(dep, &core::entry("later", &[], None, "")).unwrap();
        let n = b.create("T", &format!("Depends on #{dep}"), &["taskq".into()]).unwrap();
        let other = b.create("O", "", &["taskq".into(), "host-win".into()]).unwrap();
        let user = b.create("U", "", &[]).unwrap();
        let c = cfg("mac");
        let mut s = service(&b, &c, Default::default());
        s.pass().unwrap();
        assert_eq!(task(&b, other).state, State::Ready);
        assert!(b.issue(n).unwrap().labels.contains(&"q-blocked".to_string()));
        b.close(dep).unwrap();
        s.pass().unwrap();
        assert_eq!(task(&b, n).state, State::Doing);
        assert!(b.comments(user).unwrap().is_empty() && b.issue(user).unwrap().labels.is_empty(), "no taskq label: never a task");
    }

    #[test]
    fn canonical_and_legacy_priorities_filters_and_conflicts() {
        let b = Fake::default();
        let old = b.create("old", "", &["taskq".into(), "priority-1".into()]).unwrap();
        let new = b.create("new", "", &["taskq".into(), "p1".into()]).unwrap();
        let conflict = b.create("bad", "", &["taskq".into(), "p1".into(), "p2".into()]).unwrap();
        assert_eq!(priority(&b.issue(old).unwrap()), priority(&b.issue(new).unwrap()));
        let mut c = cfg("mac"); c.limits.insert("claude".into(), 3);
        let mut s = service(&b, &c, Default::default()); s.focus = vec!["p1".into()];
        s.pass().unwrap();
        assert_eq!(task(&b, old).state, State::Doing);
        assert_eq!(task(&b, new).state, State::Doing);
        assert_eq!(task(&b, conflict).state, State::Ready);
        assert!(b.comments(conflict).unwrap().is_empty(), "conflicting restriction never claimed");
    }

    #[test]
    fn p0_passes_any_focus_but_not_pause_or_limits() {
        let b = Fake::default();
        let p1 = b.create("P1", "", &["taskq".into(), "p1".into(), "area-maps".into()]).unwrap();
        let p0 = b.create("P0", "", &["taskq".into(), "p0".into(), "area-ui".into()]).unwrap();
        let other = b.create("O", "", &["taskq".into(), "area-ui".into()]).unwrap();
        assert!(priority(&b.issue(p0).unwrap()) < priority(&b.issue(p1).unwrap()));
        let c = cfg("mac");
        let mut s = service(&b, &c, Default::default());
        s.focus = vec![];
        s.pass().unwrap();
        assert_eq!(task(&b, p0).state, State::Ready, "pause: p0 waits too");
        s.focus = vec!["area-maps".into()];
        s.pass().unwrap();
        assert_eq!((task(&b, p0).state, task(&b, p1).state, task(&b, other).state), (State::Doing, State::Ready, State::Ready), "p0 first; limit 1 holds");
    }

    #[test]
    fn focus_and_needs_choose_tasks() {
        let b = Fake::default();
        let win = b.create("W", "", &["taskq".into(), "needs-windows".into()]).unwrap();
        let maps = b.create("M", "", &["taskq".into(), "area-maps".into()]).unwrap();
        let other = b.create("O", "", &["taskq".into()]).unwrap();
        let c = cfg("mac");
        let mut s = service(&b, &c, Default::default());
        s.focus = vec![];
        s.pass().unwrap();
        assert_eq!(task(&b, other).state, State::Ready, "focus off: nothing starts");
        s.focus = vec!["area-maps".into()];
        s.pass().unwrap();
        assert_eq!((task(&b, maps).state, task(&b, other).state), (State::Doing, State::Ready));
        let mut c = cfg("mac");
        c.limits.insert("claude".into(), 9);
        let mut s = service(&b, &c, Default::default());
        s.pass().unwrap();
        assert_eq!((task(&b, win).state, task(&b, other).state), (State::Ready, State::Doing), "mac has no windows");
        let w = Config { has: vec!["windows".into()], ..c.clone() };
        let mut s = service(&b, &w, Default::default());
        s.pass().unwrap();
        assert_eq!(task(&b, win).state, State::Doing);
    }

    #[test]
    fn events_append_dedup_and_trim() {
        let p = std::env::temp_dir().join(format!("taskq-events-{}", std::process::id()));
        let _ = std::fs::remove_file(&p);
        assert_eq!(record(&p, 1000, &[(1, "close", 7)]).unwrap(), 1000);
        record(&p, 2000, &[(1, "close", 7), (2, "failed", 7), (3, "close", 8)]).unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "since 1000\n1000 #1 close 7\n2000 #2 failed 7\n2000 #3 close 8\n", "same attempt and kind once");
        let late = 1500 + MONTH;
        assert_eq!(record(&p, late, &[]).unwrap(), 1500);
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "since 1500\n2000 #2 failed 7\n2000 #3 close 8\n", "older than 30 days dropped");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn a_pass_records_close_and_failed_and_a_restart_keeps_them() {
        let root = std::env::temp_dir().join(format!("taskq-ev-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join(".taskq")).unwrap();
        let b = Fake::default();
        let (x, y) = (b.create("X", "", &["taskq".into()]).unwrap(), b.create("Y", "", &["taskq".into()]).unwrap());
        let mut c = cfg("mac");
        c.root = root.clone();
        c.limits.insert("claude".into(), 2);
        let e: Exits = Default::default();
        let mut s = service(&b, &c, e.clone());
        s.pass().unwrap();
        let (ax, ay) = (task(&b, x).attempt.unwrap(), task(&b, y).attempt.unwrap());
        b.comment(x, &core::entry("result", &["0123456789abcdef0123456789abcdef01234567"], Some(ax), "ok")).unwrap();
        *e.borrow()[0].1.borrow_mut() = Some(0);
        s.pass().unwrap();
        let r = task(&b, x).attempt.unwrap();
        b.comment(x, &core::entry("close", &[], Some(r), "ok")).unwrap();
        b.close(x).unwrap();
        b.comment(y, &core::entry("failed", &[], None, "no way")).unwrap();
        s.pass().unwrap();
        let ev = std::fs::read_to_string(root.join(".taskq/events")).unwrap();
        let lines: Vec<&str> = ev.lines().skip(1).map(|l| l.split_once(' ').unwrap().1).collect();
        assert_eq!(lines, [format!("#{x} close {r}"), format!("#{y} failed {ay}")], "{ev}");
        let mut again = service(&b, &c, Default::default());
        again.pass().unwrap();
        again.pass().unwrap();
        assert_eq!(std::fs::read_to_string(root.join(".taskq/events")).unwrap(), ev, "a restart keeps history");
        assert!(again.events_since.is_some());
        // a new task enters waiting (no comment: the issue number), a worker asks (its attempt)
        let z = b.create("Z", &format!("Depends on #{y}"), &["taskq".into()]).unwrap();
        let w = b.create("W", "", &["taskq".into()]).unwrap();
        again.pass().unwrap();
        let aw = task(&b, w).attempt.unwrap();
        b.comment(w, &core::entry("ask", &[], Some(aw), "Which?\n1. x\n2. y ★")).unwrap();
        again.pass().unwrap();
        again.pass().unwrap();
        let ev = std::fs::read_to_string(root.join(".taskq/events")).unwrap();
        let lines: Vec<&str> = ev.lines().skip(3).map(|l| l.split_once(' ').unwrap().1).collect();
        assert_eq!(lines, [format!("#{z} blocked {z}"), format!("#{w} ask {aw}")], "{ev}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn closed_tasks_lose_clean_worktrees_only() {
        let root = std::env::temp_dir().join(format!("taskq-prune-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let git = |args: &[&str]| assert!(std::process::Command::new("git").arg("-C").arg(&root).args(["-c", "user.name=t", "-c", "user.email=t@t"]).args(args).output().unwrap().status.success(), "{args:?}");
        git(&["init", "-q"]);
        git(&["commit", "-q", "--allow-empty", "-m", "init"]);
        for n in ["1", "2", "3"] {
            git(&["worktree", "add", "-q", "-b", &format!("taskq-{n}"), &format!(".worktrees/taskq-{n}")]);
        }
        std::fs::write(root.join(".worktrees/taskq-2/wip"), "x").unwrap();
        let b = Fake::default();
        for _ in 0..3 {
            b.create("T", "", &["taskq".into()]).unwrap();
        }
        b.close(1).unwrap();
        b.close(2).unwrap();
        let mut c = cfg("mac");
        c.root = root.clone();
        let mut s = service(&b, &c, Default::default());
        s.pass().unwrap();
        let wt = |n: &str| root.join(format!(".worktrees/taskq-{n}")).exists();
        assert_eq!((wt("1"), wt("2"), wt("3")), (false, true, true), "clean closed removed; dirty and open stay");
        cleanup(&b, &root, |_| false, true);
        assert_eq!((wt("2"), wt("3")), (false, true), "force removes dirty closed; open stays");
        assert!(std::fs::read_to_string(root.join(".taskq/trash/taskq-2.patch")).unwrap().contains("wip"), "dirty work saved");
        let _ = std::fs::remove_dir_all(&root);
    }
}
