//! Installation-local telemetry. Writers hold taskq-profile.lock; export owns read-only snapshots.
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

fn legacy_offset(db: &Connection, text: &str) -> Result<Option<usize>, String> {
    let previous: Option<String> = db.query_row("SELECT text FROM legacy WHERE id=1", [], |r| r.get(0)).optional().map_err(|e| e.to_string())?;
    let previous = previous.map(|s| serde_json::from_str::<serde_json::Value>(&s).map_err(|e| format!("legacy checkpoint: {e}"))).transpose()?;
    let matches = |n: usize, digest: &str| text.get(..n).is_some_and(|s| (s.is_empty() || s.ends_with('\n')) && hash(s) == digest);
    match previous.as_ref() {
        None => Ok(None),
        Some(old) => {
            let n = old["bytes"].as_u64().ok_or("legacy checkpoint: missing length")? as usize;
            let digest = old["sha256"].as_str().ok_or("legacy checkpoint: missing digest")?;
            let trimmed = old["trimmed_bytes"].as_u64().ok_or("legacy checkpoint: missing trimmed length")? as usize;
            let trimmed_hash = old["trimmed_sha256"].as_str().ok_or("legacy checkpoint: missing trimmed digest")?;
            if matches(n, digest) { Ok(Some(n)) }
            else if trimmed > 0 && matches(trimmed, trimmed_hash) { Ok(Some(trimmed)) }
            else { Err("legacy taskq.log rewritten or lost; retained SQLite history kept; coverage unknown".into()) }
        }
    }
}

/// Existing records have no event IDs: track stream position, never deduplicate by record contents.
fn import(db: &mut Connection, install: &Path, now: u64) -> Result<(), String> {
    let text = legacy_text(install)?;
    let previous=legacy_offset(db,&text)?;let start=previous.unwrap_or(0);
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

/// Transitional archive view; the service alone imports text and performs retention.
fn legacy_export(install: &Path,since:Option<u64>) -> Result<(serde_json::Value,String),String> {
    use serde_json::json;
    let path=install.join("telemetry.log/telemetry.sqlite3");
    match std::fs::metadata(&path) {
        Err(e) if e.kind()==std::io::ErrorKind::NotFound => {
            if legacy_text(install)?.lines().any(telemetry) {return Err("legacy telemetry awaits service import".into());}
            return Ok((json!({"state":"absent","samples":0}),String::new()));
        }
        Err(e)=>return Err(e.to_string()),Ok(_)=>{}
    }
    let _lock=crate::profile::read_log_lock(install).map_err(|e|format!("legacy snapshot lock: {e}"))?;
    let db=Connection::open_with_flags(&path,rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|e|e.to_string())?;
    db.busy_timeout(std::time::Duration::from_secs(1)).map_err(|e|e.to_string())?;
    db.execute_batch("PRAGMA query_only=ON; BEGIN;").map_err(|e|e.to_string())?;
    let app:i64=db.pragma_query_value(None,"application_id",|r|r.get(0)).map_err(|e|e.to_string())?;
    let version:i64=db.pragma_query_value(None,"user_version",|r|r.get(0)).map_err(|e|e.to_string())?;
    if app!=APP||version!=1 {return Err("legacy snapshot: foreign or unsupported schema; preserved".into());}
    let text=legacy_text(install)?;let offset=legacy_offset(&db,&text)?.unwrap_or(0);
    if text[offset..].lines().any(telemetry) {return Err("legacy telemetry awaits service import".into());}
    let mut statement=db.prepare("SELECT seq,retained_ms,line FROM records ORDER BY seq").map_err(|e|e.to_string())?;
    let rows=statement.query_map([],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?,r.get::<_,String>(2)?))).map_err(|e|e.to_string())?
        .collect::<Result<Vec<_>,_>>().map_err(|e|e.to_string())?;
    drop(statement);drop(db);drop(_lock);
    let bounds=rows.first().zip(rows.last()).map(|(first,last)|[first.0,last.0]);
    let mut out=String::new();let mut samples=0;let mut unknown=0;
    let (mut first,mut last)=(None::<u64>,None::<u64>);
    for (seq,received,line) in rows {
        if seq<=0||received<0||line.len()>LINE_MAX {return Err("legacy snapshot: invalid retained record".into());}
        let value=semantic::observation(&line)?;
        if let Some(time)=value["started_ms"].as_u64() {
            if time>i64::MAX as u64 {return Err("legacy snapshot: invalid source time".into());}
            first=Some(first.map_or(time,|old|old.min(time)));last=Some(last.map_or(time,|old|old.max(time)));
        } else {unknown+=1;}
        if since.is_some_and(|since|value["started_ms"].as_u64().is_some_and(|time|time<since)) {continue;}
        out.push_str(&json!({"source":"legacy","seq":seq,"id":null,"received_ms":received,"observation":value}).to_string());out.push('\n');samples+=1;
    }
    Ok((json!({"state":"read","samples":samples,"retained_seq":bounds,"retained_ms":[first,last],"unknown_started_count":unknown,"id":"not-recorded","pruned_count":null,"unknown":"legacy-retention-not-recorded"}),out))
}

