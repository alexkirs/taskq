//! Installation-local telemetry. All storage mutations share the installation budget lock; export is read-only.
mod semantic;
pub(crate) mod diagnostic;
mod source;

use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;
use std::io::Read;

const APP: i64 = 0x5451544d;
const LINE_MAX: usize = 64 << 10;
const WINDOW_MS: u64 = 30 * 86400 * 1000;
const LEGACY_MAX: u64 = crate::data::LOG_MAX + LINE_MAX as u64;

/// Reserve bounded text growth, peer SHM creation and fixed IPC/lock metadata; count existing files too.
pub(crate) fn budget(install:&Path,peer:&str)->Result<coreq::telemetry::Budget,String> {
    let install=std::path::absolute(install).map_err(|e|e.to_string())?;
    let dir=install.join("telemetry.log");
    Ok(coreq::telemetry::Budget {lock:install.join("taskq-profile.lock"),
        peers:vec![dir.join(peer),dir.join(format!("{peer}-wal")),dir.join(format!("{peer}-shm")),install.join("taskq.log")],
        total:64<<20,reserved:LEGACY_MAX+(2<<20)})
}

fn open(install: &Path) -> Result<Connection, String> {
    // Older release sweep keeps *.log entries, including directories; rollback must preserve DB and sidecars together.
    let dir = install.join("telemetry.log");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join("telemetry.sqlite3");
    let ceiling=budget(install,"universal.sqlite3")?.check_files(&path)?;
    let exists = path.exists();
    let db = Connection::open(&path).map_err(|e| format!("telemetry SQLite: {e}"))?;
    db.busy_timeout(std::time::Duration::from_secs(1)).map_err(|e| e.to_string())?;
    let app: i64 = db.query_row("PRAGMA application_id", [], |r| r.get(0)).map_err(|e| e.to_string())?;
    let version: i64 = db.query_row("PRAGMA user_version", [], |r| r.get(0)).map_err(|e| e.to_string())?;
    if exists && (app != APP || version != 1) {
        return Err("telemetry SQLite: foreign or unsupported schema; file kept".into());
    }
    let page:i64=db.pragma_query_value(None,"page_size",|r|r.get(0)).map_err(|e|e.to_string())?;
    let pages:i64=db.pragma_query_value(None,"page_count",|r|r.get(0)).map_err(|e|e.to_string())?;
    if page<1024||pages>ceiling/page {return Err("telemetry installation budget exhausted; legacy archive preserved".into());}
    db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA temp_store=MEMORY; PRAGMA cache_spill=OFF; PRAGMA cache_size=-32768; PRAGMA wal_autocheckpoint=0;")
        .map_err(|e| e.to_string())?;
    db.pragma_update(None, "max_page_count", ceiling/page).map_err(|e| e.to_string())?;
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
    checkpoint(db)?;
    let page:i64=db.pragma_query_value(None,"page_size",|r|r.get(0)).map_err(|e|e.to_string())?;
    let ceiling:i64=db.pragma_query_value(None,"max_page_count",|r|r.get(0)).map_err(|e|e.to_string())?;
    let pages:i64=db.pragma_query_value(None,"page_count",|r|r.get(0)).map_err(|e|e.to_string())?;
    let free:i64=db.pragma_query_value(None,"freelist_count",|r|r.get(0)).map_err(|e|e.to_string())?;
    let reserve=(bytes as u64).div_ceil(page as u64)*2+32;
    if (pages-free).max(0) as u64+reserve>=ceiling as u64 {
        return Err("legacy import full; source archive preserved; migration required".into());
    }
    Ok(())
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

/// Metadata-only product records share the installed capture queue and its health.
pub fn observe(line:&str)->Result<(),String> {
    if line.len()>LINE_MAX {return Err("telemetry append exceeds 64 KiB".into());}
    coreq::telemetry::capture::observe(semantic::observation(line.trim_end())?)
}

/// One owner uses the existing election and budget; live collectors must be quiesced first.
pub fn migrate(install:&Path)->Result<u64,String> {
    let path=install.join("telemetry.log/universal.sqlite3");
    let _owner=coreq::telemetry::maintenance(&path)?;
    if !has_legacy(install)? {return Ok(0);}
    let mut archive=source::prepare(install)?;
    let target=coreq::telemetry::Store::open_bounded(&path,budget(install,"telemetry.sqlite3")?)?;
    source::import(&mut archive,&target,install)
}

pub fn report_input(install:&Path)->Result<String,String> {
    // Include this process's admitted records before taking the committed snapshot.
    tracing::dispatcher::get_default(|d|d.downcast_ref::<coreq::telemetry::capture::Capture>().cloned()).map(|capture|capture.flush()).transpose()?;
    tracing::subscriber::with_default(tracing::subscriber::NoSubscriber::default(),|| {
        let path=install.join("telemetry.log/universal.sqlite3");
        let reader=path.try_exists().map_err(|e|e.to_string())?.then(||coreq::telemetry::Reader::open(&path)).transpose()?;
        let cursor=reader.as_ref().map(|r|r.import_cursor("taskq.legacy")).transpose()?.flatten();
        let archive=legacy_rows(install,cursor.as_deref())?.into_iter().map(|(_,_,line)|line+"\n").collect::<String>();
        match reader {Some(reader)=>semantic::read(&reader,semantic::KINDS,&archive),None=>Ok(archive)}
    })
}

#[cfg(test)]
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
    if !has_legacy(install)? {return Ok(String::new());}
    let mut db = open(install)?;
    import(&mut db, install, now)?;
    let mut statement = db.prepare("SELECT line FROM records ORDER BY seq").map_err(|e| e.to_string())?;
    let rows = statement.query_map([], |r| r.get::<_, String>(0)).map_err(|e| e.to_string())?;
    let mut text = String::new();
    for row in rows { text.push_str(&row.map_err(|e| e.to_string())?); text.push('\n'); }
    Ok(text)
}

