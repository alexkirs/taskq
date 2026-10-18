//! Host installation recovery in the existing migration originals and registry prepare record.
use super::*;
use serde::{Deserialize,Serialize};

#[derive(Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
struct Original {path:PathBuf,digest:Option<String>,owner:Value,mode:u32,readonly:bool}
#[derive(Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
struct Baseline {schema:u8,version:String,roots:Vec<PathBuf>,files:Vec<Original>}
#[derive(Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
struct Pending {
    schema:u8,id:String,baseline:String,root:PathBuf,version:String,candidate:String,
    original:Option<String>,next:String,phase:String,restore:bool,attach:bool,owner:Option<InstallOwner>,command:String,
    #[serde(default,skip_serializing_if="Option::is_none")]
    legacy:Option<Legacy>,
    #[serde(default,skip_serializing_if="Option::is_none")]
    activation:Option<Baseline>,
}
#[derive(Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
struct Legacy {owner:crate::host_control::LegacyOwner,phase:String}
#[derive(Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
struct InstallOwner {pid:u32,started:String,instance:String}
fn digest(bytes:&[u8])->String {ring::digest::digest(&ring::digest::SHA256,bytes).as_ref().iter().map(|b|format!("{b:02x}")).collect()}
fn directory(install:&Path)->PathBuf {install.join("host-policy-migration/install-originals")}
fn manifest(install:&Path)->PathBuf {install.join("host-policy-migration/install.json")}
fn activation_directory(install:&Path)->PathBuf {directory(install).with_file_name("activation-originals")}
fn raw_projects(install:&Path)->Res<Option<String>> {
    match fs::read_to_string(install.join("projects")) {
        Ok(text)=>Ok(Some(text)),Err(e) if e.kind()==std::io::ErrorKind::NotFound=>Ok(None),Err(e)=>Err(e.to_string()),
    }
}
fn pending(install:&Path)->Res<Pending> {
    match fs::symlink_metadata(install.join(format!("{INSTALL_PENDING}.pending"))) {
        Ok(_)=>return Err("installation cursor publication incomplete; recovery evidence kept".into()),
        Err(e) if e.kind()==std::io::ErrorKind::NotFound=>{},Err(e)=>return Err(e.to_string()),
    }
    let path=install.join(INSTALL_PENDING);
    let meta=fs::symlink_metadata(&path).map_err(|e|e.to_string())?;
    if !meta.is_file()||meta.len()>1<<20 {return Err("projects.installing unsupported; recovery evidence kept".into());}
    let p:Pending=serde_json::from_slice(&fs::read(path).map_err(|e|e.to_string())?).map_err(|_|"projects.installing retained; installation binding unknown")?;
    if p.schema!=1||!crate::resources::id(&p.id)||!p.root.is_absolute()||p.version.is_empty()||p.version.contains("..")||!p.version.bytes().all(|b|b.is_ascii_alphanumeric()||b".-".contains(&b))
        ||!matches!(p.phase.as_str(),"initial"|"prepared"|"activated"|"starting"|"owned"|"registered")
        ||[&p.baseline,&p.candidate].iter().any(|s|s.len()!=64||!s.bytes().all(|b|b.is_ascii_hexdigit()&&!b.is_ascii_uppercase())) {
        return Err("projects.installing unsupported; installation binding kept".into());
    }
    let bytes=fs::read(manifest(install)).map_err(|e|format!("installation originals unavailable: {e}"))?;
    if digest(&bytes)!=p.baseline {return Err("installation originals changed; recovery held".into());}
    let command=CommandReceipt::decode(&p.command)?;
    let metadata:Value=serde_json::from_str(&command.encode()).map_err(|e|e.to_string())?;
    if metadata["product"]!="taskq"||metadata["operation"]!="taskq.host.install" {return Err("installation command identity invalid; retained".into());}
    let roots=project_entries(&p.next)?;
    if !roots.iter().any(|(_,root)|root==&p.root)||p.owner.as_ref().is_some_and(|o|o.pid==0||o.started.is_empty()||!crate::resources::id(&o.instance))
        ||p.legacy.as_ref().is_some_and(|l|l.owner.root!=p.root||!matches!(l.phase.as_str(),"draining"|"ready"|"retiring"|"retired")) {return Err("installation scope/owner binding invalid; retained".into());}
    if let Some(legacy)=&p.legacy {legacy.owner.validate()?;}
    Ok(p)
}
fn save(install:&Path,p:&Pending)->Res<()> {
    coreq::write_durable(&install.join(INSTALL_PENDING),serde_json::to_vec(p).map_err(|e|e.to_string())?)?;
    #[cfg(test)] tests::interrupt(install,&p.phase)?;
    Ok(())
}
fn binding(p:&Pending)->Value {
    let mut bound=json!({"local_install":{"schema":1,"id":p.id,"root":p.root,"version":p.version,"candidate":p.candidate,"command":p.command}});
    if let Some(activation)=&p.activation {bound["local_install"]["activation"]=json!(digest(json!(activation).to_string().as_bytes()));}
    bound
}
fn retained(install:&Path,p:&Pending)->Res<Value> {
    crate::resources::try_with(install,|state| {
        let op=state.ops.get(&p.id).ok_or("original installation receipt missing; transition held")?;
        if op.bound!=binding(p) {return Err("original installation receipt binding changed; transition held".into());}
        Ok(checked_receipt(op)?.clone())
    })
}
fn record(install:&Path,p:&Pending,phase:&str)->Res<()> {
    let command=CommandReceipt::decode(&p.command)?;
    let bound=binding(p);
    crate::resources::try_with(install,|state| {
        let mut receipt=match state.ops.get(&p.id) {
            Some(op) if op.bound==bound=>{
                let receipt=checked_receipt(op)?;
                if receipt["state"]=="observed"&&matches!(phase,"received"|"applied") {return Ok(());}
                if receipt["state"]=="failed"&&phase!="failed" {return Err("original installation already failed; command cannot run again".into());}
                receipt.clone()
            },
            Some(_)=>return Err("installation ID belongs to another command; retained".into()),
            None=>{
                if state.ops.len()>=crate::resources::MAX_OPS {return Err("installation receipt store full; no active effect permitted".into());}
                json!({"id":p.id,"action":"install","root":p.root,"version":p.version,"state":"queued"})
            },
        };
        let event=command.checkpoint(phase)?;
        receipt.as_object_mut().ok_or("installation receipt invalid")?.remove("reason");
        receipt[format!("{phase}_receipt")]=json!(event);
        receipt["state"]=json!(match phase {"received"=>"queued","applied"=>"applied","observed"=>"observed",_=>"failed"});
        let now=wall()?;
        state.ops.insert(p.id.clone(),crate::resources::Op {bound,receipt,expires_at:now,saved_at:Some(now),writing:false,sent:true});
        Ok(())
    })?;
    #[cfg(test)] tests::interrupt(install,phase)?;
    Ok(())
}
fn retain_reason(install:&Path,p:&Pending,reason:&str)->Res<()> {
    crate::resources::try_with(install,|state| {
        let op=state.ops.get_mut(&p.id).ok_or("installation receipt unavailable")?;
        if op.bound!=binding(p) {return Err("installation reason belongs to another command".into());}
        checked_receipt(op)?;
        op.receipt["reason"]=json!(crate::compact(reason,256));Ok(())
    })
}

fn record_legacy(install:&Path,p:&Pending,phase:&str)->Res<()> {
    let owner=&p.legacy.as_ref().ok_or("installation has no legacy scope")?.owner;
    crate::resources::try_with(install,|state| {
        let op=state.ops.get_mut(&p.id).ok_or("legacy installation receipt missing")?;
        if op.bound!=binding(p) {return Err("legacy receipt belongs to another command".into());}
        let receipt=checked_receipt(op)?;
        if !matches!(receipt["state"].as_str(),Some("applied"|"observed")) {return Err("legacy installation has no applied command receipt".into());}
        op.receipt[format!("legacy_{phase}")]=json!(owner);Ok(())
    })
}

fn legacy_proven(install:&Path,p:&Pending,phase:&str)->Res<bool> {
    let owner=&p.legacy.as_ref().ok_or("installation has no legacy scope")?.owner;
    crate::resources::try_with(install,|state| {
        let op=state.ops.get(&p.id).ok_or("legacy installation receipt missing")?;
        if op.bound!=binding(p) {return Err("legacy receipt belongs to another command".into());}
        let receipt=checked_receipt(op)?;
        if !matches!(receipt["state"].as_str(),Some("applied"|"observed")) {return Err("legacy installation has no applied command receipt".into());}
        Ok(op.receipt.get(format!("legacy_{phase}")).is_some_and(|bound|*bound==json!(owner)))
    })
}

fn checked_receipt(op:&crate::resources::Op)->Res<&Value> {
let b=&op.bound["local_install"];
let mut keys=vec!["schema","id","root","version","candidate","command"];
if let Some(activation)=b.get("activation") {
    if activation.as_str().is_none_or(|s|s.len()!=64||!s.bytes().all(|b|b.is_ascii_hexdigit()&&!b.is_ascii_uppercase())) {return Err("installation activation binding corrupt; evidence kept".into());}
    keys.push("activation");
}
if !crate::resources::keys(&op.bound,&["local_install"])||!crate::resources::keys(b,&keys)
    ||b["schema"]!=1||b["id"]!=op.receipt["id"]||op.receipt["action"]!="install"||op.receipt["root"]!=b["root"]||op.receipt["version"]!=b["version"] {
    return Err("installation receipt binding corrupt; evidence kept".into());
}
let command=CommandReceipt::decode(b["command"].as_str().ok_or("installation command missing")?)?;
let metadata:Value=serde_json::from_str(&command.encode()).map_err(|e|e.to_string())?;
if metadata["product"]!="taskq"||metadata["operation"]!="taskq.host.install" {return Err("installation command identity invalid".into());}
let required=match op.receipt["state"].as_str() {Some("queued")=>vec!["received"],Some("applied")=>vec!["received","applied"],Some("observed")=>vec!["received","applied","observed"],Some("failed")=>vec!["received","failed"],_=>return Err("installation receipt phase unsupported".into())};
for phase in required {if op.receipt.get(format!("{phase}_receipt")).is_none() {return Err("installation phase proof missing".into());}}
for phase in ["received","applied","observed","failed"] {
    if let Some(value)=op.receipt.get(format!("{phase}_receipt")) {if *value!=command.event_id(phase)? {return Err("installation phase identity differs".into());}}
}
    Ok(&op.receipt)
}

