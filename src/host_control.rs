//! Short native controller outside the host unit; task/worker ownership stays with the host.
use crate::{Res,lifecycle};
use coreq::process::{Identity,Failure};
use std::{fs,path::{Path,PathBuf},process::Command,time::{Duration,Instant}};

pub struct Control {pub id:String,worker:Option<std::thread::JoinHandle<Res<()>>>}
impl Control {
    pub fn poll(&mut self)->Option<Res<()>> {
        if !self.worker.as_ref().is_some_and(|worker|worker.is_finished()) {return None;}
        Some(self.worker.take().unwrap().join().unwrap_or_else(|_|Err("Stop controller owner panicked; outcome unknown".into())))
    }
    pub fn wait(mut self)->Res<()> {
        self.worker.take().map_or(Ok(()),|worker|worker.join().unwrap_or_else(|_|Err("Stop controller owner panicked; outcome unknown".into())))
    }
}

fn definition(tool:&coreq::Tool)->Res<PathBuf> {
    if cfg!(windows) {return Ok(tool.install.join(format!("{}.xml",tool.title)));}
    let home=std::env::var_os("HOME").filter(|p|!p.is_empty()).ok_or("native user home unknown")?;
    Ok(PathBuf::from(home).join(if cfg!(target_os="macos") {format!("Library/LaunchAgents/com.{}.service.plist",tool.name)}else{format!(".config/systemd/user/{}.service",tool.name)}))
}
pub fn revision(install:&Path)->Res<String> {revision_for(&crate::platform::tool(install))}
fn revision_for(tool:&coreq::Tool)->Res<String> {
    let path=definition(tool)?;let metadata=fs::symlink_metadata(&path).map_err(|e|format!("host unit definition {} unavailable: {e}",path.display()))?;
    if !metadata.is_file()||metadata.len()>65536 {return Err("host unit definition unsupported; kept".into());}
    fs::read(path).map(|bytes|ring::digest::digest(&ring::digest::SHA256,&bytes).as_ref().iter().map(|b|format!("{b:02x}")).collect()).map_err(|e|e.to_string())
}
fn output(name:&'static str,command:&mut Command)->Res<std::process::Output> {
    match coreq::process::output(name,command,Duration::from_secs(30)) {
        Ok(output)=>Ok(output),Err(Failure::Exit(output))=>Err(format!("{name}: {}: {}",output.status,String::from_utf8_lossy(&output.stderr))),Err(error)=>Err(format!("{name}: {error}")),
    }
}
#[cfg(windows)]
fn quote(value:&str)->String {format!("'{}'",value.replace('\'',"''"))}
#[cfg(windows)]
fn powershell(code:&str)->Command {
    let bytes:Vec<u8>=code.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let mut command=Command::new("powershell.exe");command.args(["-NoProfile","-NonInteractive","-WindowStyle","Hidden","-EncodedCommand",&crate::tray::base64(&bytes)]);command
}

#[cfg(windows)]
fn controller_args(install:&Path,pending:&lifecycle::Pending,exe:&Path)->Res<String> {
    let context=pending.context()?;
    let bootstrap=format!("$ErrorActionPreference='Stop';$env:TASKQ_INSTALL={};$env:COREQ_TRACE_CONTEXT={};Remove-Item Env:TASKQ_ROOT,Env:TASKQ_ATTEMPT,Env:TASKQ_TASK -ErrorAction SilentlyContinue;& {} service complete-stop --request {};exit $LASTEXITCODE",quote(&install.to_string_lossy()),quote(&context),quote(&exe.to_string_lossy()),quote(pending.id()));
    let invocation=powershell(&bootstrap);Ok(invocation.get_args().map(|a|a.to_string_lossy()).collect::<Vec<_>>().join(" "))
}

