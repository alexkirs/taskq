//! Legacy stream identity. Qualification-only until single-store cutover is enabled.
use rusqlite::{Connection,TransactionBehavior,params};
use coreq::telemetry::{Event,Store};
use ring::rand::{SecureRandom,SystemRandom};

type Res<T> = Result<T,String>;
const SOURCE: &str="taskq.legacy";
const INSERT: &str="CREATE TRIGGER taskq_source_insert AFTER INSERT ON records BEGIN
 UPDATE telemetry_source SET epoch=epoch+(new.seq<=high_seq),high_seq=new.seq WHERE id=1;
 UPDATE records SET telemetry_epoch=(SELECT epoch FROM telemetry_source WHERE id=1) WHERE seq=new.seq;
 END";
const UPDATE: &str="CREATE TRIGGER taskq_source_update AFTER UPDATE OF seq,retained_ms,line ON records
 WHEN old.seq!=new.seq OR old.retained_ms!=new.retained_ms OR old.line!=new.line BEGIN
 UPDATE telemetry_source SET invalid=1 WHERE id=1; END";
const DELETE: &str="CREATE TRIGGER taskq_source_delete AFTER DELETE ON records BEGIN
 UPDATE telemetry_source SET removed_seq=CASE WHEN old.telemetry_epoch>removed_epoch OR (old.telemetry_epoch=removed_epoch AND old.seq>removed_seq) THEN old.seq ELSE removed_seq END,
 removed_epoch=max(removed_epoch,old.telemetry_epoch) WHERE id=1; END";
fn error(e: impl std::fmt::Display) -> String {format!("legacy source: {e}")}
fn namespace(text: &str) -> bool {text.len()==32&&text.bytes().all(|v|v.is_ascii_digit()||(b'a'..=b'f').contains(&v))}

