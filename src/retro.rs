//! Request log and retrospective (taskq.md § 4, § 5): `<root>/.taskq/requests.log` keeps one JSON line per owner or
//! operator request; `taskq retro` runs the operator (R3) over it and the use-case proposals and prints numbered
//! recommendations; `taskq add --retro K` files one of them.

use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Request log bound: the newest lines kept (§ 4).
pub use crate::data::REQUEST_LOG as BOUND;
const KINDS: [&str; 4] = ["flow", "option", "operation", "quick command"];

pub fn log_path(root: &Path) -> PathBuf {
    crate::dot(root).join("requests.log")
}

fn report_path(root: &Path) -> PathBuf {
    crate::dot(root).join("retro.json")
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// `YYYY-MM-DD HH:MM` UTC of unix seconds.
fn utc(t: u64) -> String {
    let (d, s) = ((t / 86400) as i64, t % 86400);
    // days to civil date (H. Hinnant)
    let z = d + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let (day, month) = (doy - (153 * mp + 2) / 5 + 1, if mp < 10 { mp + 3 } else { mp - 9 });
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02} {:02}:{:02}", s / 3600, s % 3600 / 60)
}

/// Append one entry and prune to `BOUND` lines, under `requests.log.lock`. Lines are kept verbatim, a corrupt one
/// included: pruning counts lines, never parses them (specq data: corrupt is kept).
pub fn append(root: &Path, source: &str, task: Option<u64>, text: &str) -> Result<(), String> {
    let path = log_path(root);
    fs::create_dir_all(crate::dot(root)).map_err(|e| e.to_string())?;
    let lock = path.with_extension("log.lock");
    let mut waited = 0;
    loop {
        match fs::OpenOptions::new().write(true).create_new(true).open(&lock) {
            Ok(_) => break,
            // ponytail: a lock older than 30 s is a crashed writer's; a holder never runs that long
            Err(_) if crate::file_age(&lock).is_some_and(|a| a > 30) => { let _ = fs::remove_file(&lock); }
            Err(_) if waited < 50 => { std::thread::sleep(Duration::from_millis(100)); waited += 1; }
            Err(e) => return Err(format!("request log busy: {}: {e}", lock.display())),
        }
    }
    let old = fs::read_to_string(&path).unwrap_or_default();
    let mut lines: Vec<&str> = old.lines().collect();
    let entry = json!({"at": now(), "source": source, "task": task, "text": text}).to_string();
    lines.push(&entry);
    let keep = &lines[lines.len().saturating_sub(BOUND)..];
    let r = crate::platform::write_atomic(&path, &(keep.join("\n") + "\n")).map_err(|e| format!("{}: {e}", path.display()));
    let _ = fs::remove_file(&lock);
    r
}

/// Log an accepted CLI write; a failed log write is reported, never fails the command (§ 4).
pub fn record(root: &Path, source: &str, task: u64, text: &str) {
    if let Err(e) = append(root, source, Some(task), text) {
        eprintln!("request log: {e}");
    }
}

/// The log entries, oldest first; missing is empty, an existing empty, unreadable or corrupt log is unknown.
pub fn read(root: &Path) -> Result<Vec<Value>, String> {
    let path = log_path(root);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(format!("request log unreadable: {e}")),
    };
    let mut out = vec![];
    for (i, l) in text.lines().enumerate().filter(|(_, l)| !l.trim().is_empty()) {
        let v: Value = serde_json::from_str(l).map_err(|_| format!("request log line {} is not JSON; file kept", i + 1))?;
        if !v["at"].is_u64() || !v["source"].is_string() || !v["text"].is_string() {
            return Err(format!("request log line {} lacks at, source or text; file kept", i + 1));
        }
        out.push(v);
    }
    if out.is_empty() {
        return Err("request log is empty".into());
    }
    Ok(out)
}

