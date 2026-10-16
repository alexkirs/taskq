//! Focus correlation only; settings and admission policy remain authoritative.
use crate::{Res,data,dot};
use coreq::telemetry::capture::{CommandReceipt,operation};
use serde_json::{Value,json};
use std::{fs,io::Read,path::Path};

pub struct Pending {
    project: String,
    receipt: CommandReceipt,
    phase: String,
    focus: Option<String>,
    previous: Option<CommandReceipt>,
}

fn descriptor(value: &Value) -> Res<CommandReceipt> {
    let text=value.as_str().ok_or("focus command descriptor invalid; file kept")?;
    let receipt=CommandReceipt::decode(text)?;
    let metadata:Value=serde_json::from_str(text).map_err(|_|"focus command descriptor invalid; file kept")?;
    if metadata["product"]!="taskq" {return Err("focus command product mismatch; file kept".into());}
    Ok(receipt)
}

fn staged(root: &Path) -> Res<bool> {
    match fs::symlink_metadata(dot(root).join("focus.command.pending")) {
        Ok(_)=>Ok(true),
        Err(e) if e.kind()==std::io::ErrorKind::NotFound=>Ok(false),
        Err(_)=>Err("focus command temporary unreadable; file kept".into()),
    }
}

fn read(root: &Path) -> Res<Option<Pending>> {
    if staged(root)? {return Err("focus command temporary retained; interrupted write needs reconciliation".into());}
    read_file(root,&dot(root).join("focus.command"))
}

fn read_file(root: &Path, path: &Path) -> Res<Option<Pending>> {
    operation("command.state.read",|| {
        match fs::symlink_metadata(path) {
            Ok(m) if m.file_type().is_file()=>{},
            Ok(_)=>return Err("focus command not a regular file; kept".into()),
            Err(e) if e.kind()==std::io::ErrorKind::NotFound=>return Ok(None),
            Err(_)=>return Err("focus command unreadable; file kept".into()),
        }
        let file=match fs::File::open(path) {
            Ok(f)=>f,
            Err(e) if e.kind()==std::io::ErrorKind::NotFound=>return Ok(None),
            Err(_)=>return Err("focus command unreadable; file kept".into()),
        };
        let mut text=String::new();file.take(4097).read_to_string(&mut text).map_err(|_|"focus command unreadable; file kept")?;
        if text.len()>4096 {return Err("focus command oversized; file kept".into());}
        let v:Value=serde_json::from_str(&text).map_err(|_|"focus command corrupt; file kept")?;
        let object=v.as_object().ok_or("focus command invalid; file kept")?;
        if object.len()!=6 || !["schema","project","command","phase","focus","previous"].iter().all(|k|object.contains_key(*k)) || v["schema"]!=1 {
            return Err("focus command schema unsupported; file kept".into());
        }
        let project=project_id(root)?;
        if v["project"]!=project {return Err("focus command belongs to another project; file kept".into());}
        let receipt=descriptor(&v["command"])?;
        let phase=v["phase"].as_str().filter(|s|matches!(*s,"pending"|"applied"|"failed")).ok_or("focus command phase invalid; file kept")?.to_owned();
        let focus=if phase=="applied" {
            Some(v["focus"].as_str().filter(|s|s.len()==64&&s.bytes().all(|b|b.is_ascii_digit()||(b'a'..=b'f').contains(&b)))
                .ok_or("focus command fingerprint invalid; file kept")?.to_owned())
        } else {if !v["focus"].is_null() {return Err("focus command fingerprint invalid; file kept".into());}None};
        let previous=if v["previous"].is_null() {None} else {Some(descriptor(&v["previous"])?)};
        if phase!="pending"&&previous.is_some() {return Err("focus command terminal retains predecessor; file kept".into());}
        if previous.as_ref().is_some_and(|p|p.event_id("received").ok()==receipt.event_id("received").ok()) {
            return Err("focus command predecessor repeats current identity; file kept".into());
        }
        Ok(Some(Pending{project,receipt,phase,focus,previous}))
    },|_|"command-state-read")
}