pub fn installation_status(install:&Path,id:Option<&str>)->Value {
    crate::resources::try_with(install,|state| {
        let op=if let Some(id)=id {state.ops.get(id).filter(|op|op.bound.get("local_install").is_some())}
            else {
                let latest=state.ops.values().filter(|op|op.bound.get("local_install").is_some()).max_by_key(|op|op.saved_at);
                if latest.is_some_and(|latest|state.ops.values().filter(|op|op.bound.get("local_install").is_some()&&op.saved_at==latest.saved_at).count()>1) {
                    return Err("installation observation time is ambiguous; use service status --request ID".into());
                }
                latest
            };
        let Some(op)=op else {return Ok(json!({"state":"unknown","reason":"no retained installation receipt"}));};
        Ok(checked_receipt(op).cloned().unwrap_or_else(|reason|json!({"id":op.bound["local_install"]["id"],"action":"install","state":"unknown","reason":reason})))
    }).unwrap_or_else(|reason|json!({"state":"unknown","reason":reason}))
}

pub(super) fn registered_projects(install:&Path)->Res<Option<String>> {
    let p=pending(install)?;
    if p.phase!="registered"||raw_projects(install)?.as_deref()!=Some(p.next.as_str()) {
        return Err("projects.installing retained; project membership unknown".into());
    }
    Ok(Some(p.next))
}
/// A prior status for the same PID/root cannot acknowledge another registration.
pub fn install_acknowledgement(install:&Path,root:&Path,instance:&str)->Option<String> {
    let p=pending(install).ok()?;
    (p.phase=="registered"&&p.root==root&&p.owner.as_ref().is_some_and(|owner|owner.instance==instance)
        &&raw_projects(install).ok().flatten().as_deref()==Some(p.next.as_str())).then_some(p.id)
}
pub(super) fn admission_reason(install:&Path)->Option<String> {
    match fs::symlink_metadata(install.join(INSTALL_PENDING)) {
        Err(e) if e.kind()==std::io::ErrorKind::NotFound=>None,
        Err(e)=>Some(format!("installation prepare unknown: {e}")),
        Ok(_)=>Some("installation acknowledgement pending; owned work remains supervised".into()),
    }
}

fn snapshot(install:&Path,roots:Vec<PathBuf>,paths:Vec<PathBuf>)->Res<String> {
    let path=manifest(install);
    match fs::symlink_metadata(&path) {
        Ok(meta) if meta.is_file()=>return fs::read(path).map(|b|digest(&b)).map_err(|e|e.to_string()),
        Ok(_)=>return Err("installation originals are not a regular file; kept".into()),
        Err(e) if e.kind()==std::io::ErrorKind::NotFound=>{},Err(e)=>return Err(e.to_string()),
    }
    let baseline=capture_originals(&directory(install),roots,paths)?;
    let bytes=serde_json::to_vec(&baseline).map_err(|e|e.to_string())?;
    coreq::write_durable(&path,&bytes)?;
    #[cfg(test)] tests::interrupt(install,"snapshot")?;
    Ok(digest(&bytes))
}

fn capture_originals(folder:&Path,roots:Vec<PathBuf>,paths:Vec<PathBuf>)->Res<Baseline> {
    // No active effect precedes the manifest. Rebuild only this installer's fixed, uncommitted copy slots.
    match fs::symlink_metadata(&folder) {
        Ok(meta) if meta.is_dir()=>{
            ownership(&folder)?;
            for entry in fs::read_dir(&folder).map_err(|e|e.to_string())? {
                let entry=entry.map_err(|e|e.to_string())?;
                if entry.file_name().to_str().and_then(|n|n.parse::<usize>().ok()).is_none_or(|n|n>=paths.len())
                    ||!entry.file_type().map_err(|e|e.to_string())?.is_file() {return Err("unknown installation snapshot contents; originals kept".into());}
            }
        },
        Ok(_)=>return Err("installation snapshot directory is not owned regular storage; kept".into()),
        Err(e) if e.kind()==std::io::ErrorKind::NotFound=>{},Err(e)=>return Err(e.to_string()),
    }
    fs::create_dir_all(&folder).map_err(|e|e.to_string())?;
    #[cfg(unix)] {use std::os::unix::fs::PermissionsExt;fs::set_permissions(&folder,fs::Permissions::from_mode(0o700)).map_err(|e|e.to_string())?;}
    let mut files=vec![];
    for (i,path) in paths.into_iter().enumerate() {
        let meta=match fs::symlink_metadata(&path) {
            Ok(m) if m.is_file()=>Some(m),Ok(_)=>return Err(format!("installation input {} is not a regular file; kept",path.display())),
            Err(e) if e.kind()==std::io::ErrorKind::NotFound=>None,Err(e)=>return Err(e.to_string()),
        };
        let (hash,owner,mode,readonly)=if let Some(meta)=meta {
            let owner=ownership(&path)?;
            let bytes=fs::read(&path).map_err(|e|e.to_string())?;
            let dst=folder.join(i.to_string());
            if fs::read(&dst).ok().as_deref()!=Some(bytes.as_slice()) {coreq::write_durable(&dst,&bytes)?;}
            fs::set_permissions(&dst,meta.permissions()).map_err(|e|e.to_string())?;
            #[cfg(not(unix))] let mode=0;
            #[cfg(unix)] let mode={use std::os::unix::fs::PermissionsExt;meta.permissions().mode()};
            if fs::read(&dst).map_err(|e|e.to_string())?!=bytes {return Err("installation snapshot readback differs".into());}
            (Some(digest(&bytes)),owner,mode,meta.permissions().readonly())
        } else {(None,Value::Null,0,false)};
        files.push(Original {path,digest:hash,owner,mode,readonly});
    }
    for saved in &files {
        let current=match fs::read(&saved.path) {Ok(b)=>Some(digest(&b)),Err(e) if e.kind()==std::io::ErrorKind::NotFound=>None,Err(e)=>return Err(e.to_string())};
        if current!=saved.digest||saved.digest.is_some()&&ownership(&saved.path)?!=saved.owner {return Err("installation input changed during snapshot; active files unchanged".into());}
    }
    Ok(Baseline {schema:1,version:crate::platform::VERSION.into(),roots,files})
}

fn verify_originals(install:&Path)->Res<()> {
    let path=manifest(install);let meta=fs::symlink_metadata(&path).map_err(|e|e.to_string())?;
    if !meta.is_file()||meta.len()>1<<20 {return Err("installation originals manifest unsupported; kept".into());}
    let baseline:Baseline=serde_json::from_slice(&fs::read(path).map_err(|e|e.to_string())?).map_err(|e|e.to_string())?;
    if baseline.schema!=1 {return Err("installation originals schema unsupported; kept".into());}
    verify_copies(&directory(install),&baseline)
}

fn verify_copies(folder:&Path,baseline:&Baseline)->Res<()> {
    if baseline.schema!=1 {return Err("installation originals schema unsupported; kept".into());}
    if !fs::symlink_metadata(&folder).map_err(|e|e.to_string())?.is_dir() {return Err("installation original storage is not a directory".into());}
    for (i,saved) in baseline.files.iter().enumerate() {
        if let Some(hash)=&saved.digest {
            let path=folder.join(i.to_string());
            if !fs::symlink_metadata(&path).map_err(|e|e.to_string())?.is_file()||digest(&fs::read(path).map_err(|e|e.to_string())?)!=*hash {
                return Err("installation original checksum differs; transition held before active effects".into());
            }
        }
    }
    Ok(())
}

fn verify_legacy_original(install:&Path,p:&Pending)->Res<()> {
    let Some(legacy)=&p.legacy else {return Ok(());};
    let baseline:Baseline=serde_json::from_slice(&fs::read(manifest(install)).map_err(|e|e.to_string())?).map_err(|e|e.to_string())?;
    let unit=coreq::service::unit(&crate::platform::tool(install),&legacy.owner.root,&legacy.owner.project);
    if !baseline.files.iter().any(|saved|saved.path==unit.path&&saved.digest.as_ref()==Some(&legacy.owner.revision)) {
        return Err("legacy native definition is not bound to the preserved topology originals; kept".into());
    }
    Ok(())
}

/// Restore only pre-native activation effects. Native submission uncertainty never enters this route.
fn restore_activation(install:&Path,p:&Pending)->Res<()> {
    let initial:Baseline;
    let (baseline,folder)=if let Some(activation)=&p.activation {(activation,activation_directory(install))}else {
        initial=serde_json::from_slice(&fs::read(manifest(install)).map_err(|e|e.to_string())?).map_err(|e|e.to_string())?;
        (&initial,directory(install))
    };
    if !p.restore||baseline.schema!=1||baseline.version!=p.version||!matches!(p.phase.as_str(),"prepared"|"activated") {
        return Err("native installation may own work; automatic topology restore refused".into());
    }
    if native_owner(install,p.legacy.as_ref().map(|l|&l.owner))?.is_some()||!crate::host_control::native_absent(&crate::platform::tool(install))? {return Err("native owner/start cannot be excluded; originals retained".into());}
    let _owner=coreq::file_lock::try_lock(&install.join("service.lock")).map_err(|e|format!("native ownership unknown during restore: {e}"))?;
    verify_copies(&folder,baseline)?;
    guard_restore(install,p,&folder,baseline)?;
    // A caller lost during restore must resume the failure, never activate again.
    record(install,p,"failed")?;
    restore_files(install,&folder,baseline)?;
    fs::remove_file(install.join(INSTALL_PENDING)).map_err(|e|e.to_string())
}

fn activation_paths(install:&Path)->Res<Vec<PathBuf>> {
    let paths=vec![install.join(crate::platform::tool(install).exe_name()),install.join("current"),install.join("previous"),crate::host_control::definition(&crate::platform::tool(install))?];
    #[cfg(all(windows,target_env="gnu"))] let paths={let mut paths=paths;paths.push(install.join("WebView2Loader.dll"));paths};
    Ok(paths)
}