/// Transitional archive view; the service alone imports text and performs retention.
fn legacy_rows(install: &Path,cursor:Option<&str>) -> Result<Vec<(i64,i64,String)>,String> {
    let path=install.join("telemetry.log/telemetry.sqlite3");
    match std::fs::metadata(&path) {
        Err(e) if e.kind()==std::io::ErrorKind::NotFound => {
            if cursor.is_some() {return Err("legacy source: imported archive missing; coverage unknown".into());}
            if legacy_text(install)?.lines().any(telemetry) {return Err("legacy telemetry awaits service import".into());}
            return Ok(Vec::new());
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
    let after=source::pending(&db,cursor)?;
    let sql=if after.is_some() {"SELECT seq,retained_ms,line FROM records WHERE telemetry_epoch>?1 OR (telemetry_epoch=?1 AND seq>?2) ORDER BY telemetry_epoch,seq"} else {"SELECT seq,retained_ms,line FROM records WHERE ?1=-1 AND ?2=0 ORDER BY seq"};
    let mut statement=db.prepare(sql).map_err(|e|e.to_string())?;
    let after=after.unwrap_or((-1,0));
    let rows=statement.query_map(params![after.0,after.1],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?,r.get::<_,String>(2)?))).map_err(|e|e.to_string())?
        .collect::<Result<Vec<_>,_>>().map_err(|e|e.to_string())?;
    drop(statement);drop(db);drop(_lock);
    if rows.iter().any(|(seq,received,line)|*seq<=0||*received<0||line.len()>LINE_MAX) {
        return Err("legacy snapshot: invalid retained record".into());
    }
    Ok(rows)
}

