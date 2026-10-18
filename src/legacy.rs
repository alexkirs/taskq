//! Native legacy service identity. Cached status and cwd alone never select a retiring native unit.
use super::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Owner {
    pub root: PathBuf,
    pub project: String,
    pub pid: u32,
    pub started: String,
    pub revision: String,
    pub launcher: Option<serde_json::Value>,
}
impl Owner {
    pub fn validate(&self)->Res<()> {
        if !self.root.is_absolute()||self.project.is_empty()||matches!(self.project.as_str(),"."|"..")||self.project.contains(['/', '\\'])||self.project.chars().any(char::is_control)
            ||self.pid==0||self.pid>i32::MAX as u32||self.started.is_empty()||self.started.len()>128||!self.started.bytes().all(|b|b.is_ascii_alphanumeric()||b==b'-')
            ||self.revision.len()!=64||!self.revision.bytes().all(|b|b.is_ascii_digit()||(b'a'..=b'f').contains(&b)) {
            return Err("legacy native scope binding invalid; kept".into());
        }
        #[cfg(windows)] {launcher(self)?;}
        #[cfg(not(windows))] if self.launcher.is_some() {return Err("legacy scope belongs to another native platform; kept".into());}
        Ok(())
    }
    pub fn identity(&self) -> Identity {
        Identity {
            pid: self.pid,
            started: self.started.clone(),
        }
    }
    pub fn verify(&self, tool: &coreq::Tool) -> Res<()> {
        self.validate()?;
        if read(tool, &self.identity())?.as_ref() != Some(self) {
            return Err("legacy native binding changed; handoff held".into());
        }
        Ok(())
    }
    pub fn retired(&self,tool:&coreq::Tool)->Res<bool> {
        self.validate()?;
        if coreq::process::identity(self.pid).map_err(|e|e.to_string())?.as_ref()==Some(&self.identity()) {return Ok(false);}
        let unit=coreq::service::unit(tool,&self.root,&self.project);
        match fs::symlink_metadata(&unit.path) {Ok(_)=>return Ok(false),Err(e) if e.kind()==std::io::ErrorKind::NotFound=>{},Err(e)=>return Err(e.to_string())}
        legacy_absent(tool,self)?;Ok(true)
    }
    /// Retire only the native unit that originally owned this draining daemon.  A replay may
    /// finish an already-issued stop, but never turns a missing daemon into authority to stop
    /// or remove a replacement unit.
    pub fn retire(&self, tool: &coreq::Tool, _id: &str) -> Res<()> {
        self.validate()?;
        if cfg!(target_os = "macos") && self.project == "service" {
            return Err("legacy label conflicts with the host unit; handoff held".into());
        }
        let unit = coreq::service::unit(tool, &self.root, &self.project);
        let expected = self.identity();
        match coreq::process::identity(expected.pid).map_err(|e| e.to_string())? {
            Some(live) => {
                if live != expected {
                    return Err("legacy native incarnation changed; handoff held".into());
                }
                self.verify(tool)?;
                if file_revision(&unit.path)? != self.revision {
                    return Err("legacy definition changed; handoff held".into());
                }
                stop_live(tool, self, &unit, &expected)?;
                super::wait_exit(&expected)?;
            }
            None => legacy_idle(tool, self)?,
        }
        // Do not let a lost stop response authorize deletion.  The manager must still show this
        // exact unit idle (or absent), and the saved definition must still be the recorded one.
        if unit.path.try_exists().map_err(|e| e.to_string())?
            && file_revision(&unit.path)? != self.revision
        {
            return Err("legacy definition changed before removal; handoff held".into());
        }
        legacy_idle(tool, self)?;
        coreq::service::legacy_unit(tool, &self.project)?;
        if unit.path.try_exists().map_err(|e| e.to_string())? {
            return Err("legacy definition remains after removal".into());
        }
        legacy_absent(tool, self)?;
        Ok(())
    }
}