/// Exclusive signal owner only: reconcile metadata, never replay a setting mutation.
fn recover(root: &Path) -> Res<()> {
    operation("command.recover",|| {
        let path=dot(root).join("focus.command.pending");
        let Some(next)=read_file(root,&path)? else {return Ok(())};
        let current=read_file(root,&dot(root).join("focus.command"))?;
        let same=current.as_ref().is_some_and(|p|p.receipt.encode()==next.receipt.encode());
        if next.phase=="pending" {
            let previous=current.as_ref().filter(|p|p.phase!="failed").map(|p|p.receipt.encode());
            if same || next.previous.as_ref().map(CommandReceipt::encode)!=previous {
                return Err("focus command temporary predecessor mismatch; files kept".into());
            }
            // The initial pointer never replaced current, so this writer could not enter its effect closure.
            next.receipt.checkpoint("failed")?;
        } else {
            let old=current.as_ref().filter(|_|same).ok_or("focus command temporary identity mismatch; files kept")?;
            if old.phase!="pending" && (old.phase!=next.phase || old.focus!=next.focus) {
                return Err("focus command temporary terminal mismatch; files kept".into());
            }
            if next.phase=="applied" && next.focus.as_deref()!=Some(fingerprint(&data::focus(root)?).as_str()) {
                return Err("focus command temporary differs from settings; files kept".into());
            }
        }
        // Only a validated, attributable temporary may be removed. Canonical pending stays safe if re-save fails.
        fs::remove_file(&path).map_err(|_|"focus command temporary removal failed; reconciliation unknown")?;
        if next.phase!="pending" {save(root,&next)?;}
        Ok(())
    },|_|"command-recover")
}

fn save(root: &Path, pending: &Pending) -> Res<()> {
    operation("command.state.write",|| {
        let text=json!({"schema":1,"project":pending.project,"command":pending.receipt.encode(),"phase":pending.phase,
            "focus":pending.focus,"previous":pending.previous.as_ref().map(CommandReceipt::encode)}).to_string();
        if text.len()>4096 {return Err("focus command oversized".into());}
        coreq::write_durable(&dot(root).join("focus.command"),&text)
    },|_|"command-state-write")
}

fn fingerprint(focus: &[String]) -> String {
    let bytes=serde_json::to_vec(focus).expect("string vector");
    digest(&bytes)
}

fn digest(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256,bytes).as_ref().iter().map(|b|format!("{b:02x}")).collect()
}

fn project_id(root: &Path) -> Res<String> {
    let root=root.canonicalize().map_err(|_|"focus command project unavailable; file kept")?;
    #[cfg(unix)] {
        use std::os::unix::ffi::OsStrExt;
        Ok(digest(root.as_os_str().as_bytes()))
    }
    #[cfg(windows)] {
        use std::os::windows::ffi::OsStrExt;
        let bytes:Vec<_>=root.as_os_str().encode_wide().flat_map(u16::to_le_bytes).collect();
        Ok(digest(&bytes))
    }
}

/// Caller owns the exclusive signal lock; no setting may change before this returns.
pub fn begin(root: &Path, receipt: &CommandReceipt) -> Res<Pending> {
    if staged(root)? {recover(root)?;}
    let prior=read(root)?;
    if let Some(old)=&prior {
        if old.receipt.event_id("received")?==receipt.event_id("received")? {return Err("focus command already tracked; check status before retry".into());}
        if let Some(previous)=&old.previous {previous.checkpoint("superseded")?;}
    }
    let mut next=Pending{project:project_id(root)?,receipt:receipt.clone(),phase:"pending".into(),focus:None,
        previous:prior.filter(|p|p.phase!="failed").map(|p|p.receipt)};
    save(root,&next)?;
    // Only a persisted invalidation makes the previous command superseded.
    if let Some(previous)=&next.previous {previous.checkpoint("superseded")?;}
    next.previous=None;
    Ok(next)
}