// Validate the whole write set before restoring any file, including an otherwise untouched native definition.
fn guard_restore(install:&Path,p:&Pending,folder:&Path,baseline:&Baseline)->Res<()> {
    let tool=crate::platform::tool(install);
    // The cursor uses the same create/rename writer and install directory as activation's fresh files.
    let created=fs::metadata(install.join(INSTALL_PENDING)).map_err(|e|e.to_string())?;
    let created_owner=ownership(&install.join(INSTALL_PENDING))?;
    let previous=baseline.files.iter().enumerate().find(|(_,saved)|saved.path==install.join("current"))
        .filter(|(_,saved)|saved.digest.is_some()).map(|(i,_)|fs::read_to_string(folder.join(i.to_string())).map_err(|e|e.to_string())).transpose()?;
    for path in activation_paths(install)? {
        let saved=baseline.files.iter().find(|saved|saved.path==path).ok_or_else(||format!("installation original missing: {}",path.display()))?;
        let metadata=match fs::symlink_metadata(&path) {
            Ok(meta) if meta.is_file()=>Some(meta),Err(e) if e.kind()==std::io::ErrorKind::NotFound=>None,
            _=>return Err(format!("installation input changed before restore: {}; originals retained",path.display())),
        };
        let actual=metadata.as_ref().map(|_|fs::read(&path).map(|b|digest(&b)).map_err(|e|e.to_string())).transpose()?;
        let expected=if path==install.join(tool.exe_name()) {Some(p.candidate.clone())}
            else if path==install.join("current") {previous.as_deref().is_none_or(|v|v.trim()!=p.version).then(||digest(p.version.as_bytes()))}
            else if path==install.join("previous") {previous.as_deref().filter(|v|v.trim()!=p.version).map(|v|digest(v.trim().as_bytes()))}
            else {None};
        #[cfg(all(windows,target_env="gnu"))] let expected=if path==install.join("WebView2Loader.dll") {
            Some(digest(&fs::read(tool.release_exe(&p.version).with_file_name("WebView2Loader.dll")).map_err(|e|e.to_string())?))
        } else {expected};
        if actual!=saved.digest&&(expected.is_none()||actual!=expected) {
            return Err(format!("installation input changed before restore: {}; originals retained",path.display()));
        }
        if let Some(meta)=metadata {
            let owner=ownership(&path)?;
            let original=actual==saved.digest&&owner==saved.owner&&meta.permissions().readonly()==saved.readonly;
            let candidate=actual==expected&&owner==created_owner;
            #[cfg(unix)] let (original,candidate)={
                use std::os::unix::fs::PermissionsExt;
                let mode=if path==install.join(tool.exe_name()) {
                    if saved.digest.is_some() {saved.mode}else{fs::metadata(tool.release_exe(&p.version)).map_err(|e|e.to_string())?.permissions().mode()}
                }else{created.permissions().mode()};
                (original&&meta.permissions().mode()==saved.mode,candidate&&meta.permissions().mode()==mode)
            };
            #[cfg(windows)] let candidate=candidate&&meta.permissions().readonly()==if path==install.join(tool.exe_name()) {
                fs::metadata(tool.release_exe(&p.version)).map_err(|e|e.to_string())?.permissions().readonly()
            }else{created.permissions().readonly()};
            if !original&&!candidate {return Err("installation ownership/permissions changed before restore; originals retained".into());}
        }
    }
    Ok(())
}

fn original_file_matches(saved:&Original,bytes:&[u8])->Res<bool> {
    let metadata=match fs::symlink_metadata(&saved.path) {
        Ok(meta) if meta.is_file()=>meta,
        Err(e) if e.kind()==std::io::ErrorKind::NotFound=>return Ok(false),
        Ok(_)=>return Err("installation restore target is not a regular file; originals retained".into()),
        Err(e)=>return Err(format!("installation restore metadata {}: {e}",saved.path.display())),
    };
    let permissions=metadata.permissions();
    #[cfg(unix)] let same_permissions={use std::os::unix::fs::PermissionsExt;permissions.mode()==saved.mode};
    #[cfg(windows)] let same_permissions=permissions.readonly()==saved.readonly;
    Ok(same_permissions&&fs::read(&saved.path).map_err(|e|format!("installation restore readback {}: {e}",saved.path.display()))?==bytes&&ownership(&saved.path)?==saved.owner)
}

fn restore_files(install:&Path,folder:&Path,baseline:&Baseline)->Res<()> {
    let mut originals=vec![];
    let names=vec![crate::platform::tool(install).exe_name(),"current".into(),"previous".into()];
    #[cfg(all(windows,target_env="gnu"))] let names={let mut names=names;names.push("WebView2Loader.dll".into());names};
    let mut paths:Vec<_>=names.into_iter().map(|name|install.join(name)).collect();
    let definition=crate::host_control::definition(&crate::platform::tool(install))?;
    if baseline.files.iter().any(|f|f.path==definition) {paths.push(definition);}
    for path in paths {
        let (i,saved)=baseline.files.iter().enumerate().find(|(_,f)|f.path==path).ok_or_else(||format!("installation original missing: {}",path.display()))?;
        if let Some(hash)=&saved.digest {
            let bytes=fs::read(folder.join(i.to_string())).map_err(|e|e.to_string())?;
            if digest(&bytes)!=*hash {return Err("installation original checksum differs; restore held".into());}
            originals.push((saved,Some(bytes)));
        } else {originals.push((saved,None));}
    }
    for (saved,bytes) in originals {
        let path=&saved.path;
        if let Some(bytes)=bytes {
            if original_file_matches(saved,&bytes)? {continue;}
            coreq::write_durable(path,&bytes).map_err(|e|format!("installation restore write {}: {e}",path.display()))?;
            // Windows needs a writable handle for flush, retained before restoring readonly/DACL.
            let file=fs::OpenOptions::new().write(true).open(path).map_err(|e|format!("installation restore sync handle {}: {e}",path.display()))?;
            #[cfg(unix)] {use std::os::unix::fs::PermissionsExt;fs::set_permissions(&path,fs::Permissions::from_mode(saved.mode)).map_err(|e|e.to_string())?;}
            #[cfg(windows)] {let mut mode=fs::metadata(&path).map_err(|e|e.to_string())?.permissions();mode.set_readonly(saved.readonly);fs::set_permissions(&path,mode).map_err(|e|e.to_string())?;}
            stage_owner(&path,&saved.owner)?;
            file.sync_all().map_err(|e|format!("installation restore sync {}: {e}",path.display()))?;
            drop(file);
            if !original_file_matches(saved,&bytes)? {return Err(format!("installation restore readback differs: {}",path.display()));}
        } else {
            match fs::remove_file(&path) {Ok(())=>{},Err(e) if e.kind()==std::io::ErrorKind::NotFound=>{},Err(e)=>return Err(e.to_string())}
        }
        #[cfg(test)] tests::interrupt(install,"restored")?;
    }
    Ok(())
}

fn native_owner(install:&Path,legacy:Option<&crate::host_control::LegacyOwner>)->Res<Option<coreq::process::Identity>> {
    match service_record(install)? {
        None=>Ok(None),Some(OwnerRecord::Legacy(pid))=>{
            if let Some(owner)=legacy.filter(|owner|owner.pid==pid) {owner.verify(&crate::platform::tool(install))?;return Ok(None);}
            Err("legacy host owner requires migration; installation refused".into())
        },
        Some(OwnerRecord::Current(r))=>{
            let expected=coreq::process::Identity {pid:r.pid,started:r.started};
            if let Some(owner)=legacy.filter(|owner|owner.identity()==expected) {owner.verify(&crate::platform::tool(install))?;return Ok(None);}
            Ok(coreq::process::identity(r.pid).map_err(|e|e.to_string())?.filter(|native|native==&expected))
        },
    }
}

/// Saved units are a conservative admission barrier, never a substitute for native retirement readback.
pub(super) fn legacy_definitions(install:&Path)->Res<Vec<PathBuf>> {
    let tool=crate::platform::tool(install);let host=crate::host_control::definition(&tool)?;
    let directory=host.parent().ok_or("native definitions directory unknown")?;
    let entries=match fs::read_dir(directory) {Ok(entries)=>entries,Err(e) if e.kind()==std::io::ErrorKind::NotFound=>return Ok(vec![]),Err(e)=>return Err(e.to_string())};
    let (prefix,suffix)=if cfg!(target_os="macos") {(format!("com.{}.",tool.name),".plist")}else if cfg!(windows) {(format!("{}-",tool.title),".xml")}else{(format!("{}-",tool.name),".service")};
    let mut paths=vec![];let mut bytes=0;
    for entry in entries {
        let entry=entry.map_err(|e|e.to_string())?;let path=entry.path();let name=entry.file_name();let name=name.to_string_lossy();
        if path==host||!name.starts_with(&prefix)||!name.ends_with(suffix) {continue;}
        let metadata=fs::symlink_metadata(&path).map_err(|e|e.to_string())?;
        if !metadata.is_file()||metadata.len()>65536 {return Err("legacy definition storage unknown; host admission held".into());}
        ownership(&path)?;bytes+=path.as_os_str().len();
        if bytes>1<<20 {return Err("legacy definition inventory exceeds migration cursor bound; kept".into());}
        paths.push(path);
    }
    paths.sort();Ok(paths)
}

pub(super) fn legacy_projection(install:&Path,pid:u32,started:Option<&str>)->Res<bool> {
    match fs::symlink_metadata(install.join(INSTALL_PENDING)) {Err(e) if e.kind()==std::io::ErrorKind::NotFound=>return Ok(false),Err(e)=>return Err(e.to_string()),Ok(_)=>{}}
    let p=pending(install)?;
    let Some(legacy)=p.legacy.filter(|l|l.owner.pid==pid&&started.is_none_or(|s|s==l.owner.started)) else {return Ok(false);};
    legacy.owner.verify(&crate::platform::tool(install))?;Ok(true)
}

pub(super) fn controller_id(install:&Path)->Res<Option<String>> {
    match fs::symlink_metadata(install.join(INSTALL_PENDING)) {Err(e) if e.kind()==std::io::ErrorKind::NotFound=>return Ok(None),Err(e)=>return Err(e.to_string()),Ok(_)=>{}}
    let p=pending(install)?;
    Ok(p.legacy.as_ref().is_some_and(|legacy|matches!(legacy.phase.as_str(),"ready"|"retiring"|"retired")).then_some(p.id))
}

fn verify_legacy_caller(install:&Path,owner:&crate::host_control::LegacyOwner)->Res<()> {
    if coreq::process::identity(std::process::id()).map_err(|e|e.to_string())?.as_ref()!=Some(&owner.identity()) {return Err("only the original native legacy daemon may prepare its drain".into());}
    owner.verify(&crate::platform::tool(install))
}

#[tracing::instrument(name="host.install",skip_all)]
pub fn legacy_install(install:&Path,owner:&crate::host_control::LegacyOwner)->Res<String> {
    verify_legacy_caller(install,owner)?;
    install_bound(install,&owner.root,None,Some(owner))
}

