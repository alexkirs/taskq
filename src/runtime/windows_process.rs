//! Native Windows execution scope; a launcher exit does not release its surviving descendants.
use std::{ffi::c_void, io, os::windows::{io::{AsRawHandle,FromRawHandle,OwnedHandle},process::CommandExt}, process::{Child,Command}, time::{Duration,Instant}};
use super::{Proc,StartError};
type Handle=*mut c_void;
#[link(name="kernel32")]
unsafe extern "system" {
    fn GetCurrentProcess()->Handle;
    fn DuplicateHandle(source:Handle,handle:Handle,target:Handle,remote:*mut Handle,access:u32,inherit:i32,options:u32)->i32;
    fn OpenProcess(access:u32,inherit:i32,pid:u32)->Handle;
    fn CreateJobObjectW(attributes:Handle,name:*const u16)->Handle;
    fn OpenJobObjectW(access:u32,inherit:i32,name:*const u16)->Handle;
    fn AssignProcessToJobObject(job:Handle,process:Handle)->i32;
    fn IsProcessInJob(process:Handle,job:Handle,result:*mut i32)->i32;
    fn QueryInformationJobObject(job:Handle,class:u32,data:Handle,size:u32,returned:*mut u32)->i32;
    fn WaitForSingleObject(handle:Handle,milliseconds:u32)->u32;
    fn TerminateJobObject(job:Handle,code:u32)->i32;
    fn CreateToolhelp32Snapshot(flags:u32,pid:u32)->Handle;
    fn Thread32First(snapshot:Handle,entry:*mut Thread)->i32;
    fn Thread32Next(snapshot:Handle,entry:*mut Thread)->i32;
    fn OpenThread(access:u32,inherit:i32,tid:u32)->Handle;
    fn GetProcessIdOfThread(thread:Handle)->u32;
    fn ResumeThread(thread:Handle)->u32;
}
#[repr(C)]
#[derive(Default)]
struct Thread {size:u32,usage:u32,id:u32,process:u32,priority:i32,delta:i32,flags:u32}
#[repr(C)]
#[derive(Default)]
struct Accounting {user:i64,kernel:i64,period_user:i64,period_kernel:i64,faults:u32,total:u32,active:u32,terminated:u32}

fn name(key:&str)->Result<Vec<u16>,String> {
    if !crate::data::job_key(key) {return Err("native job identity invalid".into());}
    Ok(format!("Global\\TaskQ-execution-{key}\0").encode_utf16().collect())
}
fn owned(raw:Handle,error:io::Error)->Result<OwnedHandle,String> {
    // Capture native errors before entering this automatically instrumented helper.
    if raw.is_null() || raw as isize == -1 {return Err(error.to_string());}
    // SAFETY: callers pass a newly created or opened native handle; OwnedHandle closes it on every path.
    Ok(unsafe {OwnedHandle::from_raw_handle(raw)})
}
fn active(job:&OwnedHandle)->Result<u32,String> {
    let mut info=Accounting::default();let mut returned=0;
    // SAFETY: JobObjectBasicAccountingInformation is the documented 48-byte layout.
    if unsafe {QueryInformationJobObject(job.as_raw_handle(),1,(&mut info as *mut Accounting).cast(),std::mem::size_of::<Accounting>() as u32,&mut returned)}==0 {return Err(io::Error::last_os_error().to_string());}
    if returned!=std::mem::size_of::<Accounting>() as u32 {return Err("native job accounting incomplete".into());}
    Ok(info.active)
}
fn members(job:&OwnedHandle,known:&mut std::collections::BTreeMap<u32,OwnedHandle>)->Result<(),String> {
    let mut ended=Vec::new();
    for (pid,handle) in known.iter() {
        match unsafe {WaitForSingleObject(handle.as_raw_handle(),0)} {
            0=>ended.push(*pid),258=>{},
            _=>return Err(io::Error::last_os_error().to_string()),
        }
    }
    for pid in ended {known.remove(&pid);}
    // Native membership is bounded; a larger/changing scope stays unknown instead of truncating proof.
    let mut ids=vec![0usize;4098];let size=(ids.len()*std::mem::size_of::<usize>()) as u32;let mut returned=0;
    if unsafe {QueryInformationJobObject(job.as_raw_handle(),3,ids.as_mut_ptr().cast(),size,&mut returned)}==0 {return Err(format!("native job membership unavailable or over4096 processes: {}",io::Error::last_os_error()));}
    let header=ids.as_ptr().cast::<u32>();
    let (assigned,count)=unsafe {(*header,*header.add(1))};
    if assigned!=count || count>4096 || returned<8+count*std::mem::size_of::<usize>() as u32 || returned>size {return Err("native job membership incomplete; retry observation".into());}
    let pids=unsafe {std::slice::from_raw_parts(ids.as_ptr().cast::<u8>().add(8).cast::<usize>(),count as usize)};
    for pid in pids {
        let pid=u32::try_from(*pid).ok().filter(|id|*id>0 && *id<=i32::MAX as u32).ok_or("native job process id invalid")?;
        if known.contains_key(&pid) {continue;}
        let raw=unsafe {OpenProcess(0x1000|0x00100000,0,pid)};
        let error=io::Error::last_os_error();
        if raw.is_null() && error.raw_os_error()==Some(87) {continue;}
        let handle=owned(raw,error)?;let mut member=0;
        if unsafe {IsProcessInJob(handle.as_raw_handle(),job.as_raw_handle(),&mut member)}==0 {return Err(io::Error::last_os_error().to_string());}
        if member!=0 {
            match unsafe {WaitForSingleObject(handle.as_raw_handle(),0)} {
                0=>{},
                258=>{if known.len()>=4096 {return Err("native job wait-handle limit; retain execution until cleanup".into());}known.insert(pid,handle);},
                _=>return Err(io::Error::last_os_error().to_string()),
            }
        }
    }
    Ok(())
}