/// Caller still owns the signal lock and has acknowledged this command's terminal receipt.
pub fn finish(root: &Path, mut pending: Pending, applied: bool) -> Res<()> {
    pending.phase=if applied {"applied"} else {"failed"}.into();
    pending.focus=if applied {Some(fingerprint(&data::focus(root)?))} else {None};
    save(root,&pending)
}

/// Caller holds the shared signal lock through snapshot and receipt acknowledgment.
fn observe(root: &Path, focus: &[String], seen: &mut Option<String>) -> Res<Value> {
    operation("command.observe",|| {
    let Some(pending)=read(root)? else {
        *seen=None;return Ok(json!({"state":"untracked","reason":"legacy or no tracked focus command"}));
    };
    if !dot(root).join("poke").is_file() {return Err("focus command signal missing; observation unknown".into());}
    if let Some(previous)=&pending.previous {previous.checkpoint("superseded")?;}
    if pending.phase!="applied" {return Err(format!("focus command {}; inspect receipt {} before another command",pending.phase,pending.receipt.event_id("received")?));}
    if pending.focus.as_deref()!=Some(fingerprint(focus).as_str()) {return Err("focus differs from tracked command; observation unknown".into());}
    let id=pending.receipt.event_id("observed")?;
    if seen.as_ref()!=Some(&id) {pending.receipt.checkpoint("observed")?;*seen=Some(id.clone());}
    Ok(json!({"state":"observed","scope":"tracked-desired-focus","applied_receipt":pending.receipt.event_id("applied")?,"observed_receipt":id}))
    },|_|"command-observe")
}

pub struct Snapshot {
    pub link: Option<coreq::telemetry::capture::TraceContext>,
    pub focus: Vec<String>,
    pub command: Value,
}

/// Recover with exclusive ownership, then take a fresh shared snapshot; a concurrent command may win between them.
pub fn snapshot(root: &Path, signal: &mut Option<String>, command: &mut Option<String>) -> Res<Snapshot> {
    if staged(root)? {
        let file=fs::OpenOptions::new().read(true).write(true).open(dot(root).join("poke")).map_err(|_|"focus command recovery signal unavailable; files kept")?;
        let _owner=coreq::file_lock::try_lock_file(file).map_err(|_|"focus command recovery busy; files kept")?;
        recover(root)?;
    }
    let (link,(focus,command))=crate::wake_read(&dot(root).join("poke"),signal,|| {
        let focus=data::focus(root)?;
        let observed=observe(root,&focus,command)?;
        Ok((focus,observed))
    })?;
    Ok(Snapshot{link,focus,command})
}

#[cfg(test)]
mod tests {
    use super::*;
    fn snapshot(root: &Path, seen: &mut Option<String>) -> Res<Value> {
        super::snapshot(root,&mut None,seen).map(|v|v.command)
    }
    fn receipts(store: &coreq::telemetry::Store) -> Vec<Value> {
        store.export(0,0,1024).unwrap()["events"].as_array().unwrap().iter()
            .filter(|r|r["observation"]["source"]=="command").cloned().collect()
    }