fn legacy_idle(tool: &coreq::Tool, owner: &Owner) -> Res<()> {
    #[cfg(target_os = "macos")]
    {
        let domain = format!("gui/{}", unsafe { libc::geteuid() });
        let label = format!("com.{}.{}", tool.name, owner.project);
        let native = super::output(
            "legacy.retire.readback",
            std::process::Command::new("launchctl").args(["print", &domain]),
        )?;
        let text = String::from_utf8(native.stdout).map_err(|e| e.to_string())?;
        if super::launchd_bindings(&text,&format!("com.{}.",tool.name)).iter().any(|(_,name)|*name==label) {
            return Err("legacy launchd unit remains loaded; handoff held".into());
        }
        return Ok(());
    }
    #[cfg(target_os = "linux")]
    {
        let name = format!("{}-{}.service", tool.name, owner.project);
        let native = super::output(
            "legacy.retire.readback",
            std::process::Command::new("systemctl").args([
                "--user",
                "show",
                &name,
                "--property=LoadState",
                "--property=MainPID",
                "--property=ActiveState",
                "--no-pager",
            ]),
        )?;
        let text = String::from_utf8(native.stdout).map_err(|e| e.to_string())?;
        let field = |key: &str| text.lines().find_map(|line| line.strip_prefix(key));
        if field("LoadState=") == Some("not-found")
            && field("MainPID=") == Some("0")
            && field("ActiveState=") == Some("inactive")
        {
            return Ok(());
        }
        if field("LoadState=") == Some("loaded")
            && field("MainPID=") == Some("0")
            && field("ActiveState=") == Some("inactive")
        {
            return Ok(());
        }
        return Err("legacy systemd unit is not proven idle; handoff held".into());
    }
    #[cfg(windows)]
    return windows_idle(tool, owner);
    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    Err("native legacy retirement unsupported".into())
}

fn legacy_absent(tool: &coreq::Tool, owner: &Owner) -> Res<()> {
    #[cfg(target_os = "macos")]
    {
        return legacy_idle(tool, owner);
    }
    #[cfg(target_os = "linux")]
    {
        let name = format!("{}-{}.service", tool.name, owner.project);
        let native = super::output(
            "legacy.retire.absence",
            std::process::Command::new("systemctl").args([
                "--user",
                "show",
                &name,
                "--property=LoadState",
                "--property=MainPID",
                "--property=ActiveState",
                "--no-pager",
            ]),
        )?;
        let text = String::from_utf8(native.stdout).map_err(|e| e.to_string())?;
        if text.lines().any(|line| line == "LoadState=not-found")
            && text.lines().any(|line| line == "MainPID=0")
            && text.lines().any(|line| line == "ActiveState=inactive")
        {
            return Ok(());
        }
        return Err("legacy systemd unit remains after removal".into());
    }
    #[cfg(windows)]
    return windows_absent(tool, owner);
    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    Err("native legacy retirement unsupported".into())
}

fn stop_live(
    tool: &coreq::Tool,
    owner: &Owner,
    unit: &coreq::service::Unit,
    _expected: &Identity,
) -> Res<()> {
    #[cfg(not(windows))]
    {
        super::commands("legacy.retire.stop", &unit.stop)?;
        legacy_idle(tool, owner)?;
        return Ok(());
    }
    #[cfg(windows)]
    return windows_stop(tool, owner, _expected);
}