/// Only the service's reconciled idle state may call this; the prepared loop then admits/resumes nothing.
pub fn prepare_legacy(install:&Path,owner:&crate::host_control::LegacyOwner)->Res<bool> {
    let _update=crate::platform::update_lock(install)?;
    let _projects=coreq::file_lock::try_lock(&install.join("projects.lock")).map_err(|e|format!("project registry busy: {e}"))?;
    match fs::symlink_metadata(install.join(INSTALL_PENDING)) {Err(e) if e.kind()==std::io::ErrorKind::NotFound=>return Ok(false),Err(e)=>return Err(e.to_string()),Ok(_)=>{}}
    let mut p=pending(install)?;
    if p.root!=owner.root {return Ok(false);}
    let Some(legacy)=p.legacy.as_mut() else {return Ok(false);};
    verify_legacy_caller(install,owner)?;
    if legacy.phase=="retired" {return Err("retired legacy unit reappeared; native reconciliation required".into());}
    if legacy.owner!=*owner {
        let prior=&legacy.owner;
        if prior.root!=owner.root||prior.project!=owner.project||prior.revision!=owner.revision
            ||coreq::process::identity(prior.pid).map_err(|e|e.to_string())?.as_ref()==Some(&prior.identity()) {return Err("prior legacy native ownership remains; preparation held".into());}
        if let Some(launcher)=&prior.launcher {
            let pid=launcher["pid"].as_u64().and_then(|pid|u32::try_from(pid).ok()).ok_or("prior legacy launcher unknown")?;
            if coreq::process::identity(pid).map_err(|e|e.to_string())?.is_some_and(|native|Some(native.started.as_str())==launcher["started"].as_str()) {return Err("prior legacy launcher remains; preparation held".into());}
        }
        legacy.owner=owner.clone();legacy.phase="draining".into();save(install,&p)?;
    }
    if p.phase!="registered" {return Ok(false);}
    let legacy=p.legacy.as_mut().unwrap();
    if legacy.phase=="draining" {record_legacy(install,&p,"prepared")?;p.legacy.as_mut().unwrap().phase="ready".into();save(install,&p)?;}
    Ok(matches!(p.legacy.as_ref().unwrap().phase.as_str(),"ready"|"retiring"))
}

pub fn legacy_latched(install:&Path,owner:&crate::host_control::LegacyOwner)->Res<bool> {
    match fs::symlink_metadata(install.join(INSTALL_PENDING)) {Err(e) if e.kind()==std::io::ErrorKind::NotFound=>return Ok(false),Err(e)=>return Err(e.to_string()),Ok(_)=>{}}
    let p=pending(install)?;
    let latched=p.root==owner.root&&p.legacy.as_ref().is_some_and(|legacy|matches!(legacy.phase.as_str(),"ready"|"retiring"|"retired"));
    if latched&&!legacy_proven(install,&p,"prepared")? {return Err("legacy drain proof missing or changed; launches held".into());}
    Ok(latched)
}

fn context(p:&Pending)->Res<String> {
    let command:Value=serde_json::from_str(&p.command).map_err(|e|e.to_string())?;
    coreq::telemetry::capture::TraceContext::decode(&format!("1 {} {}",command["trace"].as_str().ok_or("installation trace missing")?,command["span"].as_str().ok_or("installation span missing")?)).map(|context|context.encode())
}

pub fn dispatch_legacy(install:&Path)->Res<crate::host_control::Control> {
    let p=pending(install)?;
    if p.phase!="registered"||!p.legacy.as_ref().is_some_and(|legacy|matches!(legacy.phase.as_str(),"ready"|"retiring")) {return Err("legacy handoff has no prepared original command".into());}
    if !legacy_proven(install,&p,"prepared")? {return Err("legacy controller has no original drain proof".into());}
    crate::host_control::dispatch_command(install,&p.id,"complete-legacy",&context(&p)?)
}

pub fn complete_legacy(install:&Path,id:&str)->Res<()> {
    if std::env::var_os("TASKQ_ATTEMPT").is_some() {return Err("a worker cannot retire a legacy unit".into());}
    let result=(||->Res<()> {
    let _update=crate::platform::update_lock(install)?;
    let _projects=coreq::file_lock::try_lock(&install.join("projects.lock")).map_err(|e|format!("project registry busy: {e}"))?;
    let mut p=pending(install)?;
    if p.id!=id||p.phase!="registered"||raw_projects(install)?.as_deref()!=Some(p.next.as_str()) {return Err("legacy retirement binding changed; kept".into());}
    let legacy=p.legacy.as_mut().ok_or("installation has no native legacy owner")?;
    if !matches!(legacy.phase.as_str(),"ready"|"retiring"|"retired") {return Err("legacy ownership drain not confirmed; kept".into());}
    let owner=legacy.owner.clone();
    let retired=legacy.phase=="retired";
    if !legacy_proven(install,&p,"prepared")? {return Err("legacy drain proof missing or belongs to another native owner; kept".into());}
    verify_originals(install)?;verify_legacy_original(install,&p)?;
    if !retired {
        p.legacy.as_mut().unwrap().phase="retiring".into();save(install,&p)?;
    }
    owner.retire(&crate::platform::tool(install),id)?;
    record_legacy(install,&p,"retired")?;
    p.legacy.as_mut().unwrap().phase="retired".into();save(install,&p)?;
    let context=context(&p)?;drop(_projects);drop(_update);
    crate::host_control::retire_command(install,id,"complete-legacy",&context)
    })();
    if let Err(reason)=&result {
        if let Ok(p)=pending(install)&&p.id==id {retain_reason(install,&p,reason).map_err(|e|format!("{reason}; legacy failure could not be retained: {e}"))?;}
    }
    result
}

pub(super) fn recover(install:&Path)->Res<String> {
    let p=pending(install)?;
    let context=coreq::telemetry::capture::TraceContext::decode(&context(&p)?)?;
    let _context=context.span().entered();
    if p.legacy.as_ref().is_some_and(|legacy|matches!(legacy.phase.as_str(),"ready"|"retiring")) {dispatch_legacy(install)?.wait()?;}
    install_bound(install,&p.root,Some(&p.id),None)
}

fn acknowledge(install:&Path,expected:&Pending)->Res<bool> {
    let _update=match coreq::file_lock::try_lock(&install.join("update.lock")) {
        Ok(lock)=>lock,Err(std::fs::TryLockError::WouldBlock)=>return Ok(false),Err(e)=>return Err(e.to_string()),
    };
    let _projects=match coreq::file_lock::try_lock(&install.join("projects.lock")) {
        Ok(lock)=>lock,Err(std::fs::TryLockError::WouldBlock)=>return Ok(false),Err(e)=>return Err(e.to_string()),
    };
    match fs::symlink_metadata(install.join(INSTALL_PENDING)) {
        Err(e) if e.kind()==std::io::ErrorKind::NotFound=>return if retained(install,expected)?["state"]=="observed" {Ok(true)}else{Err("installation cursor absent without its observed receipt".into())},
        Err(e)=>return Err(e.to_string()),Ok(_)=>{},
    }
    let p=pending(install)?;
    if p.id!=expected.id||p.command!=expected.command||p.root!=expected.root||p.version!=expected.version||p.candidate!=expected.candidate||p.next!=expected.next {
        return Err("installation binding changed before acknowledgement; retained".into());
    }
    if p.phase!="registered"||raw_projects(install)?.as_deref()!=Some(p.next.as_str()) {return Ok(false);}
    if let Some(legacy)=&p.legacy {
        if legacy.phase!="retired"||!legacy_proven(install,&p,"retired")?||!legacy.owner.retired(&crate::platform::tool(install))? {return Ok(false);}
    }
    let owner=p.owner.as_ref().ok_or("installation native owner missing")?;
    let service=service_status(install);
    let status=crate::data::json(&crate::dot(&p.root).join("status.json"))?;
    if !status.as_ref().is_some_and(|status|status["host_install"]==p.id)||!installation_observed(&service,status.as_ref(),&p.version)
        ||service["pid"]!=owner.pid||service["started"]!=owner.started||service["instance"]!=owner.instance {return Ok(false);}
    record(install,&p,"observed")?;
    fs::remove_file(install.join(INSTALL_PENDING)).map_err(|e|e.to_string())?;
    Ok(true)
}

#[tracing::instrument(name="host.install",skip_all)]
pub fn install_project(install:&Path,root:&Path)->Res<String> {
    install_bound(install,root,None,None)
}

fn receive_initial(install:&Path,p:&mut Pending)->Res<()> {
    let saved=pending(install)?;
    if saved.phase!="initial"||saved.id!=p.id||saved.command!=p.command||binding(&saved)!=binding(p) {
        return Err("original initial installation intent not committed; held".into());
    }
    let exists=crate::resources::try_with(install,|state|Ok(state.ops.contains_key(&p.id)))?;
    if !exists {record(install,p,"received")?;}
    if retained(install,p)?["state"]!="queued" {return Err("initial installation intent has an incompatible receipt; held".into());}
    p.phase=if p.attach {"owned"}else{"prepared"}.into();save(install,p)
}

