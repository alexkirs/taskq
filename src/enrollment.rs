//! Inactive observation adapter: no transport, command authority or task-state storage.
#![allow(dead_code)] // Enrollment delivery is a separate implementation task.

use crate::{Res, core, snapshot::Snapshot};
use ring::rand::{SecureRandom, SystemRandom};
use serde::Serialize;
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs, io::{Read, Write}, path::Path};

/// Create once, outside release folders; a corrupt identity never silently becomes a new installation.
pub fn installation_id(install: &Path) -> Res<String> {
    let path = install.join("installation-id");
    let read = || -> Res<Option<String>> {
        let bytes = fs::File::open(&path).and_then(|f| {
            let mut bytes = Vec::new();
            f.take(65).read_to_end(&mut bytes)?;
            Ok(bytes)
        });
        match bytes {
            Ok(b) if b.len() == 64 && b.iter().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(c)) =>
                Ok(Some(String::from_utf8(b).unwrap())),
            Ok(_) => Err("installation-id: corrupt; file kept".into()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("installation-id: unreadable ({e}); file kept")),
        }
    };
    if let Some(id) = read()? { return Ok(id) }
    fs::create_dir_all(install).map_err(|e| e.to_string())?;
    let mut bytes = [0; 32];
    SystemRandom::new().fill(&mut bytes).map_err(|_| "installation-id: randomness unavailable")?;
    let id: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    let temp = install.join(format!("installation-id-{id}.tmp"));
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)] {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let publish = || -> Res<()> {
        let mut file = opts.open(&temp).map_err(|e| e.to_string())?;
        file.write_all(id.as_bytes()).and_then(|_| file.sync_all()).map_err(|e| e.to_string())?;
        drop(file);
        // A hard link publishes complete bytes without overwriting another project's winner.
        match fs::hard_link(&temp, &path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
            Err(e) => Err(format!("installation-id: publish failed ({e})")),
        }
    };
    let result = publish();
    let _ = fs::remove_file(&temp);
    result?;
    read()?.ok_or("installation-id: missing after publication".into())
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct BoardScope {
    kind: String,
    host: String,
    repo: String,
}

impl BoardScope {
    pub fn new(kind: &str, host: &str, repo: &str) -> Res<Self> {
        let safe = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b".-_".contains(&b)) && s != "." && s != "..";
        if !matches!(kind, "github" | "gitlab") || !safe(host) || repo.split('/').count() < 2 || !repo.split('/').all(safe) {
            return Err("invalid board scope".into());
        }
        Ok(Self { kind: kind.into(), host: host.to_ascii_lowercase(), repo: if kind == "github" { repo.to_ascii_lowercase() } else { repo.into() } })
    }

    fn snapshot(s: &Snapshot) -> Res<Self> {
        let field = |k: &str| s.board[k].as_str().ok_or("missing board identity".to_string());
        Self::new(field("kind")?, field("host")?, field("repo")?)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct Choice { number: usize, text: String, recommended: bool }

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct TaskObservation {
    n: u64,
    state: String,
    revision: u64,
    choices: Vec<Choice>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Observation {
    schema: u64,
    installation: String,
    board: BoardScope,
    observed: i64,
    complete: bool,
    reason: Option<&'static str>,
    totals: Option<BTreeMap<&'static str, usize>>,
    tasks: Vec<TaskObservation>,
}

/// The scope is the approved board list, never inferred from host names or projects on disk.
pub fn project(s: &Snapshot, installation: &str, approved: &[BoardScope]) -> Res<Observation> {
    let board = BoardScope::snapshot(s)?;
    if !approved.contains(&board) { return Err("board outside approved scope".into()) }
    if installation.len() != 64 || !installation.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)) {
        return Err("invalid installation identity".into());
    }
    Ok(Observation {
        schema: 1, installation: installation.into(), board, observed: s.observed,
        complete: s.reason.is_none(), reason: s.reason.as_ref().map(|_| "source incomplete"),
        totals: s.reason.is_none().then(|| s.totals()),
        tasks: s.tasks.iter().map(|r| TaskObservation {
            n: r.n, state: r.task.state.name().into(), revision: r.revision,
            choices: core::options(r.task.card.as_deref().unwrap_or_default()).into_iter()
                .map(|(number, text, recommended)| Choice { number, text, recommended }).collect(),
        }).collect(),
    })
}

/// One row per approved board, never the sum of host snapshots. Delivery time cannot refresh data.
pub fn coalesce(observations: &[Observation], approved: &[BoardScope], now: i64, max_age: u64) -> Vec<Value> {
    let mut boards = BTreeMap::new();
    for board in approved {
        let latest = observations.iter().filter(|o| &o.board == board).max_by_key(|o| o.observed);
        let row = match latest {
            None => json!({"board": board, "freshness": "missing", "complete": false, "reason": "no observation", "totals": null, "observation": null}),
            Some(o) => {
                // Same-time reports differing only in installation are the same board observation.
                let conflict = observations.iter().filter(|v| &v.board == board && v.observed == o.observed)
                    .any(|v| v.complete != o.complete || v.totals != o.totals || v.tasks != o.tasks);
                let age = now.checked_sub(o.observed);
                let freshness = if conflict || o.observed <= 0 || age.is_none_or(|a| a < 0) { "unknown" }
                    else if age.unwrap() as u64 > max_age { "stale" } else { "fresh" };
                json!({"board": board, "freshness": freshness, "complete": o.complete && !conflict,
                    "reason": if conflict { Some("conflicting observations") } else { o.reason },
                    "totals": if freshness == "fresh" && !conflict { json!(o.totals) } else { Value::Null },
                    "observation": if conflict { Value::Null } else { json!(o) }})
            }
        };
        boards.insert(board.clone(), row);
    }
    boards.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Chain: identity creation -> concurrent project startup -> update/data pruning -> reread.
    #[test]
    fn identity_survives_pruning_and_concurrent_creation() {
        let root = std::env::temp_dir().join(format!("taskq-enrollment-{}", std::process::id()));
        let install = root.join("install");
        let _ = fs::remove_dir_all(&root);
        let ids = std::thread::scope(|s| {
            let threads: Vec<_> = (0..8).map(|_| s.spawn(|| installation_id(&install).unwrap())).collect();
            threads.into_iter().map(|t| t.join().unwrap()).collect::<Vec<_>>()
        });
        assert!(ids.iter().all(|id| id == &ids[0]));
        crate::platform::sweep(&install, &root.join("absent-shim"));
        crate::data::prune(&root, &install, "p", &[]);
        assert_eq!(installation_id(&install).unwrap(), ids[0]);
        fs::write(install.join("installation-id"), "broken").unwrap();
        assert!(installation_id(&install).unwrap_err().contains("corrupt"));
        assert_eq!(fs::read_to_string(install.join("installation-id")).unwrap(), "broken");
        fs::remove_file(install.join("installation-id")).unwrap();
        assert_ne!(installation_id(&install).unwrap(), ids[0]);
        fs::remove_dir_all(root).unwrap();
    }
}