    // Chain: local command -> service snapshot -> replacement -> fresh observer; no effect replay.
    #[test]
    fn service_observes_original_command_and_supersession_idempotently() {
        crate::with_test_capture(|store,dir| {
            let root=dir.join("project");fs::create_dir_all(dot(&root)).unwrap();
            assert_eq!(snapshot(&root,&mut None).unwrap()["state"],"untracked");
            crate::set_focus(&root,"area-private",Some("private owner words")).unwrap();
            let pointer=fs::read_to_string(dot(&root).join("focus.command")).unwrap();
            assert!(!pointer.contains("area-private")&&!pointer.contains("private owner words")&&!pointer.contains(root.to_str().unwrap()));
            let mut seen=None;
            let absent=tracing::dispatcher::with_default(&tracing::Dispatch::none(),||snapshot(&root,&mut seen));
            assert!(absent.unwrap_err().contains("collector unavailable"));assert!(seen.is_none());
            let first=snapshot(&root,&mut seen).unwrap();
            assert_eq!(first["state"],"observed");
            assert_eq!(snapshot(&root,&mut seen).unwrap(),first);
            assert_eq!(snapshot(&root,&mut None).unwrap(),first,"new observer changed the receipt identity");
            let rows=receipts(store);
            assert_eq!(rows.iter().filter(|r|r["id"]==first["observed_receipt"]).count(),1);
            let applied=rows.iter().find(|r|r["id"]==first["applied_receipt"]).unwrap();
            let observed=rows.iter().find(|r|r["id"]==first["observed_receipt"]).unwrap();
            for key in ["trace","span","operation","version"] {assert_eq!(applied["observation"][key],observed["observation"][key]);}
            crate::set_focus(&root,"area-private",None).unwrap();
            let second=snapshot(&root,&mut seen).unwrap();
            assert_ne!(first["observed_receipt"],second["observed_receipt"],"identical focus reused an old command");
            crate::pause(&root,false).unwrap();
            let paused=read(&root).unwrap().unwrap().receipt;
            crate::resume(&root).unwrap();
            assert_eq!(snapshot(&root,&mut seen).unwrap()["state"],"observed");
            let rows=receipts(store);
            assert!(rows.iter().any(|r|r["id"]==paused.event_id("superseded").unwrap()));
            assert!(!rows.iter().any(|r|r["id"]==paused.event_id("observed").unwrap()),"coalesced Pause falsely acknowledged");
            assert_eq!(rows.iter().filter(|r|r["observation"]["kind"]=="command.superseded").count(),3);
            assert_eq!(data::focus(&root).unwrap(),["area-private"]);
        });
    }

    // Chain: interruption or partial mutation -> explicit unknown/failed -> new command replaces without replay.
    #[test]
    fn interrupted_and_failed_commands_never_acknowledge_an_equal_focus() {
        crate::with_test_capture(|store,dir| {
            let root=dir.join("project");fs::create_dir_all(dot(&root)).unwrap();
            crate::set_focus(&root,"all",None).unwrap();
            let interrupted={
                let _action=tracing::info_span!("fixture.interrupted").entered();
                let r=CommandReceipt::current().unwrap();r.checkpoint("received").unwrap();
                let _locked=crate::wake_lock(&root).unwrap();
                let _pending=begin(&root,&r).unwrap();r
            };
            assert!(snapshot(&root,&mut None).unwrap_err().contains("pending"));
            assert_eq!(data::focus(&root).unwrap(),["all"]);
            crate::set_focus(&root,"all",None).unwrap();
            assert_eq!(snapshot(&root,&mut None).unwrap()["state"],"observed");
            let rows=receipts(store);
            assert!(rows.iter().any(|r|r["id"]==interrupted.event_id("superseded").unwrap()));
            assert!(!rows.iter().any(|r|r["id"]==interrupted.event_id("observed").unwrap()));
            fs::create_dir(dot(&root).join("focus.request")).unwrap();
            assert!(crate::set_focus(&root,"area-next",None).unwrap_err().contains("partially saved"));
            assert_eq!(data::focus(&root).unwrap(),["area-next"]);
            assert!(snapshot(&root,&mut None).unwrap_err().contains("failed"));
            fs::remove_dir(dot(&root).join("focus.request")).unwrap();
            crate::set_focus(&root,"all",None).unwrap();
            assert_eq!(snapshot(&root,&mut None).unwrap()["state"],"observed");
        });
    }