/// Dispatch only a prepared, immutable command. Repeated dispatch is serialized again by projects.lock.
pub fn dispatch(install:&Path,pending:&lifecycle::Pending)->Res<Control> {
    if std::env::var_os("TASKQ_ATTEMPT").is_some() {return Err("a worker cannot dispatch host Stop".into());}
    if pending.action()!=lifecycle::Action::Stop||!pending.prepared() {return Err("Stop has no qualified prepared intent".into());}
    let exe=std::env::current_exe().map_err(|e|e.to_string())?;
    #[cfg(not(windows))] let context=pending.context()?;
    #[cfg(target_os="macos")] {
        use std::os::unix::process::CommandExt;
        let mut command=Command::new(exe);command.args(["service","complete-stop","--request",pending.id()]).current_dir(install)
            .env("TASKQ_INSTALL",install).env("COREQ_TRACE_CONTEXT",&context).env_remove("TASKQ_ROOT").env_remove("TASKQ_ATTEMPT").env_remove("TASKQ_TASK").process_group(0);
        let worker=std::thread::Builder::new().name("taskq-stop-controller".into()).spawn(coreq::telemetry::capture::bind(move || {
            let context=coreq::telemetry::capture::TraceContext::decode(&context)?;let _context=context.span().entered();
            coreq::process::output("host.stop.dispatch",&mut command,Duration::from_secs(120)).map(|_|()).map_err(|e|e.to_string())
        })).map_err(|e|format!("Stop controller not started: {e}"))?;
        return Ok(Control {id:pending.id().into(),worker:Some(worker)});
    }
    #[cfg(target_os="linux")] {
        output("host.stop.dispatch",Command::new("systemd-run").args(["--user","--quiet","--collect","--property=RuntimeMaxSec=120"])
            .arg(format!("--unit=taskq-stop-{}",pending.id())).arg(format!("--working-directory={}",install.display()))
            .arg(format!("--setenv=TASKQ_INSTALL={}",install.display())).arg(format!("--setenv=COREQ_TRACE_CONTEXT={context}"))
            .arg(exe).args(["service","complete-stop","--request",pending.id()]))?;
    }
    #[cfg(windows)] {
        let args=controller_args(install,pending,&exe)?;
        let code=format!(r#"$ErrorActionPreference='Stop';$ProgressPreference='SilentlyContinue'
$s=New-Object -ComObject 'Schedule.Service';$s.Connect();$r=$s.GetFolder('\');$name={name};$path=(Get-Command powershell.exe -ErrorAction Stop).Source;$args={args};$cwd={cwd}
try{{$t=$r.GetTask($name)}}catch{{if(($_.Exception.GetBaseException()).HResult -ne -2147024894){{throw}};$t=$null}}
if($t){{$a=$t.Definition.Actions.Item(1);$identity=[Security.Principal.WindowsIdentity]::GetCurrent();if($t.Definition.Principal.LogonType -ne 3 -or $t.Definition.Principal.RunLevel -ne 0 -or ($t.Definition.Principal.UserId -ne $identity.Name -and $t.Definition.Principal.UserId -ne $identity.User.Value) -or $t.Definition.Triggers.Count -ne 0 -or $t.Definition.Settings.ExecutionTimeLimit -ne 'PT2M' -or $t.Definition.Actions.Count -ne 1 -or $a.Path -ne $path -or $a.Arguments -ne $args -or $a.WorkingDirectory -ne $cwd){{throw 'existing controller definition differs'}}}}
else{{$d=$s.NewTask(0);$d.Principal.UserId=[Security.Principal.WindowsIdentity]::GetCurrent().Name;$d.Principal.LogonType=3;$d.Principal.RunLevel=0;$d.Settings.ExecutionTimeLimit='PT2M';$d.Settings.MultipleInstances=2;$d.Settings.DisallowStartIfOnBatteries=$false;$d.Settings.StopIfGoingOnBatteries=$false;$a=$d.Actions.Create(0);$a.Path=$path;$a.Arguments=$args;$a.WorkingDirectory=$cwd;$t=$r.RegisterTaskDefinition($name,$d,2,$null,$null,3)}}
if($t.State -ne 4){{$null=$t.Run($null)}}"#,name=quote(&format!("TaskQ-Stop-{}",pending.id())),args=quote(&args),cwd=quote(&install.to_string_lossy()));
        output("host.stop.dispatch",&mut powershell(&code))?;
    }
    #[cfg(not(any(target_os="macos",target_os="linux",windows)))] return Err("native Stop controller unsupported".into());
    #[cfg(not(target_os="macos"))] Ok(Control {id:pending.id().into(),worker:None})
}

