//! Local data rules (taskq.md § 4 Data): every growing file has a named bound and a pruner the service runs; a corrupt
//! state file reads unknown with a reason and stays where it is, never reset by a reader.

use serde_json::Value;
use std::fs;
use std::path::Path;

/// `<install>/taskq.log` and `<install>/<project>.log`: past it the newest half stays.
pub const LOG_MAX: u64 = 4 << 20;
/// `<root>/.taskq/cli-errors.log`: past it the newest half stays.
pub const CLI_ERRORS_MAX: u64 = 256 << 10;
/// `<root>/.taskq/<T|R><N>-<attempt>.log`: the newest by mtime; a live attempt's log always stays.
pub const ATTEMPT_LOGS: usize = 50;
/// `<root>/.taskq/incidents/<fingerprint>.json`: the newest by mtime.
pub const INCIDENTS: usize = 50;
/// `<root>/.taskq/inputs/*.txt`: newest by mtime plus locked Claude/App files; CLI stdin owns an open read handle.
pub const INPUTS: usize = 50;
/// A turn owns the lock, even when a forked child still holds a copy of its descriptor.
pub struct InputLock(fs::File);

impl InputLock {
    pub fn new(file: fs::File) -> std::io::Result<Self> {
        file.try_lock_shared()?;
        Ok(Self(file))
    }
}

impl Drop for InputLock {
    fn drop(&mut self) {
        // Closing alone leaves the lock held by descriptors inherited across fork.
        let _ = self.0.unlock();
    }
}

/// Lines of `<root>/.taskq/requests/comments`: past it the newest half and every comment still pending stay.
pub const COMMENT_IDS: usize = 1000;
/// Lines of `<root>/.taskq/requests.log`: the newest stay; pruned on every append (`retro::append`).
pub const REQUEST_LOG: usize = 500;
const FOCUS_MAX: u64 = 4096;

/// Keep the newest `max / 2` bytes of `p`, from a line start, once it is over `max`. Rewritten in place, not renamed:
/// the launcher and the service's own stderr append to the open file.
pub fn trim_tail(p: &Path, max: u64) -> bool {
    let profile_log = p.file_name().is_some_and(|n| n == "taskq.log");
    let _profile_lock = if profile_log {
        match crate::profile::log_lock(p) { Ok(lock) => Some(lock), Err(_) => return false }
    } else { None };
    trim_tail_locked(p,max).unwrap_or(false)
}

/// Caller owns the profile lock for taskq.log; keep import and in-place rotation in that same critical section.
pub(crate) fn trim_tail_locked(p: &Path, max: u64) -> Result<bool,String> {
    let profile_log=p.file_name().is_some_and(|n|n=="taskq.log");
    let threshold=if profile_log {max.saturating_sub(512)} else {max};
    match fs::metadata(p) {
        Ok(meta) if meta.len()<=threshold=>return Ok(false),
        Err(e) if e.kind()==std::io::ErrorKind::NotFound=>return Ok(false),
        Err(e)=>return Err(format!("log retention: {e}")),
        Ok(_)=>{},
    }
    if profile_log {crate::telemetry::read(p.parent().unwrap(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0,|d|d.as_millis() as u64))?;}
    let b=fs::read(p).map_err(|e|format!("log retention: {e}"))?;
    let tail=&b[b.len().saturating_sub((max/2) as usize)..];
    let tail=tail.iter().position(|c|*c==b'\n').map_or(tail,|i|&tail[i+1..]);
    fs::write(p,tail).map_err(|e|format!("log retention: {e}"))?;
    Ok(true)
}

/// Remove the files of `dir` that `pick` names beyond the `n` newest by mtime, except those `keep` names. Returns
/// how many went.
pub fn keep_newest(dir: &Path, n: usize, pick: impl Fn(&str) -> bool, keep: impl Fn(&str) -> bool) -> usize {
    let mut v: Vec<(std::time::SystemTime, std::path::PathBuf, String)> = fs::read_dir(dir).into_iter().flatten().flatten()
        .filter_map(|e| {
            let name = e.file_name().to_str()?.to_string();
            let m = e.metadata().ok().filter(|m| m.is_file() && pick(&name))?;
            Some((m.modified().ok()?, e.path(), name))
        }).collect();
    v.sort_by(|a, b| b.0.cmp(&a.0));
    v.iter().skip(n).filter(|(_, p, name)| !keep(name) && fs::remove_file(p).is_ok()).count()
}