    // Chain: corrupt/mismatched tracking -> refusal before Resume -> safe Pause retains evidence.
    #[test]
    fn bad_tracking_is_preserved_and_never_blocks_safe_restriction() {
        crate::with_test_capture(|store,dir| {
            let root=dir.join("project");fs::create_dir_all(dot(&root)).unwrap();
            crate::set_focus(&root,"all",None).unwrap();
            let path=dot(&root).join("focus.command");let valid=fs::read_to_string(&path).unwrap();
            let original=read(&root).unwrap().unwrap().receipt;
            fs::write(dot(&root).join("focus"),"off").unwrap();
            assert!(snapshot(&root,&mut None).unwrap_err().contains("differs"));
            assert!(!receipts(store).iter().any(|r|r["id"]==original.event_id("observed").unwrap()));
            fs::write(dot(&root).join("focus.paused"),"all").unwrap();
            let mut newer:Value=serde_json::from_str(&valid).unwrap();newer["schema"]=json!(2);
            for bad in ["{".to_owned(),newer.to_string(),"x".repeat(4097)] {
                fs::write(&path,&bad).unwrap();fs::write(dot(&root).join("focus"),"off").unwrap();
                assert!(crate::resume(&root).unwrap_err().contains("tracking unavailable"));
                assert!(data::focus(&root).unwrap().is_empty());
                fs::write(dot(&root).join("focus"),"all").unwrap();
                assert!(crate::pause(&root,false).unwrap_err().contains("tracking unknown"));
                assert!(data::focus(&root).unwrap().is_empty());assert_eq!(fs::read_to_string(&path).unwrap(),bad);
                assert!(snapshot(&root,&mut None).is_err());
            }
            fs::write(&path,&valid).unwrap();
            fs::write(dot(&root).join("focus.command.pending"),"interrupted bytes").unwrap();
            assert!(crate::resume(&root).is_err());
            assert!(snapshot(&root,&mut None).unwrap_err().contains("corrupt"));
            assert_eq!(fs::read_to_string(dot(&root).join("focus.command.pending")).unwrap(),"interrupted bytes");
            assert_eq!(fs::read_to_string(path).unwrap(),valid);
            let other=dir.join("other");fs::create_dir_all(dot(&other)).unwrap();
            fs::write(dot(&other).join("focus.command"),&valid).unwrap();
            fs::write(dot(&other).join("focus"),"all").unwrap();
            assert!(snapshot(&other,&mut None).unwrap_err().contains("another project"));
            assert_eq!(fs::read_to_string(dot(&other).join("focus.command")).unwrap(),valid);
        });
    }