/// PM proposals: the `- YYYY-MM-DD <text>` lines of the `## Proposals` section of the use cases (§ 4).
pub fn proposals(readme: &str) -> Vec<(String, String)> {
    readme.split("\n## ").find(|s| s.starts_with("Proposals")).into_iter().flat_map(str::lines)
        .filter_map(|l| l.strip_prefix("- ")?.split_once(' '))
        .filter(|(d, _)| d.len() == 10 && d.bytes().enumerate().all(|(i, b)| if i == 4 || i == 7 { b == b'-' } else { b.is_ascii_digit() }))
        .map(|(d, t)| (d.to_string(), t.trim().to_string()))
        .collect()
}

/// The numbered inputs: `r<k>` per log entry, `p<k>` per proposal.
fn inputs(log: &[Value], props: &[(String, String)]) -> Vec<(String, String)> {
    let r = log.iter().enumerate().map(|(i, e)| {
        let task = e["task"].as_u64().map(|n| format!(" #{n}")).unwrap_or_default();
        (format!("r{}", i + 1), format!("{} {}{task}: {}", utc(e["at"].as_u64().unwrap_or(0)), e["source"].as_str().unwrap_or(""), e["text"].as_str().unwrap_or("")))
    });
    let p = props.iter().enumerate().map(|(i, (d, t))| (format!("p{}", i + 1), format!("{d} pm: {t}")));
    r.chain(p).collect()
}

fn brief(inputs: &[(String, String)], use_cases: &str) -> String {
    let list: String = inputs.iter().map(|(id, t)| format!("{id} {}\n", t.replace('\n', " / "))).collect();
    format!(
        "You are the taskq operator (taskq.md R3) running a retrospective: read the owner's and operator's requests below and recommend changes to taskq that simplify flows or add options, operations or quick commands. This is a read/recommend-only request: inspect relevant project evidence if needed, but perform no edits, queue writes or external actions.\n\
         Reply with exactly one JSON object and nothing else: {{\"recommendations\": [{{\"kind\": \"flow|option|operation|quick command\", \"title\": \"<task title, at most 80 characters>\", \"text\": \"<one line: what changes and why>\", \"from\": [\"r3\", \"p1\"], \"goal\": \"<one line user outcome>\", \"acceptance\": [\"<one line observable check>\"]}}]}}.\n\
         Each recommendation names in from the request ids it comes from (at least one); 1-4 acceptance items; most useful first; at most 10. Skip what the use cases already cover as shipped. Use English, short plain lines (taskq.md R7).\n\
         Requests:\n{list}\nUse cases:\n{use_cases}\n"
    )
}