fn resume(child:&Child)->Result<u32,String> {
    // A suspended, newly owned process has only its primary thread; keep its process handle alive throughout discovery.
    let snapshot=owned(unsafe {CreateToolhelp32Snapshot(4,0)},io::Error::last_os_error())?;
    let mut entry=Thread {size:std::mem::size_of::<Thread>() as u32,..Thread::default()};let mut found=None;
    let mut more=unsafe {Thread32First(snapshot.as_raw_handle(),&mut entry)};
    while more!=0 {
        if entry.process==child.id() {
            if found.is_some() {return Err("suspended native process has multiple threads".into());}
            found=Some(entry.id);
        }
        entry.size=std::mem::size_of::<Thread>() as u32;
        more=unsafe {Thread32Next(snapshot.as_raw_handle(),&mut entry)};
    }
    if io::Error::last_os_error().raw_os_error()!=Some(18) {return Err(io::Error::last_os_error().to_string());}
    let thread=owned(unsafe {OpenThread(2|0x0800,0,found.ok_or("suspended native thread missing")?)},io::Error::last_os_error())?;
    if unsafe {GetProcessIdOfThread(thread.as_raw_handle())}!=child.id() {return Err("native thread ownership changed".into());}
    let count=unsafe {ResumeThread(thread.as_raw_handle())};
    if count==u32::MAX {Err(io::Error::last_os_error().to_string())} else {Ok(count)}
}

