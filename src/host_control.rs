//! Short native controller outside the host unit; task/worker ownership stays with the host.
use crate::{Res,lifecycle};
use coreq::process::{Identity,Failure};
use std::{fs,path::{Path,PathBuf},process::Command,time::{Duration,Instant}};
#[path="legacy.rs"]
mod legacy;
pub use legacy::{Owner as LegacyOwner,owner as legacy_owner};

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

pub(crate) fn definition(tool:&coreq::Tool)->Res<PathBuf> {
    if cfg!(windows) {return Ok(tool.install.join(format!("{}.xml",tool.title)));}
    let home=std::env::var_os("HOME").filter(|p|!p.is_empty()).ok_or("native user home unknown")?;
    Ok(PathBuf::from(home).join(if cfg!(target_os="macos") {format!("Library/LaunchAgents/com.{}.service.plist",tool.name)}else{format!(".config/systemd/user/{}.service",tool.name)}))
}
pub fn revision(install:&Path)->Res<String> {revision_for(&crate::platform::tool(install))}

#[cfg(target_os="macos")]
fn launchd_bindings<'a>(text:&'a str,prefix:&str)->Vec<(u32,&'a str)> {
    text.lines().filter_map(|line| {
        let pid=line.split_whitespace().next()?.parse().ok()?;
        let label=line.get(line.find(prefix)?..)?.trim().trim_end_matches('"');
        Some((pid,label))
    }).collect()
}

/// Loaded legacy units remain an admission barrier even if their saved definition disappeared.
pub(crate) fn legacy_inventory(tool:&coreq::Tool,controllers:&[String])->Res<Vec<String>> {
    #[cfg(target_os="macos")] let (text,prefix,host,suffix)={
        let domain=format!("gui/{}",unsafe {libc::geteuid()});
        let native=output("host.legacy.inventory",Command::new("launchctl").args(["print",&domain]))?;
        (String::from_utf8(native.stdout).map_err(|e|e.to_string())?,format!("com.{}.",tool.name),format!("com.{}.service",tool.name),"")
    };
    #[cfg(target_os="linux")] let (text,prefix,host,suffix)={
        let native=output("host.legacy.inventory",Command::new("systemctl").args(["--user","list-units","--all","--plain","--no-legend","--no-pager"]))?;
        (String::from_utf8(native.stdout).map_err(|e|e.to_string())?,format!("{}-",tool.name),format!("{}.service",tool.name),".service")
    };
    #[cfg(windows)] let (text,prefix,host,suffix)={
        let code="$ErrorActionPreference='Stop';[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false);$s=New-Object -ComObject 'Schedule.Service';$s.Connect();foreach($t in $s.GetFolder('\\').GetTasks(1)){$t.Name}";
        let native=output("host.legacy.inventory",&mut powershell(code))?;
        (String::from_utf8(native.stdout).map_err(|e|e.to_string())?,format!("{}-",tool.title),tool.title.to_string(),"")
    };
    #[cfg(any(target_os="macos",target_os="linux",windows))] {
        let mut names=std::collections::BTreeSet::new();
        for line in text.lines() {
            #[cfg(target_os="macos")] let fields:Vec<_>=launchd_bindings(line,&prefix).into_iter().map(|(_,label)|label).collect();
            #[cfg(target_os="linux")] let fields:Vec<_>=line.split_whitespace().take(1).collect();
            #[cfg(windows)] let fields=vec![line.trim()];
            for field in fields {
                let name=field.trim_matches('"');
                if name==host||!name.starts_with(&prefix)||!name.ends_with(suffix) {continue;}
                if controllers.iter().any(|id|name==format!("{}-Stop-{id}",tool.title)||name==format!("{}-stop-{id}.service",tool.name)) {continue;}
                names.insert(name.to_string());
            }
        }
        return Ok(names.into_iter().collect());
    }
    #[cfg(not(any(target_os="macos",target_os="linux",windows)))] Err("native legacy inventory unsupported".into())
}

/// Read native scope before install attaches a root; a daemon projection alone cannot authorize a unit edit.
#[derive(Clone,Copy,Debug,PartialEq)]
pub enum InstallState {Absent,Saved,Loaded}