/// The operator's reply as a report: every recommendation valid, else an error naming the first fault.
pub fn parse(text: &str, ids: &[String]) -> Result<Vec<Value>, String> {
    let v: Value = match (text.find('{'), text.rfind('}')) {
        (Some(a), Some(b)) if a < b => serde_json::from_str(&text[a..=b]).map_err(|_| "the operator gave no report JSON")?,
        _ => return Err("the operator gave no report JSON".into()),
    };
    let recs = v["recommendations"].as_array().ok_or("the operator gave no recommendations")?;
    for (i, r) in recs.iter().enumerate() {
        let s = |k: &str| r[k].as_str().filter(|t| !t.trim().is_empty() && !t.contains('\n'));
        let from: Vec<&str> = r["from"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
        let acc = r["acceptance"].as_array().map_or(0, Vec::len);
        let ok = s("kind").is_some_and(|k| KINDS.contains(&k)) && s("title").is_some_and(|t| t.chars().count() <= 80) && s("text").is_some() && s("goal").is_some()
            && !from.is_empty() && from.iter().all(|f| ids.iter().any(|x| x == f)) && (1..=4).contains(&acc);
        if !ok {
            return Err(format!("recommendation {} is invalid (kind, title, text, goal, from or acceptance)", i + 1));
        }
    }
    Ok(recs.clone())
}

/// The printed report: counts, numbered recommendations with kind and sources, the filing command.
fn render(inputs: &[(String, String)], recs: &[Value], since: u64) -> String {
    let n = inputs.iter().filter(|(id, _)| id.starts_with('r')).count();
    let mut out = format!("Retro: {n} requests since {}, {} proposals\n", utc(since), inputs.len() - n);
    if recs.is_empty() {
        out += "No recommendations: the operator found nothing to change.\n";
        return out;
    }
    for (i, r) in recs.iter().enumerate() {
        let from: Vec<String> = r["from"].as_array().into_iter().flatten().filter_map(Value::as_str)
            .map(|f| format!("{f} {}", crate::compact(&inputs.iter().find(|(id, _)| id == f).map(|(_, t)| t.clone()).unwrap_or_default(), 60))).collect();
        out += &format!("{}. [{}] {}\n   {}\n   From: {}\n", i + 1, r["kind"].as_str().unwrap_or(""), r["title"].as_str().unwrap_or(""), r["text"].as_str().unwrap_or(""), from.join("; "));
    }
    out + "Next: file one with `taskq add --retro K` (K = its number); nothing is filed until you pick.\n"
}

/// `taskq retro`: the operator over the log and the use cases; the report is saved for `add --retro`.
pub fn run(root: &Path, rt: Option<&str>) -> Result<String, String> {
    let log = read(root).map_err(|e| format!("Requests: unknown ({e}); no retro run"))?;
    let use_cases = fs::read_to_string(root.join("docs/use-cases/README.md")).unwrap_or_default();
    let ins = inputs(&log, &proposals(&use_cases));
    let rt = rt.ok_or("no runtime with a nonzero limit on this host")?;
    fs::create_dir_all(crate::dot(root)).map_err(|e| e.to_string())?;
    let text = crate::operator::exec(rt, &brief(&ins, &use_cases), root, &crate::dot(root), "retro", None)?;
    let ids: Vec<String> = ins.iter().map(|(id, _)| id.clone()).collect();
    let recs = parse(&text, &ids)?;
    let report = json!({"at": now(), "recommendations": recs});
    crate::platform::write_atomic(&report_path(root), &report.to_string()).map_err(|e| e.to_string())?;
    Ok(render(&ins, &recs, log.first().and_then(|e| e["at"].as_u64()).unwrap_or(0)))
}

/// Recommendation `k` of the last report: title, goal, acceptance for `taskq add` (§ 5).
pub fn pick(root: &Path, k: &str) -> Result<(String, String, Vec<String>), String> {
    let v: Value = fs::read_to_string(report_path(root)).ok().and_then(|t| serde_json::from_str(&t).ok()).ok_or("no retro report; run `taskq retro` first")?;
    let recs = v["recommendations"].as_array().ok_or("retro report unreadable; run `taskq retro` again")?;
    let r = k.parse::<usize>().ok().filter(|i| (1..=recs.len()).contains(i)).map(|i| &recs[i - 1]).ok_or(format!("--retro {k}: no such recommendation (1-{})", recs.len()))?;
    let s = |k: &str| r[k].as_str().unwrap_or("").to_string();
    Ok((s("title"), s("goal"), r["acceptance"].as_array().into_iter().flatten().filter_map(|a| a.as_str().map(String::from)).collect()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("taskq-retro-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    // Chain: § 4 request log bound. Risk: the log grows without limit or the pruner drops the newest entries.
    #[test]
    fn the_request_log_keeps_the_newest_bound_lines() {
        let root = temp("bound");
        for i in 0..BOUND + 7 {
            append(&root, "tray", None, &format!("request {i}")).unwrap();
        }
        let log = read(&root).unwrap();
        assert_eq!(log.len(), BOUND);
        assert_eq!(log[0]["text"], "request 7");
        assert_eq!(log[BOUND - 1]["text"], format!("request {}", BOUND + 6));
        assert!(!log_path(&root).with_extension("log.lock").exists());
    }

    // Chain: § 4 request log corruption (A5). Risk: an empty or corrupt log reads as no requests or is reset.
    #[test]
    fn an_empty_or_corrupt_log_is_unknown_and_kept() {
        let root = temp("corrupt");
        assert!(read(&root).unwrap().is_empty());
        fs::create_dir_all(crate::dot(&root)).unwrap();
        fs::write(log_path(&root), "").unwrap();
        assert_eq!(read(&root).unwrap_err(), "request log is empty");
        fs::write(log_path(&root), "{\"at\": 1, \"source\": \"tray\", \"text\": \"a\"}\nnot json\n").unwrap();
        assert!(read(&root).unwrap_err().contains("line 2 is not JSON"));
        append(&root, "ask", Some(3), "b").unwrap();
        assert!(fs::read_to_string(log_path(&root)).unwrap().contains("not json\n"), "a corrupt line survives an append");
        assert!(run(&root, Some("claude")).unwrap_err().starts_with("Requests: unknown (request log line 2"), "no operator runs over a corrupt log");
    }

    // Chain: § 4 request log → § 5 retro refusal. Risk: a failed read is mistaken for a missing log.
    #[test]
    fn an_existing_unreadable_log_is_unknown_and_kept() {
        let root = temp("unreadable");
        fs::create_dir_all(log_path(&root)).unwrap();
        assert!(run(&root, Some("claude")).unwrap_err().starts_with("Requests: unknown (request log unreadable:"));
        assert!(log_path(&root).is_dir());
    }

    // Chain: § 5 retro report (A2-A4). Risk: a recommendation without a source or kind is shown or filed.
    #[test]
    fn a_report_names_sources_kinds_and_the_filing_command() {
        let readme = "# Use cases\n\n## Proposals\n\nLine: `- YYYY-MM-DD <proposal> (from <request>)`.\n\n- 2026-10-14 Quick command to answer all asks (from tray)\n- not a proposal\n\n## Other\n- 2026-10-15 ignored\n";
        assert_eq!(proposals(readme), [("2026-10-14".to_string(), "Quick command to answer all asks (from tray)".to_string())]);
        let empty = temp("missing-log");
        let missing = read(&empty).unwrap();
        let first = inputs(&missing, &proposals(readme));
        assert!(missing.is_empty() && first.len() == 1 && first[0].0 == "p1");
        assert!(!log_path(&empty).exists(), "a missing log stays missing; PM proposals survive");
        let log = vec![json!({"at": 1_760_400_000u64, "source": "tray", "task": null, "text": "pause maps"})];
        let ins = inputs(&log, &proposals(readme));
        assert_eq!(ins[0], ("r1".into(), "2025-10-14 00:00 tray: pause maps".into()));
        assert_eq!(ins[1].0, "p1");
        let ids: Vec<String> = ins.iter().map(|(i, _)| i.clone()).collect();
        let rec = json!({"kind": "quick command", "title": "Answer all", "text": "One command answers every ask", "from": ["r1", "p1"], "goal": "Fewer steps", "acceptance": ["taskq answer all works"]});
        let recs = parse(&format!("Here:\n{}", json!({"recommendations": [rec]})), &ids).unwrap();
        let out = render(&ins, &recs, 1_760_400_000);
        assert!(out.starts_with("Retro: 1 requests since 2025-10-14 00:00, 1 proposals\n1. [quick command] Answer all\n"), "{out}");
        assert!(out.contains("From: r1 2025-10-14 00:00 tray: pause maps; p1 2026-10-14 pm: Quick command") && out.contains("taskq add --retro K"), "{out}");
        for bad in [json!({"from": ["r9"]}), json!({"from": []}), json!({"kind": "wish"}), json!({"acceptance": []})] {
            let mut r = rec.clone();
            r.as_object_mut().unwrap().extend(bad.as_object().unwrap().clone());
            assert!(parse(&json!({"recommendations": [r]}).to_string(), &ids).is_err(), "{bad}");
        }
        let root = temp("pick");
        assert!(pick(&root, "1").is_err(), "nothing is filed without a report");
        fs::create_dir_all(crate::dot(&root)).unwrap();
        fs::write(report_path(&root), json!({"recommendations": recs}).to_string()).unwrap();
        assert_eq!(pick(&root, "1").unwrap(), ("Answer all".into(), "Fewer steps".into(), vec!["taskq answer all works".into()]));
        assert!(pick(&root, "2").is_err());
    }
}
