//! AI requests (taskq.md R3 operator, § 4): the tray writes `<root>/.taskq/requests/<id>.json`, the service runs one
//! operator child per request and writes `<id>.answer.json`; a proposal is applied only after `<id>.confirm` names
//! its `rev`. One writer per file: the tray writes the request and the confirm, the service the answer.

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
            answer(root, id, json!({"state": "applying", "text": a["text"], "ops": a["ops"]}));
            let (ok, lines) = apply(install, exe, a["ops"].as_array().map_or(&[][..], |v| v));
            answer(root, id, json!({"state": if ok { "applied" } else { "apply failed" }, "text": a["text"], "ops": a["ops"], "readback": lines}));
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
            Ok(text) => parse(&text, &projects(&install)),
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
    let before = req["after"].as_str().filter(|a| a.bytes().all(|b| b.is_ascii_digit())).map(|a| {
        format!("Earlier request: {}\nYour answer then: {}\n", read(&dir(root).join(format!("{a}.json")))["text"], read(&answer_path(root, a))["text"])
    }).unwrap_or_default();
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
         Projects: {}\n{before}Request: {}\n",
        serde_json::to_string_pretty(&ps).unwrap_or_default(),
        req["text"].as_str().unwrap_or("")
    )
}

/// The operator's reply as an answer: one JSON object of a known kind; a proposal only of valid ops (§ 4).
pub fn parse(text: &str, projects: &[(String, PathBuf)]) -> Value {
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
    if let Some(e) = ops.iter().find_map(|o| validate(o, projects).err()) {
        return json!({"state": "failed", "reason": format!("refused proposal: {e}")});
    }
    json!({"state": "proposal", "text": body, "ops": ops})
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
        assert_eq!(parse("Sure:\n{\"kind\": \"result\", \"text\": \"2 in work\"}", &ps), json!({"state": "result", "text": "2 in work"}));
        assert_eq!(parse("no json", &ps)["state"], "failed");
        assert_eq!(parse("{\"kind\": \"done\"}", &ps)["state"], "failed");
        assert_eq!(parse("{\"kind\": \"proposal\", \"text\": \"x\", \"ops\": []}", &ps)["state"], "failed");
        assert_eq!(parse("{\"kind\": \"proposal\", \"text\": \"x\", \"ops\": [{\"op\": \"close\", \"project\": \"a\", \"task\": 1}]}", &ps)["state"], "failed");
        assert_eq!(parse("{\"kind\": \"proposal\", \"text\": \"Pause a\", \"ops\": [{\"op\": \"pause\", \"project\": \"a\"}]}", &ps)["state"], "proposal");
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