#[cfg(windows)]
fn launcher(owner: &Owner) -> Res<(Identity, String)> {
    let value = owner
        .launcher
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .ok_or("legacy scheduler launcher missing")?;
    if value.len() != 3
        || !value.contains_key("pid")
        || !value.contains_key("started")
        || !value.contains_key("instance")
    {
        return Err("legacy scheduler launcher binding invalid".into());
    }
    let pid = value["pid"]
        .as_u64()
        .filter(|pid| *pid > 0 && *pid <= i32::MAX as u64)
        .ok_or("legacy launcher pid invalid")? as u32;
    let started = value["started"]
        .as_str()
        .filter(|started| {
            !started.is_empty()
                && started.len() <= 128
                && started
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
        .ok_or("legacy launcher identity invalid")?;
    let instance = value["instance"]
        .as_str()
        .filter(|instance| !instance.is_empty()&&instance.len()<=64&&instance.bytes().all(|b|b.is_ascii_hexdigit()||b"-{}".contains(&b)))
        .ok_or("legacy scheduler instance invalid")?;
    Ok((
        Identity {
            pid,
            started: started.into(),
        },
        instance.into(),
    ))
}

#[cfg(windows)]
fn windows_task(tool: &coreq::Tool, owner: &Owner, mode: &str) -> Res<()> {
    let (launcher, instance) = launcher(owner)?;
    let name = format!("{}-{}", tool.title, owner.project);
    let code = match mode {
        "idle" => format!(
            r#"$ErrorActionPreference='Stop';$s=New-Object -ComObject 'Schedule.Service';$s.Connect();$r=$s.GetFolder('\')
$p=Get-Process -Id {pid} -ErrorAction SilentlyContinue;if($p -and $p.StartTime.ToFileTimeUtc().ToString() -eq {started}){{throw 'legacy launcher remains'}}
try{{$t=$r.GetTask({name})}}catch{{if(($_.Exception.GetBaseException()).HResult -eq -2147024894){{'absent';exit 0}};throw}}
$instances=@(Get-TaskQInstances $s $t);if($instances.Count -ne 0){{throw 'legacy scheduler instance remains'}}
'idle'
"#,
            name = super::quote(&name),
            pid = launcher.pid,
            started = super::quote(&launcher.started)
        ),
        "absent" => format!(
            r#"$ErrorActionPreference='Stop';$s=New-Object -ComObject 'Schedule.Service';$s.Connect();$r=$s.GetFolder('\')
$p=Get-Process -Id {pid} -ErrorAction SilentlyContinue;if($p -and $p.StartTime.ToFileTimeUtc().ToString() -eq {started}){{throw 'legacy launcher remains'}}
try{{$null=$r.GetTask({name});throw 'legacy scheduler task remains'}}catch{{if(($_.Exception.GetBaseException()).HResult -eq -2147024894){{'absent';exit 0}};throw}}
"#,
            name = super::quote(&name),
            pid = launcher.pid,
            started = super::quote(&launcher.started)
        ),
        _ => return Err("legacy scheduler retirement mode invalid".into()),
    };
    let output = super::output("legacy.retire.readback", &mut super::powershell(&code))?;
    match String::from_utf8(output.stdout)
        .map_err(|e| e.to_string())?
        .trim()
    {
        "idle" | "absent" => Ok(()),
        _ => Err("legacy scheduler readback unknown".into()),
    }
}

#[cfg(windows)]
fn windows_idle(tool: &coreq::Tool, owner: &Owner) -> Res<()> {
    windows_task(tool, owner, "idle")
}
#[cfg(windows)]
fn windows_absent(tool: &coreq::Tool, owner: &Owner) -> Res<()> {
    windows_task(tool, owner, "absent")
}

#[cfg(windows)]
fn windows_stop(tool: &coreq::Tool, owner: &Owner, expected: &Identity) -> Res<()> {
    owner.verify(tool)?;
    let (launcher, instance) = launcher(owner)?;
    let name = format!("{}-{}", tool.title, owner.project);
    let code = format!(
        r#"$ErrorActionPreference='Stop';$s=New-Object -ComObject 'Schedule.Service';$s.Connect();$t=$s.GetFolder('\').GetTask({name})
$instances=@(Get-TaskQInstances $s $t);if($instances.Count -ne 1){{throw 'legacy scheduler instance differs'}}
$i=$instances[0];if($i.InstanceGuid -ne {instance} -or $i.EnginePID -ne {launcher_pid} -or (Get-Process -Id {launcher_pid} -ErrorAction Stop).StartTime.ToFileTimeUtc().ToString() -ne {launcher_started}){{throw 'legacy launcher identity differs'}}
$daemon=Get-CimInstance Win32_Process -Filter 'ProcessId = {daemon_pid}' -ErrorAction Stop
if(!$daemon -or $daemon.ParentProcessId -ne {launcher_pid} -or (Get-Process -Id {daemon_pid} -ErrorAction Stop).StartTime.ToFileTimeUtc().ToString() -ne {daemon_started}){{throw 'legacy daemon identity differs'}}
$i.Stop();$until=[DateTime]::UtcNow.AddSeconds(10);while(@(Get-TaskQInstances $s $t).Count -ne 0 -and [DateTime]::UtcNow -lt $until){{Start-Sleep -Milliseconds 20}}
if(@(Get-TaskQInstances $s $t).Count -ne 0){{throw 'legacy scheduler stop not observed'}}
"#,
        name = super::quote(&name),
        instance = super::quote(&instance),
        launcher_pid = launcher.pid,
        launcher_started = super::quote(&launcher.started),
        daemon_pid = expected.pid,
        daemon_started = super::quote(&expected.started)
    );
    super::output("legacy.retire.stop", &mut super::powershell(&code))?;
    super::terminate_owned(&launcher)?;
    super::wait_exit(&launcher)?;
    super::terminate_owned(expected)?;
    super::wait_exit(expected)?;
    windows_idle(tool, owner)
}

fn file_revision(path: &Path) -> Res<String> {
    let meta = fs::symlink_metadata(path).map_err(|e| format!("legacy definition unknown: {e}"))?;
    if !meta.is_file() || meta.len() > 65536 {
        return Err("legacy definition is not supported regular storage; kept".into());
    }
    crate::host::ownership(path)?;
    fs::read(path)
        .map(|bytes| {
            ring::digest::digest(&ring::digest::SHA256, &bytes)
                .as_ref()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect()
        })
        .map_err(|e| e.to_string())
}
fn finish(
    tool: &coreq::Tool,
    project: String,
    root: PathBuf,
    expected: &Identity,
    launcher: Option<serde_json::Value>,
) -> Res<Owner> {
    if project.is_empty()
        || project == "."
        || project == ".."
        || project.contains(['/', '\\'])
        || project.chars().any(char::is_control)
    {
        return Err("legacy unit name unsupported; ownership retained".into());
    }
    let root = root
        .canonicalize()
        .map_err(|e| format!("legacy root unknown: {e}"))?;
    let unit = coreq::service::unit(tool, &root, &project);
    let revision = file_revision(&unit.path)?;
    if coreq::process::identity(expected.pid)
        .map_err(|e| e.to_string())?
        .as_ref()
        != Some(expected)
    {
        return Err("legacy native incarnation changed; kept".into());
    }
    Ok(Owner {
        root,
        project,
        pid: expected.pid,
        started: expected.started.clone(),
        revision,
        launcher,
    })
}
#[cfg(target_os = "macos")]
fn arguments(text: &str) -> Vec<&str> {
    text.lines()
        .skip_while(|line| line.trim() != "arguments = {")
        .skip(1)
        .take_while(|line| line.trim() != "}")
        .map(str::trim)
        .collect()
}

/// Inspect only this process's native manager binding, including the old label's original spelling.
pub fn owner(tool: &coreq::Tool) -> Res<Option<Owner>> {
    let expected = coreq::process::identity(std::process::id())
        .map_err(|e| e.to_string())?
        .ok_or("service native identity unavailable")?;
    read(tool, &expected)
}
pub(super) fn read(tool: &coreq::Tool, expected: &Identity) -> Res<Option<Owner>> {
    if coreq::process::identity(expected.pid)
        .map_err(|e| e.to_string())?
        .as_ref()
        != Some(expected)
    {
        return Err("legacy native incarnation changed; kept".into());
    }
    let exe = tool.install.join(tool.exe_name());
    #[cfg(target_os = "macos")]
    {
        let domain = format!("gui/{}", unsafe { libc::geteuid() });
        let listing = output(
            "legacy.owner.inventory",
            Command::new("launchctl").args(["print", &domain]),
        )?;
        let listing = String::from_utf8(listing.stdout).map_err(|e| e.to_string())?;
        let prefix = format!("com.{}.", tool.name);
        let labels:Vec<_>=super::launchd_bindings(&listing,&prefix).into_iter().filter_map(|(pid,label)|(pid==expected.pid).then_some(label)).collect();
        if labels.is_empty() {
            return Ok(None);
        }
        if labels.len() != 1 {
            return Err("multiple native service bindings; legacy scope unknown".into());
        }
        let target = format!("{domain}/{}", labels[0]);
        let native = output(
            "legacy.owner.read",
            Command::new("launchctl").args(["print", &target]),
        )?;
        let text = String::from_utf8(native.stdout).map_err(|e| e.to_string())?;
        let field = |key: &str| text.lines().find_map(|line| line.trim().strip_prefix(key));
        let project = labels[0]
            .strip_prefix(&prefix)
            .ok_or("legacy native label invalid")?;
        let root =
            PathBuf::from(field("working directory = ").ok_or("legacy native directory unknown")?);
        if project == "service" && root.canonicalize().ok() == tool.install.canonicalize().ok() {
            return Ok(None);
        }
        if project == "service" {
            return Err("legacy label conflicts with the host unit; handoff held".into());
        }
        let unit = coreq::service::unit(tool, &root, project);
        if field("pid = ").and_then(|v| v.parse::<u32>().ok()) != Some(expected.pid)
            || field("program = ") != exe.to_str()
            || field("path = ") != unit.path.to_str()
            || arguments(&text)
                != [
                    exe.to_str().ok_or("legacy executable is not UTF-8")?,
                    "service",
                    "run",
                ]
        {
            return Err("legacy launchd action/path/owner differs; kept".into());
        }
        let saved = output(
            "legacy.definition.read",
            Command::new("plutil")
                .args(["-convert", "json", "-o", "-"])
                .arg(&unit.path),
        )?;
        let saved: serde_json::Value =
            serde_json::from_slice(&saved.stdout).map_err(|e| e.to_string())?;
        if saved["Label"] != labels[0]
            || saved["ProgramArguments"] != serde_json::json!([exe, "service", "run"])
            || saved["WorkingDirectory"]
                .as_str()
                .map(PathBuf::from)
                .and_then(|p| p.canonicalize().ok())
                != root.canonicalize().ok()
        {
            return Err("legacy saved action/directory differs from native owner; kept".into());
        }
        return finish(tool, project.into(), root, expected, None).map(Some);
    }
    #[cfg(target_os = "linux")]
    {
        let groups = fs::read_to_string(format!("/proc/{}/cgroup", expected.pid))
            .map_err(|e| format!("native cgroup unknown: {e}"))?;
        let prefix = format!("{}-", tool.name);
        let names: std::collections::BTreeSet<_> = groups
            .lines()
            .filter_map(|line| line.splitn(3, ':').nth(2))
            .flat_map(|path| path.split('/'))
            .filter(|part| part.starts_with(&prefix) && part.ends_with(".service"))
            .collect();
        if names.is_empty() {
            return Ok(None);
        }
        if names.len() != 1 {
            return Err("multiple native legacy cgroups; scope unknown".into());
        }
        let name = names.first().unwrap();
        let project = name
            .strip_prefix(&prefix)
            .and_then(|s| s.strip_suffix(".service"))
            .ok_or("legacy unit name invalid")?;
        let native = output(
            "legacy.owner.read",
            Command::new("systemctl").args([
                "--user",
                "show",
                name,
                "--property=MainPID",
                "--property=LoadState",
                "--property=ExecStart",
                "--property=WorkingDirectory",
                "--property=FragmentPath",
                "--no-pager",
            ]),
        )?;
        let text = String::from_utf8(native.stdout).map_err(|e| e.to_string())?;
        let field = |key: &str| text.lines().find_map(|line| line.strip_prefix(key));
        let pid = field("MainPID=")
            .and_then(|s| s.parse::<u32>().ok())
            .ok_or("legacy native PID unknown")?;
        // A CLI child inside the service cgroup does not inherit the daemon's authority.
        if pid != expected.pid {
            return Ok(None);
        }
        let root =
            PathBuf::from(field("WorkingDirectory=").ok_or("legacy native directory unknown")?);
        let unit = coreq::service::unit(tool, &root, project);
        if field("LoadState=") != Some("loaded")
            || field("FragmentPath=") != unit.path.to_str()
            || !field("ExecStart=").is_some_and(|v| {
                v.starts_with(&format!(
                    "{{ path={} ; argv[]={} service run ;",
                    exe.display(),
                    exe.display()
                ))
            })
        {
            return Err("legacy systemd action/path/owner differs; kept".into());
        }
        let saved = fs::read_to_string(&unit.path).map_err(|e| e.to_string())?;
        if saved
            .lines()
            .filter(|l| l.starts_with("ExecStart="))
            .collect::<Vec<_>>()
            != [format!("ExecStart=\"{}\" service run", exe.display())]
            || saved
                .lines()
                .filter_map(|l| l.strip_prefix("WorkingDirectory="))
                .map(PathBuf::from)
                .collect::<Vec<_>>()
                != [root.clone()]
            || saved.lines().any(|l| {
                [
                    "ExecStartPre=",
                    "ExecStartPost=",
                    "ExecStop=",
                    "ExecStopPost=",
                    "ExecReload=",
                ]
                .iter()
                .any(|key| l.starts_with(key))
            })
        {
            return Err("legacy saved definition contains an unqualified action; kept".into());
        }
        return finish(tool, project.into(), root, expected, None).map(Some);
    }
    #[cfg(windows)]
    {
        let code = format!(
            r#"$ErrorActionPreference='Stop';[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false)
$self=Get-CimInstance Win32_Process -Filter 'ProcessId = {pid}';$sid=[Security.Principal.WindowsIdentity]::GetCurrent().User.Value
if(!$self -or (Get-Process -Id {pid}).StartTime.ToFileTimeUtc().ToString() -ne {started}){{throw 'legacy native incarnation differs'}}
$s=New-Object -ComObject 'Schedule.Service';$s.Connect();$hits=@()
foreach($t in $s.GetFolder('\').GetTasks(1)){{if(!$t.Name.StartsWith({prefix})){{continue}};foreach($i in @(Get-TaskQInstances $s $t)){{if($i.EnginePID -eq $self.ParentProcessId){{$hits+=,@($t,$i)}}}}}}
if($hits.Count -eq 0){{'null';exit 0}};if($hits.Count -ne 1){{throw 'multiple native legacy task bindings'}}
$t=$hits[0][0];$i=$hits[0][1];$d=$t.Definition;$a=$d.Actions.Item(1)
$parent=Get-CimInstance Win32_Process -Filter ('ProcessId = '+$i.EnginePID);$created=(Get-Process -Id $i.EnginePID).StartTime.ToFileTimeUtc().ToString()
if(!$parent -or $parent.ExecutablePath -ne {exe} -or [long]$created -gt [long]{started}){{throw 'legacy launcher scope differs'}}
$who=$d.Principal.UserId;try{{$who=[Security.Principal.SecurityIdentifier]::new($who).Value}}catch{{$who=[Security.Principal.NTAccount]::new($who).Translate([Security.Principal.SecurityIdentifier]).Value}}
if($who -ne $sid -or $d.Principal.LogonType -ne 3 -or $d.Principal.RunLevel -ne 0 -or $d.Actions.Count -ne 1 -or $a.Path -ne {exe} -or $a.Arguments -ne 'service run'){{throw 'legacy task action/principal differs'}}
if($d.Triggers.Count -ne 1 -or $d.Triggers.Item(1).Type -ne 9 -or !$d.Triggers.Item(1).Enabled){{throw 'legacy native trigger differs'}}
$triggerWho=$d.Triggers.Item(1).UserId;try{{$triggerWho=[Security.Principal.SecurityIdentifier]::new($triggerWho).Value}}catch{{$triggerWho=[Security.Principal.NTAccount]::new($triggerWho).Translate([Security.Principal.SecurityIdentifier]).Value}}
if($triggerWho -ne $sid){{throw 'legacy native trigger user differs'}}
$project=$t.Name.Substring({prefixlen});$root=$a.WorkingDirectory;$path=Join-Path {install} ($t.Name+'.xml');[xml]$saved=Get-Content -Raw -LiteralPath $path;$e=$saved.Task.Actions.Exec
$p=$saved.Task.Principals.Principal;$savedWho=[string]$p.UserId;try{{$savedWho=[Security.Principal.SecurityIdentifier]::new($savedWho).Value}}catch{{$savedWho=[Security.Principal.NTAccount]::new($savedWho).Translate([Security.Principal.SecurityIdentifier]).Value}}
if(@($saved.Task.Principals.Principal).Count -ne 1 -or $savedWho -ne $sid -or [string]$p.LogonType -ne 'InteractiveToken' -or [string]$p.RunLevel -ne 'LeastPrivilege' -or @($saved.Task.Actions.ChildNodes).Count -ne 1 -or [string]$e.Command -ne $a.Path -or [string]$e.Arguments -ne $a.Arguments -or [string]$e.WorkingDirectory -ne $root){{throw 'legacy saved task principal/action/directory differs'}}
if(@($saved.Task.Triggers.ChildNodes).Count -ne 1 -or @($saved.Task.Triggers.LogonTrigger).Count -ne 1 -or [string]$saved.Task.Triggers.LogonTrigger.Enabled -ne 'true'){{throw 'legacy saved trigger differs'}}
$savedTriggerWho=[string]$saved.Task.Triggers.LogonTrigger.UserId;try{{$savedTriggerWho=[Security.Principal.SecurityIdentifier]::new($savedTriggerWho).Value}}catch{{$savedTriggerWho=[Security.Principal.NTAccount]::new($savedTriggerWho).Translate([Security.Principal.SecurityIdentifier]).Value}}
if($savedTriggerWho -ne $sid){{throw 'legacy saved trigger user differs'}}
[pscustomobject]@{{project=$project;root=$root;launcher=@{{pid=$i.EnginePID;started=$created;instance=$i.InstanceGuid}}}}|ConvertTo-Json -Compress -Depth 4
"#,
            pid = expected.pid,
            started = quote(&expected.started),
            prefix = quote(&format!("{}-", tool.title)),
            prefixlen = tool.title.len() + 1,
            exe = quote(&exe.to_string_lossy()),
            install = quote(&tool.install.to_string_lossy())
        );
        let native = output("legacy.owner.read", &mut powershell(&code))?;
        let value: serde_json::Value = serde_json::from_slice(&native.stdout)
            .map_err(|e| format!("legacy native reply invalid: {e}"))?;
        if value.is_null() {
            return Ok(None);
        }
        let project = value["project"]
            .as_str()
            .ok_or("legacy project label missing")?;
        let root = PathBuf::from(value["root"].as_str().ok_or("legacy native root missing")?);
        return finish(
            tool,
            project.into(),
            root,
            expected,
            Some(value["launcher"].clone()),
        )
        .map(Some);
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    Err("native legacy ownership unsupported".into())
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    // H-A5 native adapter: an externally owned temporary legacy unit runs this process, with no board access.
    #[test]
    #[ignore = "requires TASKQ_NATIVE_LEGACY_CASE and a temporary launchd unit; never a production unit"]
    fn native_legacy_owner() {
        let install = PathBuf::from(
            std::env::var_os("TASKQ_NATIVE_LEGACY_CASE").expect("owned fixture required"),
        );
        let nonce = install
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .strip_prefix("taskq-legacy-native-")
            .unwrap();
        assert_eq!(nonce.len(), 32);
        assert!(nonce.bytes().all(|b| b.is_ascii_hexdigit()));
        let tool = coreq::Tool {
            name: "taskq-legacy-probe",
            title: "TaskQ-Legacy-Probe",
            repo: "fixture/unused",
            version: "fixture",
            install: install.clone(),
            fault: None,
        };
        let expected: serde_json::Value =
            serde_json::from_slice(&fs::read(install.join("fixture.json")).unwrap()).unwrap();
        let bound = owner(&tool)
            .unwrap()
            .expect("this fixture must own the native legacy unit");
        assert_eq!(
            bound.root,
            PathBuf::from(expected["root"].as_str().unwrap())
                .canonicalize()
                .unwrap()
        );
        assert_eq!(bound.project, expected["project"].as_str().unwrap());
        bound.verify(&tool).unwrap();
        assert_eq!(super::super::legacy_inventory(&tool,&[]).unwrap(),vec![format!("com.{}.{}",tool.name,bound.project)]);
        let mut wrong = bound.clone();
        wrong.started.push_str("-wrong");
        assert!(wrong.verify(&tool).is_err());
        let mut wrong = bound.clone();
        wrong.revision = "0".repeat(64);
        assert!(wrong.verify(&tool).is_err());
        let mut wrong = bound.clone();
        wrong.project.push_str("-wrong");
        assert!(wrong.verify(&tool).is_err());
        bound.verify(&tool).unwrap();
        coreq::write_durable(
            &install.join("legacy-owner.json"),
            serde_json::to_vec(&bound).unwrap(),
        )
        .unwrap();
        if std::env::var_os("TASKQ_LEGACY_HANDOFF_FIXTURE").is_some() {
            let mut command=super::super::controller_command(&std::env::current_exe().unwrap());
            command.args(["--exact","host_control::legacy::tests::native_legacy_retire_helper","--ignored","--nocapture"]);
            std::thread::spawn(move ||super::super::output("qualification.legacy.dispatch",&mut command).unwrap());
        }
        let until = Instant::now() + Duration::from_secs(60);
        while !install.join("fixture-exit").exists() {
            assert!(Instant::now() < until, "fixture controller did not finish");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    #[ignore="owned native legacy fixture helper; never a production unit"]
    fn native_legacy_retire_helper() {
        let install=PathBuf::from(std::env::var_os("TASKQ_NATIVE_LEGACY_CASE").expect("owned fixture required"));
        let nonce=install.file_name().unwrap().to_str().unwrap().strip_prefix("taskq-legacy-native-").unwrap();
        assert_eq!(nonce.len(),32);assert!(nonce.bytes().all(|b|b.is_ascii_hexdigit()));
        // Parent-owned output disappears during bootout; preserve the actual fixture failure.
        let errors=install.join("helper-panic.txt");
        std::panic::set_hook(Box::new(move |info| {let _=fs::write(&errors,info.to_string());}));
        let tool=coreq::Tool {name:"taskq-legacy-probe",title:"TaskQ-Legacy-Probe",repo:"fixture/unused",version:"fixture",install:install.clone(),fault:None};
        let bound:Owner=serde_json::from_slice(&fs::read(install.join("legacy-owner.json")).unwrap()).unwrap();
        let helper=coreq::process::identity(std::process::id()).unwrap().unwrap();
        coreq::write_durable(&install.join("helper-ready.json"),serde_json::to_vec(&serde_json::json!({"pid":helper.pid,"started":helper.started})).unwrap()).unwrap();
        let until=Instant::now()+Duration::from_secs(20);
        while !install.join("retire-now").exists() {assert!(Instant::now()<until,"fixture controller did not request retirement");std::thread::sleep(Duration::from_millis(20));}
        if coreq::process::identity(bound.pid).unwrap().as_ref()==Some(&bound.identity()) {
            let mut wrong=bound.clone();wrong.revision="0".repeat(64);assert!(wrong.retire(&tool,"fixture").is_err());
            assert_eq!(coreq::process::identity(bound.pid).unwrap().as_ref(),Some(&bound.identity()));
        }
        fs::write(install.join("helper-phase.txt"),"retire-live").unwrap();
        bound.retire(&tool,"fixture").unwrap();
        eprintln!("fixture controller continued after parent bootout");
        fs::write(install.join("helper-phase.txt"),"retire-replay").unwrap();
        bound.retire(&tool,"fixture").unwrap();assert!(bound.retired(&tool).unwrap());
        assert!(super::super::legacy_inventory(&tool,&[]).unwrap().is_empty());
        coreq::write_durable(&install.join("helper-retired.json"),serde_json::to_vec(&serde_json::json!({"pid":helper.pid,"started":helper.started})).unwrap()).unwrap();
    }
}