fn legacy_export(install:&Path,since:Option<u64>,cursor:Option<&str>)->Result<(serde_json::Value,String),String> {
    use serde_json::json;
    let rows=legacy_rows(install,cursor)?;
    if !install.join("telemetry.log/telemetry.sqlite3").try_exists().map_err(|e|e.to_string())? {
        return Ok((json!({"state":"absent","samples":0}),String::new()));
    }
    let bounds=rows.first().zip(rows.last()).map(|(first,last)|[first.0,last.0]);
    let mut out=String::new();let mut samples=0;let mut unknown=0;
    let (mut first,mut last)=(None::<u64>,None::<u64>);
    for (seq,received,line) in rows {
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
#[tracing::instrument(name="telemetry.export",skip_all)]
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
    universal["samples"]=json!(samples);
    let cursor=reader.import_cursor("taskq.legacy")?;
    let (legacy,old)=legacy_export(install,since,cursor.as_deref())?;
    drop(reader); // Source filtering and target rows share one import-cursor snapshot.
    let header=json!({"schema":2,"coverage":"committed-import-cursor-snapshot","since_ms":since,"sources":{"universal":universal,"legacy":legacy}});
    Ok(header.to_string()+"\n"+&out+&old)
}

fn has_legacy(install:&Path)->Result<bool,String> {
    Ok(install.join("telemetry.log/telemetry.sqlite3").try_exists().map_err(|e|e.to_string())?
        || legacy_text(install)?.lines().any(telemetry))
}

/// Existing service owns archive retention; never create an empty replacement legacy sink.
pub fn prune(install: &Path, now: u64) -> Result<(), String> {
    if !has_legacy(install)?||source::preserved(install)? {return Ok(());}
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
        let d = std::env::temp_dir().join(format!("taskq-sqlite-{}-{}", std::process::id(), crate::runtime::uuid()));
        std::fs::create_dir(&d).unwrap(); d
    }
    // Chain: product migration retry -> mixed reports -> original clocks, archive and rollback reader.
    #[test]
    fn maintenance_migration_retries_without_duplicate_reports_or_retention_reset() {
        let d=dir();drop(crate::profile::log_lock(&d.join("taskq.log")).unwrap());
        let now=std::time::SystemTime::now().duration_since(std::time::SystemTime::UNIX_EPOCH).unwrap().as_millis() as u64;
        let received=now-60_000;
        let line=r#"taskq-profile {"op":"spawn","us":7,"error":"none","at_ms":1}"#;
        let mut archive=open(&d).unwrap();
        {let tx=archive.transaction().unwrap();for i in 0..1100 {
            let stage=if i==1023 {Some(r#"taskq-stage {"project":"o/r","host":"mac","stage":"build","task":1,"attempt":1,"rt":"claude","open":true,"outcome":"unknown","error":"none","at_ms":1,"unknown":"open-stage"}"#)}
                else if i==1024 {Some(r#"taskq-stage {"project":"o/r","host":"mac","stage":"build","task":1,"attempt":1,"rt":"claude","open":false,"outcome":"ok","error":"none","at_ms":1}"#)} else {None};
            insert(&tx,stage.unwrap_or(line),received).unwrap();
        }tx.commit().unwrap();}
        checkpoint(&archive).unwrap();drop(archive);
        let expected=crate::profile::legacy_reports(&report_input(&d).unwrap(),now).unwrap();
        let path=d.join("telemetry.log/universal.sqlite3");drop(coreq::telemetry::Store::open(&path).unwrap());
        let inject=Connection::open(&path).unwrap();
        inject.execute_batch("CREATE TABLE imports(source TEXT PRIMARY KEY,cursor TEXT NOT NULL,receipt TEXT NOT NULL); CREATE TRIGGER refuse_second BEFORE UPDATE ON imports BEGIN SELECT RAISE(ABORT,'injected second batch failure'); END").unwrap();
        assert!(migrate(&d).unwrap_err().contains("injected second batch failure"));
        assert_eq!(crate::profile::legacy_reports(&report_input(&d).unwrap(),now).unwrap(),expected,"partly committed import double-counted history");
        let cursor=coreq::telemetry::Reader::open(&path).unwrap().import_cursor("taskq.legacy").unwrap().unwrap();
        assert!(cursor.ends_with(".0.1024"));
        inject.execute_batch("DROP TRIGGER refuse_second").unwrap();drop(inject);
        assert_eq!(migrate(&d).unwrap(),76);assert_eq!(migrate(&d).unwrap(),0);
        assert_eq!(crate::profile::legacy_reports(&report_input(&d).unwrap(),now).unwrap(),expected);
        let exported=export(&d,None).unwrap();let rows=exported.lines().map(|s|serde_json::from_str::<serde_json::Value>(s).unwrap()).collect::<Vec<_>>();
        assert_eq!(rows.len(),1101);assert_eq!(rows[0]["sources"]["legacy"]["samples"],0);
        assert!(rows[1..].iter().all(|r|r["received_ms"]==received));
        let old=open(&d).unwrap();insert(&old,line,received+1).unwrap();checkpoint(&old).unwrap();
        assert_eq!(report_input(&d).unwrap().lines().count(),1101,"old writer append vanished");
        drop(old);assert_eq!(migrate(&d).unwrap(),1);
        assert_eq!(read(&d,now).unwrap().lines().count(),1101,"compatible rollback reader lost history");
        // Expired target records stay pruned even though the preserved archive still holds them.
        let old=open(&d).unwrap();let seq:i64=old.query_row("SELECT max(seq) FROM records",[],|r|r.get(0)).unwrap();
        insert(&old,line,1).unwrap();checkpoint(&old).unwrap();drop(old);
        assert_eq!(migrate(&d).unwrap(),1);
        assert_eq!(report_input(&d).unwrap().lines().count(),1101);
        assert_eq!(migrate(&d).unwrap(),0);
        assert_eq!(read(&d,now).unwrap().lines().count(),(seq+1) as usize);
        prune(&d,now+WINDOW_MS+1).unwrap();
        assert_eq!(read(&d,now).unwrap().lines().count(),(seq+1) as usize,"service retired the unaccepted migration source");
        std::fs::remove_dir_all(d).unwrap();
    }

    // Chain: retained semantic history + automatic observations -> read-only schema2 agent export.
    #[test]
    fn live_capacity_counts_retained_archive_without_deleting_it() {
        let d=dir();drop(crate::profile::log_lock(&d.join("taskq.log")).unwrap());
        let path=d.join("telemetry.log/universal.sqlite3");std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let live=coreq::telemetry::Store::open_bounded(&path,budget(&d,"telemetry.sqlite3").unwrap()).unwrap();
        let before=live.storage().unwrap()["database_limit"].as_i64().unwrap();
        let archive_count;
        {
            let _lock=crate::profile::log_lock(&d.join("taskq.log")).unwrap();let old=open(&d).unwrap();
            old.execute("INSERT INTO records(retained_ms,line) VALUES(1,?1)",[r#"taskq-profile {"op":"spawn","us":1,"error":"none","at_ms":1}"#]).unwrap();
            for _ in 0..17 {checkpoint(&old).unwrap();old.execute("INSERT INTO records(retained_ms,line) SELECT retained_ms,line FROM records",[]).unwrap();}
            checkpoint(&old).unwrap();archive_count=old.query_row("SELECT count(*) FROM records",[],|r|r.get::<_,i64>(0)).unwrap();
        }
        let event=coreq::telemetry::Event::new(semantic::observation(r#"taskq-profile {"op":"spawn","us":2,"error":"none","at_ms":2}"#).unwrap()).unwrap();
        live.append(&[event]).unwrap();let storage=live.storage().unwrap();
        assert!(storage["database_limit"].as_i64().unwrap()<before);
        assert!(storage["installation_accounted_bytes"].as_u64().unwrap()<=64<<20);
        let _lock=crate::profile::log_lock(&d.join("taskq.log")).unwrap();let old=open(&d).unwrap();
        assert_eq!(old.query_row("SELECT count(*) FROM records",[],|r|r.get::<_,i64>(0)).unwrap(),archive_count);
        drop(old);drop(_lock);drop(live);std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn live_semantics_use_the_installed_capture_and_leave_no_legacy_sink() {
        use tracing_subscriber::prelude::*;
        let d=dir();std::fs::create_dir(d.join("telemetry.log")).unwrap();
        let store=std::sync::Arc::new(coreq::telemetry::Store::open_bounded(&d.join("telemetry.log/universal.sqlite3"),budget(&d,"telemetry.sqlite3").unwrap()).unwrap());
        let capture=coreq::telemetry::capture::Capture::new(store.clone(),"taskq","test").unwrap();
        tracing::subscriber::with_default(tracing_subscriber::registry().with(capture.clone()),|| {
            let _action=tracing::info_span!("fixture.command").entered();
            crate::profile::append_line(&d.join("taskq.log"),r#"taskq-stage {"stage":"claim","task":1,"attempt":2,"rt":"other","us":1,"outcome":"ok","error":"none","at_ms":1,"unknown":"no-model"}"#).unwrap();
            for line in [
                r#"taskq-profile {"op":"spawn","us":1,"error":"none","at_ms":1}"#,
                r#"taskq-board {"op":"close","us":12,"outcome":"failed","error":"http4xx","remaining":null,"at_ms":1}"#,
                r#"taskq-transition {"project":"other","task":1,"entry":2,"from":"doing","to":"ask","prev_s":null,"rt":null,"host":null,"at_ms":1}"#,
                r#"taskq-requests {"at_ms":1,"source":"worker service status","ops":{"GET issues/N/comments":7}}"#,
            ] {crate::profile::append_line(&d.join("taskq.log"),line).unwrap();}
        });
        capture.flush().unwrap();
        assert!(!d.join("telemetry.log/telemetry.sqlite3").exists(),"live semantic events must use the installed writer");
        let rows=store.export_kinds(&["stage"],0,0,1024).unwrap();
        let event=&rows["events"][0]["observation"];
        assert_eq!(event["version"],"test");assert!(event["trace"].is_string());
        let report=crate::profile::read_log(&d).unwrap();
        assert_eq!(report.lines().count(),5);
        for prefix in ["taskq-profile ","taskq-stage ","taskq-board ","taskq-transition ","taskq-requests "] {
            assert!(report.lines().any(|line|line.starts_with(prefix)),"{prefix}");
        }
        drop(capture);drop(store);std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn export_preserves_history_unknowns_and_refuses_unimported_tail() {
        let d=dir();
        let old="taskq-profile {\"op\":\"spawn\",\"us\":7,\"error\":\"none\",\"at_ms\":1}\n";
        let unknown="taskq-profile {\"op\":\"spawn\",\"us\":8,\"error\":\"none\",\"at_ms\":null}\n";
        drop(crate::profile::log_lock(&d.join("taskq.log")).unwrap());
        append(&d,old,1).unwrap();
        append(&d,unknown,1).unwrap();
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
    fn archive_pressure_refuses_growth_without_deleting_history() {
        let d=dir();let mut accepted=0;
        for n in 0..600 {
            match append(&d,&format!("{n}:{}\n","x".repeat(60<<10)),100) {
                Ok(())=>accepted+=1,
                Err(error)=>{assert!(error.contains("source archive preserved"),"append {n} at {}: {error}",d.display());break;}
            }
            let bytes:u64=std::fs::read_dir(d.join("telemetry.log")).unwrap().map(|e|e.unwrap().metadata().unwrap().len()).sum();
            assert!(bytes<64<<20);
        }
        assert!(accepted>0&&accepted<600);
        let text=read(&d,100).unwrap();assert_eq!(text.lines().count(),accepted);assert!(text.starts_with("0:"));
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