pub fn install_state(tool:&coreq::Tool,owner:Option<&Identity>)->Res<InstallState> {
    let saved=match fs::symlink_metadata(definition(tool)?) {
        Ok(_)=>{revision_for(tool)?;true},
        Err(e) if e.kind()==std::io::ErrorKind::NotFound=>false,
        Err(e)=>return Err(format!("host definition unknown: {e}")),
    };
    if saved {verify_saved_install_scope(tool)?;}
    if native_absent(tool)? {
        if owner.is_some() {return Err("live host has no native unit; installation refused".into());}
        return Ok(if saved {InstallState::Saved}else{InstallState::Absent});
    }
    if !saved {return Err("native host has no saved definition; installation refused".into());}
    #[cfg(target_os="macos")] {
        let unit=coreq::service::unit(tool,&tool.install,"");
        let target=unit.stop.first().and_then(|command|command.last()).ok_or("native host target missing")?;
        let exe=tool.install.join(tool.exe_name());
        let native=output("host.install.owner",Command::new("launchctl").args(["print",target]))?;
        let text=String::from_utf8(native.stdout).map_err(|e|e.to_string())?;
        let field=|key:&str|text.lines().find_map(|line|line.trim().strip_prefix(key));
        if field("program = ")!=exe.to_str()||field("working directory = ")!=tool.install.to_str()
            ||field("path = ")!=unit.path.to_str() {return Err("loaded host unit scope differs; kept".into());}
        let arguments:Vec<_>=text.lines().skip_while(|line|line.trim()!="arguments = {").skip(1).take_while(|line|line.trim()!="}").map(str::trim).collect();
        if arguments!=[exe.to_str().ok_or("host executable is not UTF-8")?,"service","run"] {return Err("loaded host arguments differ; kept".into());}
        let pid=field("pid = ").map(str::parse::<u32>).transpose().map_err(|e|e.to_string())?.unwrap_or(0);
        verify_install_pid(pid,owner)?;
    }
    #[cfg(target_os="linux")] {
        let unit=coreq::service::unit(tool,&tool.install,"");
        let exe=tool.install.join(tool.exe_name());
        let query=output("host.install.owner",Command::new("systemctl").args(["--user","show",&format!("{}.service",tool.name),"--property=MainPID","--property=ExecStart","--property=WorkingDirectory","--property=FragmentPath","--no-pager"]))?;
        let text=String::from_utf8(query.stdout).map_err(|e|e.to_string())?;
        let field=|key:&str|text.lines().find_map(|line|line.strip_prefix(key));
        if field("WorkingDirectory=")!=tool.install.to_str()||field("FragmentPath=")!=unit.path.to_str()
            ||!field("ExecStart=").is_some_and(|value|value.starts_with(&format!("{{ path={} ; argv[]={} service run ;",exe.display(),exe.display()))) {return Err("loaded host unit scope differs; kept".into());}
        let pid=field("MainPID=").ok_or("native host pid unknown")?.parse::<u32>().map_err(|e|e.to_string())?;
        verify_install_pid(pid,owner)?;
    }
    #[cfg(windows)] {
        let code=format!(r#"$ErrorActionPreference='Stop';$s=New-Object -ComObject 'Schedule.Service';$s.Connect();$t=$s.GetFolder('\').GetTask({name});$d=$t.Definition;[xml]$xml=$t.Xml;[xml]$saved=Get-Content -Raw -LiteralPath {definition};$sid=[Security.Principal.WindowsIdentity]::GetCurrent().User.Value
$a=$d.Actions.Item(1);$e=$saved.Task.Actions.Exec
$principal=[string]$xml.Task.Principals.Principal.UserId
$owner=if($principal.StartsWith('S-1-',[StringComparison]::OrdinalIgnoreCase)){{[Security.Principal.SecurityIdentifier]::new($principal).Value}}else{{[Security.Principal.NTAccount]::new($principal).Translate([Security.Principal.SecurityIdentifier]).Value}}
if($d.Principal.LogonType -ne 3 -or $d.Principal.RunLevel -ne 0 -or $owner -ne $sid -or $d.Actions.Count -ne 1 -or @($e).Count -ne 1 -or !(Test-CoreQPath $a.Path {exe}) -or $a.Arguments -ne 'service run' -or !(Test-CoreQPath $a.WorkingDirectory {cwd}) -or !(Test-CoreQPath $a.Path ([string]$e.Command)) -or $a.Arguments -ne [string]$e.Arguments -or !(Test-CoreQPath $a.WorkingDirectory ([string]$e.WorkingDirectory))){{throw 'host task scope differs'}}
if($d.Triggers.Count -ne 1 -or $d.Triggers.Item(1).Type -ne 9 -or !$d.Triggers.Item(1).Enabled){{throw 'host task trigger differs'}}
$principal=[string]$d.Triggers.Item(1).UserId
$trigger=if($principal.StartsWith('S-1-',[StringComparison]::OrdinalIgnoreCase)){{[Security.Principal.SecurityIdentifier]::new($principal).Value}}else{{[Security.Principal.NTAccount]::new($principal).Translate([Security.Principal.SecurityIdentifier]).Value}}
if($trigger -ne $sid){{throw 'host task trigger owner differs'}}
if({pid} -ne 0){{$acl=[Security.AccessControl.RawSecurityDescriptor]::new($t.GetSecurityDescriptor(4));$aces=@($acl.DiscretionaryAcl)
if(($acl.ControlFlags -band [Security.AccessControl.ControlFlags]::DiscretionaryAclProtected) -eq 0 -or $aces.Count -ne 2){{throw 'host task requires explicit idle installer repair'}}
foreach($who in @($sid,'S-1-5-18')){{$rights=@($aces|Where-Object {{$_.AceQualifier -eq 'AccessAllowed' -and $_.SecurityIdentifier.Value -eq $who -and (($_.AccessMask -band 0x10000000) -ne 0 -or ($_.AccessMask -band 0x1f01ff) -eq 0x1f01ff)}});if($rights.Count -ne 1){{throw 'host task management rights not observed; drain before installer repair'}}}}}}
$instances=@(Get-TaskQInstances $s $t);if($instances.Count -gt 1){{throw 'multiple native host instances'}}
if($instances.Count -eq 0){{if({pid} -ne 0){{throw 'live host has no scheduler instance'}};exit 0}}
if({pid} -eq 0){{throw 'scheduler instance has no attributable host'}}
$engine=$instances[0].EnginePID;$daemon=Get-CimInstance Win32_Process -Filter 'ProcessId = {pid}'
if(!$daemon -or $daemon.ParentProcessId -ne $engine -or (Get-Process -Id {pid}).StartTime.ToFileTimeUtc().ToString() -ne {started}){{throw 'native host incarnation differs'}}
"#,name=quote(tool.title),definition=quote(&definition(tool)?.to_string_lossy()),exe=quote(&tool.install.join(tool.exe_name()).to_string_lossy()),cwd=quote(&tool.install.to_string_lossy()),pid=owner.map_or(0,|p|p.pid),started=quote(owner.map_or("",|p|p.started.as_str())));
        let code=format!("{}\n{code}",coreq::service::windows_path_identity());
        output("host.install.owner",&mut powershell(&code))?;
    }
    Ok(InstallState::Loaded)
}

