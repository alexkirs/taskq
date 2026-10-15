//! Typed legacy conversion. Enabled for qualification until the single-store cutover is wired.
use serde_json::{Value,json};

type Res<T> = Result<T,String>;

fn canonical(line: &str) -> Res<Value> {
    if line.starts_with("taskq-requests ") {crate::requests::legacy_value(line)} else {crate::profile::legacy_value(line)}
}
fn fields(kind: &str) -> Res<&'static [(&'static str,&'static str)]> {
    Ok(match kind {
        "operation" => &[("op","operation"),("us","duration_us"),("error","error"),("at_ms","started_ms")],
        "stage" => &[("host","host"),("open","open"),("project","project"),("stage","operation"),("task","task"),("attempt","attempt"),("rt","runtime"),("us","duration_us"),("outcome","outcome"),("error","error"),("at_ms","started_ms"),("calls","calls"),("tokens","tokens"),("unknown","usage_status")],
        "board" => &[("op","operation"),("us","duration_us"),("outcome","outcome"),("error","error"),("remaining","remaining"),("at_ms","started_ms")],
        "transition" => &[("project","project"),("task","task"),("entry","entry"),("from","from"),("to","to"),("prev_s","previous_seconds"),("rt","runtime"),("host","host"),("at_ms","started_ms")],
        "requests" => &[("at_ms","started_ms")],
        _ => return Err("unknown semantic kind".into()),
    })
}
fn prefix(kind: &str) -> Res<&'static str> {
    match kind {"operation"=>Ok("taskq-profile"),"stage"=>Ok("taskq-stage"),"board"=>Ok("taskq-board"),"transition"=>Ok("taskq-transition"),"requests"=>Ok("taskq-requests"),_=>Err("unknown semantic kind".into())}
}

pub(super) fn observation(line: &str) -> Res<Value> {
    let original=canonical(line)?;
    let kind=match line.split_once(' ').map(|(p,_)|p) {Some("taskq-profile")=>"operation",Some("taskq-stage")=>"stage",Some("taskq-board")=>"board",Some("taskq-transition")=>"transition",Some("taskq-requests")=>"requests",_=>return Err("unknown legacy kind".into())};
    let mut value=json!({"product":"taskq","version":null,"kind":kind,"operation":if kind=="requests" {"requests.summary"} else {"queue.transition"},
        "trace":null,"span":null,"started_ms":null,"outcome":"unknown","error":"unknown",
        "unknown":{"version":"not-recorded","trace":"not-recorded","span":"not-recorded","started_ms":"not-recorded"}});
    for (old,new) in fields(kind)? {if let Some(field)=original.get(*old) {value[*new]=field.clone();}}
    if !value["started_ms"].is_null() {value["unknown"].as_object_mut().unwrap().remove("started_ms");}
    for name in ["duration_us","remaining","previous_seconds","calls","tokens"] {
        if value.get(name).is_some_and(Value::is_null) {value["unknown"][name]=original.get("unknown").cloned().unwrap_or(json!("not-recorded"));}
    }
    if kind=="operation" {value["outcome"]=json!(if value["error"]=="none" {"ok"} else {"failed"});}
    if let Some(trace)=original.get("trace") {
        for (old,new) in [("run","run_id"),("process","process_id"),("span","span_id"),("parent","parent_id")] {
            value[new]=trace[old].clone();if value[new].is_null() {value["unknown"][new]=json!("not-recorded");}
        }
        value["unknown"]["trace"]=json!("numeric-trace-only");value["unknown"]["span"]=json!("numeric-trace-only");
    }
    if kind=="requests" {
        value["source"]=json!(original["source"].as_str().ok_or("missing request source")?.replace(' ',"."));
        let mut counts=serde_json::Map::new();
        for (name,count) in original["ops"].as_object().ok_or("missing request counts")? {
            let (method,path)=name.split_once(' ').ok_or("invalid request operation")?;
            counts.insert(format!("{method}:{}",path.replace('/',".").replace('*',":")),count.clone());
        }
        value["counts"]=Value::Object(counts);
    }
    Ok(value)
}