    // Chain: validated crash residue -> phase-aware metadata recovery -> same observation, never an effect replay.
    #[test]
    fn staged_writes_reconcile_without_replacing_current_with_unapplied_intent() {
        crate::with_test_capture(|store,dir| {
            let root=dir.join("project");fs::create_dir_all(dot(&root)).unwrap();
            crate::set_focus(&root,"all",None).unwrap();
            let path=dot(&root).join("focus.command");let stage=dot(&root).join("focus.command.pending");
            let current=fs::read_to_string(&path).unwrap();let old=read(&root).unwrap().unwrap().receipt;
            let never_applied={
                let _action=tracing::info_span!("fixture.staged_initial").entered();
                let r=CommandReceipt::current().unwrap();r.checkpoint("received").unwrap();r
            };
            let mut v:Value=serde_json::from_str(&current).unwrap();
            v["command"]=json!(never_applied.encode());v["phase"]=json!("pending");v["focus"]=Value::Null;v["previous"]=json!(old.encode());
            for _ in 0..2 {
                fs::write(&stage,v.to_string()).unwrap();
                assert_eq!(snapshot(&root,&mut None).unwrap()["applied_receipt"],old.event_id("applied").unwrap());
                assert_eq!(fs::read_to_string(&path).unwrap(),current);assert!(!stage.exists());
            }
            let rows=receipts(store);
            assert_eq!(rows.iter().filter(|r|r["id"]==never_applied.event_id("failed").unwrap()).count(),1);
            assert!(!rows.iter().any(|r|r["id"]==old.event_id("superseded").unwrap()),"uncommitted intent superseded the actual current command");
            for phase in ["applied","failed"] {
                let r={
                    let _action=tracing::info_span!("fixture.staged_terminal").entered();
                    let r=CommandReceipt::current().unwrap();r.checkpoint("received").unwrap();
                    let owner=crate::wake_lock(&root).unwrap();let _pending=begin(&root,&r).unwrap();
                    if phase=="applied" {crate::platform::write_atomic(&dot(&root).join("focus"),"area-staged").unwrap();}
                    crate::poke_locked(&owner).unwrap();r.checkpoint(phase).unwrap();
                    let mut v:Value=serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
                    v["phase"]=json!(phase);v["previous"]=Value::Null;
                    v["focus"]=if phase=="applied" {json!(fingerprint(&data::focus(&root).unwrap()))} else {Value::Null};
                    fs::write(&stage,v.to_string()).unwrap();r
                };
                let before=fs::read(dot(&root).join("focus")).unwrap();
                let observed=snapshot(&root,&mut None);
                if phase=="applied" {assert_eq!(observed.unwrap()["observed_receipt"],r.event_id("observed").unwrap());}
                else {assert!(observed.unwrap_err().contains("failed"));}
                assert!(!stage.exists());assert_eq!(fs::read(dot(&root).join("focus")).unwrap(),before);
                assert_eq!(read(&root).unwrap().unwrap().phase,phase);
            }
            crate::set_focus(&root,"all",None).unwrap();
            assert_eq!(snapshot(&root,&mut None).unwrap()["state"],"observed");
            let mut wrong:Value=serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
            wrong["command"]=json!(old.encode());
            fs::write(&stage,wrong.to_string()).unwrap();
            assert!(snapshot(&root,&mut None).unwrap_err().contains("identity mismatch"));
            assert_eq!(fs::read_to_string(stage).unwrap(),wrong.to_string());
        });
    }