/// Starting a verified saved/idle definition never replaces its configuration.
pub struct InstallFailure {pub submitted:bool,pub reason:String}
impl InstallFailure {
    fn before(reason:String)->Self {Self {submitted:false,reason}}
    fn unknown(reason:String)->Self {Self {submitted:true,reason}}
}
pub fn start_install(tool:&coreq::Tool,state:InstallState)->Result<(),InstallFailure> {
    if install_state(tool,None).map_err(InstallFailure::before)?!=state {return Err(InstallFailure::before("native host scope changed before start; kept".into()));}
    let unit=coreq::service::unit(tool,&tool.install,"");
    if state==InstallState::Absent {
        #[cfg(windows)] coreq::service::host_unit(tool).map_err(InstallFailure::unknown)?;
        #[cfg(not(windows))] {
            coreq::service::write_unit(&unit).map_err(InstallFailure::before)?;
            commands("host.install.start",&unit.load).map_err(InstallFailure::unknown)?;
        }
        return Ok(());
    }
    #[cfg(target_os="macos")] {
        if state==InstallState::Saved {commands("host.install.start",&unit.load).map_err(InstallFailure::unknown)?;}
        else {
            let target=unit.stop.first().and_then(|command|command.last()).ok_or_else(||InstallFailure::before("native host target missing".into()))?;
            output("host.install.start",Command::new("launchctl").args(["kickstart",target])).map_err(InstallFailure::unknown)?;
        }
    }
    #[cfg(target_os="linux")] {commands("host.install.start",&unit.load).map_err(InstallFailure::unknown)?;}
    #[cfg(windows)] {
        let code=if state==InstallState::Saved {format!(r#"$ErrorActionPreference='Stop';$s=New-Object -ComObject 'Schedule.Service';$s.Connect();$sid=[Security.Principal.WindowsIdentity]::GetCurrent().User.Value;$xml=[IO.File]::ReadAllText({path});$acl='D:P(A;;GA;;;'+$sid+')(A;;GA;;;SY)';$t=$s.GetFolder('\').RegisterTask({name},$xml,18,$null,$null,3,$acl);$null=$t.Run($null)"#,path=quote(&unit.path.to_string_lossy()),name=quote(tool.title))}
            else {format!(r#"$ErrorActionPreference='Stop';$s=New-Object -ComObject 'Schedule.Service';$s.Connect();$null=$s.GetFolder('\').GetTask({}).Run($null)"#,quote(tool.title))};
        output("host.install.start",&mut powershell(&code)).map_err(InstallFailure::unknown)?;
    }
    Ok(())
}

fn verify_saved_install_scope(tool:&coreq::Tool)->Res<()> {
    #[cfg(target_os="macos")] {
        let saved=output("host.install.definition",Command::new("plutil").args(["-convert","json","-o","-"]).arg(definition(tool)?))?;
        let saved:serde_json::Value=serde_json::from_slice(&saved.stdout).map_err(|e|e.to_string())?;
        if saved["Label"]!=format!("com.{}.service",tool.name)||saved["WorkingDirectory"]!=tool.install.to_string_lossy().as_ref()
            ||saved["ProgramArguments"]!=serde_json::json!([tool.install.join(tool.exe_name()),"service","run"]) {return Err("saved host unit scope differs; kept".into());}
    }
    #[cfg(target_os="linux")] {
        let text=fs::read_to_string(definition(tool)?).map_err(|e|e.to_string())?;
        if !text.lines().any(|line|line==format!("ExecStart=\"{}\" service run",tool.install.join(tool.exe_name()).display()))
            ||!text.lines().any(|line|line==format!("WorkingDirectory={}",tool.install.display())) {return Err("saved host unit scope differs; kept".into());}
    }
    #[cfg(windows)] {
        let code=format!(r#"$ErrorActionPreference='Stop';[xml]$saved=Get-Content -Raw -LiteralPath {definition};$e=$saved.Task.Actions.Exec
$p=$saved.Task.Principals.Principal;$sid=[Security.Principal.WindowsIdentity]::GetCurrent().User.Value
$principal=[string]$p.UserId
$owner=if($principal.StartsWith('S-1-',[StringComparison]::OrdinalIgnoreCase)){{[Security.Principal.SecurityIdentifier]::new($principal).Value}}else{{[Security.Principal.NTAccount]::new($principal).Translate([Security.Principal.SecurityIdentifier]).Value}}
if(@($saved.Task.Principals.Principal).Count -ne 1 -or $owner -ne $sid -or [string]$p.LogonType -ne 'InteractiveToken' -or [string]$p.RunLevel -ne 'LeastPrivilege' -or @($saved.Task.Actions.Exec).Count -ne 1 -or @($saved.Task.Actions.ChildNodes).Count -ne 1 -or !(Test-CoreQPath ([string]$e.Command) {exe}) -or [string]$e.Arguments -ne 'service run' -or !(Test-CoreQPath ([string]$e.WorkingDirectory) {cwd})){{throw 'saved host task scope differs'}}
if(@($saved.Task.Triggers.ChildNodes).Count -ne 1 -or @($saved.Task.Triggers.LogonTrigger).Count -ne 1 -or [string]$saved.Task.Triggers.LogonTrigger.Enabled -ne 'true'){{throw 'saved host task trigger differs'}}
$principal=[string]$saved.Task.Triggers.LogonTrigger.UserId
$trigger=if($principal.StartsWith('S-1-',[StringComparison]::OrdinalIgnoreCase)){{[Security.Principal.SecurityIdentifier]::new($principal).Value}}else{{[Security.Principal.NTAccount]::new($principal).Translate([Security.Principal.SecurityIdentifier]).Value}}
if($trigger -ne $sid){{throw 'saved host task trigger owner differs'}}
"#,definition=quote(&definition(tool)?.to_string_lossy()),exe=quote(&tool.install.join(tool.exe_name()).to_string_lossy()),cwd=quote(&tool.install.to_string_lossy()));
        let code=format!("{}\n{code}",coreq::service::windows_path_identity());
        output("host.install.definition",&mut powershell(&code))?;
    }
    Ok(())
}

#[cfg(unix)]
fn verify_install_pid(pid:u32,owner:Option<&Identity>)->Res<()> {
    if pid==0&&owner.is_none() {return Ok(());}
    let expected=owner.ok_or("native host has no attributable service projection")?;
    if pid!=expected.pid||coreq::process::identity(pid).map_err(|e|e.to_string())?.as_ref()!=Some(expected) {return Err("native host incarnation differs; kept".into());}
    Ok(())
}
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
fn commands(name:&'static str,commands:&[Vec<String>])->Res<()> {
    for args in commands {
        let (program,args)=args.split_first().ok_or("native command missing")?;
        output(name,Command::new(program).args(args))?;
    }
    Ok(())
}
#[cfg(windows)]
fn quote(value:&str)->String {format!("'{}'",value.replace('\'',"''"))}
#[cfg(windows)]
fn powershell(code:&str)->Command {
    // GetInstances(0) omits hidden tasks; match the documented hidden-inclusive query by exact task path.
    let code=format!("function Get-TaskQInstances($service,$task) {{ $service.GetRunningTasks(1) | Where-Object {{ $_.Path -eq $task.Path }} }}\n{code}");
    let bytes:Vec<u8>=code.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let mut command=Command::new("powershell.exe");command.args(["-NoProfile","-NonInteractive","-WindowStyle","Hidden","-EncodedCommand",&crate::tray::base64(&bytes)]);command
}

#[cfg(windows)]
fn controller_args(install:&Path,id:&str,action:&str,context:&str,exe:&Path)->Res<String> {
    let install=install.canonicalize().map_err(|e|format!("controller installation identity unknown: {e}"))?;
    let exe=exe.canonicalize().map_err(|e|format!("controller executable identity unknown: {e}"))?;
    let bootstrap=format!("$ErrorActionPreference='Stop';$env:TASKQ_INSTALL={};$env:COREQ_TRACE_CONTEXT={};Remove-Item Env:TASKQ_ROOT,Env:TASKQ_ATTEMPT,Env:TASKQ_TASK -ErrorAction SilentlyContinue;& {} service {} --request {};exit $LASTEXITCODE",quote(&install.to_string_lossy()),quote(context),quote(&exe.to_string_lossy()),action,quote(id));
    let invocation=powershell(&bootstrap);Ok(invocation.get_args().map(|a|a.to_string_lossy()).collect::<Vec<_>>().join(" "))
}

/// Dispatch only a prepared, immutable command. Repeated dispatch is serialized again by projects.lock.
pub fn dispatch(install:&Path,pending:&lifecycle::Pending)->Res<Control> {
    if std::env::var_os("TASKQ_ATTEMPT").is_some() {return Err("a worker cannot dispatch host Stop".into());}
    if !pending.stops()||!pending.prepared() {return Err("Stop has no qualified prepared intent".into());}
    dispatch_command(install,pending.id(),"complete-stop",&pending.context()?)
}

#[cfg(target_os="macos")]
fn controller_command(exe:&Path)->Command {
    use std::os::unix::process::CommandExt;
    // CoreQ captures pipes; exec redirects them before the controller outlives their reader.
    let mut command=Command::new("/bin/sh");
    command.args(["-c","exec \"$@\" </dev/null >/dev/null 2>&1","taskq-controller"]).arg(exe).process_group(0);
    command
}