// Caller owns taskq-profile.lock; install changes source metadata only, in one FULL transaction.
fn install(db: &mut Connection) -> Res<()> {
    if db.pragma_query_value(None,"application_id",|r|r.get::<_,i64>(0)).map_err(error)?!=super::APP
        ||db.pragma_query_value(None,"user_version",|r|r.get::<_,i64>(0)).map_err(error)?!=1 {return Err("legacy source: unsupported archive; preserved".into());}
    let check:String=db.query_row("PRAGMA quick_check",[],|r|r.get(0)).map_err(error)?;
    if check!="ok" {return Err("legacy source: corrupt archive; preserved".into());}
    let page:i64=db.pragma_query_value(None,"page_size",|r|r.get(0)).map_err(error)?;
    let pages:i64=db.pragma_query_value(None,"page_count",|r|r.get(0)).map_err(error)?;
    if page<1024||pages>28*1024*1024/page {return Err("legacy source: oversized archive; preserved".into());}
    db.execute_batch("PRAGMA synchronous=FULL; PRAGMA temp_store=MEMORY; PRAGMA cache_spill=OFF; PRAGMA wal_autocheckpoint=0;").map_err(error)?;
    db.pragma_update(None,"max_page_count",28*1024*1024/page).map_err(error)?;
    super::checkpoint(db)?;
    let tx=db.transaction_with_behavior(TransactionBehavior::Immediate).map_err(error)?;
    let exists:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='telemetry_source')",[],|r|r.get(0)).map_err(error)?;
    let epoch:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info('records') WHERE name='telemetry_epoch')",[],|r|r.get(0)).map_err(error)?;
    let triggers:i64=tx.query_row("SELECT count(*) FROM sqlite_master WHERE name IN ('taskq_source_insert','taskq_source_update','taskq_source_delete')",[],|r|r.get(0)).map_err(error)?;
    if !exists {
        if epoch||triggers!=0 {return Err("legacy source: incomplete identity extension; preserved".into());}
        let mut id=[0;16];SystemRandom::new().fill(&mut id).map_err(|_|"legacy source: random identity unavailable")?;
        let id=id.iter().map(|byte|format!("{byte:02x}")).collect::<String>();
        tx.execute_batch("ALTER TABLE records ADD COLUMN telemetry_epoch INTEGER NOT NULL DEFAULT 0;
          CREATE TABLE telemetry_source(id INTEGER PRIMARY KEY CHECK(id=1),version INTEGER NOT NULL,namespace TEXT NOT NULL,epoch INTEGER NOT NULL,high_seq INTEGER NOT NULL,invalid INTEGER NOT NULL,removed_epoch INTEGER NOT NULL,removed_seq INTEGER NOT NULL);").map_err(error)?;
        tx.execute("INSERT INTO telemetry_source SELECT 1,1,?1,0,coalesce(max(seq),0),0,-1,0 FROM records",[id]).map_err(error)?;
        for sql in [INSERT,UPDATE,DELETE] {tx.execute_batch(sql).map_err(error)?;}
    } else if !epoch||triggers!=3 {return Err("legacy source: incomplete identity extension; preserved".into());}
    for (name,sql) in [("taskq_source_insert",INSERT),("taskq_source_update",UPDATE),("taskq_source_delete",DELETE)] {
        let actual:String=tx.query_row("SELECT sql FROM sqlite_master WHERE name=?1",[name],|r|r.get(0)).map_err(error)?;
        if actual!=sql {return Err("legacy source: unsupported identity trigger; preserved".into());}
    }
    state(&tx)?;tx.commit().map_err(error)
}
struct State {namespace:String,epoch:i64,high:i64,removed:(i64,i64)}
fn state(db: &Connection) -> Res<State> {
    let count:i64=db.query_row("SELECT count(*) FROM telemetry_source",[],|r|r.get(0)).map_err(error)?;
    if count!=1 {return Err("legacy source: invalid identity cardinality; preserved".into());}
    let (version,id,epoch,high,invalid,removed,seq):(i64,String,i64,i64,i64,i64,i64)=db.query_row("SELECT version,namespace,epoch,high_seq,invalid,removed_epoch,removed_seq FROM telemetry_source WHERE id=1",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?))).map_err(error)?;
    if version!=1||!namespace(&id)||epoch<0||high<0||invalid!=0||removed< -1||seq<0||removed>epoch {return Err("legacy source: invalid or mutated source identity; preserved".into());}
    Ok(State{namespace:id,epoch,high,removed:(removed,seq)})
}
fn position(cursor: Option<&str>, identity: &str) -> Res<(i64,i64)> {
    let Some(cursor)=cursor else {return Ok((-1,0));};let words=cursor.split('.').collect::<Vec<_>>();
    if words.len()!=4||words[0]!="v1"||words[1]!=identity {return Err("legacy source: replaced archive identity; resume refused".into());}
    let epoch=words[2].parse::<i64>().map_err(error)?;let seq=words[3].parse::<i64>().map_err(error)?;
    if epoch<0||seq<=0||cursor!=format!("v1.{identity}.{epoch}.{seq}") {return Err("legacy source: invalid progress; preserved".into());}Ok((epoch,seq))
}