/// The attempt of a `<T|R><N>-<attempt>.log` name.
fn attempt_of(name: &str) -> Option<u64> {
    let (n, a) = name.strip_suffix(".log")?.strip_prefix(['T', 'R'])?.split_once('-')?;
    n.parse::<u64>().ok()?;
    a.parse().ok()
}

/// Input bindings of retained attempts; uncertainty preserves inputs instead of deleting recovery evidence.
fn bound_inputs(root:&Path,live:&[u64])->Result<std::collections::HashSet<String>,String> {
    use std::io::Read;
    let mut names=std::collections::HashSet::new();
    if live.is_empty() {return Ok(names);}
    let mut budget=8*1024*1024u64;
    let mut seen=std::collections::HashSet::new();
    for entry in fs::read_dir(crate::dot(root)).map_err(|e|e.to_string())? {
        let entry=entry.map_err(|e|e.to_string())?;
        let name=entry.file_name();
        let Some(attempt)=name.to_str().and_then(attempt_of).filter(|a|live.contains(a)) else {continue};
        if !entry.file_type().map_err(|e|e.to_string())?.is_file() {return Err("nonregular native recovery log".into());}
        let mut text=String::new();
        fs::File::open(entry.path()).map_err(|e|e.to_string())?.take(budget+1).read_to_string(&mut text).map_err(|e|e.to_string())?;
        if text.len() as u64>budget {return Err("native recovery scan exceeds 8 MiB; inputs kept".into());}
        budget-=text.len() as u64;seen.insert(attempt);
        for line in text.lines().filter(|l|l.trim_start().starts_with('{')) {
            let v:Value=serde_json::from_str(line).map_err(|_|"incomplete native recovery log; inputs kept")?;
            if v["type"]!="taskq.input" {continue;}
            if v["attempt"].as_u64()!=Some(attempt) || !v["execution"].as_u64().is_some_and(|e|e>=attempt) {return Err("invalid native input binding; inputs kept".into());}
            let path=Path::new(v["path"].as_str().ok_or("native input path missing; inputs kept")?);
            let name=path.file_name().and_then(|n|n.to_str()).filter(|n|n.ends_with(".txt")).ok_or("native input filename invalid; inputs kept")?;
            names.insert(name.to_owned());
        }
    }
    if live.iter().any(|a|*a>0 && !seen.contains(a)) {return Err("retained attempt log missing; inputs kept".into());}
    Ok(names)
}

/// One pruning round of the service (§ 4 Data); `live`: native handles plus unresolved reservations.
pub fn prune(root: &Path, install: &Path, project: &str, live: &[u64]) -> Vec<String> {
    let d = crate::dot(root);
    let mut out = vec![];
    for p in [install.join("taskq.log"), install.join(format!("{project}.log"))] {
        if !(crate::host::active() && p.file_name().is_some_and(|n| n == "taskq.log")) && trim_tail(&p, LOG_MAX) {
            out.push(format!("pruned {} to its newest {} KiB", p.display(), LOG_MAX / 2048));
        }
    }
    if trim_tail(&d.join("cli-errors.log"), CLI_ERRORS_MAX) {
        out.push(format!("pruned cli-errors.log to its newest {} KiB", CLI_ERRORS_MAX / 2048));
    }
    let n = keep_newest(&d, ATTEMPT_LOGS, |f| attempt_of(f).is_some(), |f| attempt_of(f).is_some_and(|a| live.contains(&a)));
    if n > 0 {
        out.push(format!("pruned {n} attempt logs beyond the newest {ATTEMPT_LOGS}"));
    }
    let inputs = d.join("inputs");
    let bound=bound_inputs(root,live);
    if let Err(e)=&bound {out.push(format!("input retention unknown: {e}"));}
    let n = if let Ok(bound)=bound {keep_newest(&inputs, INPUTS, |f| f.ends_with(".txt"), |f| {
        if bound.contains(f) {return true;}
        // A live turn holds a shared lock; an unreadable file stays rather than risking its input.
        fs::File::options().read(true).write(true).open(inputs.join(f))
            .map_or(true, |file| file.try_lock().is_err())
    })} else {0};
    if n > 0 {
        out.push(format!("pruned {n} input files beyond the newest {INPUTS}"));
    }
    let n = keep_newest(&d.join("incidents"), INCIDENTS, |f| f.ends_with(".json"), |_| false);
    if n > 0 {
        out.push(format!("pruned {n} incident reports beyond the newest {INCIDENTS}"));
    }
    out
}