pub(crate) fn dispatch_command(install:&Path,id:&str,action:&str,context:&str)->Res<Control> {
    if std::env::var_os("TASKQ_ATTEMPT").is_some()||!crate::resources::id(id)||!matches!(action,"complete-stop"|"complete-legacy") {return Err("native controller scope refused".into());}
    coreq::telemetry::capture::TraceContext::decode(context)?;
    let exe=std::env::current_exe().map_err(|e|e.to_string())?;
    #[cfg(target_os="macos")] {
        let mut command=controller_command(&exe);
        #[cfg(test)] let fixture=std::env::var_os("TASKQ_INSTALL_PROCESS_CASE").as_deref()==Some(install.as_os_str())
            &&std::env::var("TASKQ_INSTALL_PROCESS_ROLE").as_deref()==Ok("legacy")&&action=="complete-legacy"
            &&install.parent().and_then(Path::file_name).is_some_and(|name|name.to_string_lossy().starts_with("taskq-install-process-"));
        #[cfg(not(test))] let fixture=false;
        if fixture {
            command.args(["--exact","host::installation::tests::installation_process_fixture","--ignored","--nocapture"])
                .env("TASKQ_INSTALL_PROCESS_ROLE","legacy-controller").env("TASKQ_INSTALL_PROCESS_REQUEST",id);
        } else {command.args(["service",action,"--request",id]);}
        command.current_dir(install)
            .env("TASKQ_INSTALL",install).env("COREQ_TRACE_CONTEXT",&context).env_remove("TASKQ_ROOT").env_remove("TASKQ_ATTEMPT").env_remove("TASKQ_TASK");
        let context=context.to_string();
        let worker=std::thread::Builder::new().name("taskq-stop-controller".into()).spawn(coreq::telemetry::capture::bind(move || {
            let context=coreq::telemetry::capture::TraceContext::decode(&context)?;let _context=context.span().entered();
            coreq::process::output("host.stop.dispatch",&mut command,Duration::from_secs(120)).map(|_|()).map_err(|e|e.to_string())
        })).map_err(|e|format!("Stop controller not started: {e}"))?;
        return Ok(Control {id:id.into(),worker:Some(worker)});
    }
    #[cfg(target_os="linux")] {
        output("host.stop.dispatch",Command::new("systemd-run").args(["--user","--quiet","--collect","--property=RuntimeMaxSec=120"])
            .arg(format!("--unit=taskq-stop-{id}")).arg(format!("--working-directory={}",install.display()))
            .arg(format!("--setenv=TASKQ_INSTALL={}",install.display())).arg(format!("--setenv=COREQ_TRACE_CONTEXT={context}"))
            .arg(exe).args(["service",action,"--request",id]))?;
    }
    #[cfg(windows)] {
        let args=controller_args(install,id,action,context,&exe)?;
        let code=format!(r#"$ErrorActionPreference='Stop';$ProgressPreference='SilentlyContinue'
$s=New-Object -ComObject 'Schedule.Service';$s.Connect();$r=$s.GetFolder('\');$name={name};$path=(Get-Command powershell.exe -ErrorAction Stop).Source;$args={args};$cwd={cwd};$identity=[Security.Principal.WindowsIdentity]::GetCurrent()
try{{$t=$r.GetTask($name)}}catch{{if(($_.Exception.GetBaseException()).HResult -ne -2147024894){{throw}};$t=$null}}
if($t){{$a=$t.Definition.Actions.Item(1);if($t.Definition.Principal.LogonType -ne 3 -or $t.Definition.Principal.RunLevel -ne 0 -or ($t.Definition.Principal.UserId -ne $identity.Name -and $t.Definition.Principal.UserId -ne $identity.User.Value) -or $t.Definition.Triggers.Count -ne 0 -or $t.Definition.Settings.ExecutionTimeLimit -ne 'PT2M' -or $t.Definition.Actions.Count -ne 1 -or !(Test-CoreQPath $a.Path $path) -or $a.Arguments -ne $args -or !(Test-CoreQPath $a.WorkingDirectory $cwd)){{throw 'existing controller definition differs'}}}}
else{{$d=$s.NewTask(0);$d.Principal.UserId=$identity.Name;$d.Principal.LogonType=3;$d.Principal.RunLevel=0;$d.Settings.ExecutionTimeLimit='PT2M';$d.Settings.MultipleInstances=2;$d.Settings.DisallowStartIfOnBatteries=$false;$d.Settings.StopIfGoingOnBatteries=$false;$a=$d.Actions.Create(0);$a.Path=$path;$a.Arguments=$args;$a.WorkingDirectory=$cwd;$sddl='D:P(A;;GA;;;'+$identity.User.Value+')(A;;GA;;;SY)';$t=$r.RegisterTaskDefinition($name,$d,18,$null,$null,3,$sddl)}}
if($t.State -ne 4){{$null=$t.Run($null)}}"#,name=quote(&format!("TaskQ-Stop-{id}")),args=quote(&args),cwd=quote(&install.to_string_lossy()));
        let code=format!("{}\n{code}",coreq::service::windows_path_identity());
        output("host.stop.dispatch",&mut powershell(&code))?;
    }
    #[cfg(not(any(target_os="macos",target_os="linux",windows)))] return Err("native Stop controller unsupported".into());
    #[cfg(not(target_os="macos"))] Ok(Control {id:id.into(),worker:None})
}

/// Never signal a replacement or use daemon exit as evidence about workers.
pub fn stop(install:&Path,id:&str,expected:&Identity,unit_revision:&str)->Res<()> {stop_unit(&crate::platform::tool(install),id,expected,unit_revision)}

pub fn remove(install:&Path,id:&str,expected:&Identity,unit_revision:&str)->Res<()> {
    remove_unit(&crate::platform::tool(install),id,expected,unit_revision)
}
fn remove_unit(tool:&coreq::Tool,id:&str,expected:&Identity,unit_revision:&str)->Res<()> {
    let install=&tool.install;let path=definition(tool)?;
    let present=match fs::symlink_metadata(&path) {Ok(_)=>true,Err(e) if e.kind()==std::io::ErrorKind::NotFound=>false,Err(e)=>return Err(e.to_string())};
    if present&&revision_for(&tool)?!=unit_revision {return Err("Uninstall unit definition changed; kept".into());}
    if !native_absent(&tool)? {stop_unit(&tool,id,expected,unit_revision)?;}
    wait_exit(expected)?;
    if present {
        let unit=coreq::service::unit(&tool,install,"");
        #[cfg(target_os="linux")] commands("host.uninstall.disable",&unit.unload)?;
        #[cfg(windows)] if !native_absent(&tool)? {commands("host.uninstall.delete",std::slice::from_ref(unit.unload.last().ok_or("native delete command missing")?))?;}
        #[cfg(not(target_os="linux"))] if !native_absent(&tool)? {return Err("native unit remains; definition kept".into());}
        #[cfg(target_os="macos")] let _=unit;
        if revision_for(&tool)?!=unit_revision {return Err("Uninstall unit changed before removal; kept".into());}
        crate::host::ownership(&path)?;fs::remove_file(&path).map_err(|e|e.to_string())?;
    }
    #[cfg(target_os="linux")] output("host.uninstall.reload",Command::new("systemctl").args(["--user","daemon-reload"]))?;
    if path.try_exists().map_err(|e|e.to_string())?||!native_absent(&tool)? {return Err("host unit removal not observed; retry the same Uninstall".into());}
    Ok(())
}