    // Chain: service snapshot -> blocked observed commit -> next command cannot replace the snapshot.
    #[test]
    fn observation_keeps_the_signal_locked_through_required_commit() {
        use std::{sync::Arc,time::{Duration,Instant}};
        use tracing_subscriber::prelude::*;
        let dir=std::env::temp_dir().join(format!("taskq-observation-lock-{}",std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let store=Arc::new(coreq::telemetry::Store::open(&dir.join("db")).unwrap());
        let capture=coreq::telemetry::capture::Capture::new(store.clone(),"taskq","fixture").unwrap();
        let layer=capture.clone().with_filter(tracing_subscriber::filter::Targets::new()
            .with_target("taskq::observation_fixture",tracing::Level::INFO));
        let dispatch=tracing::Dispatch::new(tracing_subscriber::registry().with(layer));
        let root=dir.join("project");fs::create_dir_all(dot(&root)).unwrap();
        tracing::dispatcher::with_default(&dispatch,|| {
            let _command=tracing::info_span!(target:"taskq::observation_fixture","fixture.command").entered();
            crate::change_focus(&root,false,false,|d|crate::platform::write_atomic(&d.join("focus"),"all")).unwrap();
        });
        capture.flush().unwrap();
        let db=rusqlite::Connection::open(dir.join("db")).unwrap();
        db.execute_batch("BEGIN IMMEDIATE").unwrap();
        let baseline=capture.health()["accepted"].as_u64().unwrap();
        std::thread::scope(|scope| {
            let observer=scope.spawn(||tracing::dispatcher::with_default(&dispatch,||snapshot(&root,&mut None)));
            let until=Instant::now()+Duration::from_secs(3);
            while capture.health()["accepted"].as_u64().unwrap()==baseline {
                assert!(Instant::now()<until,"observer never submitted its receipt");
                std::thread::sleep(Duration::from_millis(5));
            }
            assert!(crate::wake_lock(&root).is_err(),"writer replaced state before observed receipt commit");
            assert_eq!(data::focus(&root).unwrap(),["all"]);
            db.execute_batch("ROLLBACK").unwrap();
            assert_eq!(observer.join().unwrap().unwrap()["state"],"observed");
        });
        drop(crate::wake_lock(&root).unwrap());
        capture.flush().unwrap();drop(db);drop(dispatch);drop(capture);drop(store);fs::remove_dir_all(dir).unwrap();
    }

    // Chain: actual writer death before/after mutation -> no observed receipt -> explicit replacement.
    #[test]
    fn killed_command_leaves_unknown_state_without_replaying_effects() {
        use std::{io::{BufRead,BufReader},process::{Command,Stdio},time::Duration};
        crate::with_test_capture(|store,dir| {
            for phase in ["prepared","changed"] {
                let root=dir.join(phase);fs::create_dir_all(dot(&root)).unwrap();
                crate::set_focus(&root,"area-before",None).unwrap();
                let mut child=Command::new(std::env::current_exe().unwrap())
                    .args(["--exact","control::tests::interrupted_command_writer","--ignored","--nocapture"])
                    .env("TASKQ_CONTROL_FIXTURE",&root).env("TASKQ_CONTROL_PHASE",phase)
                    .env_remove("COREQ_TRACE_CONTEXT").stdout(Stdio::piped()).spawn().unwrap();
                let stdout=child.stdout.take().unwrap();let (tx,rx)=std::sync::mpsc::channel();
                let reader=std::thread::spawn(move || {
                    for line in BufReader::new(stdout).lines() {if line.unwrap()=="ACK" {let _=tx.send(());break;}}
                });
                let acknowledged=rx.recv_timeout(Duration::from_secs(15));
                let _=child.kill();child.wait().unwrap();reader.join().unwrap();acknowledged.unwrap();
                let command=read(&root).unwrap().unwrap().receipt;
                let expected=if phase=="changed" {"area-partial"} else {"area-before"};
                assert_eq!(data::focus(&root).unwrap(),[expected]);
                assert!(snapshot(&root,&mut None).unwrap_err().contains("pending"));
                assert_eq!(data::focus(&root).unwrap(),[expected],"observer replayed an interrupted effect");
                crate::set_focus(&root,"area-after",None).unwrap();
                assert_eq!(snapshot(&root,&mut None).unwrap()["state"],"observed");
                let rows=receipts(store);
                assert!(rows.iter().any(|r|r["id"]==command.event_id("superseded").unwrap()));
                for phase in ["applied","observed"] {assert!(!rows.iter().any(|r|r["id"]==command.event_id(phase).unwrap()),"interrupted command acquired an invented phase");}
            }
        });
    }

    #[test]
    #[ignore = "subprocess killed by killed_command_leaves_unknown_state_without_replaying_effects"]
    fn interrupted_command_writer() {
        use std::{io::Write,sync::Arc};
        use tracing_subscriber::prelude::*;
        let root=std::path::PathBuf::from(std::env::var_os("TASKQ_CONTROL_FIXTURE").unwrap());
        let store=Arc::new(coreq::telemetry::Store::open(&root.parent().unwrap().join("db")).unwrap());
        let capture=coreq::telemetry::capture::Capture::new(store,"taskq","crash-fixture").unwrap();
        tracing::subscriber::with_default(tracing_subscriber::registry().with(capture),|| {
            let _action=tracing::info_span!("fixture.crash").entered();
            crate::change_focus(&root,false,false,|d|->Res<()> {
                if std::env::var("TASKQ_CONTROL_PHASE").unwrap()=="changed" {crate::platform::write_atomic(&d.join("focus"),"area-partial")?;}
                println!("ACK");std::io::stdout().flush().unwrap();
                loop {std::thread::park();}
            }).unwrap();
        });
    }
}
