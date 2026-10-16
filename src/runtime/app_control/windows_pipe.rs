//! Local same-user named pipe; synchronous nonblocking polling keeps buffers caller-owned.
use std::{cell::Cell,ffi::c_void,fs::{File,OpenOptions},io::{self,Read,Write},os::windows::{fs::OpenOptionsExt,io::{AsRawHandle,FromRawHandle,OwnedHandle}},path::Path,time::{Duration,Instant}};
type Handle=*mut c_void;
#[link(name="kernel32")]
unsafe extern "system" {
    fn GetCurrentProcess()->Handle;
    fn OpenProcess(access:u32,inherit:i32,pid:u32)->Handle;
    fn GetNamedPipeServerProcessId(pipe:Handle,pid:*mut u32)->i32;
    fn SetNamedPipeHandleState(pipe:Handle,mode:*const u32,max:*const u32,timeout:*const u32)->i32;
    fn ReadFile(file:Handle,buffer:Handle,size:u32,read:*mut u32,overlapped:Handle)->i32;
    fn WriteFile(file:Handle,buffer:*const c_void,size:u32,written:*mut u32,overlapped:Handle)->i32;
}
#[link(name="advapi32")]
unsafe extern "system" {
    fn OpenProcessToken(process:Handle,access:u32,token:*mut Handle)->i32;
    fn GetTokenInformation(token:Handle,class:u32,buffer:Handle,size:u32,needed:*mut u32)->i32;
    fn EqualSid(first:Handle,second:Handle)->i32;
}
fn token_user(process:Handle)->io::Result<Vec<usize>> {
    let mut raw=std::ptr::null_mut();
    if unsafe {OpenProcessToken(process,8,&mut raw)}==0 {return Err(io::Error::last_os_error());}
    // SAFETY: successful native open transferred one token handle to this owner.
    let token=unsafe {OwnedHandle::from_raw_handle(raw)};let mut needed=0;
    let read=unsafe {GetTokenInformation(token.as_raw_handle(),1,std::ptr::null_mut(),0,&mut needed)};
    let error=io::Error::last_os_error();
    if read!=0 || error.raw_os_error()!=Some(122) || !(16..=65536).contains(&needed) {return Err(io::Error::other("native token-user size unavailable"));}
    let mut bytes=vec![0usize;(needed as usize).div_ceil(std::mem::size_of::<usize>())];let size=needed;
    if unsafe {GetTokenInformation(token.as_raw_handle(),1,bytes.as_mut_ptr().cast(),size,&mut needed)}==0 {return Err(io::Error::last_os_error());}
    if needed>size || needed<16 {return Err(io::Error::other("native token-user incomplete"));}
    Ok(bytes)
}
fn same_user(pipe:&File)->io::Result<()> {
    let mut pid=0;
    if unsafe {GetNamedPipeServerProcessId(pipe.as_raw_handle(),&mut pid)}==0 {return Err(io::Error::last_os_error());}
    let raw=unsafe {OpenProcess(0x1000,0,pid)};
    if raw.is_null() {return Err(io::Error::last_os_error());}
    let process=unsafe {OwnedHandle::from_raw_handle(raw)};
    let peer=token_user(process.as_raw_handle())?;let current=token_user(unsafe {GetCurrentProcess()})?;
    // TOKEN_USER starts with SID_AND_ATTRIBUTES; both native buffers retain their SID storage here.
    if unsafe {EqualSid(peer[0] as Handle,current[0] as Handle)}==0 {return Err(io::Error::new(io::ErrorKind::PermissionDenied,"Codex App IPC server belongs to another user"));}
    Ok(())
}
fn pause(until:Instant)->io::Result<()> {
    let left=until.saturating_duration_since(Instant::now());
    if left.is_zero() {return Err(io::Error::new(io::ErrorKind::TimedOut,"Codex App IPC deadline; cancellation outcome unknown"));}
    std::thread::sleep(left.min(Duration::from_millis(2)));Ok(())
}
pub(super) struct Stream {pipe:File,read_timeout:Cell<Duration>,write_timeout:Cell<Duration>}
impl Stream {
    fn from_file(pipe:File)->io::Result<Self> {
        let mode=1u32; // PIPE_READMODE_BYTE | PIPE_NOWAIT; every read/write returns immediately.
        if unsafe {SetNamedPipeHandleState(pipe.as_raw_handle(),&mode,std::ptr::null(),std::ptr::null())}==0 {return Err(io::Error::last_os_error());}
        Ok(Self {pipe,read_timeout:Cell::new(Duration::from_secs(10)),write_timeout:Cell::new(Duration::from_secs(10))})
    }
    pub(super) fn connect(path:&Path,until:Instant)->io::Result<Self> {
        let text=path.to_str().ok_or_else(||io::Error::other("native pipe path is not Unicode"))?;
        let key=text.strip_prefix(r"\\.\pipe\").ok_or_else(||io::Error::other("native pipe must be local"))?;
        if key.is_empty() || key.len()>128 || !key.bytes().all(|c|c.is_ascii_alphanumeric()||c==b'-') {return Err(io::Error::other("native pipe name invalid"));}
        loop {
            if Instant::now()>=until {return Err(io::Error::new(io::ErrorKind::TimedOut,"native pipe connect deadline"));}
            // Identification-only SQOS prevents a pipe server impersonating this client.
            match OpenOptions::new().read(true).write(true).custom_flags(0x00100000|0x00010000).open(path) {
                Ok(pipe)=>{same_user(&pipe)?;return Self::from_file(pipe);},
                Err(e) if e.raw_os_error()==Some(231)=>pause(until)?,
                Err(e)=>return Err(e),
            }
        }
    }
    pub(super) fn set_read_timeout(&self,value:Option<Duration>)->io::Result<()> {
        self.read_timeout.set(value.filter(|v|!v.is_zero()).ok_or_else(||io::Error::other("native pipe needs a positive read timeout"))?);Ok(())
    }
    pub(super) fn set_write_timeout(&self,value:Option<Duration>)->io::Result<()> {
        self.write_timeout.set(value.filter(|v|!v.is_zero()).ok_or_else(||io::Error::other("native pipe needs a positive write timeout"))?);Ok(())
    }
}
impl Read for Stream {
    fn read(&mut self,bytes:&mut [u8])->io::Result<usize> {
        if bytes.is_empty() {return Ok(0);}
        let until=Instant::now()+self.read_timeout.get();
        loop {
            let mut count=0;
            let read=unsafe {ReadFile(self.pipe.as_raw_handle(),bytes.as_mut_ptr().cast(),bytes.len().min(u32::MAX as usize) as u32,&mut count,std::ptr::null_mut())};
            let error=io::Error::last_os_error();
            if read!=0 {return Ok(count as usize);}
            // std File reads flatten ERROR_NO_DATA to EOF; preserve native pending versus disconnect.
            if error.raw_os_error()==Some(232) {pause(until)?;} else {return Err(error);}
        }
    }
}
impl Write for Stream {
    fn write(&mut self,bytes:&[u8])->io::Result<usize> {
        if bytes.is_empty() {return Ok(0);}
        let until=Instant::now()+self.write_timeout.get();
        loop {
            let mut count=0;
            // An oversized NOWAIT write can report success with zero bytes despite free quota.
            let written=unsafe {WriteFile(self.pipe.as_raw_handle(),bytes.as_ptr().cast(),bytes.len().min(4096) as u32,&mut count,std::ptr::null_mut())};
            let error=io::Error::last_os_error();
            if written!=0 && count>0 {return Ok(count as usize);}
            if written!=0 || error.raw_os_error()==Some(232) {pause(until)?;} else {return Err(error);}
        }
    }
    fn flush(&mut self)->io::Result<()> {Ok(())}
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use std::os::windows::ffi::OsStrExt;
    #[link(name="kernel32")]
    unsafe extern "system" {
        fn CreateNamedPipeW(name:*const u16,open:u32,mode:u32,max:u32,out:u32,input:u32,timeout:u32,security:Handle)->Handle;
        fn ConnectNamedPipe(pipe:Handle,overlapped:Handle)->i32;
    }
    pub struct Listener(File);
    impl Listener {
        pub fn bind(path:&Path)->io::Result<Self> {
            let wide:Vec<u16>=path.as_os_str().encode_wide().chain(Some(0)).collect();
            let raw=unsafe {CreateNamedPipeW(wide.as_ptr(),3|0x00080000,1|8,1,4096,4096,0,std::ptr::null_mut())};
            if raw as isize == -1 {return Err(io::Error::last_os_error());}
            Ok(Self(unsafe {File::from_raw_handle(raw)}))
        }
        pub fn accept(self)->io::Result<(Stream,())> {
            let until=Instant::now()+Duration::from_secs(3);
            loop {
                let connected=unsafe {ConnectNamedPipe(self.0.as_raw_handle(),std::ptr::null_mut())};let error=io::Error::last_os_error();
                if connected!=0 || error.raw_os_error()==Some(535) {
                    let stream=Stream::from_file(self.0)?;
                    stream.set_read_timeout(Some(Duration::from_secs(2)))?;stream.set_write_timeout(Some(Duration::from_secs(2)))?;
                    return Ok((stream,()));
                }
                if !matches!(error.raw_os_error(),Some(536|232)) {return Err(error);}
                pause(until)?;
            }
        }
    }
    // Chain: silent native peer -> saturated client writes -> bounded timeout, no retained I/O.
    #[test]
    fn native_pipe_bounds_unread_writes_and_empty_reads() {
        let remote=Stream::connect(Path::new(r"\\other-host\pipe\codex-ipc"),Instant::now()+Duration::from_secs(1)).err().unwrap();
        assert!(remote.to_string().contains("must be local"),"remote endpoints must be rejected before connection");
        for mode in ["read","write"] {
            let path=std::path::PathBuf::from(format!(r"\\.\pipe\taskq-pipe-{}",super::super::super::uuid()));
            let listener=Listener::bind(&path).unwrap();
            let (finished,completion)=std::sync::mpsc::channel();
            let peer=std::thread::spawn(move|| {let (_stream,_)=listener.accept().unwrap();let _=completion.recv_timeout(Duration::from_secs(5));});
            let mut stream=Stream::connect(&path,Instant::now()+Duration::from_secs(1)).unwrap();
            stream.set_read_timeout(Some(Duration::from_millis(100))).unwrap();stream.set_write_timeout(Some(Duration::from_millis(100))).unwrap();
            let began=Instant::now();
            let error=if mode=="read" {stream.read_exact(&mut [0]).unwrap_err()} else {stream.write_all(&vec![0;1024*1024]).unwrap_err()};
            assert_eq!(error.kind(),io::ErrorKind::TimedOut,"{mode}: {error}");assert!(began.elapsed()<Duration::from_secs(1));
            drop(stream);let _=finished.send(());peer.join().unwrap();
        }
    }
}