pub(crate) fn native_absent(tool:&coreq::Tool)->Res<bool> {
    #[cfg(target_os="macos")] {
        let unit=coreq::service::unit(tool,&tool.install,"");
        let target=unit.stop.first().and_then(|command|command.last()).ok_or("native unit target missing")?;
        let (domain,label)=target.rsplit_once('/').ok_or("launchd domain missing")?;
        let query=output("host.uninstall.readback",Command::new("launchctl").args(["print",domain]))?;
        return Ok(!String::from_utf8(query.stdout).map_err(|e|e.to_string())?.lines().any(|line|line.split_whitespace().any(|field|field.trim_matches('"')==label)));
    }
    #[cfg(windows)] {
        let code=format!(r#"$ErrorActionPreference='Stop';$s=New-Object -ComObject 'Schedule.Service';$s.Connect();$r=$s.GetFolder('\')
try{{$null=$r.GetTask({name});'present'}}catch{{if(($_.Exception.GetBaseException()).HResult -ne -2147024894){{throw}};'absent'}}
exit 0"#,name=quote(tool.title));
        let query=output("host.uninstall.readback",&mut powershell(&code))?;
        return match String::from_utf8(query.stdout).map_err(|e|e.to_string())?.trim() {"absent"=>Ok(true),"present"=>Ok(false),_=>Err("native unit inventory unknown".into())};
    }
    #[cfg(target_os="linux")] {
        let query=crate::platform::output("host.uninstall.readback",Command::new("systemctl").args(["--user","show",&format!("{}.service",tool.name),"--property=LoadState","--property=MainPID","--property=ActiveState","--no-pager"]),Duration::from_secs(30))?;
        let text=String::from_utf8(query.stdout).map_err(|e|e.to_string())?;
        return systemd_absence(&text,query.status.success()).map_err(|reason|format!("{reason}; {}",query.status));
    }
    #[cfg(not(any(target_os="macos",target_os="linux",windows)))] Err("native unit inventory unsupported".into())
}