/// Never signal a replacement or use daemon exit as evidence about workers.
pub fn stop(install:&Path,id:&str,expected:&Identity,unit_revision:&str)->Res<()> {stop_unit(&crate::platform::tool(install),id,expected,unit_revision)}
fn stop_unit(tool:&coreq::Tool,id:&str,expected:&Identity,unit_revision:&str)->Res<()> {
    #[cfg(not(windows))] let _=id;
    let install=&tool.install;
    if revision_for(tool)?!=unit_revision {return Err("prepared host unit changed; Stop outcome unknown".into());}
    if coreq::process::identity(expected.pid).map_err(|e|e.to_string())?.is_some_and(|native|&native!=expected) {return Err("native host incarnation changed; Stop refused".into());}
    #[cfg(not(windows))] let unit=coreq::service::unit(tool,install,"");
    #[cfg(not(windows))] if unit.path!=definition(tool)? {return Err("native unit scope differs; Stop refused".into());}
    #[cfg(target_os="macos")] {
        let target=unit.stop.first().and_then(|c|c.last()).ok_or("native Stop target missing")?;
        let live=coreq::process::identity(expected.pid).map_err(|e|e.to_string())?;
        if live.is_some() {
            if live.as_ref()!=Some(expected) {return Err("native host incarnation changed; Stop refused".into());}
            let query=output("host.stop.owner",Command::new("launchctl").args(["print",target]))?;
            let text=String::from_utf8(query.stdout).map_err(|e|e.to_string())?;
            if !text.lines().any(|line|line.trim()==format!("pid = {}",expected.pid)) {return Err("launchd owner differs; Stop refused".into());}
            coreq::service::run_all(&unit.stop,false)?;
        } else {
            // A lost response can be reconciled only by a successful native-domain inventory.
            let (domain,label)=target.rsplit_once('/').ok_or("launchd domain missing")?;
            let query=output("host.stop.readback",Command::new("launchctl").args(["print",domain]))?;
            if String::from_utf8(query.stdout).map_err(|e|e.to_string())?.lines().any(|l|l.split_whitespace().any(|f|f.trim_matches('"')==label)) {return Err("host unit remains loaded with unknown owner; Stop outcome unknown".into());}
        }
    }
    #[cfg(target_os="linux")] {
        let name=unit.stop.first().and_then(|command|command.last()).ok_or("systemd unit target missing")?.as_str();
        let query=output("host.stop.owner",Command::new("systemctl").args(["--user","show",name,"--property=MainPID","--property=LoadState","--property=ActiveState","--no-pager"]))?;
        let text=String::from_utf8(query.stdout).map_err(|e|e.to_string())?;
        let pid=text.lines().find_map(|l|l.strip_prefix("MainPID=")).and_then(|p|p.parse::<u32>().ok()).ok_or("systemd owner unknown")?;
        if !text.lines().any(|l|l=="LoadState=loaded")||pid!=0&&pid!=expected.pid {return Err("systemd owner differs; Stop refused".into());}
        if pid==0&&coreq::process::identity(expected.pid).map_err(|e|e.to_string())?.as_ref()==Some(expected) {return Err("live daemon has no attributable systemd owner; Stop refused".into());}
        if pid!=0&&coreq::process::identity(pid).map_err(|e|e.to_string())?.as_ref()!=Some(expected) {return Err("systemd native incarnation differs; Stop refused".into());}
        coreq::service::run_all(&unit.stop,false)?;
        let query=output("host.stop.readback",Command::new("systemctl").args(["--user","show",name,"--property=MainPID","--property=ActiveState","--no-pager"]))?;
        let text=String::from_utf8(query.stdout).map_err(|e|e.to_string())?;
        if !text.lines().any(|l|l=="MainPID=0")||!text.lines().any(|l|l=="ActiveState=inactive") {return Err("systemd Stop not observed".into());}
    }
    #[cfg(windows)] {
        let setup=format!(r#"$ErrorActionPreference='Stop';$ProgressPreference='SilentlyContinue'
$s=New-Object -ComObject 'Schedule.Service';$s.Connect();$r=$s.GetFolder('\');$t=$r.GetTask({name});$a=$t.Definition.Actions.Item(1);[xml]$saved=Get-Content -Raw -LiteralPath {definition};$exec=$saved.Task.Actions.Exec
if($t.Definition.Actions.Count -ne 1 -or @($exec).Count -ne 1 -or $a.Path -ne [string]$exec.Command -or $a.Arguments -ne [string]$exec.Arguments -or $a.WorkingDirectory -ne [string]$exec.WorkingDirectory -or $a.WorkingDirectory -ne {cwd}){{throw 'scheduled host scope differs'}}
"#,name=quote(tool.title),definition=quote(&definition(tool)?.to_string_lossy()),cwd=quote(&install.to_string_lossy()));
        let query=format!(r#"{setup}
$instances=$t.GetInstances(0);if($instances.Count -gt 1){{throw 'multiple native host instances'}}
if($instances.Count -eq 0){{'null';exit 0}}
$instance=$instances.Item(1);$engine=$instance.EnginePID;$daemon=Get-CimInstance Win32_Process -Filter 'ProcessId = {pid}' -ErrorAction Stop
$created=(Get-Process -Id $engine -ErrorAction Stop).StartTime.ToFileTimeUtc()
if(!$daemon -or $daemon.ParentProcessId -ne $engine -or (Get-Process -Id {pid}).StartTime.ToFileTimeUtc().ToString() -ne {started} -or $created -gt [long]{started}){{throw 'scheduled host owner differs'}}
[pscustomobject]@{{pid=$engine;started=$created.ToString();instance=$instance.InstanceGuid}}|ConvertTo-Json -Compress"#,pid=expected.pid,started=quote(&expected.started));
        let observed=output("host.stop.owner",&mut powershell(&query))?;
        let observed:serde_json::Value=serde_json::from_slice(&observed.stdout).map_err(|e|format!("native launcher observation invalid: {e}"))?;
        let mut scope=lifecycle::saved_launcher(install,id,expected)?;
        if !observed.is_null() {lifecycle::save_launcher(install,id,expected,&observed)?;scope=Some(observed);}
        if let Some(scope)=scope {
            let launcher=Identity {pid:scope["pid"].as_u64().ok_or("launcher pid unknown")? as u32,started:scope["started"].as_str().ok_or("launcher creation unknown")?.into()};
            let stop=format!(r#"{setup}
$instances=$t.GetInstances(0);if($instances.Count -gt 1){{throw 'multiple native host instances'}}
if($instances.Count -eq 1){{$instance=$instances.Item(1);if($instance.InstanceGuid -ne {instance} -or $instance.EnginePID -ne {pid} -or (Get-Process -Id {pid} -ErrorAction Stop).StartTime.ToFileTimeUtc().ToString() -ne {started}){{throw 'native launcher incarnation differs'}};$instance.Stop()}}
$until=[DateTime]::UtcNow.AddSeconds(10);while($t.GetInstances(0).Count -ne 0 -and [DateTime]::UtcNow -lt $until){{Start-Sleep -Milliseconds 20}}
if($t.GetInstances(0).Count -ne 0){{throw 'scheduled host stop not observed'}}"#,instance=quote(scope["instance"].as_str().ok_or("launcher instance unknown")?),pid=launcher.pid,started=quote(&launcher.started));
            output("host.stop.native",&mut powershell(&stop))?;
            terminate_owned(&launcher)?;
            wait_exit(&launcher)?;
            terminate_owned(expected)?;
        } else if coreq::process::identity(expected.pid).map_err(|e|e.to_string())?.as_ref()==Some(expected) {
            return Err("live daemon has no attributable scheduler launcher; Stop refused".into());
        }
    }
    wait_exit(expected)?;
    if revision_for(tool)?!=unit_revision {return Err("host unit definition changed during Stop; outcome unknown".into());}
    Ok(())
}

#[cfg(windows)]
fn terminate_owned(expected:&Identity)->Res<()> {
    if coreq::process::identity(expected.pid).map_err(|e|e.to_string())?.as_ref()!=Some(expected) {return Ok(());}
    match coreq::process::terminate(expected) {
        Ok(())=>Ok(()),Err(error)=>if coreq::process::identity(expected.pid).map_err(|e|e.to_string())?.as_ref()!=Some(expected) {Ok(())}else{Err(error.to_string())},
    }
}
fn wait_exit(expected:&Identity)->Res<()> {
    let until=Instant::now()+Duration::from_secs(10);
    loop {
        if coreq::process::identity(expected.pid).map_err(|e|e.to_string())?.as_ref()!=Some(expected) {break;}
        if Instant::now()>=until {return Err("native host exit unconfirmed; inspect service status".into());}
        std::thread::sleep(Duration::from_millis(20));
    }
    Ok(())
}

pub fn retire_controller(install:&Path,id:&str)->Res<()> {
    #[cfg(windows)] {
        let pending=lifecycle::pending(install)?.filter(|p|p.id()==id&&p.action()==lifecycle::Action::Stop).ok_or("Stop controller binding unavailable")?;
        let args=controller_args(install,&pending,&std::env::current_exe().map_err(|e|e.to_string())?)?;
        let code=format!(r#"$ErrorActionPreference='Stop';$s=New-Object -ComObject 'Schedule.Service';$s.Connect();$r=$s.GetFolder('\');$name={name}
try{{$t=$r.GetTask($name)}}catch{{if(($_.Exception.GetBaseException()).HResult -ne -2147024894){{throw}};$t=$null}}
if($t){{$a=$t.Definition.Actions.Item(1);if($t.Definition.Actions.Count -ne 1 -or $a.Path -ne (Get-Command powershell.exe -ErrorAction Stop).Source -or $a.Arguments -ne {args} -or $a.WorkingDirectory -ne {cwd} -or $t.Definition.Principal.RunLevel -ne 0 -or $t.Definition.Triggers.Count -ne 0){{throw 'controller ownership differs; definition kept'}};$r.DeleteTask($name,0)}}
try{{$null=$r.GetTask($name);throw 'controller definition remains'}}catch{{if(($_.Exception.GetBaseException()).HResult -ne -2147024894){{throw}}}}"#,name=quote(&format!("TaskQ-Stop-{id}")),args=quote(&args),cwd=quote(&install.to_string_lossy()));
        output("host.stop.controller.retire",&mut powershell(&code))?;
    }
    #[cfg(not(windows))] let _=(install,id);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value,json};

    // H-A5: run the real adapter only against a unique, externally owned temporary unit.
    #[test]
    #[ignore="requires TASKQ_NATIVE_STOP_CASE and an owned temporary native unit; no production unit"]
    fn native_unit_stop() {
        let install=PathBuf::from(std::env::var_os("TASKQ_NATIVE_STOP_CASE").expect("owned fixture root required"));
        let nonce=install.file_name().unwrap().to_str().unwrap().strip_prefix("taskq-stop-native-").unwrap();
        assert_eq!(nonce.len(),32);assert!(nonce.bytes().all(|b|b.is_ascii_hexdigit()));
        let name=Box::leak(format!("taskq-stop-probe-{nonce}").into_boxed_str());
        let title=Box::leak(format!("TaskQ-Stop-Probe-{nonce}").into_boxed_str());
        let tool=coreq::Tool {name,title,repo:"fixture/unused",version:"fixture",install:install.clone(),fault:None};
        let fixture:Value=serde_json::from_slice(&fs::read(install.join("native.json")).unwrap()).unwrap();
        let native=coreq::process::identity(fixture["pid"].as_u64().unwrap().try_into().unwrap()).unwrap().unwrap();
        if let Some(started)=fixture["started"].as_str() {assert_eq!(native.started,started);}
        let revision=revision_for(&tool).unwrap();let definition=definition(&tool).unwrap();let original=fs::read(&definition).unwrap();
        let (capture,guard)=crate::profile::tests::collecting(&install);
        let command=coreq::telemetry::capture::CommandReceipt::decode(&json!({"wire":1,"product":"taskq","version":"fixture","operation":"taskq.host.stop.request","trace":format!("{}.1","a".repeat(32)),"span":format!("{}.2","a".repeat(32)),"parent":null,"started_ms":1}).to_string()).unwrap();
        crate::resources::with(&install,|state| {
            state.ops.insert("native-stop".into(),crate::resources::Op {
                bound:json!({"local_stop":{"request":{"id":"native-stop","instance":"original","roots":[],"issued_at":1,"expires_at":301,"action":"stop"},"command":command.encode()}}),
                receipt:json!({"id":"native-stop","state":"applied","received_receipt":command.event_id("received")?,"applied_receipt":command.event_id("applied")?,"native_pid":native.pid,"native_started":native.started,"unit_revision":revision}),
                expires_at:301,saved_at:Some(1),writing:false,sent:true,
            });Ok(())
        }).unwrap();
        assert!(stop_unit(&tool,"native-stop",&native,&"0".repeat(64)).is_err());
        let mut foreign=native.clone();foreign.started.push_str("-foreign");
        assert!(stop_unit(&tool,"native-stop",&foreign,&revision).is_err());
        assert_eq!(coreq::process::identity(native.pid).unwrap().as_ref(),Some(&native),"refused requests must preserve the original daemon");
        stop_unit(&tool,"native-stop",&native,&revision).unwrap();
        assert_ne!(coreq::process::identity(native.pid).unwrap().as_ref(),Some(&native));
        stop_unit(&tool,"native-stop",&native,&revision).unwrap();
        assert_eq!(fs::read(&definition).unwrap(),original,"Stop retains the native definition");
        capture.flush().unwrap();drop(guard);drop(capture);
        fs::write(install.join("adapter-result.json"),json!({"unit":name,"native_pid":native.pid,"native_started":native.started,"unit_revision":revision,"exit":true,"replay":true,"definition_retained":true}).to_string()).unwrap();
    }
}
