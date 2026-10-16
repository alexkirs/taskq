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
    let threshold = if profile_log { max.saturating_sub(512) } else { max };
    let _profile_lock = if profile_log {
        match crate::profile::log_lock(p) { Ok(lock) => Some(lock), Err(_) => return false }
    } else { None };
    if fs::metadata(p).is_ok_and(|m| m.len() <= threshold) {
        return false;
    }
    if profile_log && crate::telemetry::read(p.parent().unwrap(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)).is_err() { return false; }
    let Ok(b) = fs::read(p) else { return false };
    let tail = &b[b.len().saturating_sub((max / 2) as usize)..];
    let tail = tail.iter().position(|c| *c == b'\n').map_or(tail, |i| &tail[i + 1..]);
    fs::write(p, tail).is_ok()
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

/// One pruning round of the service (§ 4 Data); `live`: attempts of live children. Lines name what went.
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
    let n = keep_newest(&inputs, INPUTS, |f| f.ends_with(".txt"), |f| {
        // A live turn holds a shared lock; an unreadable file stays rather than risking its input.
        fs::File::options().read(true).write(true).open(inputs.join(f))
            .map_or(true, |file| file.try_lock().is_err())
    });
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

/// The kids file of the § 8 step 3 handover: the writer's pid and `task attempt runtime pid delivered` per child.
/// Err with the reason on any malformed line (unknown: nothing is adopted, the file stays).
pub type KidLine = (u64, u64, String, i32, u64);

pub fn kids(text: &str) -> Result<(u32, Vec<KidLine>), String> {
    let mut lines = text.lines();
    let pid = lines.next().and_then(|l| l.strip_prefix("pid ")?.parse().ok()).ok_or("kids: no `pid <n>` first line; file kept")?;
    let kid = |l: &str| -> Option<KidLine> {
        let w: Vec<&str> = l.split_whitespace().collect();
        let [task, attempt, rt, pid, delivered] = w[..] else { return None };
        Some((task.parse().ok()?, attempt.parse().ok()?, rt.into(), pid.parse().ok()?, delivered.parse().ok()?))
    };
    let v = lines.enumerate().map(|(i, l)| kid(l).ok_or(format!("kids: line {} malformed; file kept", i + 2))).collect::<Result<_, _>>()?;
    Ok((pid, v))
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
        prune(&root, &root.join("install"), "p", &[]);
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 51);
        assert!(dir.join("0.txt").exists());
        assert!(!dir.join("1.txt").exists());
        drop(active);
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
        assert_eq!(kids("pid 9\n7 41 claude 123 40\n"), Ok((9, vec![(7, 41, "claude".into(), 123, 40)])));
        assert_eq!(kids("pid 9\n"), Ok((9, vec![])));
        assert!(kids("pid x\n").unwrap_err().contains("pid"));
        assert!(kids("pid 9\n7 41 claude\n").unwrap_err().contains("line 2"));
        assert_eq!(json(&root.join("missing")), Ok(None));
        assert_eq!(json(&{ fs::write(d.join("t.json"), json!({"a": 1}).to_string()).unwrap(); d.join("t.json") }), Ok(Some(json!({"a": 1}))));
        let _ = fs::remove_dir_all(&root);
    }
}