#[cfg(any(target_os="linux",test))]
fn systemd_absence(text:&str,success:bool)->Res<bool> {
    let field=|key:&str|text.lines().find_map(|line|line.strip_prefix(key));
    let (load,pid,active)=(field("LoadState="),field("MainPID="),field("ActiveState="));
    // systemd retains failed history after the definition and daemon have gone.
    if success&&load==Some("not-found")&&pid==Some("0")&&matches!(active,Some("inactive"|"failed")) {return Ok(true);}
    if success&&load==Some("loaded") {return Ok(false);}
    Err(format!("systemd unit inventory unknown (LoadState={}, MainPID={}, ActiveState={})",load.unwrap_or("missing"),pid.unwrap_or("missing"),active.unwrap_or("missing")))
}
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
            commands("host.stop.effect",&unit.stop)?;
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
        commands("host.stop.effect",&unit.stop)?;
        let query=output("host.stop.readback",Command::new("systemctl").args(["--user","show",name,"--property=MainPID","--property=ActiveState","--no-pager"]))?;
        let text=String::from_utf8(query.stdout).map_err(|e|e.to_string())?;
        if !text.lines().any(|l|l=="MainPID=0")||!text.lines().any(|l|l=="ActiveState=inactive") {return Err("systemd Stop not observed".into());}
    }
    #[cfg(windows)] {
        let setup=format!(r#"$ErrorActionPreference='Stop';$ProgressPreference='SilentlyContinue'
$s=New-Object -ComObject 'Schedule.Service';$s.Connect();$r=$s.GetFolder('\');$t=$r.GetTask({name});$a=$t.Definition.Actions.Item(1);[xml]$saved=Get-Content -Raw -LiteralPath {definition};$exec=$saved.Task.Actions.Exec
if($t.Definition.Actions.Count -ne 1 -or @($exec).Count -ne 1 -or !(Test-CoreQPath $a.Path ([string]$exec.Command)) -or $a.Arguments -ne [string]$exec.Arguments -or !(Test-CoreQPath $a.WorkingDirectory ([string]$exec.WorkingDirectory)) -or !(Test-CoreQPath $a.WorkingDirectory {cwd})){{throw 'scheduled host scope differs'}}
"#,name=quote(tool.title),definition=quote(&definition(tool)?.to_string_lossy()),cwd=quote(&install.to_string_lossy()));
        let setup=format!("{}\n{setup}",coreq::service::windows_path_identity());
        let query=format!(r#"{setup}
$instances=@(Get-TaskQInstances $s $t);if($instances.Count -gt 1){{throw 'multiple native host instances'}}
if($instances.Count -eq 0){{'null';exit 0}}
$instance=$instances[0];$engine=$instance.EnginePID;$daemon=Get-CimInstance Win32_Process -Filter 'ProcessId = {pid}' -ErrorAction Stop
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
$instances=@(Get-TaskQInstances $s $t);if($instances.Count -gt 1){{throw 'multiple native host instances'}}
if($instances.Count -eq 1){{$instance=$instances[0];if($instance.InstanceGuid -ne {instance} -or $instance.EnginePID -ne {pid} -or (Get-Process -Id {pid} -ErrorAction Stop).StartTime.ToFileTimeUtc().ToString() -ne {started}){{throw 'native launcher incarnation differs'}};$instance.Stop()}}
$until=[DateTime]::UtcNow.AddSeconds(10);while(@(Get-TaskQInstances $s $t).Count -ne 0 -and [DateTime]::UtcNow -lt $until){{Start-Sleep -Milliseconds 20}}
if(@(Get-TaskQInstances $s $t).Count -ne 0){{throw 'scheduled host stop not observed'}}"#,instance=quote(scope["instance"].as_str().ok_or("launcher instance unknown")?),pid=launcher.pid,started=quote(&launcher.started));
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
        let pending=lifecycle::pending(install)?.filter(|p|p.id()==id&&p.stops()).ok_or("Stop controller binding unavailable")?;
        return retire_command(install,id,"complete-stop",&pending.context()?);
    }
    #[cfg(not(windows))] let _=(install,id);
    Ok(())
}

pub(crate) fn retire_command(install:&Path,id:&str,action:&str,context:&str)->Res<()> {
    if !crate::resources::id(id)||!matches!(action,"complete-stop"|"complete-legacy") {return Err("native controller retirement scope refused".into());}
    coreq::telemetry::capture::TraceContext::decode(context)?;
    #[cfg(windows)] {
        let args=controller_args(install,id,action,context,&std::env::current_exe().map_err(|e|e.to_string())?)?;
        let code=format!(r#"$ErrorActionPreference='Stop';$s=New-Object -ComObject 'Schedule.Service';$s.Connect();$r=$s.GetFolder('\');$name={name}
try{{$t=$r.GetTask($name)}}catch{{if(($_.Exception.GetBaseException()).HResult -ne -2147024894){{throw}};$t=$null}}
if($t){{$a=$t.Definition.Actions.Item(1);if($t.Definition.Actions.Count -ne 1 -or !(Test-CoreQPath $a.Path (Get-Command powershell.exe -ErrorAction Stop).Source) -or $a.Arguments -ne {args} -or !(Test-CoreQPath $a.WorkingDirectory {cwd}) -or $t.Definition.Principal.RunLevel -ne 0 -or $t.Definition.Triggers.Count -ne 0){{throw 'controller ownership differs; definition kept'}};$r.DeleteTask($name,0)}}
try{{$null=$r.GetTask($name);throw 'controller definition remains'}}catch{{if(($_.Exception.GetBaseException()).HResult -ne -2147024894){{throw}}}}
exit 0"#,name=quote(&format!("TaskQ-Stop-{id}")),args=quote(&args),cwd=quote(&install.to_string_lossy()));
        let code=format!("{}\n{code}",coreq::service::windows_path_identity());
        output("host.stop.controller.retire",&mut powershell(&code))?;
    }
    #[cfg(not(windows))] let _=install;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value,json};

    #[cfg(windows)]
    #[test]
    fn controller_replay_keeps_arguments_across_install_aliases() {
        let install=std::env::temp_dir().join(format!("taskq-controller-alias-{}",crate::runtime::uuid()));
        fs::create_dir_all(&install).unwrap();let exe=std::env::current_exe().unwrap();
        let canonical=install.canonicalize().unwrap();
        let args=controller_args(&install,"same-request","complete-stop","same-context",&exe).unwrap();
        assert_eq!(args,controller_args(&canonical,"same-request","complete-stop","same-context",&exe.canonicalize().unwrap()).unwrap());
        assert_ne!(args,controller_args(&install,"other-request","complete-stop","same-context",&exe).unwrap());
        assert!(controller_args(&install.join("missing"),"same-request","complete-stop","same-context",&exe).is_err());
        fs::remove_dir_all(install).unwrap();
    }

    #[test]
    fn removed_systemd_failed_history_is_absent_but_incomplete_inventory_is_unknown() {
        for active in ["inactive","failed"] {
            assert!(systemd_absence(&format!("MainPID=0\nLoadState=not-found\nActiveState={active}\n"),true).unwrap());
        }
        assert!(!systemd_absence("MainPID=0\nLoadState=loaded\nActiveState=inactive\n",true).unwrap());
        for (text,success) in [("LoadState=not-found\nActiveState=failed\n",true),("MainPID=7\nLoadState=not-found\nActiveState=failed\n",true),
            ("MainPID=0\nLoadState=not-found\nActiveState=active\n",true),("MainPID=0\nLoadState=not-found\nActiveState=failed\n",false),
            ("MainPID=0\nLoadState=loaded\nActiveState=inactive\n",false),("",false)] {
            assert!(systemd_absence(text,success).unwrap_err().contains("systemd unit inventory unknown"));
        }
    }

    // H-A5: run the real adapter only against a unique, externally owned temporary unit.
    #[test]
    #[ignore="requires TASKQ_NATIVE_STOP_CASE and an owned temporary native unit; no production unit"]
    fn native_unit_stop() {
        let install=PathBuf::from(std::env::var_os("TASKQ_NATIVE_STOP_CASE").expect("owned fixture root required"));
        let nonce=install.file_name().unwrap().to_str().unwrap().strip_prefix("taskq-stop-native-").unwrap();
        let id=nonce;
        assert_eq!(nonce.len(),32);assert!(nonce.bytes().all(|b|b.is_ascii_hexdigit()));
        let name=Box::leak(format!("taskq-stop-probe-{nonce}").into_boxed_str());
        let title=Box::leak(format!("TaskQ-Stop-Probe-{nonce}").into_boxed_str());
        let tool=coreq::Tool {name,title,repo:"fixture/unused",version:crate::platform::VERSION,install:install.clone(),fault:None};
        #[cfg(any(target_os="linux",windows))]
        if std::env::var_os("TASKQ_NATIVE_INSTALL_PREPARE").is_some() {
            assert_eq!(install_state(&tool,None).unwrap(),InstallState::Absent);
            start_install(&tool,InstallState::Absent).unwrap_or_else(|e|panic!("initial native fixture start: {}",e.reason));
            wait_fixture_host(&tool,None);
        }
        let fixture:Value=serde_json::from_slice(&fs::read(install.join("native.json")).unwrap()).unwrap();
        let native=coreq::process::identity(fixture["pid"].as_u64().unwrap().try_into().unwrap()).unwrap().unwrap();
        if let Some(started)=fixture["started"].as_str() {assert_eq!(native.started,started);}
        let revision=revision_for(&tool).unwrap();let definition=definition(&tool).unwrap();let original=fs::read(&definition).unwrap();
        if std::env::var_os("TASKQ_NATIVE_INSTALL_VERIFY").is_some() {
            #[cfg(windows)] {
                let inventory_tool=coreq::Tool {title:"TaskQ-Stop-Probe",name,repo:"fixture/unused",version:tool.version,install:install.clone(),fault:None};
                assert!(legacy_inventory(&inventory_tool,&[]).unwrap().iter().any(|value|value==title),"hidden registered units must remain admission barriers");
                let bound=legacy::read(&inventory_tool,&native).unwrap().expect("hidden native parent must bind the original scope");
                assert_eq!(bound.project,nonce);assert_eq!(bound.pid,native.pid);assert_eq!(bound.started,native.started);
                assert_eq!(bound.root,install.canonicalize().unwrap());assert_eq!(bound.revision,revision);
            }
            assert_eq!(install_state(&tool,Some(&native)).unwrap(),InstallState::Loaded);
            assert!(install_state(&tool,None).is_err(),"an unattributed native daemon is never idle");
            let mut wrong=native.clone();wrong.started.push_str("-wrong");
            assert!(install_state(&tool,Some(&wrong)).is_err());
            assert_eq!(fs::read(&definition).unwrap(),original,"verification cannot edit the unit");
            fs::write(&definition,b"foreign invalid definition").unwrap();
            let refused=install_state(&tool,Some(&native));
            let kept=fs::read(&definition).unwrap();fs::write(&definition,&original).unwrap();
            assert!(refused.is_err());assert_eq!(kept,b"foreign invalid definition");
            #[cfg(windows)] {
                let canonical=coreq::Tool {install:install.canonicalize().unwrap(),..tool.clone()};
                assert_eq!(install_state(&canonical,Some(&native)).unwrap(),InstallState::Loaded,"native/saved ordinary paths accept the canonical caller");
                let other=install.join("other.exe");fs::copy(install.join(tool.exe_name()),&other).unwrap();
                for (field,path,accepted) in [("Command",install.join(tool.exe_name()).canonicalize().unwrap(),true),("WorkingDirectory",install.canonicalize().unwrap(),true),
                    ("Command",other.clone(),false),("Command",install.join("missing.exe"),false),("WorkingDirectory",install.parent().unwrap().to_path_buf(),false),("WorkingDirectory",install.join("missing"),false)] {
                    let code=format!("$ErrorActionPreference='Stop';[xml]$xml=Get-Content -Raw -LiteralPath {definition};$xml.Task.Actions.Exec.{field}={path};$xml.Save({definition})",definition=quote(&definition.to_string_lossy()),path=quote(&path.to_string_lossy()));
                    output("host.install.fixture.path",&mut powershell(&code)).unwrap();
                    let changed=fs::read(&definition).unwrap();let checked=install_state(&tool,Some(&native));
                    let kept=fs::read(&definition).unwrap();fs::write(&definition,&original).unwrap();
                    assert_eq!(checked.is_ok(),accepted,"{field} {}: {checked:?}",path.display());assert_eq!(kept,changed);
                    assert_eq!(coreq::process::identity(native.pid).unwrap().as_ref(),Some(&native));
                }
                fs::remove_file(other).unwrap();
                let sid=output("host.install.fixture.principal",&mut powershell("[Security.Principal.WindowsIdentity]::GetCurrent().User.Value")).unwrap();
                let sid=String::from_utf8(sid.stdout).unwrap();
                for (principal,accepted) in [(sid.trim(),true),("S-1-5-18",false),("S-1-invalid",false)] {
                    let code=format!("$ErrorActionPreference='Stop';[xml]$xml=Get-Content -Raw -LiteralPath {path};$xml.Task.Principals.Principal.UserId={principal};$xml.Save({path})",path=quote(&definition.to_string_lossy()),principal=quote(principal));
                    output("host.install.fixture.definition",&mut powershell(&code)).unwrap();
                    let changed=fs::read(&definition).unwrap();let checked=install_state(&tool,Some(&native));
                    let kept=fs::read(&definition).unwrap();fs::write(&definition,&original).unwrap();
                    assert_eq!(checked.is_ok(),accepted,"principal {principal}: {checked:?}");assert_eq!(kept,changed);
                    assert_eq!(coreq::process::identity(native.pid).unwrap().as_ref(),Some(&native));
                }
                for change in ["$xml.Task.Triggers.LogonTrigger.UserId='S-1-5-18'","$xml.Task.Triggers.LogonTrigger.UserId='S-1-invalid'","$null=$xml.Task.Triggers.AppendChild($xml.Task.Triggers.LogonTrigger.CloneNode($true))"] {
                    let code=format!("$ErrorActionPreference='Stop';[xml]$xml=Get-Content -Raw -LiteralPath {path};{change};$xml.Save({path})",path=quote(&definition.to_string_lossy()));
                    output("host.install.fixture.trigger",&mut powershell(&code)).unwrap();
                    let changed=fs::read(&definition).unwrap();let checked=install_state(&tool,Some(&native));
                    let kept=fs::read(&definition).unwrap();fs::write(&definition,&original).unwrap();
                    assert!(checked.is_err(),"foreign/invalid/extra saved trigger accepted");assert_eq!(kept,changed);
                    assert_eq!(coreq::process::identity(native.pid).unwrap().as_ref(),Some(&native));
                }
                let code=format!("$ErrorActionPreference='Stop';$s=New-Object -ComObject 'Schedule.Service';$s.Connect();$r=$s.GetFolder('\\');$t=$r.GetTask({name});$d=$t.Definition;$extra=$d.Triggers.Create(9);$extra.Enabled=$true;$extra.UserId=[Security.Principal.WindowsIdentity]::GetCurrent().User.Value;$null=$r.RegisterTaskDefinition({name},$d,4,$null,$null,3,$null)",name=quote(tool.title));
                output("host.install.fixture.native-trigger",&mut powershell(&code)).unwrap();
                let checked=install_state(&tool,Some(&native));
                let restore=format!("$ErrorActionPreference='Stop';$s=New-Object -ComObject 'Schedule.Service';$s.Connect();$null=$s.GetFolder('\\').RegisterTask({name},[IO.File]::ReadAllText({path}),4,$null,$null,3,$null)",name=quote(tool.title),path=quote(&definition.to_string_lossy()));
                output("host.install.fixture.native-restore",&mut powershell(&restore)).unwrap();
                assert!(checked.is_err(),"extra native trigger accepted");assert_eq!(fs::read(&definition).unwrap(),original);
                assert_eq!(install_state(&tool,Some(&native)).unwrap(),InstallState::Loaded);
            }
        }
        #[cfg(windows)] let collateral=if std::env::var_os("TASKQ_NATIVE_INSTALL_START_VERIFY").is_some() {
            let bytes=fs::read(install.join(tool.exe_name())).unwrap();
            let prior=tool.release_exe("4.4.0");fs::create_dir_all(prior.parent().unwrap()).unwrap();fs::write(prior,&bytes).unwrap();
            coreq::write_durable(&install.join("current"),"4.4.0").unwrap();
            coreq::update::prepare_install(&tool,&bytes).unwrap();coreq::update::activate_install(&tool).unwrap();
            let old=install.join(format!("{}.old",tool.exe_name()));
            assert_eq!(fs::read(&old).unwrap(),bytes);assert_eq!(fs::read(install.join(tool.exe_name())).unwrap(),bytes);
            assert_eq!(coreq::process::identity(native.pid).unwrap().as_ref(),Some(&native),"launcher activation must preserve its existing daemon");
            assert!(fs::remove_file(&old).is_err(),"live launcher collateral must remain locked");
            json!({"old":old,"locked_while_running":true,"daemon_preserved":true})
        }else{Value::Null};
        #[cfg(not(windows))] let collateral=Value::Null;
        let (capture,guard)=crate::profile::tests::collecting(&install);
        fixture_stop_binding(&install,id,&native,&revision);
        #[cfg(windows)] {
            dispatch(&install,&lifecycle::pending(&install).unwrap().unwrap()).unwrap().wait().unwrap();
            let check=format!(r#"$ErrorActionPreference='Stop';$s=New-Object -ComObject 'Schedule.Service';$s.Connect();$t=$s.GetFolder('\').GetTask({name});$sid=[Security.Principal.WindowsIdentity]::GetCurrent().User.Value
$acl=[Security.AccessControl.RawSecurityDescriptor]::new($t.GetSecurityDescriptor(4));$allowed=@($acl.DiscretionaryAcl|Where-Object {{$_.AceQualifier -eq 'AccessAllowed' -and $_.SecurityIdentifier.Value -eq $sid -and (($_.AccessMask -band 0x10000000) -ne 0 -or ($_.AccessMask -band 0x10000) -ne 0)}})
if(($acl.ControlFlags -band [Security.AccessControl.ControlFlags]::DiscretionaryAclProtected) -eq 0 -or @($acl.DiscretionaryAcl).Count -ne 2){{throw 'controller task ACL differs from the requested protected scope'}}
if($allowed.Count -eq 0){{throw 'limited controller cannot retire its own task'}}"#,name=quote(&format!("TaskQ-Stop-{id}")));
            let checked=output("host.stop.controller.permissions",&mut powershell(&check));
            let cleaned=retire_controller(&install,id);cleaned.unwrap();checked.unwrap();
        }
        assert!(stop_unit(&tool,id,&native,&"0".repeat(64)).is_err());
        let mut foreign=native.clone();foreign.started.push_str("-foreign");
        assert!(stop_unit(&tool,id,&foreign,&revision).is_err());
        assert_eq!(coreq::process::identity(native.pid).unwrap().as_ref(),Some(&native),"refused requests must preserve the original daemon");
        stop_unit(&tool,id,&native,&revision).unwrap();
        assert_ne!(coreq::process::identity(native.pid).unwrap().as_ref(),Some(&native));
        stop_unit(&tool,id,&native,&revision).unwrap();
        assert_eq!(fs::read(&definition).unwrap(),original,"Stop retains the native definition");
        #[cfg(any(target_os="macos",target_os="linux",windows))]
        let starts=if std::env::var_os("TASKQ_NATIVE_INSTALL_START_VERIFY").is_some() {install_start_matrix(&tool,id,&native,&original)}else{json!([])};
        #[cfg(not(any(target_os="macos",target_os="linux",windows)))] let starts=json!([]);
        let uninstall=std::env::var("TASKQ_NATIVE_UNINSTALL").as_deref()==Ok("1");
        if uninstall {
            assert!(remove_unit(&tool,id,&native,&"0".repeat(64)).is_err());
            assert_eq!(fs::read(&definition).unwrap(),original,"refused Uninstall keeps the definition");
            remove_unit(&tool,id,&native,&revision).unwrap();
            assert!(!definition.try_exists().unwrap());assert!(native_absent(&tool).unwrap());
            remove_unit(&tool,id,&native,&revision).unwrap();
        }
        capture.flush().unwrap();drop(guard);drop(capture);
        fs::write(install.join("adapter-result.json"),json!({"unit":name,"native_pid":native.pid,"native_started":native.started,"unit_revision":revision,"exit":true,"replay":true,"definition_retained_after_stop":true,"definition_retained":!uninstall,"unit_removed":uninstall,"install_starts":starts,"running_launcher_collateral":collateral}).to_string()).unwrap();
    }

    fn fixture_stop_binding(install:&Path,id:&str,native:&Identity,revision:&str) {
        assert!(crate::resources::id(id),"fixture must use the production lifecycle ID format");
        let trace=crate::runtime::uuid().replace('-',"");
        let command=coreq::telemetry::capture::CommandReceipt::decode(&json!({"wire":1,"product":"taskq","version":"fixture","operation":"taskq.host.stop.request","trace":format!("{trace}.1"),"span":format!("{trace}.2"),"parent":null,"started_ms":1}).to_string()).unwrap();
        crate::resources::with(install,|state| {
            state.ops.insert(id.into(),crate::resources::Op {
                bound:json!({"local_stop":{"request":{"id":id,"instance":"original","roots":[],"issued_at":1,"expires_at":301,"action":"stop"},"command":command.encode()}}),
                receipt:json!({"id":id,"state":"applied","received_receipt":command.event_id("received")?,"applied_receipt":command.event_id("applied")?,"native_pid":native.pid,"native_started":native.started,"unit_revision":revision}),
                expires_at:301,saved_at:Some(1),writing:false,sent:true,
            });Ok(())
        }).unwrap();
        assert!(lifecycle::receipt(install,id).unwrap().is_some());
    }

    // H-A5: native start states share the existing owned-unit fixture, with no product registration.
    #[cfg(any(target_os="macos",target_os="linux",windows))]
    fn install_start_matrix(tool:&coreq::Tool,id:&str,previous:&Identity,original:&[u8])->Value {
        let definition=definition(tool).unwrap();
        let unit=coreq::service::unit(tool,&tool.install,"");
        let mut prior=previous.clone();let mut cases=vec![];
        #[cfg(target_os="macos")] let states=[InstallState::Saved,InstallState::Loaded,InstallState::Absent];
        #[cfg(target_os="linux")] let states=[InstallState::Loaded,InstallState::Absent];
        #[cfg(windows)] let states=[InstallState::Loaded,InstallState::Saved,InstallState::Absent];
        for state in states {
            if state==InstallState::Loaded {
                #[cfg(target_os="macos")] {
                for key in ["RunAtLoad","KeepAlive"] {
                    output("host.install.fixture.idle",Command::new("plutil").args(["-replace",key,"-bool","false"]).arg(&definition)).unwrap();
                }
                commands("host.install.fixture.load",&unit.load).unwrap();
                }
            } else if state==InstallState::Saved {
                #[cfg(windows)] {
                    assert_eq!(install_state(tool,None).unwrap(),InstallState::Loaded);
                    let code=format!("$ErrorActionPreference='Stop';$s=New-Object -ComObject 'Schedule.Service';$s.Connect();$s.GetFolder('\\').DeleteTask({},0)",quote(tool.title));
                    output("host.install.fixture.unload",&mut powershell(&code)).unwrap();
                    assert!(native_absent(tool).unwrap());
                }
            } else if state==InstallState::Absent {
                #[cfg(any(target_os="linux",windows))] remove_unit(tool,&format!("{id}-{}",cases.len()-1),&prior,&revision_for(tool).unwrap()).unwrap();
                assert!(native_absent(tool).unwrap());
                #[cfg(target_os="macos")] fs::remove_file(&definition).unwrap();
            }
            assert_eq!(install_state(tool,None).unwrap(),state);
            let before=fs::read(&definition).ok();
            fs::remove_file(tool.install.join("native.json")).unwrap();
            start_install(tool,state).unwrap_or_else(|e|panic!("native fixture start: {} (submitted={})",e.reason,e.submitted));
            let native=wait_fixture_host(tool,Some(&prior));
            assert_eq!(install_state(tool,Some(&native)).unwrap(),InstallState::Loaded);
            if let Some(before)=before {assert_eq!(fs::read(&definition).unwrap(),before,"saved/idle start cannot rewrite the owner's definition");}
            else {
                #[cfg(not(windows))] assert_eq!(fs::read(&definition).unwrap(),unit.text.as_bytes());
                #[cfg(windows)] verify_saved_install_scope(tool).unwrap();
            }
            let revision=revision_for(tool).unwrap();
            let stop_id=format!("{id}-{}",cases.len());
            #[cfg(windows)] fixture_stop_binding(&tool.install,&stop_id,&native,&revision);
            stop_unit(tool,&stop_id,&native,&revision).unwrap();
            #[cfg(target_os="macos")] assert!(native_absent(tool).unwrap());
            #[cfg(any(target_os="linux",windows))] assert_eq!(install_state(tool,None).unwrap(),InstallState::Loaded);
            assert_ne!(coreq::process::identity(native.pid).unwrap().as_ref(),Some(&native));
            cases.push(json!({"from":match state {InstallState::Saved=>"saved",InstallState::Loaded=>"loaded-idle",InstallState::Absent=>"absent"},"pid":native.pid,"started":native.started,"definition_preserved":state!=InstallState::Absent,"exit":true}));
            prior=native;
        }
        #[cfg(any(target_os="linux",windows))] remove_unit(tool,&format!("{id}-{}",cases.len()-1),&prior,&revision_for(tool).unwrap()).unwrap();
        assert!(native_absent(tool).unwrap());fs::write(&definition,original).unwrap();
        json!(cases)
    }

    #[cfg(any(target_os="macos",target_os="linux",windows))]
    fn wait_fixture_host(tool:&coreq::Tool,prior:Option<&Identity>)->Identity {
        let until=std::time::Instant::now()+Duration::from_secs(10);
        loop {
            let found=fs::read(tool.install.join("native.json")).ok().and_then(|bytes|serde_json::from_slice::<Value>(&bytes).ok())
                .and_then(|v|v["pid"].as_u64()).and_then(|pid|u32::try_from(pid).ok())
                .and_then(|pid|coreq::process::identity(pid).ok().flatten()).filter(|identity|Some(identity)!=prior);
            if let Some(native)=found {return native;}
            assert!(std::time::Instant::now()<until,"native fixture did not publish a new daemon");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
