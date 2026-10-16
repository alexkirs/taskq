//! Parent desktop IPC; exact-turn cancellation never establishes native exit.
use std::{io::{Read,Write},path::Path,time::{Duration,Instant}};
#[cfg(target_os="macos")]
use std::{fs,os::unix::{fs::{FileTypeExt,MetadataExt},net::UnixStream as Stream},path::PathBuf};
#[cfg(windows)]
mod windows_pipe;
#[cfg(windows)]
use windows_pipe::Stream;
use serde_json::{Value,json};

pub(super) fn cancel(thread:&str,turn:&str)->Result<(),String> {
    coreq::telemetry::capture::operation("runtime.app-cancel",|| {
        #[cfg(windows)]
        return cancel_at(Path::new(r"\\.\pipe\codex-ipc"),thread,turn,super::observation_deadline(Duration::from_secs(10)));
        #[cfg(target_os="macos")]
        {
        let home=std::env::var_os("CODEX_HOME").map(PathBuf::from)
            .or_else(||std::env::var_os("HOME").map(|h|Path::new(&h).join(".codex"))).ok_or("Codex App home unavailable")?;
        cancel_at(&home.join("ipc/ipc.sock"),thread,turn,super::observation_deadline(Duration::from_secs(10)))
        }
    }, |_|"app-cancel")
}

fn cancel_at(path:&Path,thread:&str,turn:&str,until:Instant)->Result<(),String> {
    if !id(thread)||!id(turn) {return Err("Codex App cancellation identity invalid".into());}
    #[cfg(target_os="macos")]
    {
    let parent=path.parent().ok_or("Codex App IPC parent missing")?;
    let uid=unsafe {libc::geteuid()};
    for (p,directory) in [(parent,true),(path,false)] {
        let meta=fs::symlink_metadata(p).map_err(|_|"Codex App IPC unavailable")?;
        if meta.uid()!=uid || meta.mode()&0o022!=0 || if directory {!meta.is_dir()} else {!meta.file_type().is_socket()} {
            return Err("Codex App IPC ownership/protection invalid".into());
        }
    }
    }
    #[cfg(target_os="macos")]
    let stream=coreq::local_socket::connect(path,until)?;
    #[cfg(windows)]
    let stream=Stream::connect(path,until).map_err(|e|format!("Codex App IPC connection unknown: {e}"))?;
    let mut ipc=Ipc {stream,until,received:0,client:"initializing-client".into()};
    let init=ipc.request("initialize",json!({"clientType":"cli"}),0,None)?;
    let client=init["result"]["clientId"].as_str().filter(|s|id(s)).ok_or("Codex App IPC client identity missing")?;
    if init["handledByClientId"]!=client {return Err("Codex App IPC client identity mismatch".into());}
    ipc.client=client.into();
    let found=ipc.request("thread-owner-discovery",json!({"hostId":"local","conversationId":thread}),1,None)?;
    let owner=found["handledByClientId"].as_str().filter(|s|id(s)&&*s!=ipc.client).ok_or("Codex App owner unavailable")?;
    let stopped=ipc.request("thread-follower-interrupt-turn",json!({"conversationId":thread,"mode":"system","expectedTurnId":turn}),4,Some(owner))?;
    if stopped["handledByClientId"]!=owner || stopped["result"]["ok"]!=true || stopped["result"]["interruptedTurnId"]!=turn {
        return Err("Codex App exact-turn cancellation unconfirmed; retain execution and reconcile native history".into());
    }
    Ok(())
}
fn id(s:&str)->bool { !s.is_empty() && s.len()<=128 && s.bytes().all(|b|b.is_ascii_alphanumeric()||b==b'-') }

const RESPONSE_LIMIT:usize=4*1024*1024;
const FRAME_LIMIT:usize=1024*1024;
fn frame_end(received:usize,size:usize)->Result<usize,String> {
    received.checked_add(4).and_then(|n|n.checked_add(size))
        .filter(|end|size>0 && size<=FRAME_LIMIT && *end<=RESPONSE_LIMIT)
        .ok_or_else(||"Codex App IPC response budget exceeded".into())
}