pub(super) fn import(db: &mut Connection, target: &Store) -> Res<u64> {
    install(db)?;let mut imported=0;
    loop {
        let cursor=target.import_cursor(SOURCE)?;let tx=db.transaction().map_err(error)?;let source=state(&tx)?;let after=position(cursor.as_deref(),&source.namespace)?;
        if after.0>source.epoch||(after.0==source.epoch&&after.1>source.high) {return Err("legacy source: source progress moved backwards; resume refused".into());}
        if source.removed>after {return Err("legacy source: unimported records pruned; coverage unknown".into());}
        let mut read=tx.prepare("SELECT telemetry_epoch,seq,retained_ms,line FROM records WHERE telemetry_epoch>?1 OR (telemetry_epoch=?1 AND seq>?2) ORDER BY telemetry_epoch,seq LIMIT 1024").map_err(error)?;
        let rows=read.query_map(params![after.0,after.1],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?,r.get::<_,i64>(2)?,r.get::<_,String>(3)?))).map_err(error)?.collect::<Result<Vec<_>,_>>().map_err(error)?;drop(read);
        if rows.is_empty() {return Ok(imported);}
        let mut events=Vec::with_capacity(rows.len());let mut clocks=Vec::with_capacity(rows.len());
        for (epoch,seq,received,line) in &rows {
            if *epoch<0||*epoch>source.epoch||(*epoch==source.epoch&&*seq>source.high)||*seq<=0||*received<0 {return Err("legacy source: invalid record position or clock".into());}
            events.push(Event::identified(&format!("taskq.legacy.{}",source.namespace),&format!("{epoch}.{seq}"),super::semantic::observation(line)?)?);clocks.push(*received);
        }
        let last=rows.last().unwrap();let next=format!("v1.{}.{}.{}",source.namespace,last.0,last.1);tx.commit().map_err(error)?;
        target.import_history(SOURCE,cursor.as_deref(),&next,&events,&clocks)?;imported+=events.len() as u64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::OptionalExtension;
    const LINE:&str=r#"taskq-profile {"op":"spawn","us":10,"error":"none","at_ms":null}"#;
    fn dir() -> std::path::PathBuf {
        let dir=std::env::temp_dir().join(format!("taskq-source-{}-{}",std::process::id(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));std::fs::create_dir(&dir).unwrap();dir
    }
    fn now() -> i64 {std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64}
    fn add(db: &Connection) {db.execute("INSERT INTO records(retained_ms,line) VALUES(?1,?2)",params![now(),LINE]).unwrap();}
    #[test]
    fn identical_observations_survive_sequence_reuse_and_old_writers() {
        let dir=dir();let mut archive=super::super::open(&dir).unwrap();add(&archive);add(&archive);
        let older=super::super::open(&dir).unwrap();let mut insert=older.prepare("INSERT INTO records(retained_ms,line) VALUES(?1,?2)").unwrap();
        let target=Store::open(&dir.join("common")).unwrap();assert_eq!(import(&mut archive,&target).unwrap(),2);assert_eq!(import(&mut archive,&target).unwrap(),0);
        let identity=state(&archive).unwrap().namespace;let first=target.export(0,0,1024).unwrap();assert_ne!(first["events"][0]["id"],first["events"][1]["id"]);
        archive.execute("DELETE FROM records WHERE seq=2",[]).unwrap();insert.execute(params![now(),LINE]).unwrap();assert_eq!(import(&mut archive,&target).unwrap(),1);
        assert_eq!(archive.query_row("SELECT telemetry_epoch FROM records WHERE seq=2",[],|r|r.get::<_,i64>(0)).unwrap(),1);
        archive.execute("DELETE FROM records",[]).unwrap();insert.execute(params![now(),LINE]).unwrap();assert_eq!(import(&mut archive,&target).unwrap(),1);
        let report=target.export(0,0,1024).unwrap();assert_eq!(report["events"].as_array().unwrap().len(),4);let ids=report["events"].as_array().unwrap().iter().map(|row|row["id"].as_str().unwrap()).collect::<std::collections::HashSet<_>>();assert_eq!(ids.len(),4);
        assert_eq!(state(&archive).unwrap().namespace,identity);assert_eq!(archive.pragma_query_value(None,"user_version",|r|r.get::<_,i64>(0)).unwrap(),1);
        drop(insert);drop(older);drop(archive);let mut archive=super::super::open(&dir).unwrap();assert_eq!(import(&mut archive,&target).unwrap(),0);
        assert_eq!(super::super::read(&dir,now() as u64).unwrap(),format!("{LINE}\n"));drop(archive);drop(target);std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn removed_unimported_records_and_mutation_are_explicit_unknowns() {
        let dir=dir();let mut archive=super::super::open(&dir).unwrap();add(&archive);add(&archive);install(&mut archive).unwrap();let target=Store::open(&dir.join("common")).unwrap();
        archive.execute("DELETE FROM records WHERE seq=2",[]).unwrap();assert!(import(&mut archive,&target).unwrap_err().contains("unimported records pruned"));assert_eq!(target.import_cursor(SOURCE).unwrap(),None);
        archive.execute("UPDATE records SET line=?1 WHERE seq=1",[LINE.replace("10","11")]).unwrap();assert!(import(&mut archive,&target).unwrap_err().contains("mutated"));
        assert_eq!(target.export(0,0,10).unwrap()["events"],serde_json::json!([]));drop(archive);drop(target);std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn replacement_and_rewound_identity_never_resume_another_stream() {
        let dir=dir();let mut archive=super::super::open(&dir).unwrap();add(&archive);add(&archive);let target=Store::open(&dir.join("common")).unwrap();import(&mut archive,&target).unwrap();let before=target.export(0,0,10).unwrap();
        let replacement=dir.join("replacement");std::fs::create_dir(&replacement).unwrap();let mut other=super::super::open(&replacement).unwrap();add(&other);
        assert!(import(&mut other,&target).unwrap_err().contains("replaced archive identity"));
        archive.execute("UPDATE telemetry_source SET high_seq=1",[]).unwrap();assert!(import(&mut archive,&target).unwrap_err().contains("moved backwards"));assert_eq!(target.export(0,0,10).unwrap(),before);
        drop(other);drop(archive);drop(target);std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn late_target_failure_reuses_the_durable_source_namespace_and_cursor() {
        let dir=dir();let mut archive=super::super::open(&dir).unwrap();add(&archive);add(&archive);let path=dir.join("common");let target=Store::open(&path).unwrap();
        let inject=Connection::open(&path).unwrap();inject.execute_batch("CREATE TABLE imports(source TEXT PRIMARY KEY,cursor TEXT NOT NULL,receipt TEXT NOT NULL); CREATE TRIGGER refuse_cursor BEFORE INSERT ON imports BEGIN SELECT RAISE(ABORT,'injected source progress failure'); END").unwrap();
        assert!(import(&mut archive,&target).unwrap_err().contains("injected source progress failure"));let identity=state(&archive).unwrap().namespace;
        assert_eq!(target.import_cursor(SOURCE).unwrap(),None);assert_eq!(target.export(0,0,10).unwrap()["events"],serde_json::json!([]));
        inject.execute_batch("DROP TRIGGER refuse_cursor").unwrap();drop(inject);drop(archive);let mut archive=super::super::open(&dir).unwrap();
        assert_eq!(import(&mut archive,&target).unwrap(),2);assert_eq!(state(&archive).unwrap().namespace,identity);assert_eq!(import(&mut archive,&target).unwrap(),0);
        drop(archive);drop(target);std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    #[ignore="source identity qualification on a frozen standalone TASKQ_SEMANTIC_SOURCE snapshot"]
    fn retained_source_import_replays_and_preserves_all_reports_and_receipt_clocks() {
        let snapshot=std::path::PathBuf::from(std::env::var_os("TASKQ_SEMANTIC_SOURCE").expect("explicit snapshot"));
        let mut wal=snapshot.as_os_str().to_os_string();wal.push("-wal");assert!(std::fs::metadata(std::path::Path::new(&wal)).map_or(true,|m|m.len()==0),"snapshot must have no pending WAL");
        let dir=dir();std::fs::create_dir(dir.join("telemetry.log")).unwrap();std::fs::copy(&snapshot,dir.join("telemetry.log/telemetry.sqlite3")).unwrap();
        let mut archive=super::super::open(&dir).unwrap();let target=Store::open(&dir.join("common")).unwrap();
        let mut expected=ring::digest::Context::new(&ring::digest::SHA256);let mut text=String::new();let mut count=0;
        let canonical=|line:&str|if line.starts_with("taskq-requests ") {crate::requests::legacy_value(line)} else {crate::profile::legacy_value(line)};
        {let mut read=archive.prepare("SELECT retained_ms,line FROM records ORDER BY seq").unwrap();let rows=read.query_map([],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?))).unwrap();
        for row in rows {let (received,line)=row.unwrap();expected.update(format!("{received} {} {}\n",line.split_once(' ').unwrap().0,canonical(&line).unwrap()).as_bytes());text.push_str(&line);text.push('\n');count+=1;}}
        let legacy:Option<String>=archive.query_row("SELECT text FROM legacy WHERE id=1",[],|r|r.get(0)).optional().unwrap();
        let at=now() as u64;let reports=crate::profile::legacy_reports(&text,at).unwrap();
        assert_eq!(import(&mut archive,&target).unwrap(),count);let cursor=target.import_cursor(SOURCE).unwrap();let identity=state(&archive).unwrap().namespace;
        drop(archive);let mut archive=super::super::open(&dir).unwrap();assert_eq!(import(&mut archive,&target).unwrap(),0);assert_eq!(state(&archive).unwrap().namespace,identity);assert_eq!(target.import_cursor(SOURCE).unwrap(),cursor);
        let mut actual=ring::digest::Context::new(&ring::digest::SHA256);let mut restored=String::new();let mut after=0;let mut retained=0;
        loop {let page=target.export(0,after,1024).unwrap();for row in page["events"].as_array().unwrap() {
            let line=super::super::semantic::line(&row["observation"]).unwrap();let received=row["received_ms"].as_i64().unwrap();actual.update(format!("{received} {} {}\n",line.split_once(' ').unwrap().0,canonical(&line).unwrap()).as_bytes());restored.push_str(&line);restored.push('\n');retained+=1;
        }after=page["next"].as_i64().unwrap();if page["more"]==false {break;}}
        let digest=|value:ring::digest::Digest|value.as_ref().iter().map(|byte|format!("{byte:02x}")).collect::<String>();
        let expected=digest(expected.finish());let actual=digest(actual.finish());assert_eq!(expected,actual);assert_eq!(retained,count);assert_eq!(target.storage().unwrap()["pruned_count"],0);
        assert_eq!(crate::profile::legacy_reports(&restored,at).unwrap(),reports);
        let unchanged={let mut read=archive.prepare("SELECT line FROM records ORDER BY seq").unwrap();read.query_map([],|r|r.get::<_,String>(0)).unwrap().map(|line|line.unwrap()+"\n").collect::<String>()};assert_eq!(unchanged,text);
        assert_eq!(archive.query_row("SELECT text FROM legacy WHERE id=1",[],|r|r.get::<_,String>(0)).optional().unwrap(),legacy);
        println!("source identity import: {count} rows; canonical+receipt SHA256 {actual}; 9 reports equal; replay 0; pruning 0");
        drop(archive);drop(target);std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn pinned_source_reader_refuses_identity_installation_without_partial_metadata() {
        let dir=dir();let mut archive=super::super::open(&dir).unwrap();add(&archive);let target=Store::open(&dir.join("common")).unwrap();
        let reader=Connection::open(dir.join("telemetry.log/telemetry.sqlite3")).unwrap();reader.execute_batch("BEGIN; SELECT * FROM records").unwrap();
        assert!(import(&mut archive,&target).unwrap_err().contains("checkpoint busy"));
        assert_eq!(archive.query_row("SELECT count(*) FROM pragma_table_info('records') WHERE name='telemetry_epoch'",[],|r|r.get::<_,i64>(0)).unwrap(),0);assert_eq!(target.import_cursor(SOURCE).unwrap(),None);
        reader.execute_batch("COMMIT").unwrap();drop(reader);assert_eq!(import(&mut archive,&target).unwrap(),1);
        drop(archive);drop(target);std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn incomplete_source_identity_is_refused_without_advancing_target() {
        let dir=dir();let mut archive=super::super::open(&dir).unwrap();add(&archive);let target=Store::open(&dir.join("common")).unwrap();
        archive.execute_batch("CREATE TRIGGER taskq_source_insert AFTER INSERT ON records BEGIN SELECT 1; END").unwrap();
        assert!(import(&mut archive,&target).unwrap_err().contains("incomplete identity"));
        assert_eq!(archive.query_row("SELECT count(*) FROM pragma_table_info('records') WHERE name='telemetry_epoch'",[],|r|r.get::<_,i64>(0)).unwrap(),0);
        assert_eq!(archive.query_row("SELECT count(*) FROM records",[],|r|r.get::<_,i64>(0)).unwrap(),1);assert_eq!(target.import_cursor(SOURCE).unwrap(),None);
        drop(archive);drop(target);std::fs::remove_dir_all(dir).unwrap();
    }
}