/// `requests/comments` lines after pruning (§ 4): unchanged up to `COMMENT_IDS`; past it the newest half plus every id
/// in `pending` (comments still eligible, so never made a request twice).
pub fn comment_ids(text: &str, pending: &[u64]) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= COMMENT_IDS {
        return None;
    }
    let cut = lines.len() - COMMENT_IDS / 2;
    let kept: Vec<&str> = lines.iter().enumerate().filter(|(i, l)| *i >= cut || pending.iter().any(|p| p.to_string() == **l)).map(|(_, l)| *l).collect();
    Some(kept.join("\n") + "\n")
}

/// A JSON state file: Ok(None) when missing; Err with the reason when it is unreadable or not a JSON object (unknown;
/// the file stays as it is).
pub fn json(p: &Path) -> Result<Option<Value>, String> {
    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    match fs::read(p) {
        Ok(b) => match serde_json::from_slice::<Value>(&b) {
            Ok(v) if v.is_object() => Ok(Some(v)),
            Ok(_) => Err(format!("{name}: not a JSON object; file kept")),
            Err(e) => Err(format!("{name}: corrupt ({e}); file kept")),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{name}: unreadable ({e}); file kept")),
    }
}

/// The labels of `<root>/.taskq/focus` (§ 4): missing, empty or `off` is none. Err with the reason when it is not
/// short UTF-8 text of labels (unknown: the service admits nothing; the file stays).
pub fn focus(root: &Path) -> Result<Vec<String>, String> {
    let b = match fs::read(crate::dot(root).join("focus")) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(format!("focus: unreadable ({e}); file kept")),
    };
    if b.len() as u64 > FOCUS_MAX {
        return Err(format!("focus: over {FOCUS_MAX} bytes; file kept"));
    }
    let t = String::from_utf8(b).map_err(|_| "focus: not UTF-8 text; file kept".to_string())?;
    if t.chars().any(|c| c.is_control() && !c.is_whitespace()) {
        return Err("focus: control characters; file kept".into());
    }
    Ok(t.split_whitespace().filter(|f| *f != "off").map(String::from).collect())
}

pub const KIDS_MAX: u64 = 1 << 20;

#[derive(Clone,Debug,PartialEq,Eq)]
pub struct KidLine {
    pub task:u64, pub attempt:u64, pub runtime:String, pub process:coreq::process::Identity,
    pub execution:u64, pub offset:u64, pub job:Option<String>,
}

pub fn job_key(key:&str)->bool {
    key.len()==36 && key.bytes().enumerate().all(|(i,b)|if [8,13,18,23].contains(&i) {b==b'-'} else {b.is_ascii_digit() || (b'a'..=b'f').contains(&b)})
}