pub(super) fn line(value: &Value) -> Res<String> {
    if value["product"]!="taskq" {return Err("foreign semantic product".into());}
    let kind=value["kind"].as_str().ok_or("missing semantic kind")?;let mut original=serde_json::Map::new();
    for (old,new) in fields(kind)? {if let Some(field)=value.get(*new) {original.insert((*old).into(),field.clone());}}
    if kind=="operation"&&value.get("process_id").is_some() {
        original.insert("trace".into(),json!({"run":value["run_id"],"process":value["process_id"],"span":value["span_id"],"parent":value["parent_id"]}));
    }
    if kind=="requests" {
        original.insert("source".into(),json!(value["source"].as_str().ok_or("missing request source")?.replace('.'," ")));
        let mut ops=serde_json::Map::new();
        for (name,count) in value["counts"].as_object().ok_or("missing request counts")? {
            let (method,path)=name.split_once(':').ok_or("invalid request count code")?;
            ops.insert(format!("{method} {}",path.replace('.',"/").replace(':',"*")),count.clone());
        }
        original.insert("ops".into(),Value::Object(ops));
    }
    let line=format!("{} {}",prefix(kind)?,Value::Object(original));canonical(&line)?;Ok(line)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_semantic_kinds_roundtrip_through_the_shared_store() {
        let source=[
            r#"taskq-profile {"op":"spawn","us":18446744073709551615,"error":"none","at_ms":null,"trace":{"run":null,"process":42,"span":0,"parent":0}}"#,
            r#"taskq-stage {"host":"win","open":true,"project":"group/nested/repo","stage":"build","task":1230,"attempt":6185426441,"rt":"codex","us":null,"outcome":"unknown","error":"none","at_ms":null,"unknown":"open-stage"}"#,
            r#"taskq-stage {"stage":"claim","task":1230,"attempt":9,"rt":"other","us":20,"outcome":"ok","error":"none","at_ms":1000,"unknown":"no-model"}"#,
            r#"taskq-stage {"stage":"build","task":1230,"attempt":9,"rt":"codex","us":30,"outcome":"ask","error":"none","at_ms":1001,"calls":0,"tokens":null,"unknown":"no-usage"}"#,
            r#"taskq-board {"op":"close","us":12,"outcome":"failed","error":"http4xx","remaining":null,"at_ms":1002}"#,
            r#"taskq-transition {"project":"other","task":1230,"entry":18446744073709551615,"from":"doing","to":"ask","prev_s":null,"rt":null,"host":null,"at_ms":null}"#,
            r#"taskq-requests {"at_ms":1003,"source":"worker service status","ops":{"GET issues/N/comments":18446744073709551615,"POST graphql":0,"GET repos/*/issues":2}}"#,
        ];
        let dir=std::env::temp_dir().join(format!("taskq-semantic-{}-{}",std::process::id(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));std::fs::create_dir(&dir).unwrap();
        let store=coreq::telemetry::Store::open(&dir.join("db")).unwrap();
        for (index,text) in source.iter().enumerate() {
            let value=observation(text).unwrap();let event=coreq::telemetry::Event::identified("test.archive",&index.to_string(),value).unwrap();store.append(&[event]).unwrap();
        }
        let rows=store.export(0,0,1024).unwrap();assert_eq!(rows["events"].as_array().unwrap().len(),source.len());
        for (row,text) in rows["events"].as_array().unwrap().iter().zip(source) {assert_eq!(canonical(&line(&row["observation"]).unwrap()).unwrap(),canonical(text).unwrap());}
        drop(store);std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    #[ignore="read-only archive qualification; requires TASKQ_SEMANTIC_SOURCE snapshot"]
    fn retained_archive_roundtrips_through_shared_storage() {
        let source=std::env::var_os("TASKQ_SEMANTIC_SOURCE").expect("explicit snapshot path");
        let archive=rusqlite::Connection::open_with_flags(source,rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        assert_eq!(archive.pragma_query_value(None,"application_id",|r|r.get::<_,i64>(0)).unwrap(),0x5451544d);
        assert_eq!(archive.pragma_query_value(None,"user_version",|r|r.get::<_,i64>(0)).unwrap(),1);
        let dir=std::env::temp_dir().join(format!("taskq-semantic-real-{}-{}",std::process::id(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));std::fs::create_dir(&dir).unwrap();
        let store=coreq::telemetry::Store::open(&dir.join("db")).unwrap();
        let mut statement=archive.prepare("SELECT seq,retained_ms,line FROM records ORDER BY seq").unwrap();
        let rows=statement.query_map([],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?,r.get::<_,String>(2)?))).unwrap();
        let mut expected=ring::digest::Context::new(&ring::digest::SHA256);let mut events=Vec::new();let mut clocks=Vec::new();let mut cursor=None::<String>;let mut count=0;
        for row in rows {
            let (seq,received,text)=row.unwrap();let value=observation(&text).unwrap();
            expected.update(format!("{received} {} {}\n",text.split_once(' ').unwrap().0,canonical(&text).unwrap()).as_bytes());
            events.push(coreq::telemetry::Event::identified("qualification.archive",&seq.to_string(),value).unwrap());clocks.push(received);count+=1;
            if events.len()==1024 {let next=format!("position.{seq}");store.import_history("archive",cursor.as_deref(),&next,&events,&clocks).unwrap();cursor=Some(next);events.clear();clocks.clear();}
        }
        if !events.is_empty() {store.import_history("archive",cursor.as_deref(),"done",&events,&clocks).unwrap();}
        let mut actual=ring::digest::Context::new(&ring::digest::SHA256);let mut after=0;let mut retained=0;
        loop {let page=store.export(0,after,1024).unwrap();for row in page["events"].as_array().unwrap() {
            let text=line(&row["observation"]).unwrap();let received=row["received_ms"].as_i64().unwrap();actual.update(format!("{received} {} {}\n",text.split_once(' ').unwrap().0,canonical(&text).unwrap()).as_bytes());retained+=1;
        }after=page["next"].as_i64().unwrap();if page["more"]==false {break;}}
        let digest=|value:ring::digest::Digest|value.as_ref().iter().map(|byte|format!("{byte:02x}")).collect::<String>();
        let expected=digest(expected.finish());let actual=digest(actual.finish());assert_eq!(retained,count);assert_eq!(actual,expected);assert_eq!(store.storage().unwrap()["pruned_count"],0);
        println!("semantic archive: {count} rows; canonical+receipt SHA256 {actual}; original receipt clocks preserved; pruning 0");
        drop(store);std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn unsupported_or_unsafe_source_metadata_is_refused() {
        for text in [r#"taskq-profile {"op":"unknown","us":1,"error":"none","at_ms":1}"#,
            r#"taskq-stage {"stage":"build","task":1,"attempt":1,"rt":"codex","us":1,"outcome":"ok","error":"none","at_ms":1,"prompt":"secret"}"#,
            r#"taskq-requests {"at_ms":1,"source":"cli private content","ops":{"GET issues":1}}"#,
            r#"taskq-requests {"at_ms":1,"source":"cli status","ops":{"GET issues/123/private?token=secret":1}}"#] {assert!(observation(text).is_err(),"{text}");}
    }
}