pub struct Execution {
    members:std::collections::BTreeMap<u32,OwnedHandle>,
    child:Option<Child>,native:Option<coreq::process::Identity>,key:String,job:Option<OwnedHandle>,
    assigned:bool,scope_known:bool,terminal:Option<i32>,fault:Option<String>,cancelled:bool,
}
impl Execution {
    pub fn spawn(command:&mut Command)->Result<Self,StartError> {
        let key=super::uuid();let wide=name(&key)?;
        let raw=unsafe {CreateJobObjectW(std::ptr::null_mut(),wide.as_ptr())};
        let error=io::Error::last_os_error();let collision=error.raw_os_error()==Some(183);
        let job=owned(raw,error)?;
        if collision {return Err("native job identity already exists; no process started".into());}
        // Assignment precedes all user code. No breakaway or kill-on-close flags are set.
        let child=command.creation_flags(0x08000004).spawn().map_err(|e|StartError::io(e,None))?;
        let observed=coreq::process::identity(child.id());
        let native=observed.as_ref().ok().and_then(Clone::clone);
        let mut execution=Self {members:std::collections::BTreeMap::new(),child:Some(child),native,key,job:Some(job),assigned:false,scope_known:true,terminal:None,fault:None,cancelled:false};
        let prepared=(|| {
            if execution.native.is_none() {return Err(format!("suspended native identity unavailable: {observed:?}"));}
            if unsafe {AssignProcessToJobObject(execution.job.as_ref().unwrap().as_raw_handle(),execution.child.as_ref().unwrap().as_raw_handle())}==0 {return Err(io::Error::last_os_error().to_string());}
            execution.assigned=true;
            // Kernel association keeps the object alive, but only an open handle preserves its name.
            let mut remote=std::ptr::null_mut();
            if unsafe {DuplicateHandle(GetCurrentProcess(),execution.job.as_ref().unwrap().as_raw_handle(),execution.child.as_ref().unwrap().as_raw_handle(),&mut remote,4|8,0,0)}==0 {return Err(io::Error::last_os_error().to_string());}
            resume(execution.child.as_ref().unwrap())
        })();
        match prepared {
            Ok(1)=>Ok(execution),
            Ok(count)=>{execution.scope_known=false;execution.fault=Some(format!("native thread resume count {count}; scope unknown; inspect original execution before release"));Ok(execution)},
            Err(reason)=>{
                let cleanup=execution.kill();let until=Instant::now()+Duration::from_secs(2);
                while Instant::now()<until {
                    if execution.exited().is_ok_and(|ended|ended.is_some()) {return Err(format!("native setup failed before resume; child cleanup confirmed: {reason}").into());}
                    std::thread::sleep(Duration::from_millis(10));
                }
                execution.fault=Some(format!("native setup failed; cleanup unconfirmed: {reason}; stop={cleanup:?}"));Ok(execution)
            },
        }
    }
    pub fn recover(native:coreq::process::Identity,key:&str)->Result<Self,String> {
        Self::open(native,key)?.ok_or("native job name unavailable after handle loss; descendants may survive; native reconciliation required".into())
    }
    pub fn open(native:coreq::process::Identity,key:&str)->Result<Option<Self>,String> {
        let wide=name(key)?;
        let raw=unsafe {OpenJobObjectW(4|8,0,wide.as_ptr())};
        let error=io::Error::last_os_error();
        let job=if raw.is_null() {
            if error.raw_os_error()!=Some(2) {return Err(format!("native job readback unavailable: {error}"));}None
        } else {Some(owned(raw,error)?)};
        let current=coreq::process::identity(native.pid).map_err(|e|e.to_string())?;
        if current.as_ref()==Some(&native) && job.is_none() {return Err("native job missing while original launcher is live; reconcile scope".into());}
        if current.as_ref()==Some(&native) {
            let handle=owned(unsafe {OpenProcess(0x1000,0,native.pid)},io::Error::last_os_error())?;let mut member=0;
            if unsafe {IsProcessInJob(handle.as_raw_handle(),job.as_ref().unwrap().as_raw_handle(),&mut member)}==0 {return Err(io::Error::last_os_error().to_string());}
            if coreq::process::identity(native.pid).map_err(|e|e.to_string())?.as_ref()!=Some(&native) {return Err("native launcher changed during job membership read; retry observation".into());}
            if member==0 {return Err("native launcher does not belong to recorded job".into());}
        }
        if job.is_none() {return Ok(None);}
        Ok(Some(Self {members:std::collections::BTreeMap::new(),child:None,native:Some(native),key:key.into(),job,assigned:true,scope_known:true,terminal:None,fault:None,cancelled:false}))
    }
}
impl Proc for Execution {
    fn native_identity(&self)->Result<coreq::process::Identity,String> {self.native.clone().ok_or("native creation identity unavailable before resume".into())}
    fn job(&self)->Option<String> {(self.assigned && self.scope_known).then(||self.key.clone())}
    fn id(&self)->u32 {self.child.as_ref().map(Child::id).unwrap_or_else(||self.native.as_ref().unwrap().pid)}
    fn exited(&mut self)->Result<Option<i32>,String> {
        if self.terminal.is_some() {return Ok(self.terminal);}
        if !self.scope_known {return Err(self.fault.clone().unwrap_or("native execution scope unknown".into()));}
        let root=if let Some(child)=self.child.as_mut() {child.try_wait().map_err(|e|e.to_string())?.map(|s|s.code().unwrap_or(-1))}
            else {let native=self.native_identity()?;coreq::process::identity(native.pid).map_err(|e|e.to_string())?.filter(|id|id==&native).is_none().then_some(-1)};
        if let Some(job)=&self.job {members(job,&mut self.members)?;}
        let members=self.job.as_ref().map(active).transpose()?.unwrap_or(0);
        if let Some(code)=root {if members==0 && self.members.is_empty() {self.terminal=Some(if self.cancelled || self.fault.is_some() {-1} else {code});return Ok(self.terminal);}}
        if let Some(fault)=&self.fault {return Err(fault.clone());}
        Ok(None)
    }
    fn kill(&mut self)->Result<(),String> {
        if self.cancelled || self.terminal.is_some() || self.exited().is_ok_and(|ended|ended.is_some()) {return Ok(());}
        let result=if self.assigned {
            let job=self.job.as_ref().ok_or("native job unavailable; termination unknown")?;
            if unsafe {TerminateJobObject(job.as_raw_handle(),1)}==0 {Err(io::Error::last_os_error().to_string())} else {Ok(())}
        } else {
            self.child.as_mut().ok_or("native launcher handle missing")?.kill().map_err(|e|e.to_string())
        };
        if result.is_ok() {self.cancelled=true;}result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing_subscriber::{layer::{Context,SubscriberExt},Layer};
    #[link(name="kernel32")]
    unsafe extern "system" {fn SetLastError(code:u32);}
    struct ChangesNativeError;
    // The subscriber fixture must not instrument its own tracing callback recursively.
    macro_rules! native_callback {()=> {
        impl<S:tracing::Subscriber> Layer<S> for ChangesNativeError {
            fn on_enter(&self,_:&tracing::span::Id,_:Context<'_,S>) {unsafe {SetLastError(123);}}
        }
    }}
    native_callback!();
    // Chain: native failure -> automatically captured adapter -> original diagnostic.
    #[test]
    fn native_error_survives_method_instrumentation() {
        let subscriber=tracing_subscriber::registry().with(ChangesNativeError);
        tracing::subscriber::with_default(subscriber,|| {
            unsafe {SetLastError(5);}
            let error=owned(std::ptr::null_mut(),io::Error::last_os_error()).unwrap_err();
            assert_eq!(error,io::Error::from_raw_os_error(5).to_string(),"method telemetry must not replace the failing operation's native error");
        });
    }
}