struct Ipc {stream:Stream,until:Instant,received:usize,client:String}
impl Ipc {
    fn left(&self)->Result<Duration,String> {
        let left=self.until.saturating_duration_since(Instant::now());
        if left.is_zero() {Err("Codex App IPC deadline; cancellation outcome unknown".into())} else {Ok(left)}
    }
    fn read(&mut self,mut bytes:&mut [u8])->Result<(),String> {
        while !bytes.is_empty() {
            self.stream.set_read_timeout(Some(self.left()?)).map_err(|_|"Codex App IPC read timeout unavailable")?;
            match self.stream.read(bytes) {
                Ok(0)=>return Err("Codex App IPC disconnected; cancellation outcome unknown".into()),
                Ok(n)=>bytes=&mut bytes[n..],
                Err(e) if e.kind()==std::io::ErrorKind::Interrupted=>{},
                Err(e)=>return Err(format!("Codex App IPC read failed: {e}; {} bytes pending; cancellation outcome unknown",bytes.len())),
            }
        } Ok(())
    }
    fn send(&mut self,value:&Value)->Result<(),String> {
        let body=serde_json::to_vec(value).map_err(|_|"Codex App IPC request invalid")?;
        if body.len()>4096 {return Err("Codex App IPC request too large".into());}
        let mut wire=(body.len() as u32).to_le_bytes().to_vec();wire.extend(body);let mut bytes=wire.as_slice();
        while !bytes.is_empty() {
            self.stream.set_write_timeout(Some(self.left()?)).map_err(|_|"Codex App IPC write timeout unavailable")?;
            match self.stream.write(bytes) {
                Ok(0)=>return Err("Codex App IPC write incomplete; cancellation outcome unknown".into()),
                Ok(n)=>bytes=&bytes[n..],
                Err(e) if e.kind()==std::io::ErrorKind::Interrupted=>{},
                Err(e)=>return Err(format!("Codex App IPC write failed: {e}; cancellation outcome unknown")),
            }
        } Ok(())
    }
    fn request(&mut self,method:&str,params:Value,version:u32,target:Option<&str>)->Result<Value,String> {
        let request=super::uuid();let mut value=json!({"type":"request","requestId":request,"sourceClientId":self.client,"version":version,"method":method,"params":params,"timeoutMs":4000});
        if let Some(target)=target {value["targetClientId"]=target.into();}
        self.send(&value)?;
        loop {
            if self.received>RESPONSE_LIMIT-4 {return Err("Codex App IPC response budget exceeded".into());}
            let mut prefix=[0;4];self.read(&mut prefix)?;let size=u32::from_le_bytes(prefix) as usize;
            self.received=frame_end(self.received,size)?;
            let mut body=vec![0;size];self.read(&mut body)?;
            let reply:Value=serde_json::from_slice(&body).map_err(|_|"Codex App IPC malformed response")?;
            if reply["type"]=="client-discovery-request" {
                let rid=reply["requestId"].as_str().filter(|s|id(s)).ok_or("Codex App IPC discovery identity invalid")?;
                self.send(&json!({"type":"client-discovery-response","requestId":rid,"response":{"canHandle":false}}))?;
            }
            if reply["type"]!="response" || reply["requestId"]!=request {continue;}
            if reply["resultType"]!="success" {
                let reason:String=reply["error"].as_str().unwrap_or("native error without message").chars()
                    .map(|c|if c.is_control() {' '} else {c}).take(160).collect();
                return Err(format!("Codex App IPC {method} refused: {reason}; retain execution and reconcile native history"));
            }
            if reply["method"]!=method {return Err("Codex App IPC response method mismatch".into());}
            return Ok(reply);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    #[cfg(target_os="macos")]
    use std::os::unix::{fs::PermissionsExt,net::UnixListener as Listener};
    #[cfg(windows)]
    use super::windows_pipe::tests::Listener;
    // Chain: installed desktop -> absent exact owner -> explicit refusal before any interruption.
    #[test]
    #[ignore="read-only installed desktop IPC qualification; no model, queue or existing turn control"]
    fn native_absent_owner_never_sends_interrupt() {
        let thread=super::super::uuid();let turn=super::super::uuid();let began=Instant::now();
        let error=cancel(&thread,&turn).unwrap_err();
        assert!(error.contains("thread-owner-discovery refused: no-client-found"),"{error}");
        assert!(began.elapsed()<Duration::from_secs(11));
        println!("native desktop same-user connection and absent-owner refusal passed; no interrupt or submission: {error}");
    }

    // Chain: untrusted frame sizes -> cumulative wire cap, independent of native transfer speed.
    #[test]
    fn response_budget_counts_prefixes_and_refuses_overflow() {
        let mut received=0;
        for _ in 0..3 {received=frame_end(received,FRAME_LIMIT).unwrap();}
        assert_eq!(frame_end(received,FRAME_LIMIT-16).unwrap(),RESPONSE_LIMIT);
        assert!(frame_end(received,FRAME_LIMIT-12).is_err(),"payload-only accounting would admit an extra prefix");
        for (received,size) in [(0,0),(0,FRAME_LIMIT+1),(RESPONSE_LIMIT,1),(usize::MAX,1)] {
            assert!(frame_end(received,size).is_err());
        }
    }

    // Chain: exact input -> current owner -> addressed interrupt -> validated receipt or explicit unknown.
    // Risk: stale/foreign responses, partial I/O or a silent peer release or stall an execution.
    #[test]
    fn cancellation_is_owner_bound_bounded_and_never_replays_work() {
        for case in ["ok","client","owner","turn","null","method","refused","oversize","broadcast","malformed","drop","timeout","unrelated","directory-mode","socket-mode","symlink"] {
            let root=std::env::temp_dir().join(format!("qipc-{}",super::super::uuid()));fs::create_dir(&root).unwrap();
            #[cfg(target_os="macos")]
            let path=root.join("s");
            #[cfg(windows)]
            let path=std::path::PathBuf::from(format!(r"\\.\pipe\taskq-ipc-{}",super::super::uuid()));
            #[cfg(windows)]
            if matches!(case,"directory-mode"|"socket-mode"|"symlink") {fs::remove_dir_all(root).unwrap();continue;}
            let listener=Listener::bind(&path).unwrap();
            #[cfg(target_os="macos")]
            fs::set_permissions(&path,fs::Permissions::from_mode(0o600)).unwrap();
            #[cfg(target_os="macos")]
            if matches!(case,"directory-mode"|"socket-mode"|"symlink") {
                let checked=if case=="symlink" {
                    let link=root.join("link");std::os::unix::fs::symlink(&path,&link).unwrap();link
                } else {
                    fs::set_permissions(if case=="directory-mode" {&root} else {&path},fs::Permissions::from_mode(0o777)).unwrap();path.clone()
                };
                assert!(cancel_at(&checked,"thread","turn",Instant::now()+Duration::from_secs(1)).unwrap_err().contains("ownership/protection"));
                listener.set_nonblocking(true).unwrap();
                assert_eq!(listener.accept().unwrap_err().kind(),std::io::ErrorKind::WouldBlock,"untrusted endpoint must not receive a request");
                fs::remove_dir_all(root).unwrap();continue;
            }
            let (finished,completion)=std::sync::mpsc::channel();
            let server=std::thread::spawn(move|| {
                let (mut stream,_)=listener.accept().unwrap();stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                stream.set_write_timeout(Some(Duration::from_secs(2))).unwrap();
                for method in ["initialize","thread-owner-discovery","thread-follower-interrupt-turn"] {
                    let mut size=[0;4];stream.read_exact(&mut size).unwrap();let size=u32::from_le_bytes(size) as usize;assert!(size<=4096);
                    let mut bytes=vec![0;size];stream.read_exact(&mut bytes).unwrap();let request:Value=serde_json::from_slice(&bytes).unwrap();
                    assert_eq!(request["method"],method,"cancellation never queues or resumes a turn");
                    let mut result=match method {
                        "initialize"=>json!({"clientId":"client"}),
                        "thread-owner-discovery"=>{assert_eq!(request["params"]["conversationId"],"thread");json!({})},
                        _=>{assert_eq!(request["targetClientId"],"owner");assert_eq!(request["version"],4);assert_eq!(request["params"],json!({"conversationId":"thread","expectedTurnId":"turn","mode":"system"}));json!({"ok":true,"interruptedTurnId":"turn"})},
                    };
                    if method=="initialize" && case=="client" {result["clientId"]="wrong".into();}
                    let control=method=="thread-follower-interrupt-turn";
                    let mut reply=json!({"type":"response","requestId":request["requestId"],"method":method,"resultType":"success","handledByClientId":if method=="initialize" {"client"} else {"owner"},"result":result});
                    if control {
                        match case {
                            "owner"=>reply["handledByClientId"]="foreign".into(),
                            "turn"=>reply["result"]["interruptedTurnId"]="foreign".into(),
                            "null"=>reply["result"]["interruptedTurnId"]=Value::Null,
                            "method"=>reply["method"]="foreign".into(),
                            "refused"=>{reply["resultType"]="error".into();reply["error"]="request-version-mismatch".into();},
                            "oversize"=>{stream.write_all(&(1024u32*1024+1).to_le_bytes()).unwrap();return;},
                            "broadcast"=>{
                                let body=serde_json::to_vec(&json!({"type":"broadcast","event":"changed"})).unwrap();
                                stream.write_all(&(body.len() as u32).to_le_bytes()).unwrap();stream.write_all(&body).unwrap();
                            },
                            "malformed"=>{stream.write_all(&[1,0,0,0,b'{']).unwrap();return;},
                            "drop"=>return,
                            "timeout"=>{let _=completion.recv_timeout(Duration::from_secs(5));return;},
                            "unrelated"=>reply["requestId"]="another-request".into(),
                            _=>{},
                        }
                    }
                    let body=serde_json::to_vec(&reply).unwrap();stream.write_all(&(body.len() as u32).to_le_bytes()).unwrap();
                    // Fragmentation cannot turn one frame into another response.
                    for chunk in body.chunks(7) {stream.write_all(chunk).unwrap();}
                    if case=="client" {return;}
                }
                let _=stream.read(&mut [0]);
            });
            let began=Instant::now();let budget=if case=="timeout" {Duration::from_millis(150)} else {Duration::from_secs(3)};
            let outcome=cancel_at(&path,"thread","turn",began+budget);
            let _=finished.send(());
            assert_eq!(outcome.is_ok(),matches!(case,"ok"|"broadcast"),"{case}: {outcome:?}");assert!(began.elapsed()<budget+Duration::from_secs(1));
            if case=="refused" {assert!(outcome.as_ref().unwrap_err().contains("request-version-mismatch"));}
            server.join().unwrap();fs::remove_dir_all(root).unwrap();
        }
    }
}