fn install_bound(install:&Path,root:&Path,expected:Option<&str>,migration:Option<&crate::host_control::LegacyOwner>)->Res<String> {
    if std::env::var_os("TASKQ_ATTEMPT").is_some() {return Err("a worker cannot install the host service".into());}
    let _update=crate::platform::update_lock(install)?;
    let root=root.canonicalize().map_err(|e|e.to_string())?;
    let tool=crate::platform::tool(install);
    let replay=match fs::symlink_metadata(install.join(INSTALL_PENDING)) {
        Ok(_)=>Some(pending(install)?),Err(e) if e.kind()==std::io::ErrorKind::NotFound=>None,Err(e)=>return Err(e.to_string()),
    };
    if let Some(id)=expected {
        match &replay {
            Some(p) if p.id==id=>{},Some(_)=>return Err("another installation owns the transition; original recovery cannot start a command".into()),
            None=>return if installation_status(install,Some(id))["state"]=="observed" {Ok(format!("Install receipt {id} already observed"))}else{Err("original installation cursor absent; recovery cannot start a command".into())},
        }
    }
    let (edit,mut p)=if let Some(mut p)=replay {
        if !p.attach&&p.version!=tool.version {return Err("installation candidate version differs; use the original installer to reconcile".into());}
        if p.root!=root {return Err("another installation owns projects.installing; reconcile its exact root".into());}
        let lock=coreq::file_lock::try_lock(&install.join("projects.lock")).map_err(|e|format!("project registry busy: {e}"))?;
        let current=raw_projects(install)?;
        if current!=p.original&&current.as_deref()!=Some(p.next.as_str()) {return Err("installation registry changed; originals retained".into());}
        if let Some(owner)=migration {
            let prior=p.legacy.as_mut().ok_or("another installation owns this root; legacy authority unchanged")?;
            if prior.owner!=*owner {return Err("legacy generation changed; reconcile retained native ownership before rebinding".into());}
        }
        let edit=ProjectEdit {install:install.into(),original:current,next:p.next.clone(),removed:vec![root.clone()],adding:true,_lock:lock};
        (edit,p)
    } else {
        let edit=ProjectEdit::new(install,&root,true)?;
        if let Some(legacy)=migration {verify_legacy_caller(install,legacy)?;}
        let owner=native_owner(install,migration)?;
        let mut roots:Vec<_>=project_entries(&edit.next)?.into_iter().map(|(_,root)|root).collect();roots.sort();roots.dedup();
        for project in roots.iter().filter(|_|migration.is_none()) {
            if project_service(project)?.is_some_and(|pid|owner.as_ref().is_none_or(|owner|owner.pid!=pid)) {return Err("handoff: legacy project owner must drain before host installation".into());}
            let legacy=coreq::service::unit(&tool,project,&crate::project_name(project));
            match fs::symlink_metadata(&legacy.path) {Ok(_)=>return Err("handoff: legacy unit remains; migrate before host installation".into()),Err(e) if e.kind()==std::io::ErrorKind::NotFound=>{},Err(e)=>return Err(e.to_string())}
        }
        crate::host_control::install_state(&tool,owner.as_ref())?;
        let command=CommandReceipt::current()?.encode();
        let p=if let Some(owner)=owner {
            let service=service_status(install);
            if service["install_protocol"]!=1||service["fresh"]!=true {return Err("running host has no compatible installation protocol; update/reconcile it before attaching a root".into());}
            let version=service["version"].as_str().ok_or("running host version unknown")?;
            let baseline=fs::read(manifest(install)).map_err(|e|format!("host installation originals missing; attach refused: {e}"))?;
            Pending {schema:1,id:crate::runtime::uuid(),baseline:digest(&baseline),root:root.clone(),version:version.into(),
                candidate:digest(&fs::read(tool.release_exe(version)).map_err(|e|e.to_string())?),original:edit.original.clone(),next:edit.next.clone(),
                phase:"owned".into(),restore:false,attach:true,owner:Some(InstallOwner {pid:owner.pid,started:owner.started,instance:service["instance"].as_str().ok_or("host generation missing")?.into()}),command,
                legacy:migration.map(|owner|Legacy {owner:owner.clone(),phase:"draining".into()}),activation:None}
        } else {
        let mut paths=vec![install.join(tool.exe_name()),install.join("current"),install.join("previous"),crate::host_control::definition(&tool)?,install.join("projects"),install.join("rollback-floor")];
        paths.extend(legacy_definitions(install)?);
        #[cfg(all(windows,target_env="gnu"))] paths.push(install.join("WebView2Loader.dll"));
        paths.extend(roots.iter().map(|r|crate::dot(r).join("focus")));
        for name in ["current","previous"] {
            match fs::read_to_string(install.join(name)) {
                Ok(v)=>{let v=v.trim();if v.is_empty()||v.contains("..")||!v.bytes().all(|b|b.is_ascii_alphanumeric()||b".-".contains(&b)) {return Err("installation release selection unknown; kept".into());}let path=tool.release_exe(v);if !fs::symlink_metadata(&path).map_err(|e|e.to_string())?.is_file() {return Err("installation retained release unavailable".into());}if !paths.contains(&path) {paths.push(path);}},
                Err(e) if e.kind()==std::io::ErrorKind::NotFound=>{},Err(e)=>return Err(e.to_string()),
            }
        }
        let first=!manifest(install).try_exists().map_err(|e|e.to_string())?;
        let restore=migration.is_none();
        let candidate=crate::platform::prepare_install(&tool)?;
        let activation=if first {None}else{Some(capture_originals(&activation_directory(install),vec![],activation_paths(install)?)?)};
        let baseline=snapshot(install,roots,paths)?;
        let p=Pending {schema:1,id:crate::runtime::uuid(),baseline,root:root.clone(),version:tool.version.into(),candidate,
            original:edit.original.clone(),next:edit.next.clone(),phase:"prepared".into(),restore,attach:false,owner:None,command,
            legacy:migration.map(|owner|Legacy {owner:owner.clone(),phase:"draining".into()}),activation};
            p
        };
        let mut p=p;p.phase="initial".into();
        save(install,&p)?;(edit,p)
    };
    let result=(||->Res<String> {
    verify_originals(install)?;verify_legacy_original(install,&p)?;
    if let Some(activation)=&p.activation {verify_copies(&activation_directory(install),activation)?;}
    if p.phase=="initial" {receive_initial(install,&mut p)?;}
    let receipt=retained(install,&p)?;
    if receipt["state"]=="failed" {
        restore_activation(install,&p)?;
        return Err("original installation failed; originals restored and failed cursor retired; a new command is required".into());
    }
    if receipt["state"]=="observed" {
        if p.phase!="registered" {return Err("observed installation has an incompatible cursor; transition held".into());}
        drop(edit);drop(_update);
        return if acknowledge(install,&p)? {Ok(format!("Install receipt {} already observed",p.id))}else{Err("original installation was observed; current native binding requires reconciliation".into())};
    }
    if digest(&fs::read(tool.release_exe(&p.version)).map_err(|e|e.to_string())?)!=p.candidate {return Err("prepared installation candidate changed; originals retained".into());}
    if matches!(p.phase.as_str(),"prepared"|"activated") {
        let activation=crate::platform::activate_install(&tool);
        #[cfg(test)] let activation=activation.and_then(|_|tests::activation_result(install));
        if let Err(error)=activation {
            return match restore_activation(install,&p) {Ok(())=>Err(format!("installation activation failed; originals restored: {error}")),Err(restore)=>Err(format!("installation activation unknown: {error}; {restore}"))};
        }
        let state=crate::host_control::install_state(&tool,None)?;
        p.phase="starting".into();save(install,&p)?;
        // Persist before native acceptance; replay observes it, never blindly issues another start.
        if let Err(error)=crate::host_control::start_install(&tool,state) {
            if !error.submitted {
                p.phase="prepared".into();save(install,&p)?;
                return match restore_activation(install,&p) {Ok(())=>Err(format!("native start not submitted; originals restored: {}",error.reason)),Err(restore)=>Err(format!("native start not submitted: {}; {restore}",error.reason))};
            }
            return Err(format!("native installation submitted; result unknown, repeat service install for this root to observe: {}",error.reason));
        }
    }
    if p.phase=="starting" {
        retain_reason(install,&p,"native host ownership acknowledgement pending; registry admission held")?;
        let until=Instant::now()+std::time::Duration::from_secs(60);
        loop {
            if let Some(owner)=native_owner(install,p.legacy.as_ref().map(|l|&l.owner))? {
                let service=service_status(install);
                if service["fresh"]==true&&service["version"]==tool.version {
                    crate::host_control::install_state(&tool,Some(&owner))?;
                    match coreq::file_lock::try_lock(&install.join("service.lock")) {
                        Err(std::fs::TryLockError::WouldBlock)=>{},_=>return Err("native host ownership lock not observed; installation pending".into()),
                    }
                    p.owner=Some(InstallOwner {pid:owner.pid,started:owner.started,instance:service["instance"].as_str().ok_or("host generation missing")?.into()});
                    p.phase="owned".into();save(install,&p)?;break;
                }
            }
            if Instant::now()>=until {return Err("native installation unknown; projects.installing and originals retained; repeat service install for the same root to reconcile".into());}
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
    // Past the native boundary, failure cannot restore the old topology under an owner.
    let owner=native_owner(install,p.legacy.as_ref().map(|l|&l.owner))?.ok_or("host installation pending; native owner not observed")?;
    crate::host_control::install_state(&tool,Some(&owner))?;
    let service=service_status(install);
    if service["fresh"]!=true||service["version"]!=p.version||service["install_protocol"]!=1 {
        return Err("host installation pending; exact candidate version/protocol and fresh ownership not observed".into());
    }
    if let Some(old)=&p.owner {
        let expected=coreq::process::Identity {pid:old.pid,started:old.started.clone()};
        if expected!=owner&&coreq::process::identity(old.pid).map_err(|e|e.to_string())?.as_ref()==Some(&expected) {return Err("prior installation owner remains live; reconcile native scope".into());}
    }
    p.owner=Some(InstallOwner {pid:owner.pid,started:owner.started,instance:service["instance"].as_str().ok_or("host generation missing")?.into()});save(install,&p)?;
    if !p.attach {
        if tool.current().as_deref()!=Some(tool.version) {crate::platform::activate_install(&tool)?;}
        let floor=match fs::read_to_string(install.join("rollback-floor")) {Ok(v)=>v,Err(e) if e.kind()==std::io::ErrorKind::NotFound=>service["version"].as_str().ok_or("host version missing")?.into(),Err(e)=>return Err(e.to_string())};
        coreq::launcher::set_rollback_floor(&tool,floor.trim())?;
    }
    let off=crate::install_focus(&root)?;
    p.phase="registered".into();save(install,&p)?;
    edit.commit()?;
    #[cfg(test)] tests::interrupt(install,"registry")?;
    record(install,&p,"applied")?;
    retain_reason(install,&p,if p.legacy.as_ref().is_some_and(|legacy|legacy.phase!="retired") {"legacy unit retirement pending; current work remains with its original daemon"}else{"project registration acknowledgement pending; host owns installation"})?;
    // The host may finish the original command if this caller disappears during project recovery.
    drop(_update);
    let until=Instant::now()+std::time::Duration::from_secs(60);
    loop {
        if acknowledge(install,&p)? {
            let service=service_status(install);
            return Ok(format!("installed host service (pid {}), project {}{}; Install receipt {}",service["pid"],root.display(),if off {"; Focus: off"}else{""},p.id));
        }
        if Instant::now()>=until {return Err("host owns installation; project acknowledgement pending, repeat service install for this root; originals retained".into());}
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    })();
    if let Err(reason)=&result {
        retain_reason(install,&p,reason).map_err(|e|format!("{reason}; installation reason could not be retained: {e}"))?;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    pub(super) fn interrupt(install:&Path,phase:&str)->Res<()> {
        if std::env::var("TASKQ_INSTALL_INTERRUPT").ok().as_deref()!=Some(phase) {return Ok(());}
        if std::env::var_os("TASKQ_INSTALL_PROCESS_CASE").as_deref()!=Some(install.as_os_str())
            ||!install.parent().and_then(Path::file_name).is_some_and(|name|name.to_string_lossy().starts_with("taskq-install-process-")) {return Err("owned installation process fixture required".into());}
        coreq::write_durable(&install.join("interrupted-at"),phase)?;
        let until=Instant::now()+std::time::Duration::from_secs(90);
        while !install.join("release-interruption").exists() {
            if Instant::now()>=until {return Err("installation fixture interruption expired".into());}
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        Ok(())
    }

    pub(super) fn activation_result(install:&Path)->Res<()> {
        interrupt(install,"activation")?;
        if std::env::var("TASKQ_INSTALL_FAIL_ACTIVATION").as_deref()==Ok("1") {
            if std::env::var_os("TASKQ_INSTALL_PROCESS_CASE").as_deref()!=Some(install.as_os_str()) {return Err("owned activation fixture required".into());}
            return Err("owned fixture activation failure after replacement".into());
        }
        Ok(())
    }

    #[cfg(target_os="macos")]
    fn process_case(case:&str) {
        let report=std::env::temp_dir().join(format!("taskq-install-report-{}.json",crate::runtime::uuid()));
        let result=coreq::process::output("test.install-restore",std::process::Command::new("python3")
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/install-process-macos.py"))
            .arg("--binary").arg(std::env::current_exe().unwrap()).arg("--output").arg(&report).arg(case),std::time::Duration::from_secs(150));
        let evidence=fs::read_to_string(&report).unwrap_or_default();
        if report.exists() {fs::remove_file(&report).unwrap();}
        assert!(result.is_ok(),"installation {case}: {result:?}\n{evidence}");
    }

    // H-A5: death between restore writes must finish rollback, never reactivate or start a native unit.
    #[cfg(target_os="macos")]
    #[test]
    fn interrupted_restore_never_reactivates_the_failed_installation() {process_case("interrupted-restore");}

    // H-A5: real legacy supervision retains work until exit, then the same command retires it and receives host acknowledgement.
    #[cfg(target_os="macos")]
    #[test]
    fn legacy_handoff_preserves_active_work_before_native_retirement() {process_case("legacy-handoff");}

    // H-A5 process instrument: real installer/cursor/receipt, isolated native-manager substitute.
    #[cfg(target_os="macos")]
    #[test]
    #[ignore="requires an owned TASKQ_INSTALL_PROCESS_CASE; native manager is substituted, not OS qualification"]
    fn installation_process_fixture() {
        let install=PathBuf::from(std::env::var_os("TASKQ_INSTALL_PROCESS_CASE").expect("owned installation fixture"));
        assert!(install.parent().unwrap().file_name().unwrap().to_string_lossy().starts_with("taskq-install-process-"));
        let base=install.parent().unwrap();
        assert_eq!(std::env::var_os("HOME"),Some(base.join("home").into_os_string()));
        assert_eq!(std::env::split_paths(&std::env::var_os("PATH").unwrap()).next(),Some(base.join("bin")));
        assert!(fs::symlink_metadata(base.join("bin/launchctl")).unwrap().file_type().is_file());
        let role=std::env::var("TASKQ_INSTALL_PROCESS_ROLE").unwrap();
        if role=="owned-work" {
            let native=coreq::process::identity(std::process::id()).unwrap().unwrap();
            coreq::write_durable(&install.join("owned-work.json"),serde_json::to_vec(&json!({"pid":native.pid,"started":native.started})).unwrap()).unwrap();
            let until=Instant::now()+std::time::Duration::from_secs(90);
            while !install.join("finish-owned-work").exists() {assert!(Instant::now()<until,"owned work expired");std::thread::sleep(std::time::Duration::from_millis(20));}
            return;
        }
        if role=="verify-exit" {
            let mut forced=vec![];
            for name in ["owned-work.json","controller-ready.json"] {
                let Some(value)=crate::data::json(&install.join(name)).unwrap() else {continue;};
                let native=coreq::process::Identity {pid:value["pid"].as_u64().unwrap().try_into().unwrap(),started:value["started"].as_str().unwrap().into()};
                assert_ne!(native.pid,std::process::id());
                let until=Instant::now()+std::time::Duration::from_secs(2);
                while Instant::now()<until&&coreq::process::identity(native.pid).unwrap().as_ref()==Some(&native) {std::thread::sleep(std::time::Duration::from_millis(20));}
                if coreq::process::identity(native.pid).unwrap().as_ref()==Some(&native) {
                    coreq::process::terminate(&native).unwrap();forced.push(native.pid);
                    let until=Instant::now()+std::time::Duration::from_secs(5);
                    while coreq::process::identity(native.pid).unwrap().as_ref()==Some(&native) {assert!(Instant::now()<until,"owned fixture process remains");std::thread::sleep(std::time::Duration::from_millis(20));}
                }
            }
            coreq::write_durable(&install.join("native-exit-readback.json"),serde_json::to_vec(&json!({"forced":forced,"all_absent":true})).unwrap()).unwrap();
            return;
        }
        if role=="legacy" {
            let root=PathBuf::from(std::env::var_os("TASKQ_INSTALL_PROCESS_ROOT").unwrap());
            let native=coreq::process::identity(std::process::id()).unwrap().unwrap();
            coreq::write_durable(&install.join("legacy-native.json"),serde_json::to_vec(&json!({"pid":native.pid,"started":native.started,"root":root,"project":crate::project_name(&root)})).unwrap()).unwrap();
            fs::write(install.join("legacy-active"),"").unwrap();
            let stopping=native.clone();let folder=install.clone();
            std::thread::spawn(move || {
                let until=Instant::now()+std::time::Duration::from_secs(90);
                while Instant::now()<until {
                    if let Some(request)=crate::data::json(&folder.join("stop-legacy.json")).unwrap()
                        &&request["pid"]==stopping.pid&&request["started"]==stopping.started {std::process::exit(0);}
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                std::process::exit(1);
            });
            let owner=crate::host_control::legacy_owner(&crate::platform::tool(&install)).unwrap().expect("owned legacy native binding");
            let mut cfg=crate::tests::cfg();cfg.root=root;cfg.install=install.clone();cfg.poll=1;cfg.limits.insert("codex".into(),1);
            let board=crate::board::Fake::default();
            use crate::board::Board;
            let task=board.create("Owned before handoff","",&["taskq".into()]).unwrap();
            let attempt=board.comment(task,&format!("taskq: spawn worker codex:invented {}",cfg.me)).unwrap();
            board.comment(task,&crate::core::entry("accepted",&[&attempt.to_string()],Some(attempt),"")).unwrap();
            let work=std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact","host::installation::tests::installation_process_fixture","--ignored","--nocapture"])
                .env("TASKQ_INSTALL_PROCESS_ROLE","owned-work").spawn().unwrap();
            let until=Instant::now()+std::time::Duration::from_secs(10);
            let child=loop {
                if let Some(value)=crate::data::json(&install.join("owned-work.json")).unwrap() {break value;}
                assert!(Instant::now()<until,"owned child did not become ready");std::thread::sleep(std::time::Duration::from_millis(10));
            };
            assert_eq!(child["pid"],work.id());
            fs::create_dir_all(crate::dot(&cfg.root)).unwrap();
            fs::write(crate::runtime::attempt_log(&cfg.root,"worker",task,attempt),"{\"type\":\"turn.started\"}\n").unwrap();
            crate::write_kids(&install,&cfg.root,&[crate::data::KidLine {task,attempt,execution:attempt,runtime:"codex".into(),process:coreq::process::Identity {pid:work.id(),started:child["started"].as_str().unwrap().into()},offset:0,job:None}]).unwrap();
            fs::write(install.join("service"),native.pid.to_string()).unwrap();
            let (_capture,_guard)=crate::profile::tests::collecting(&install);
            assert!(crate::service_run(&crate::Ctx {cfg,raw:json!({}),board:Box::new(board)},None,Some(&owner)).is_ok(),"legacy loop cannot return before its exact native retirement");
            return;
        }
        if role=="legacy-controller" {
            let errors=install.join("controller-error.txt");
            std::panic::set_hook(Box::new(move |info| {let _=fs::write(&errors,info.to_string());}));
            let id=std::env::var("TASKQ_INSTALL_PROCESS_REQUEST").unwrap();let p=pending(&install).unwrap();
            assert_eq!(p.id,id);let expected=context(&p).unwrap();
            let inherited=std::env::var("COREQ_TRACE_CONTEXT").unwrap();
            coreq::telemetry::capture::TraceContext::decode(&inherited).unwrap();
            assert_eq!(inherited.split(' ').nth(1),expected.split(' ').nth(1),"controller must continue the original command trace");
            let (_capture,_guard)=crate::profile::tests::collecting(&install);
            let context=coreq::telemetry::capture::TraceContext::decode(&inherited).unwrap();let _entered=context.span().entered();
            let native=coreq::process::identity(std::process::id()).unwrap().unwrap();
            coreq::write_durable(&install.join("controller-ready.json"),serde_json::to_vec(&json!({"id":id,"pid":native.pid,"started":native.started,"original_context":expected,"inherited_context":inherited})).unwrap()).unwrap();
            let until=Instant::now()+std::time::Duration::from_secs(30);
            while !install.join("allow-retirement").exists() {assert!(Instant::now()<until,"retirement not released");std::thread::sleep(std::time::Duration::from_millis(10));}
            complete_legacy(&install,&id).unwrap();
            coreq::write_durable(&install.join("controller-finished"),id.as_bytes()).unwrap();
            return;
        }
        if std::env::var("TASKQ_INSTALL_PROCESS_ROLE").as_deref()==Ok("host") {
            let _owner=owner(&install).unwrap().expect("one fixture host");
            let native=coreq::process::identity(std::process::id()).unwrap().unwrap();
            let shared=Arc::new(Mutex::new(Host::new(Policy::read(&install))));
            let instance=shared.lock().unwrap().instance.clone();
            let mut threads=BTreeMap::<PathBuf,std::thread::JoinHandle<Res<()>>>::new();
            let until=Instant::now()+std::time::Duration::from_secs(120);
            loop {
                publish_service(&install,&native,&instance).unwrap();
                shared.lock().unwrap().held=admission_reason(&install);
                if !install.join("hold-project-ack").exists() {
                    for root in projects(&install).unwrap_or_default() {
                        if threads.contains_key(&root) {continue;}
                        shared.lock().unwrap().attach(&root);
                        let host=shared.clone();let install=install.clone();
                        threads.insert(root.clone(),std::thread::spawn(move || {
                            enter(&root,&install);
                            let mut cfg=crate::tests::cfg();cfg.root=root;cfg.install=install;
                            cfg.poll=1;Policy::read(&cfg.install)?.apply(&mut cfg);
                            let board=crate::board::Fake::default();
                            crate::board::Board::create(&board,"Must remain ready","",&["taskq".into()])?;
                            crate::service_run(&crate::Ctx {cfg,raw:json!({}),board:Box::new(board)},Some(host),None)
                        }));
                    }
                }
                for thread in threads.values() {assert!(!thread.is_finished(),"project loop exited before fixture drain");}
                if install.join("exit-host").exists() {
                    shared.lock().unwrap().draining.extend(threads.keys().cloned());
                    for root in threads.keys() {crate::poke(root).unwrap();}
                    break;
                }
                assert!(Instant::now()<until,"fixture host expired");
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            for (root,thread) in threads {
                while !thread.is_finished() {assert!(Instant::now()<until,"project drain expired: {}",root.display());std::thread::sleep(std::time::Duration::from_millis(10));}
                assert_eq!(thread.join().unwrap().unwrap_err(),"project drained");
            }
            return;
        }
        let root=PathBuf::from(std::env::var_os("TASKQ_INSTALL_PROCESS_ROOT").expect("fixture root"));
        let (capture,guard)=crate::profile::tests::collecting(&install);
        let span=tracing::info_span!("host.install");let entered=span.enter();
        let request=pending(&install).ok().map(|p|p.id);
        let result=if std::env::var("TASKQ_INSTALL_PROCESS_ROLE").as_deref()==Ok("recover") {recover(&install)}else{install_project(&install,&root)};
        let operation_count=crate::resources::with(&install,|state|Ok(state.ops.len())).unwrap();
        fs::write(install.join("installer-result.json"),serde_json::to_vec(&json!({"ok":result.is_ok(),"result":result.as_ref().ok(),"error":result.as_ref().err(),"status":installation_status(&install,request.as_deref()),"operation_count":operation_count})).unwrap()).unwrap();
        capture.flush().unwrap();drop(entered);drop(span);drop(guard);drop(capture);
    }

    fn fixture()->(PathBuf,PathBuf,Vec<PathBuf>) {
        let base=std::env::temp_dir().join(format!("taskq-install-originals-{}",crate::runtime::uuid()));
        let install=base.join("install");let root=base.join("project");
        fs::create_dir_all(&install).unwrap();fs::create_dir_all(&root).unwrap();
        let paths=vec![install.join(crate::platform::tool(&install).exe_name()),install.join("current"),install.join("previous")];
        #[cfg(all(windows,target_env="gnu"))] let paths={let mut paths=paths;paths.push(install.join("WebView2Loader.dll"));paths};
        fs::write(&paths[0],[0,255,13,10,42]).unwrap();fs::write(&paths[1],"original-current\r\n").unwrap();
        #[cfg(unix)] {use std::os::unix::fs::PermissionsExt;fs::set_permissions(&paths[0],fs::Permissions::from_mode(0o751)).unwrap();}
        (install,root.canonicalize().unwrap(),paths)
    }
    // H-A5: partial activation and a failed retry restore bytes, metadata and original absence together.
    #[test]
    fn snapshot_restore_preserves_binary_bytes_metadata_and_absence() {
        let (install,root,paths)=fixture();
        let original:Vec<_>=paths.iter().map(|p|fs::read(p).ok()).collect();
        let hash=snapshot(&install,vec![root.clone()],paths.clone()).unwrap();
        let manifest_bytes=fs::read(manifest(&install)).unwrap();
        for path in &paths {fs::write(path,"partial-activation").unwrap();}
        let baseline:Baseline=serde_json::from_slice(&manifest_bytes).unwrap();
        restore_files(&install,&directory(&install),&baseline).unwrap();
        for (path,bytes) in paths.iter().zip(original) {assert_eq!(fs::read(path).ok(),bytes);}
        #[cfg(unix)] {use std::os::unix::fs::PermissionsExt;assert_eq!(fs::metadata(&paths[0]).unwrap().permissions().mode()&0o777,0o751);}
        assert_eq!(snapshot(&install,vec![root],paths.clone()).unwrap(),hash,"repeat/attach cannot overwrite topology originals");
        assert_eq!(fs::read(manifest(&install)).unwrap(),manifest_bytes);
        for label in ["later installation","new owner before intent"] {
            for path in &paths {fs::write(path,label).unwrap();}
            let folder=activation_directory(&install);
            let latest=capture_originals(&folder,vec![],paths.clone()).unwrap();
            for path in &paths {fs::write(path,"partial later activation").unwrap();}
            restore_files(&install,&folder,&latest).unwrap();
            for path in &paths {assert_eq!(fs::read_to_string(path).unwrap(),label);}
            assert_eq!(fs::read_dir(folder).unwrap().count(),paths.len(),"reuse one fixed copy set; no per-command archive");
            assert_eq!(fs::read(manifest(&install)).unwrap(),manifest_bytes,"latest-state recovery cannot replace first migration originals");
        }
        #[cfg(windows)] {
            let writable=fs::metadata(&paths[1]).unwrap().permissions();
            let mut readonly=writable.clone();readonly.set_readonly(true);fs::set_permissions(&paths[1],readonly).unwrap();
            let folder=activation_directory(&install);
            let latest=capture_originals(&folder,vec![],paths.clone()).unwrap();
            fs::set_permissions(&paths[1],writable.clone()).unwrap();
            fs::write(&paths[1],"partial readonly activation").unwrap();
            for _ in 0..2 {
                restore_files(&install,&folder,&latest).unwrap();
                assert_eq!(fs::read_to_string(&paths[1]).unwrap(),"new owner before intent");
                assert!(fs::metadata(&paths[1]).unwrap().permissions().readonly());
                assert_eq!(ownership(&paths[1]).unwrap(),latest.files[1].owner);
            }
            fs::set_permissions(&paths[1],writable.clone()).unwrap();
            fs::set_permissions(folder.join("1"),writable).unwrap();
        }
        fs::remove_dir_all(install.parent().unwrap()).unwrap();
    }
    // H-A5: corrupt evidence must be detected before the first restore effect, rather than halfway through.
    #[test]
    fn corrupt_snapshot_refuses_restore_before_any_active_write() {
        let (install,root,paths)=fixture();snapshot(&install,vec![root],paths.clone()).unwrap();
        let baseline:Baseline=serde_json::from_slice(&fs::read(manifest(&install)).unwrap()).unwrap();
        fs::write(directory(&install).join("1"),"corrupt").unwrap();
        fs::write(&paths[0],"current-launcher").unwrap();
        assert!(verify_originals(&install).unwrap_err().contains("checksum"));
        assert!(restore_files(&install,&directory(&install),&baseline).unwrap_err().contains("checksum"));
        assert_eq!(fs::read(&paths[0]).unwrap(),b"current-launcher");
        fs::remove_dir_all(install.parent().unwrap()).unwrap();
    }
    // H-A5: an interrupted pre-effect snapshot resumes, while an unknown file is preserved and refused.
    #[test]
    fn snapshot_rebuilds_only_its_uncommitted_slots() {
        let (install,root,paths)=fixture();fs::create_dir_all(directory(&install)).unwrap();
        fs::write(directory(&install).join("0"),"partial-copy").unwrap();
        fs::write(directory(&install).join("owner-note"),"preserve").unwrap();
        assert!(snapshot(&install,vec![root.clone()],paths.clone()).unwrap_err().contains("unknown"));
        assert_eq!(fs::read_to_string(directory(&install).join("owner-note")).unwrap(),"preserve");
        fs::remove_file(directory(&install).join("owner-note")).unwrap();
        snapshot(&install,vec![root],paths).unwrap();
        fs::remove_dir_all(install.parent().unwrap()).unwrap();
    }
    // H-A5: only this persisted registration and host generation can publish an installation acknowledgement.
    #[test]
    fn registered_cursor_holds_admission_and_binds_project_acknowledgement() {
        let (install,root,paths)=fixture();let baseline=snapshot(&install,vec![root.clone()],paths).unwrap();
        let mut p=Pending {schema:1,id:"invented-registration".into(),baseline,root:root.clone(),version:crate::platform::VERSION.into(),candidate:"a".repeat(64),
            original:None,next:format!("{}\n",root.display()),phase:"prepared".into(),restore:true,attach:false,
            owner:Some(InstallOwner {pid:7,started:"invented".into(),instance:"invented-host".into()}),legacy:None,activation:None,command:json!({"wire":1,"product":"taskq","version":"fixture","operation":"taskq.host.install","trace":format!("{}.1","a".repeat(32)),"span":format!("{}.2","a".repeat(32)),"parent":null,"started_ms":1}).to_string()};
        save(&install,&p).unwrap();assert!(registered_projects(&install).is_err());
        p.phase="registered".into();save(&install,&p).unwrap();
        assert!(registered_projects(&install).is_err(),"cursor cannot fabricate a registry commit");
        let lock=coreq::file_lock::try_lock(&install.join("projects.lock")).unwrap();
        ProjectEdit {install:install.clone(),original:None,next:p.next.clone(),removed:vec![],adding:true,_lock:lock}.commit().unwrap();
        assert_eq!(registered_projects(&install).unwrap(),Some(p.next.clone()));
        assert!(admission_reason(&install).is_some(),"membership is readable for recovery; launches remain held");
        assert_eq!(install_acknowledgement(&install,&root,"invented-host"),Some(p.id.clone()));
        assert_eq!(install_acknowledgement(&install,&root,"prior-host"),None);
        let prepare=install.join(format!("{INSTALL_PENDING}.pending"));
        fs::write(&prepare,"incomplete cursor publication").unwrap();
        assert!(registered_projects(&install).unwrap_err().contains("publication incomplete"));
        assert!(project_text(&install).is_err());
        assert_eq!(install_acknowledgement(&install,&root,"invented-host"),None);
        fs::remove_file(prepare).unwrap();
        p.version="4.4.0".into();p.attach=true;save(&install,&p).unwrap();
        assert_eq!(install_acknowledgement(&install,&root,"invented-host"),Some(p.id.clone()),"protocol-compatible reader accepts another installer version");
        assert!(ProjectEdit::new(&install,&root,true).is_err(),"another editor cannot consume a registered cursor");
        let retained=fs::read(install.join(INSTALL_PENDING)).unwrap();
        fs::write(manifest(&install),"changed-originals").unwrap();
        assert!(registered_projects(&install).unwrap_err().contains("originals changed"));
        assert_eq!(install_acknowledgement(&install,&root,"invented-host"),None);
        assert_eq!(fs::read(install.join(INSTALL_PENDING)).unwrap(),retained);
        fs::remove_dir_all(install.parent().unwrap()).unwrap();
    }

    // H-A5: the original observed receipt remains queryable after the installer/cursor disappears.
    #[test]
    fn installation_receipt_survives_cursor_removal_and_rejects_missing_phase_proof() {
        let (install,root,paths)=fixture();let baseline=snapshot(&install,vec![root.clone()],paths.clone()).unwrap();
        let (capture,guard)=crate::profile::tests::collecting(&install);
        let span=tracing::info_span!("host.install");let entered=span.enter();
        let command=CommandReceipt::current().unwrap().encode();
        let mut p=Pending {schema:1,id:"invented-install-receipt".into(),baseline,root:root.clone(),version:crate::platform::VERSION.into(),candidate:"a".repeat(64),
            original:None,next:format!("{}\n",root.display()),phase:"initial".into(),restore:false,attach:true,
            owner:Some(InstallOwner {pid:7,started:"invented".into(),instance:"invented-host".into()}),command,legacy:None,activation:None};
        assert!(receive_initial(&install,&mut p).is_err(),"no receipt before durable intent");
        save(&install,&p).unwrap();
        receive_initial(&install,&mut p).unwrap();assert_eq!(p.phase,"owned");
        p.phase="initial".into();save(&install,&p).unwrap();receive_initial(&install,&mut p).unwrap();
        assert_eq!(crate::resources::with(&install,|state|Ok(state.ops.len())).unwrap(),1,"received replay cannot orphan another command");
        p.activation=Some(capture_originals(&activation_directory(&install),vec![],paths).unwrap());
        assert!(retained(&install,&p).unwrap_err().contains("binding changed"),"latest restore metadata is part of the original command binding");
        p.activation=None;
        retain_reason(&install,&p,"native start response lost; original installation held").unwrap();
        assert_eq!(installation_status(&install,Some(&p.id))["reason"],"native start response lost; original installation held");
        for phase in ["applied","observed"] {record(&install,&p,phase).unwrap();}
        record(&install,&p,"applied").unwrap();
        assert_eq!(installation_status(&install,Some(&p.id))["state"],"observed","a late observer cannot regress the original outcome");
        fs::remove_file(install.join(INSTALL_PENDING)).unwrap();
        let receipt=installation_status(&install,Some(&p.id));
        assert_eq!(receipt["state"],"observed");assert!(receipt["observed_receipt"].is_string());
        assert!(receipt.get("reason").is_none(),"verified progress clears the old wait reason");
        let (first_id,first_command)=(p.id.clone(),p.command.clone());
        let second_span=tracing::info_span!("host.install");let second_enter=second_span.enter();
        p.id="invented-other-installation".into();p.command=CommandReceipt::current().unwrap().encode();
        record(&install,&p,"received").unwrap();
        crate::resources::with(&install,|state| {for id in [&first_id,&p.id] {state.ops.get_mut(id).unwrap().saved_at=Some(42);}Ok(())}).unwrap();
        assert_eq!(installation_status(&install,None)["state"],"unknown","equal observation clocks cannot attribute one command's result to another");
        assert_eq!(installation_status(&install,Some(&first_id))["state"],"observed");
        assert_eq!(installation_status(&install,Some(&p.id))["state"],"queued");
        drop(second_enter);drop(second_span);p.id=first_id;p.command=first_command;
        crate::resources::with(&install,|state| {state.ops.get_mut(&p.id).unwrap().receipt.as_object_mut().unwrap().remove("observed_receipt");Ok(())}).unwrap();
        assert_eq!(installation_status(&install,Some(&p.id))["state"],"unknown");
        capture.flush().unwrap();drop(entered);drop(span);drop(guard);drop(capture);
        fs::remove_dir_all(install.parent().unwrap()).unwrap();
    }

    // H-A5 model: caller/host observers race after registry commit; only the original command may complete.
    #[test]
    fn detached_install_observers_share_one_acknowledgement_and_cannot_create_a_command() {
        let (install,root,paths)=fixture();let baseline=snapshot(&install,vec![root.clone()],paths.clone()).unwrap();
        let original=fs::read(&paths[0]).unwrap();
        let (capture,guard)=crate::profile::tests::collecting(&install);
        let span=tracing::info_span!("host.install");let entered=span.enter();
        let native=coreq::process::identity(std::process::id()).unwrap().unwrap();
        publish_service(&install,&native,"invented-install-host").unwrap();
        let mut p=Pending {schema:1,id:"invented-detached-install".into(),baseline,root:root.clone(),version:crate::platform::VERSION.into(),candidate:"a".repeat(64),
            original:None,next:format!("{}\n",root.display()),phase:"registered".into(),restore:false,attach:true,
            owner:Some(InstallOwner {pid:native.pid,started:native.started,instance:"invented-install-host".into()}),command:CommandReceipt::current().unwrap().encode(),legacy:None,activation:None};
        save(&install,&p).unwrap();fs::write(install.join("projects"),&p.next).unwrap();
        fs::create_dir_all(crate::dot(&root)).unwrap();
        record(&install,&p,"received").unwrap();record(&install,&p,"applied").unwrap();
        let mut status=json!({"pid":p.owner.as_ref().unwrap().pid,"host_instance":"invented-install-host","host_install":"prior-registration"});
        let path=crate::dot(&root).join("status.json");fs::write(&path,status.to_string()).unwrap();
        assert!(!acknowledge(&install,&p).unwrap(),"same PID/generation cannot acknowledge a different registration");
        status["host_install"]=json!(p.id);status["host_instance"]=json!("prior-host");fs::write(&path,status.to_string()).unwrap();
        assert!(!acknowledge(&install,&p).unwrap(),"old project generation cannot acknowledge the installation");
        status["host_instance"]=json!("invented-install-host");fs::write(&path,status.to_string()).unwrap();
        let lock=coreq::file_lock::try_lock(&install.join("projects.lock")).unwrap();
        assert!(!acknowledge(&install,&p).unwrap(),"observer never waits on the registry owner");drop(lock);
        assert!(acknowledge(&install,&p).unwrap());assert!(acknowledge(&install,&p).unwrap(),"the other observer reads the retained outcome");
        assert!(install_bound(&install,&root,Some(&p.id),None).unwrap().contains("already observed"));
        assert!(!install.join(INSTALL_PENDING).exists());assert_eq!(fs::read(&paths[0]).unwrap(),original);
        assert_eq!(crate::resources::with(&install,|state|Ok(state.ops.len())).unwrap(),1);
        p.id="another-installation".into();save(&install,&p).unwrap();let marker=fs::read(install.join(INSTALL_PENDING)).unwrap();
        assert!(install_bound(&install,&root,Some("invented-detached-install"),None).unwrap_err().contains("another installation"));
        assert_eq!(fs::read(install.join(INSTALL_PENDING)).unwrap(),marker);
        capture.flush().unwrap();drop(entered);drop(span);drop(guard);drop(capture);
        fs::remove_dir_all(install.parent().unwrap()).unwrap();
    }

    // H-A5 model: a shaped cursor cannot authorize native Stop or installation completion without exact proof.
    #[test]
    fn legacy_cursor_requires_matching_drain_and_exit_proof() {
        let (install,root,paths)=fixture();let baseline=snapshot(&install,vec![root.clone()],paths).unwrap();
        let (capture,guard)=crate::profile::tests::collecting(&install);let span=tracing::info_span!("host.install");let entered=span.enter();
        let native=coreq::process::identity(std::process::id()).unwrap().unwrap();publish_service(&install,&native,"invented-host").unwrap();
        let owner=crate::host_control::LegacyOwner {root:root.clone(),project:"invented-legacy".into(),pid:native.pid,started:native.started.clone(),revision:"a".repeat(64),
            launcher:cfg!(windows).then(||json!({"pid":native.pid,"started":native.started.clone(),"instance":"{aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee}"}))};
        let mut p=Pending {schema:1,id:"invented-legacy-proof".into(),baseline,root:root.clone(),version:crate::platform::VERSION.into(),candidate:"a".repeat(64),original:None,next:format!("{}\n",root.display()),phase:"registered".into(),restore:false,attach:true,
            owner:Some(InstallOwner {pid:native.pid,started:native.started,instance:"invented-host".into()}),command:CommandReceipt::current().unwrap().encode(),legacy:Some(Legacy {owner:owner.clone(),phase:"ready".into()}),activation:None};
        fs::write(install.join("projects"),&p.next).unwrap();fs::create_dir_all(crate::dot(&root)).unwrap();
        fs::write(crate::dot(&root).join("status.json"),json!({"pid":native.pid,"host_instance":"invented-host","host_install":p.id}).to_string()).unwrap();
        record(&install,&p,"received").unwrap();record(&install,&p,"applied").unwrap();save(&install,&p).unwrap();let marker=fs::read(install.join(INSTALL_PENDING)).unwrap();
        assert!(legacy_latched(&install,&owner).unwrap_err().contains("proof missing"));
        assert!(dispatch_legacy(&install).is_err(),"unproven readiness cannot dispatch a native controller");
        assert!(complete_legacy(&install,&p.id).unwrap_err().contains("proof missing"));
        assert_eq!(fs::read(install.join(INSTALL_PENDING)).unwrap(),marker,"refusal leaves native intent intact");
        assert!(!acknowledge(&install,&p).unwrap(),"project presence cannot replace old unit retirement");
        record_legacy(&install,&p,"prepared").unwrap();assert!(legacy_latched(&install,&owner).unwrap());
        p.legacy.as_mut().unwrap().owner.started.push_str("-different");save(&install,&p).unwrap();assert!(!legacy_proven(&install,&p,"prepared").unwrap());
        p.legacy.as_mut().unwrap().owner=owner;p.legacy.as_mut().unwrap().phase="retired".into();save(&install,&p).unwrap();
        assert!(!acknowledge(&install,&p).unwrap(),"retired cursor without its native proof is unknown");
        record_legacy(&install,&p,"retired").unwrap();assert!(!acknowledge(&install,&p).unwrap(),"a still-live native identity cannot be completed by receipt shape");
        crate::resources::with(&install,|state|{state.ops.get_mut(&p.id).unwrap().receipt.as_object_mut().unwrap().remove("applied_receipt");Ok(())}).unwrap();
        assert!(record_legacy(&install,&p,"prepared").unwrap_err().contains("phase proof missing"),"corrupt required receipt cannot be silently rebuilt");
        capture.flush().unwrap();drop(entered);drop(span);drop(guard);drop(capture);fs::remove_dir_all(install.parent().unwrap()).unwrap();
    }
}
