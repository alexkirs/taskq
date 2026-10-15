//! Installation-local telemetry. Caller holds taskq-profile.lock across import, write and read.
#[cfg(test)]
mod semantic;
#[cfg(test)]
mod source;

use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;
use std::io::Read;

const APP: i64 = 0x5451544d;
const PAGES: u64 = 7168; // 28 MiB database + at most 29 MiB WAL + 1 MiB SHM + 4 MiB legacy log < 64 MiB.
const LINE_MAX: usize = 64 << 10;
const WINDOW_MS: u64 = 30 * 86400 * 1000;
const LEGACY_MAX: u64 = crate::data::LOG_MAX + LINE_MAX as u64;

fn open(install: &Path) -> Result<Connection, String> {
    // Older release sweep keeps *.log entries, including directories; rollback must preserve DB and sidecars together.
    let dir = install.join("telemetry.log");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join("telemetry.sqlite3");
    let exists = path.exists();
    let db = Connection::open(&path).map_err(|e| format!("telemetry SQLite: {e}"))?;
    db.busy_timeout(std::time::Duration::from_secs(1)).map_err(|e| e.to_string())?;
    let app: i64 = db.query_row("PRAGMA application_id", [], |r| r.get(0)).map_err(|e| e.to_string())?;
    let version: i64 = db.query_row("PRAGMA user_version", [], |r| r.get(0)).map_err(|e| e.to_string())?;
    if exists && (app != APP || version != 1) {
        return Err("telemetry SQLite: foreign or unsupported schema; file kept".into());
    }
    db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA temp_store=MEMORY; PRAGMA cache_spill=OFF; PRAGMA cache_size=-32768; PRAGMA wal_autocheckpoint=0;")
        .map_err(|e| e.to_string())?;
    db.pragma_update(None, "max_page_count", PAGES as i64).map_err(|e| e.to_string())?;
    if !exists {
        db.execute_batch(&format!("BEGIN IMMEDIATE; PRAGMA application_id={APP}; PRAGMA user_version=1;
            CREATE TABLE records(seq INTEGER PRIMARY KEY, retained_ms INTEGER NOT NULL, line TEXT NOT NULL);
            CREATE INDEX records_time ON records(retained_ms);
            CREATE TABLE legacy(id INTEGER PRIMARY KEY CHECK(id=1), text TEXT NOT NULL);
            COMMIT;")).map_err(|e| e.to_string())?;
    }
    Ok(db)
}

// No writer starts behind a pinned reader. With cache spilling off, each database page enters this WAL at most once.
fn checkpoint(db: &Connection) -> Result<(), String> {
    let (busy, frames, done): (i64, i64, i64) = db.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).map_err(|e| e.to_string())?;
    if busy != 0 || frames != done { return Err("telemetry SQLite: checkpoint busy; write refused".into()); }
    Ok(())
}


fn insert(db: &Connection, line: &str, now: u64) -> Result<(), String> {
    if line.len() > LINE_MAX { return Err("telemetry record exceeds 64 KiB; original kept".into()); }
    db.prepare_cached("INSERT INTO records(retained_ms,line) VALUES(?1,?2)").map_err(|e| e.to_string())?
        .execute(params![now.min(i64::MAX as u64) as i64, line]).map_err(|e| e.to_string())?;
    Ok(())
}

fn legacy_text(install: &Path) -> Result<String, String> {
    let mut text = String::new();
    match std::fs::File::open(install.join("taskq.log")) {
        Ok(f) => { f.take(LEGACY_MAX + 1).read_to_string(&mut text).map_err(|e| format!("legacy taskq.log: {e}"))?; }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(text),
        Err(e) => return Err(format!("legacy taskq.log: {e}")),
    }
    if text.len() as u64 > LEGACY_MAX { return Err("legacy taskq.log exceeds migration bound; file kept".into()); }
    let complete = text.rfind('\n').map_or(0, |i| i + 1);
    if telemetry(&text[complete..]) { return Err("legacy telemetry record truncated; file kept".into()); }
    // Only service diagnostics may bypass the profile lock while completing an append.
    text.truncate(complete);
    Ok(text)
}

fn hash(text: &str) -> String {
    ring::digest::digest(&ring::digest::SHA256, text.as_bytes()).as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

fn telemetry(line: &str) -> bool {
    ["taskq-profile ", "taskq-stage ", "taskq-board ", "taskq-transition ", "taskq-requests "].iter().any(|p| line.starts_with(p))
}

fn space(db: &Connection, bytes: usize) -> Result<(), String> {
    let reserve = (bytes as u64).div_ceil(4096) * 2 + 32;
    loop {
        checkpoint(db)?;
        let pages: i64 = db.query_row("PRAGMA page_count", [], |r| r.get(0)).map_err(|e| e.to_string())?;
        let free: i64 = db.query_row("PRAGMA freelist_count", [], |r| r.get(0)).map_err(|e| e.to_string())?;
        if (pages - free).max(0) as u64 + reserve < PAGES { return Ok(()); }
        if db.execute("DELETE FROM records WHERE seq IN (SELECT seq FROM records ORDER BY seq LIMIT 256)", []).map_err(|e| e.to_string())? == 0 {
            return Err("telemetry SQLite full; retained legacy checkpoint kept".into());
        }
    }
}

/// Existing records have no event IDs: track stream position, never deduplicate by record contents.
fn import(db: &mut Connection, install: &Path, now: u64) -> Result<(), String> {
    let text = legacy_text(install)?;
    let previous: Option<String> = db.query_row("SELECT text FROM legacy WHERE id=1", [], |r| r.get(0)).optional().map_err(|e| e.to_string())?;
    let previous = previous.map(|s| serde_json::from_str::<serde_json::Value>(&s).map_err(|e| format!("legacy checkpoint: {e}"))).transpose()?;
    let matches = |n: usize, digest: &str| text.get(..n).is_some_and(|s| hash(s) == digest);
    let start = match previous.as_ref() {
        None => 0,
        Some(old) => {
            let n = old["bytes"].as_u64().ok_or("legacy checkpoint: missing length")? as usize;
            let digest = old["sha256"].as_str().ok_or("legacy checkpoint: missing digest")?;
            let trimmed = old["trimmed_bytes"].as_u64().ok_or("legacy checkpoint: missing trimmed length")? as usize;
            let trimmed_hash = old["trimmed_sha256"].as_str().ok_or("legacy checkpoint: missing trimmed digest")?;
            if matches(n, digest) { n }
            else if trimmed > 0 && matches(trimmed, trimmed_hash) { trimmed }
            else { return Err("legacy taskq.log rewritten or lost; retained SQLite history kept; coverage unknown".into()); }
        }
    };
    if start == text.len() && previous.is_some() { return Ok(()); }
    let tail = &text.as_bytes()[text.len().saturating_sub((crate::data::LOG_MAX / 2) as usize)..];
    let tail = tail.iter().position(|c| *c == b'\n').map_or(tail, |i| &tail[i+1..]);
    // Store hashes only: service diagnostics are neither telemetry nor retained database payload.
    let tail = std::str::from_utf8(tail).map_err(|e| e.to_string())?;
    let state = serde_json::json!({"bytes":text.len(),"sha256":hash(&text),"trimmed_bytes":tail.len(),"trimmed_sha256":hash(tail)}).to_string();
    let added: Vec<_> = text[start..].lines().filter(|l| telemetry(l)).collect();
    if added.iter().any(|l| l.len() > LINE_MAX) { return Err("legacy telemetry record exceeds 64 KiB; file kept".into()); }
    space(db, added.iter().map(|l| l.len()).sum())?;
    let tx = db.transaction().map_err(|e| e.to_string())?;
    for line in added { insert(&tx, line, now)?; }
    tx.execute("INSERT INTO legacy(id,text) VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET text=excluded.text", [&state]).map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    checkpoint(db)
}

pub fn append(install: &Path, line: &str, now: u64) -> Result<(), String> {
    if line.len() > LINE_MAX { return Err("telemetry append exceeds 64 KiB".into()); }
    let mut db = open(install)?;
    let imported = import(&mut db, install, now);
    space(&db, line.len())?;
    let tx = db.transaction().map_err(|e| e.to_string())?;
    for line in line.lines() { insert(&tx, line, now)?; }
    tx.commit().map_err(|e| e.to_string())?;
    checkpoint(&db)?;
    imported
}

pub fn read(install: &Path, now: u64) -> Result<String, String> {
    let mut db = open(install)?;
    import(&mut db, install, now)?;
    let mut statement = db.prepare("SELECT line FROM records ORDER BY seq").map_err(|e| e.to_string())?;
    let rows = statement.query_map([], |r| r.get::<_, String>(0)).map_err(|e| e.to_string())?;
    let mut text = String::new();
    for row in rows { text.push_str(&row.map_err(|e| e.to_string())?); text.push('\n'); }
    Ok(text)
}

/// Existing service owns retention; failed pruning preserves both stores and is reported by its caller.
pub fn prune(install: &Path, now: u64) -> Result<(), String> {
    let mut db = open(install)?;
    import(&mut db, install, now)?;
    loop {
        checkpoint(&db)?;
        let n = db.execute("DELETE FROM records WHERE seq IN (SELECT seq FROM records WHERE retained_ms < ?1 LIMIT 256)", [now.saturating_sub(WINDOW_MS).min(i64::MAX as u64) as i64]).map_err(|e| e.to_string())?;
        if n == 0 { break; }
    }
    // Reuse free pages; no VACUUM copy and no transient second database.
    checkpoint(&db)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn dir() -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("taskq-sqlite-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&d).unwrap(); d
    }
    #[test]
    fn legacy_retry_append_rotation_and_identical_events() {
        let d = dir(); let p = d.join("taskq.log");
        let original = "taskq-profile same\ntaskq-profile same\ntaskq-profile last\n";
        std::fs::write(&p, original).unwrap();
        assert_eq!(read(&d, 100).unwrap(), original);
        assert_eq!(read(&d, 100).unwrap(), original);
        let rotated = "taskq-profile same\ntaskq-profile last\ntaskq-profile new\n";
        std::fs::write(&p, rotated).unwrap();
        assert_eq!(read(&d, 100).unwrap(), format!("{original}taskq-profile new\n"));
        append(&d, "same\nsame\n", 100).unwrap();
        assert_eq!(read(&d, 100).unwrap().lines().count(), 6);
        std::fs::write(&p, "unrelated\n").unwrap();
        assert!(read(&d, 100).unwrap_err().contains("coverage unknown"));
        let db = open(&d).unwrap();
        assert_eq!(db.query_row("SELECT count(*) FROM records", [], |r| r.get::<_,i64>(0)).unwrap(), 6);
        drop(db); std::fs::remove_dir_all(d).unwrap();
    }
    #[test]
    fn incomplete_legacy_record_is_unknown_but_new_collection_continues() {
        let d = dir(); let p = d.join("taskq.log");
        std::fs::write(&p,"taskq-profile {truncated").unwrap();
        assert!(read(&d,100).unwrap_err().contains("truncated"));
        assert!(append(&d,"new record\n",100).unwrap_err().contains("truncated"));
        let db = open(&d).unwrap();
        assert_eq!(db.query_row("SELECT line FROM records", [], |r| r.get::<_,String>(0)).unwrap(),"new record");
        assert_eq!(std::fs::read_to_string(&p).unwrap(),"taskq-profile {truncated");
        drop(db); std::fs::remove_dir_all(d).unwrap();
    }
    #[test]
    fn failed_transaction_foreign_corrupt_and_newer_store_are_preserved() {
        let d = dir();
        assert_eq!(read(&d,100).unwrap(), "");
        let db = open(&d).unwrap();
        db.execute_batch("CREATE TRIGGER reject_test BEFORE INSERT ON records WHEN new.line='reject' BEGIN SELECT RAISE(ABORT,'invented write failure'); END;").unwrap();
        drop(db);
        assert!(append(&d,"ok\nreject\n",100).is_err());
        assert_eq!(read(&d,100).unwrap(), "", "the first row rolls back with the failed second row");
        assert!(append(&d,&"x".repeat(LINE_MAX+1),100).is_err());
        let db = open(&d).unwrap(); db.pragma_update(None,"user_version",2).unwrap(); drop(db);
        assert!(read(&d,100).unwrap_err().contains("unsupported"));
        let p = d.join("telemetry.log/telemetry.sqlite3"); std::fs::write(&p,b"foreign bytes").unwrap();
        assert!(read(&d,100).is_err()); assert_eq!(std::fs::read(&p).unwrap(),b"foreign bytes");
        std::fs::remove_dir_all(d).unwrap();
    }
    #[test]
    fn acknowledged_write_survives_process_kill() {
        if let Some(path) = std::env::var_os("TASKQ_SQLITE_CRASH_TEST") {
            let d = Path::new(&path);
            append(d, "acknowledged\n", 100).unwrap();
            // Keep the connection and its uncommitted write alive across the forced process exit.
            let db = open(d).unwrap();
            db.execute_batch("BEGIN IMMEDIATE; INSERT INTO records(retained_ms,line) VALUES(100,'uncommitted');").unwrap();
            std::fs::write(d.join("ack"), b"ready").unwrap();
            std::thread::sleep(std::time::Duration::from_secs(30));
            panic!("parent did not kill child");
        }
        let d = dir();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "telemetry::tests::acknowledged_write_survives_process_kill", "--nocapture"])
            .env("TASKQ_SQLITE_CRASH_TEST", &d).stdout(std::process::Stdio::null()).spawn().unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !d.join("ack").exists() {
            assert!(child.try_wait().unwrap().is_none(), "writer exited before acknowledgement");
            if std::time::Instant::now() >= deadline { child.kill().unwrap(); child.wait().unwrap(); panic!("writer did not acknowledge"); }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        child.kill().unwrap(); child.wait().unwrap();
        assert_eq!(read(&d,100).unwrap(), "acknowledged\n");
        std::fs::remove_dir_all(d).unwrap();
    }
    #[test]
    fn legacy_sweep_preserves_store_and_rollback_history() {
        let d = dir();
        append(&d,"new-version\n",100).unwrap();
        crate::platform::sweep(&d, &d.join("unused-shim"));
        std::fs::write(d.join("taskq.log"),"taskq-profile old-version\n").unwrap();
        assert_eq!(read(&d,100).unwrap(),"new-version\ntaskq-profile old-version\n");
        std::fs::remove_dir_all(d).unwrap();
    }
    #[test]
    fn pressure_prunes_oldest_and_keeps_total_disk_bound() {
        let d = dir();
        for n in 0..600 {
            append(&d, &format!("{n}:{}\n", "x".repeat(60 << 10)), 100).unwrap();
            let bytes: u64 = std::fs::read_dir(d.join("telemetry.log")).unwrap().map(|e| e.unwrap().metadata().unwrap().len()).sum();
            assert!(bytes < 64 << 20, "{bytes}");
        }
        let text = read(&d,100).unwrap();
        assert!(!text.starts_with("0:")); assert!(text.lines().last().unwrap().starts_with("599:"));
        std::fs::remove_dir_all(d).unwrap();
    }
    #[test]
    fn retention_and_pinned_reader_refuse_writes_without_loss() {
        let d = dir(); append(&d,"old\n",1).unwrap();
        append(&d,"fresh\n",WINDOW_MS+2).unwrap(); prune(&d,WINDOW_MS+2).unwrap();
        assert_eq!(read(&d,WINDOW_MS+2).unwrap(),"fresh\n");
        let reader = open(&d).unwrap(); reader.execute_batch("BEGIN; SELECT * FROM records;").unwrap();
        let writer = open(&d).unwrap(); writer.execute("INSERT INTO records(retained_ms,line) VALUES(1,'committed')",[]).unwrap();
        assert!(append(&d,"refused\n",100).unwrap_err().contains("checkpoint busy"));
        reader.execute_batch("ROLLBACK").unwrap(); drop(reader); drop(writer);
        assert_eq!(read(&d,100).unwrap(),"fresh\ncommitted\n");
        std::fs::remove_dir_all(d).unwrap();
    }
}