/// Read committed metadata without acquiring write permission, migration, retention or network access.
pub fn export(install:&Path,since:Option<u64>) -> Result<String,String> {
    // Record the export boundary, not a recursive trace for each decoded telemetry row.
    tracing::subscriber::with_default(tracing::subscriber::NoSubscriber::default(),||export_snapshot(install,since))
}

fn export_snapshot(install:&Path,since:Option<u64>) -> Result<String,String> {
    use serde_json::json;
    let reader=coreq::telemetry::Reader::open(&install.join("telemetry.log/universal.sqlite3"))?;
    let since_i64=since.unwrap_or(0).min(i64::MAX as u64) as i64;
    let mut page=reader.export(since_i64,0,1024)?;
    let mut universal=page.clone();
    for key in ["events","after","next","more","schema"] {universal.as_object_mut().unwrap().remove(key);}
    universal["state"]=json!("read");
    let mut out=String::new();let mut samples=0;
    loop {
        for row in page["events"].as_array().ok_or("invalid telemetry export page")? {
            let mut row=row.clone();row["source"]=json!("universal");out.push_str(&row.to_string());out.push('\n');samples+=1;
        }
        if page["more"]==false {break;}
        page=reader.export(since_i64,page["next"].as_i64().ok_or("invalid telemetry export cursor")?,1024)?;
    }
    drop(reader); // Release the WAL snapshot before archive locking and external output.
    universal["samples"]=json!(samples);
    let (legacy,old)=legacy_export(install,since)?;
    let header=json!({"schema":2,"coverage":"independent-committed-snapshots","since_ms":since,"sources":{"universal":universal,"legacy":legacy}});
    Ok(header.to_string()+"\n"+&out+&old)
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
    // Chain: retained semantic history + automatic observations -> read-only schema2 agent export.
    #[test]
    fn export_preserves_history_unknowns_and_refuses_unimported_tail() {
        let d=dir();
        let old="taskq-profile {\"op\":\"spawn\",\"us\":7,\"error\":\"none\",\"at_ms\":1}\n";
        let unknown="taskq-profile {\"op\":\"spawn\",\"us\":8,\"error\":\"none\",\"at_ms\":null}\n";
        crate::profile::append_line(&d.join("taskq.log"),old).unwrap();
        crate::profile::append_line(&d.join("taskq.log"),unknown).unwrap();
        let store=coreq::telemetry::Store::open(&d.join("telemetry.log/universal.sqlite3")).unwrap();
        store.append(&[coreq::telemetry::Event::new(serde_json::json!({"product":"taskq","version":"1","operation":"resume","kind":"method","trace":"t","span":"s","started_ms":2000,"outcome":"failed","error":"returned-error"})).unwrap()]).unwrap();
        drop(store);
        let archive=d.join("telemetry.log/telemetry.sqlite3");let before=std::fs::read(&archive).unwrap();
        let output=export(&d,Some(1000)).unwrap();
        let rows:Vec<serde_json::Value>=output.lines().map(|s|serde_json::from_str(s).unwrap()).collect();
        assert_eq!(rows[0]["sources"]["universal"]["samples"],1);assert_eq!(rows[0]["sources"]["legacy"]["samples"],1);
        assert_eq!(rows[1]["observation"]["operation"],"resume");
        assert_eq!(rows[2]["source"],"legacy");assert_eq!(rows[2]["id"],serde_json::Value::Null);
        assert_eq!(rows[2]["observation"]["started_ms"],serde_json::Value::Null);
        assert_eq!(rows[2]["observation"]["unknown"]["started_ms"],"not-recorded");
        assert_eq!(std::fs::read(&archive).unwrap(),before);
        std::fs::write(d.join("taskq.log"),old).unwrap();
        assert!(export(&d,None).unwrap_err().contains("awaits service import"));
        assert_eq!(std::fs::read(&archive).unwrap(),before);assert_eq!(std::fs::read_to_string(d.join("taskq.log")).unwrap(),old);
        std::fs::remove_dir_all(d).unwrap();
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