/// Existing handover record; legacy/invalid evidence is preserved, never guessed from a delivery cursor.
pub fn kids(text: &str) -> Result<(coreq::process::Identity, Vec<KidLine>), String> {
    if text.len() as u64 > KIDS_MAX { return Err("kids: over 1 MiB; file kept".into()); }
    let identity=|pid:&str,started:&str|->Option<coreq::process::Identity> {
        let pid=pid.parse::<u32>().ok().filter(|n| *n>0 && *n<=i32::MAX as u32)?;
        if started.is_empty() || started.len()>128 || !started.bytes().all(|b|b.is_ascii_alphanumeric() || b==b'-') { return None; }
        Some(coreq::process::Identity {pid,started:started.into()})
    };
    let mut lines = text.lines();
    let header=lines.next().unwrap_or_default().split_whitespace().collect::<Vec<_>>();
    let owner=match header.as_slice() { ["pid",pid,started,"executor-v1"]=>identity(pid,started), _=>None }
        .ok_or("kids: native executor identity version missing; legacy launcher or malformed file kept")?;
    let kid = |l: &str| -> Option<KidLine> {
        let w: Vec<&str> = l.split_whitespace().collect();
        let (fields,job)=match w.as_slice() {
            [_,_,_,_,_,_,_] =>(w.as_slice(),None),
            [fields @ ..,scope] if fields.len()==7 =>{
                let key=scope.strip_prefix("job-global:")?;
                if !job_key(key) {return None;}
                (fields,Some(key.to_owned()))
            },
            _=>return None,
        };
        let [task, attempt, rt, pid, started, execution, offset] = fields[..] else { return None };
        let task=task.parse::<u64>().ok().filter(|n|*n>0)?;
        let attempt=attempt.parse::<u64>().ok().filter(|n|*n>0)?;
        let execution=execution.parse::<u64>().ok().filter(|n|*n>=attempt)?;
        if !matches!(rt,"codex"|"claude") || rt=="claude" && (offset!="0" || job.is_some()) { return None; }
        Some(KidLine {task,attempt,runtime:rt.into(),process:identity(pid,started)?,execution,offset:offset.parse().ok()?,job})
    };
    let v: Vec<KidLine> = lines.enumerate().map(|(i, l)| kid(l).ok_or(format!("kids: line {} malformed; file kept", i + 2))).collect::<Result<_, _>>()?;
    let (mut tasks,mut pids,mut jobs)=(std::collections::HashSet::new(),std::collections::HashSet::new(),std::collections::HashSet::new());
    if v.iter().any(|k|!tasks.insert(k.task) || !pids.insert(k.process.pid) || k.process.pid==owner.pid || k.job.as_ref().is_some_and(|job|!jobs.insert(job))) { return Err("kids: duplicate or service identity used as child; file kept".into()); }
    Ok((owner, v))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("taskq-data-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(d.join(".taskq/incidents")).unwrap();
        d
    }

    // Chain: service pass, prune. Risk: a growing file fills the disk or a live attempt loses its log
    #[test]
    fn every_growing_file_has_a_bound() {
        let root = temp("prune");
        let install = root.join("install");
        fs::create_dir_all(&install).unwrap();
        let d = crate::dot(&root);
        let line = "x".repeat(99) + "\n";
        fs::write(install.join("taskq.log"), line.repeat((LOG_MAX / 100 + 10) as usize)).unwrap();
        fs::write(install.join("p.log"), "small\n").unwrap();
        fs::write(d.join("cli-errors.log"), line.repeat((CLI_ERRORS_MAX / 100 + 10) as usize)).unwrap();
        for a in 0..ATTEMPT_LOGS as u64 + 5 {
            fs::write(d.join(format!("T7-{a}.log")), "").unwrap();
            fs::write(d.join("incidents").join(format!("{a}.json")), "{}").unwrap();
            // mtimes order the files: 0 is the oldest
            let t = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000 + a);
            fs::File::options().write(true).open(d.join(format!("T7-{a}.log"))).unwrap().set_modified(t).unwrap();
            fs::File::options().write(true).open(d.join("incidents").join(format!("{a}.json"))).unwrap().set_modified(t).unwrap();
        }
        fs::write(d.join("status.json"), "{}").unwrap();
        let out = prune(&root, &install, "p", &[0]);
        assert_eq!(out.len(), 4, "{out:?}");
        let big = fs::metadata(install.join("taskq.log")).unwrap().len();
        assert!(big <= LOG_MAX / 2 && big > LOG_MAX / 2 - 200 && fs::read(install.join("taskq.log")).unwrap().starts_with(b"xxx"));
        assert_eq!(fs::read_to_string(install.join("p.log")).unwrap(), "small\n");
        assert!(fs::metadata(d.join("cli-errors.log")).unwrap().len() <= CLI_ERRORS_MAX / 2);
        // live attempt 0 stays though oldest; 1..=4 go
        assert!(d.join("T7-0.log").exists() && !d.join("T7-1.log").exists() && !d.join("T7-4.log").exists() && d.join("T7-5.log").exists());
        assert_eq!(fs::read_dir(d.join("incidents")).unwrap().count(), INCIDENTS);
        assert!(d.join("status.json").exists());
        assert!(prune(&root, &install, "p", &[0]).is_empty(), "a second round finds nothing");
        // comment ids: past the bound the newest half and the pending ones stay
        let text: String = (1..=COMMENT_IDS as u64 + 1).map(|i| format!("{i}\n")).collect();
        assert_eq!(comment_ids(&text[..text.len() - 5], &[]), None);
        let kept = comment_ids(&text, &[3]).unwrap();
        assert_eq!(kept.lines().count(), COMMENT_IDS / 2 + 1);
        assert!(kept.starts_with("3\n") && kept.ends_with(&format!("{}\n", COMMENT_IDS + 1)));
        let _ = fs::remove_dir_all(&root);
    }

    // Chain: §4 hourly input-cache pruning. Risk: unbounded briefs or deletion during a live turn.
    #[test]
    fn input_cache_is_bounded_and_live_files_stay() {
        let root = temp("inputs");
        let dir = root.join(".taskq/inputs");
        fs::create_dir_all(&dir).unwrap();
        for i in 0..55 {
            let path = dir.join(format!("{i}.txt"));
            fs::write(&path, "invented brief").unwrap();
            fs::File::options().write(true).open(path).unwrap().set_modified(
                std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(i)).unwrap();
        }
        let active = fs::File::options().read(true).write(true).open(dir.join("0.txt")).unwrap();
        let active = InputLock::new(active).unwrap();
        fs::write(root.join(".taskq/T7-42.log"),format!("{}\n",serde_json::json!({"type":"taskq.input","task":7,"attempt":42,"execution":44,"runtime":"claude","path":dir.join("1.txt")}))).unwrap();
        prune(&root, &root.join("install"), "p", &[42]);
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 52);
        assert!(dir.join("0.txt").exists());
        assert!(dir.join("1.txt").exists(),"restart loses the lock, not the pending input binding");
        assert!(!dir.join("2.txt").exists());
        drop(active);
        prune(&root, &root.join("install"), "p", &[42]);
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 51);
        assert!(dir.join("1.txt").exists());
        let log=root.join(".taskq/T7-42.log");
        for bad in ["{\"type\":\"taskq.input\"".to_owned(),serde_json::json!({"type":"taskq.input","attempt":42,"execution":41}).to_string(),"x".repeat(8*1024*1024+1)] {
            fs::write(&log,bad).unwrap();
            assert!(prune(&root,&root.join("install"),"p",&[42]).iter().any(|s|s.starts_with("input retention unknown:")));
            assert_eq!(fs::read_dir(&dir).unwrap().count(),51,"uncertain recovery evidence must preserve the unlocked input");
        }
        fs::remove_file(log).unwrap();
        assert!(prune(&root,&root.join("install"),"p",&[42]).iter().any(|s|s.contains("retained attempt log missing")));
        assert!(dir.join("1.txt").exists());
        prune(&root, &root.join("install"), "p", &[]);
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 50);
        fs::remove_dir_all(root).unwrap();
    }

    // Chain: §4 input pruning after a turn ends. Risk: a pre-exec child retains the turn's lock.
    #[cfg(unix)]
    #[test]
    fn input_cache_unlocks_while_fork_child_lives() {
        let root = temp("inputs-fork");
        let path = root.join("input.txt");
        let file = fs::File::options().read(true).write(true).create_new(true).open(&path).unwrap();
        let held = InputLock::new(file).unwrap();
        let mut pipe = [0; 2];
        assert_eq!(unsafe { libc::pipe(pipe.as_mut_ptr()) }, 0);
        let pid = unsafe { libc::fork() };
        assert!(pid >= 0);
        if pid == 0 {
            // Only async-signal-safe calls after fork; wait until the parent finishes probing.
            unsafe {
                libc::close(pipe[1]);
                let mut byte = 0u8;
                libc::read(pipe[0], (&mut byte as *mut u8).cast(), 1);
                libc::_exit(0);
            }
        }
        unsafe { libc::close(pipe[0]); }
        let probe = fs::File::options().read(true).write(true).open(&path).unwrap();
        let before = probe.try_lock();
        drop(held);
        let after = probe.try_lock();
        unsafe {
            libc::write(pipe[1], b"x".as_ptr().cast(), 1);
            libc::close(pipe[1]);
        }
        let mut status = 0;
        assert_eq!(unsafe { libc::waitpid(pid, &mut status, 0) }, pid);
        assert_eq!(status, 0);
        assert!(matches!(before, Err(std::fs::TryLockError::WouldBlock)));
        after.unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    // Chain: status.json, tray.json, kids, focus readers. Risk: corrupt state reads as zero or default and is reset
    #[test]
    fn corrupt_state_reads_unknown_and_stays() {
        let root = temp("corrupt");
        let d = crate::dot(&root);
        assert_eq!(json(&d.join("status.json")), Ok(None));
        fs::write(d.join("status.json"), "{\"tasks\": [").unwrap();
        assert!(json(&d.join("status.json")).unwrap_err().starts_with("status.json: corrupt"));
        fs::write(d.join("status.json"), "[]").unwrap();
        assert!(json(&d.join("status.json")).unwrap_err().contains("not a JSON object"));
        assert_eq!(json(&d.join("status.json")).ok(), None);
        assert!(d.join("status.json").exists());
        assert_eq!(focus(&root), Ok(vec![]));
        fs::write(d.join("focus"), "area-x off p0\n").unwrap();
        assert_eq!(focus(&root), Ok(vec!["area-x".to_string(), "p0".into()]));
        for (bad, why) in [(&b"\xff\xfe"[..], "not UTF-8"), (b"area\0x", "control"), (&[b'a'; 5000][..], "over")] {
            fs::write(d.join("focus"), bad).unwrap();
            assert!(focus(&root).unwrap_err().contains(why));
            assert_eq!(fs::read(d.join("focus")).unwrap(), bad);
        }
        let owner=coreq::process::Identity {pid:9,started:"service".into()};
        let row=KidLine {task:7,attempt:41,runtime:"codex".into(),process:coreq::process::Identity {pid:123,started:"child".into()},execution:43,offset:40,job:None};
        assert_eq!(kids("pid 9 service executor-v1\n7 41 codex 123 child 43 40\n"), Ok((owner.clone(),vec![row.clone()])));
        let key="11111111-1111-4111-8111-111111111111";
        let scoped=format!("pid 9 service executor-v1\n7 41 codex 123 child 43 40 job-global:{key}\n");
        let mut with_job=row.clone();with_job.job=Some(key.into());
        assert_eq!(kids(&scoped),Ok((owner.clone(),vec![with_job])));
        for scope in ["job-global:","job-global:../foreign","job-global:11111111-1111-4111-8111-11111111111G","job:11111111-1111-4111-8111-111111111111"] {assert!(kids(&format!("pid 9 service executor-v1\n7 41 codex 123 child 43 40 {scope}\n")).is_err());}
        assert!(kids(&(scoped+&format!("8 42 codex 124 other 44 0 job-global:{key}\n"))).is_err(),"one native scope cannot own two tasks");
        assert!(kids(&format!("pid 9 service executor-v1\n7 41 claude 123 child 43 0 job-global:{key}\n")).is_err());
        assert!(kids("pid 9\n7 41 codex 123 40\n").is_err(),"legacy cursors never identify an execution");
        assert_eq!(kids("pid 9 service executor-v1\n"), Ok((owner, vec![])));
        assert!(kids("pid 9 service\n7 41 codex 123 child 43 40\n").unwrap_err().contains("legacy launcher"),"a historical wrapper PID is not native exit proof");
        for row in ["7 41 codex 9 service 43 40", "7 41 codex 123 child 40 0", "7 41 codex 123 ../child 43 0", "7 41 claude 123 child 43 1", "7 41 codex-app 123 child 43 0"] {
            assert!(kids(&format!("pid 9 service executor-v1\n{row}\n")).is_err(),"{row}");
        }
        assert!(kids("pid 9 service executor-v1\n7 41 codex 123 child 43 40\n7 41 codex 124 child2 44 40\n").is_err());
        assert!(kids(&"x".repeat(KIDS_MAX as usize+1)).unwrap_err().contains("1 MiB"));
        assert!(kids("pid x\n").unwrap_err().contains("identity"));
        assert!(kids("pid 9 service executor-v1\n7 41 claude\n").unwrap_err().contains("line 2"));
        assert_eq!(json(&root.join("missing")), Ok(None));
        assert_eq!(json(&{ fs::write(d.join("t.json"), json!({"a": 1}).to_string()).unwrap(); d.join("t.json") }), Ok(Some(json!({"a": 1}))));
        let _ = fs::remove_dir_all(&root);
    }
}
