//! 仅供 S7 独立 example；Get/Set 的四元组接口与身份核验并非原子操作。
use serde_json::{Value, json};
use std::{fs::{self, File, OpenOptions}, io::Write, net::Ipv4Addr, path::PathBuf, time::{Duration, Instant, SystemTime, UNIX_EPOCH}};

type Result<T> = std::result::Result<T, String>;
const INTERVAL_MS: u64 = 1000;
const MAX_SECONDS: u64 = 180;

#[derive(Clone, Debug, PartialEq)]
struct Process { pid: u32, created: u64, exe: String }
impl Process {
    fn value(&self) -> Value { json!({"pid":self.pid,"createdFiletime":self.created.to_string(),"exeAbsolutePath":self.exe}) }
}
#[derive(Clone, Debug, PartialEq)]
struct Row { pid:u32, state:u32, local:u32, local_port:u32, remote:u32, remote_port:u32, created:i64 }
impl Row {
    fn value(&self) -> Value { json!({"localIp":Ipv4Addr::from(self.local.to_ne_bytes()).to_string(),"localPort":u16::from_be(self.local_port as u16),"remoteIp":Ipv4Addr::from(self.remote.to_ne_bytes()).to_string(),"remotePort":u16::from_be(self.remote_port as u16),"connectionCreatedFiletime":self.created.to_string()}) }
    fn tuple_eq(&self, r:&Self) -> bool { (self.local,self.local_port,self.remote,self.remote_port)==(r.local,r.local_port,r.remote,r.remote_port) }
}
#[derive(Clone)]
struct Config { process:Process, local:u32, remote:u32, port:u32, duration_ms:u64, dir:PathBuf }
#[derive(Clone, Copy, Debug, PartialEq)]
enum Group { Path, SndCong }
const GROUPS:[Group;2]=[Group::Path,Group::SndCong];
impl Group {
    fn index(self)->usize { match self { Self::Path=>0,Self::SndCong=>1 } }
    fn size(self)->usize { match self { Self::Path=>160,Self::SndCong=>88 } }
}

// 私有注入边界：测试替换整个宿主，不发起 native、文件或睡眠操作。
trait Backend {
    fn elapsed(&self)->u64;
    fn sleep(&mut self, ms:u64);
    fn stopped(&self)->bool;
    fn process(&mut self)->Result<Option<Process>>;
    fn rows(&mut self)->Result<Vec<Row>>;
    fn get(&mut self,row:&Row,group:Group,dynamic:bool)->Result<(u8,Vec<u8>)>;
    fn set(&mut self,row:&Row,group:Group,value:u8)->Result<()>;
    fn emit(&mut self,value:Value)->Result<()>;
    fn ready(&mut self,row:&Row)->Result<()>;
}
#[derive(Debug,PartialEq)]
enum Identity { Same, Terminated, Unsafe }
fn check_active(b:&impl Backend,c:&Config)->Result<()> {
    if b.stopped() { return Err("stop_requested".into()); }
    if b.elapsed()>=c.duration_ms { return Err("observation_deadline".into()); }
    Ok(())
}
fn identity(b:&mut impl Backend,c:&Config,r:&Row,restoring:bool)->Result<Identity> {
    if !restoring { check_active(b,c)?; }
    let p=b.process()?;
    // 身份核验也受观测期限约束；恢复时即使进程退出也查表，识别同四元组替代行。
    if !restoring { check_active(b,c)?; }
    let rows=b.rows()?;
    let tuple:Vec<_>=rows.iter().filter(|x|x.tuple_eq(r)).collect();
    if tuple.is_empty() { return Ok(Identity::Terminated); }
    if p.as_ref()!=Some(&c.process) || tuple.len()!=1 || tuple[0]!=r { return Ok(Identity::Unsafe); }
    if r.state!=5 { return Ok(Identity::Terminated); }
    Ok(Identity::Same)
}
fn require_same(b:&mut impl Backend,c:&Config,r:&Row,restoring:bool)->Result<()> {
    match identity(b,c,r,restoring)? { Identity::Same=>Ok(()),x=>Err(format!("connection_identity:{x:?}")) }
}
fn boolean(v:u8)->Result<bool> { match v {0=>Ok(false),1=>Ok(true),_=>Err(format!("invalid_BOOLEAN:{v}"))} }

fn get(b:&mut impl Backend,c:&Config,r:&Row,g:Group,dynamic:bool,restoring:bool)->Result<(bool,Vec<u8>)> {
    require_same(b,c,r,restoring)?;
    if !restoring { check_active(b,c)?; }
    let start=b.elapsed();
    let result=b.get(r,g,dynamic);
    let end=b.elapsed();
    b.emit(json!({"event":"get","group":format!("{g:?}"),"startMs":start,"endMs":end,"dynamic":dynamic,"apiStatus":if result.is_ok() {json!(0)} else {json!(result.as_ref().err())},"enableCollectionByte":result.as_ref().ok().map(|v|v.0)}))?;
    let (rw,rod)=result?;
    Ok((boolean(rw)?,rod))
}
fn set(b:&mut impl Backend,c:&Config,r:&Row,g:Group,value:u8,changed:&mut [bool;2])->Result<()> {
    // 本观察器仅用 1 启用、用 0 恢复；恢复必须在 stop/deadline 后继续执行。
    let restoring=value==0;
    require_same(b,c,r,restoring)?;
    if !restoring { check_active(b,c)?; }
    let start=b.elapsed();
    let result=b.set(r,g,value);
    // 成功后先登记，再进行日志/读回；日志失败同样必须恢复。
    if result.is_ok() && value==1 { changed[g.index()]=true; }
    b.emit(json!({"event":"set","group":format!("{g:?}"),"value":value,"startMs":start,"endMs":b.elapsed(),"apiStatus":if result.is_ok() {json!(0)} else {json!(result.as_ref().err())}}))?;
    result
}

fn u32_at(bytes:&[u8],offset:usize)->Result<u32> {
    let raw=bytes.get(offset..offset+4).ok_or("ROD_length")?;
    Ok(u32::from_ne_bytes(raw.try_into().map_err(|_|"ROD_length")?))
}
fn fields(g:Group,bytes:&[u8])->Result<Value> {
    if bytes.len()!=g.size() { return Err("ROD_length".into()); }
    let selected:&[(&str,usize)]=match g {
        Group::Path=>&[("FastRetran",0),("Timeouts",4),("SubsequentTimeouts",8),("PktsRetrans",20),("BytesRetrans",24),("DupAcksIn",28),("CongSignals",40),("SampleRtt",104),("SmoothedRtt",108),("CurRto",132)],
        Group::SndCong=>&[("SndLimTransRwin",0),("SndLimTimeRwin",4),("SndLimTransCwnd",16),("SndLimTimeCwnd",20),("SndLimTransSnd",32),("SndLimTimeSnd",36),("CurCwnd",60),("CurSsthresh",72)],
    };
    let mut out=serde_json::Map::new();
    for &(name,offset) in selected { out.insert(name.into(),json!(u32_at(bytes,offset)?)); }
    Ok(Value::Object(out))
}
fn delta(old:&Value,new:&Value)->Value {
    let mut out=serde_json::Map::new();
    for name in ["FastRetran","Timeouts","SubsequentTimeouts","PktsRetrans","BytesRetrans","DupAcksIn","CongSignals","SndLimTransRwin","SndLimTimeRwin","SndLimTransCwnd","SndLimTimeCwnd","SndLimTransSnd","SndLimTimeSnd"] {
        if let (Some(a),Some(z))=(old[name].as_u64(),new[name].as_u64()) { out.insert(name.into(),json!({"value":(z as u32).wrapping_sub(a as u32),"wrapped":z<a})); }
    }
    Value::Object(out)
}
struct Outcome { error:Option<String>, restoration:&'static str, restoration_errors:Vec<String>, row:Option<Row>, samples:u64, first:[Option<Value>;2], last:[Option<Value>;2] }
fn observe(b:&mut impl Backend,c:&Config)->Outcome {
    let mut frozen=None;
    let mut changed=[false;2];
    let mut samples=0;
    let mut previous:[Option<Value>;2]=[None,None];
    let mut first:[Option<Value>;2]=[None,None];
    let mut last:[Option<Value>;2]=[None,None];
    let result=(||->Result<()> {
        let row=loop {
            if b.stopped() { return Err("stop_before_ready".into()); }
            if b.elapsed()>=c.duration_ms { return Err("deadline_before_ready".into()); }
            if b.process()?.as_ref()!=Some(&c.process) { return Err("process_identity".into()); }
            check_active(b,c)?;
            let rows:Vec<_>=b.rows()?.into_iter().filter(|r|r.pid==c.process.pid && r.state==5 && r.local==c.local && r.remote==c.remote && r.remote_port==c.port).collect();
            check_active(b,c)?;
            match rows.len() { 1=>break rows[0].clone(),0=>b.sleep(INTERVAL_MS.min(c.duration_ms.saturating_sub(b.elapsed()))),_=>return Err("ambiguous_connection".into()) }
        };
        frozen=Some(row.clone());
        // 两组原值完整读完后才允许第一条 Set。
        let mut original=[false;2];
        for g in GROUPS {
            if b.stopped() || b.elapsed()>=c.duration_ms {return Err("stopped_during_preparation".into());}
            original[g.index()]=get(b,c,&row,g,false,false)?.0;
        }
        for g in GROUPS {
            if b.stopped() || b.elapsed()>=c.duration_ms {return Err("stopped_during_preparation".into());}
            if !original[g.index()] { set(b,c,&row,g,1,&mut changed)?; }
            if !get(b,c,&row,g,false,false)?.0 { return Err(format!("enable_readback_false:{g:?}")); }
        }
        let mut next=b.elapsed();
        loop {
            if b.stopped() || b.elapsed()>=c.duration_ms { break; }
            if b.elapsed()<next { b.sleep(next.saturating_sub(b.elapsed()).min(100)); continue; }
            for g in GROUPS {
                let start=b.elapsed();
                let (enabled,raw)=get(b,c,&row,g,true,false)?;
                check_active(b,c)?;
                if !enabled { return Err(format!("sample_disabled:{g:?}")); }
                let values=fields(g,&raw)?;
                let difference=previous[g.index()].as_ref().map(|p|delta(p,&values));
                let snapshot=json!({"startMs":start,"endMs":b.elapsed(),"fields":values});
                if first[g.index()].is_none() {first[g.index()]=Some(snapshot.clone());}
                last[g.index()]=Some(snapshot);
                b.emit(json!({"event":"sample","group":format!("{g:?}"),"startMs":start,"endMs":b.elapsed(),"fields":values,"deltaPrevious":difference,"counterArithmetic":"u32 wrapping; multiple wraps unknown","units":{"rtt":"ms","rto":"ms","SndLimTime":"ms","cwnd":"bytes","ssthresh":"bytes"}}))?;
                previous[g.index()]=Some(values);
            }
            samples+=1;
            if samples==1 { check_active(b,c)?; b.ready(&row)?; }
            // 每次实际采样开始至少间隔一秒，不追赶延迟造成密集采样。
            next=b.elapsed().saturating_add(INTERVAL_MS);
        }
        if samples==0 { return Err("no_valid_sample".into()); }
        Ok(())
    })();
    // 所有可控出口均经此路径。同步API、崩溃及硬杀不具备硬实时 finally 保证。
    let mut restoration_errors=Vec::new();
    let restoration=if let Some(row)=&frozen {
        match identity(b,c,row,true) {
            Ok(Identity::Terminated)=>"connection_terminated",
            Ok(Identity::Unsafe)=>"identity_unsafe_unknown",
            Err(e)=>{restoration_errors.push(e);"identity_unsafe_unknown"},
            Ok(Identity::Same)=> {
                let mut failed=false;
                for g in GROUPS.into_iter().rev() {
                    if changed[g.index()] {
                        if let Err(e)=set(b,c,row,g,0,&mut changed) { failed=true; restoration_errors.push(format!("{g:?}:{e}")); continue; }
                        match get(b,c,row,g,false,true) { Ok((false,_))=>{},Ok((true,_))=>{failed=true;restoration_errors.push(format!("{g:?}:restore_readback_true"));},Err(e)=>{failed=true;restoration_errors.push(format!("{g:?}:{e}"));} }
                    }
                }
                if failed {
                    match identity(b,c,row,true) {
                        Ok(Identity::Terminated)=>"connection_terminated",
                        Ok(Identity::Unsafe)=>"identity_unsafe_unknown",
                        Err(e)=>{restoration_errors.push(e);"identity_unsafe_unknown"},
                        Ok(Identity::Same)=>"failed",
                    }
                } else { "restored" }
            }
        }
    } else { "restored" };
    Outcome { error:result.err(),restoration,restoration_errors,row:frozen,samples,first,last }
}

fn parse(args:Vec<String>)->Result<Option<Config>> {
    if args.is_empty() { return Ok(None); }
    if args.first().map(String::as_str)!=Some("--run") { return Err("需要显式 --run".into()); }
    let mut map=std::collections::BTreeMap::new();
    let mut it=args.into_iter().skip(1);
    while let Some(k)=it.next() {
        if !["--pid","--process-created-filetime","--exe-absolute-path","--local-ip","--remote-ip","--remote-port","--control-dir","--duration-seconds"].contains(&k.as_str()) { return Err(format!("未知参数:{k}")); }
        let v=it.next().ok_or("参数缺值")?;
        if map.insert(k,v).is_some() { return Err("重复参数".into()); }
    }
    let req=|k:&str|map.get(k).cloned().ok_or_else(||format!("缺少参数:{k}"));
    let exe=req("--exe-absolute-path")?;
    let dir=PathBuf::from(req("--control-dir")?);
    if !PathBuf::from(&exe).is_absolute() || !dir.is_absolute() { return Err("路径必须为绝对路径".into()); }
    let seconds=map.get("--duration-seconds").map_or(Ok(MAX_SECONDS),|v|v.parse::<u64>().map_err(|e|e.to_string()))?;
    if !(1..=MAX_SECONDS).contains(&seconds) { return Err("时长应为1..180秒".into()); }
    let ip=|k:&str|->Result<u32> { Ok(u32::from_ne_bytes(req(k)?.parse::<Ipv4Addr>().map_err(|e|e.to_string())?.octets())) };
    let port=req("--remote-port")?.parse::<u16>().map_err(|e|e.to_string())?;
    let pid=req("--pid")?.parse::<u32>().map_err(|e|e.to_string())?;
    if pid==0 || port==0 { return Err("PID和端口必须非零".into()); }
    Ok(Some(Config { process:Process {pid,created:req("--process-created-filetime")?.parse::<u64>().map_err(|e|e.to_string())?,exe},local:ip("--local-ip")?,remote:ip("--remote-ip")?,port:u32::from(port.to_be()),duration_ms:seconds*1000,dir }))
}
pub fn main_entry()->Result<()> {
    let Some(config)=parse(std::env::args().skip(1).collect())? else {
        println!("S7 TCP EStats: --run --pid N --process-created-filetime DECIMAL --exe-absolute-path PATH --local-ip IPv4 --remote-ip IPv4 --remote-port N --control-dir EMPTY_ABSOLUTE_DIR [--duration-seconds 1..180]");
        return Ok(());
    };
    #[cfg(all(windows,target_pointer_width="64"))]
    { native::run(config) }
    #[cfg(not(all(windows,target_pointer_width="64")))]
    { let _=config; Err("本example仅支持Windows x64".into()) }
}

#[cfg(all(windows,target_pointer_width="64"))]
mod native {
    use super::*;
    use windows::{core::PWSTR,Win32::{Foundation::{CloseHandle,FILETIME},System::Threading::{OpenProcess,GetProcessTimes,QueryFullProcessImageNameW,PROCESS_QUERY_LIMITED_INFORMATION,PROCESS_NAME_WIN32},NetworkManagement::IpHelper::*}};
    fn process(pid:u32)->Result<Option<Process>> {
        let handle=match unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION,false,pid) } { Ok(h)=>h,Err(e) if e.code().0 as u32==0x80070057=>return Ok(None),Err(e)=>return Err(format!("OpenProcess:{e}")) };
        let result=(|| {
            let mut created=FILETIME::default(); let mut exit=FILETIME::default(); let mut kernel=FILETIME::default(); let mut user=FILETIME::default();
            unsafe { GetProcessTimes(handle,&mut created,&mut exit,&mut kernel,&mut user) }.map_err(|e|format!("GetProcessTimes:{e}"))?;
            let mut path=vec![0u16;32768]; let mut len=path.len() as u32;
            unsafe { QueryFullProcessImageNameW(handle,PROCESS_NAME_WIN32,PWSTR(path.as_mut_ptr()),&mut len) }.map_err(|e|format!("QueryFullProcessImageNameW:{e}"))?;
            Ok(Some(Process {pid,created:(u64::from(created.dwHighDateTime)<<32)|u64::from(created.dwLowDateTime),exe:String::from_utf16(&path[..len as usize]).map_err(|e|e.to_string())?}))
        })();
        let _=unsafe { CloseHandle(handle) }; result
    }
    fn rows()->Result<Vec<Row>> {
        let mut size=0;
        let first=unsafe { GetExtendedTcpTable(None,&mut size,false,2,TCP_TABLE_OWNER_MODULE_ALL,0) };
        if first!=122 { return Err(format!("GetExtendedTcpTable:size:{first}")); }
        for _ in 0..3 {
            let mut bytes=vec![0u8;size as usize];
            let code=unsafe { GetExtendedTcpTable(Some(bytes.as_mut_ptr().cast()),&mut size,false,2,TCP_TABLE_OWNER_MODULE_ALL,0) };
            if code==122 { continue; }
            if code!=0 { return Err(format!("GetExtendedTcpTable:{code}")); }
            let count=u32_at(&bytes,0)? as usize;
            let offset=std::mem::offset_of!(MIB_TCPTABLE_OWNER_MODULE,table);
            let stride=std::mem::size_of::<MIB_TCPROW_OWNER_MODULE>();
            if offset.checked_add(count.checked_mul(stride).ok_or("TCP_table_size")?).ok_or("TCP_table_size")?>bytes.len() { return Err("TCP_table_truncated".into()); }
            let mut out=Vec::new();
            for i in 0..count {
                // OWNER_MODULE的全部成员均为整数；非对齐读取不构造BOOLEAN。
                let r=unsafe { bytes.as_ptr().add(offset+i*stride).cast::<MIB_TCPROW_OWNER_MODULE>().read_unaligned() };
                out.push(Row {pid:r.dwOwningPid,state:r.dwState,local:r.dwLocalAddr,local_port:r.dwLocalPort,remote:r.dwRemoteAddr,remote_port:r.dwRemotePort,created:r.liCreateTimestamp});
            }
            return Ok(out);
        }
        Err("GetExtendedTcpTable:table_changed_repeatedly".into())
    }
    fn api_row(r:&Row)->MIB_TCPROW_LH { MIB_TCPROW_LH {Anonymous:MIB_TCPROW_LH_0 {dwState:r.state},dwLocalAddr:r.local,dwLocalPort:r.local_port,dwRemoteAddr:r.remote,dwRemotePort:r.remote_port} }
    fn kind(g:Group)->TCP_ESTATS_TYPE { match g {Group::Path=>TcpConnectionEstatsPath,Group::SndCong=>TcpConnectionEstatsSndCong} }
    fn publish(dir:&std::path::Path,name:&str,value:&Value)->Result<()> {
        let final_path=dir.join(name);
        if final_path.exists() { return Err(format!("控制结果已存在:{name}")); }
        let temp=dir.join(format!("{name}.tmp"));
        let mut f=OpenOptions::new().write(true).create_new(true).open(&temp).map_err(|e|e.to_string())?;
        serde_json::to_writer(&mut f,value).map_err(|e|e.to_string())?; f.sync_all().map_err(|e|e.to_string())?;
        fs::rename(temp,final_path).map_err(|e|e.to_string())
    }
    struct Native {config:Config, own:Process, start:Instant, utc_ms:u64, log:File}
    impl Native {
        fn target(&self,row:Option<&Row>)->Value { let mut v=self.config.process.value(); if let Some(r)=row { for (k,val) in r.value().as_object().expect("row object") { v[k]=val.clone(); } } v }
    }
    impl Backend for Native {
        fn elapsed(&self)->u64 { self.start.elapsed().as_millis() as u64 }
        fn sleep(&mut self,ms:u64) { std::thread::sleep(Duration::from_millis(ms)); }
        fn stopped(&self)->bool { self.config.dir.join("stop.request").exists() }
        fn process(&mut self)->Result<Option<Process>> { process(self.config.process.pid) }
        fn rows(&mut self)->Result<Vec<Row>> { rows() }
        fn get(&mut self,r:&Row,g:Group,dynamic:bool)->Result<(u8,Vec<u8>)> {
            let mut rw=[255u8]; let mut rod=vec![0u8;g.size()];
            let code=unsafe { GetPerTcpConnectionEStats(&api_row(r),kind(g),Some(&mut rw),0,None,0,if dynamic {Some(rod.as_mut_slice())} else {None},0) };
            if code!=0 { return Err(format!("GetPerTcpConnectionEStats:{code}")); }
            Ok((rw[0],rod))
        }
        fn set(&mut self,r:&Row,g:Group,v:u8)->Result<()> { let code=unsafe {SetPerTcpConnectionEStats(&api_row(r),kind(g),&[v],0,0)}; if code==0 {Ok(())} else {Err(format!("SetPerTcpConnectionEStats:{code}"))} }
        fn emit(&mut self,mut v:Value)->Result<()> { v["monotonicMs"]=json!(self.elapsed()); v["utcUnixMs"]=json!(self.utc_ms.saturating_add(self.elapsed())); serde_json::to_writer(&mut self.log,&v).map_err(|e|e.to_string())?; self.log.write_all(b"\n").and_then(|()|self.log.flush()).map_err(|e|e.to_string()) }
        fn ready(&mut self,r:&Row)->Result<()> { publish(&self.config.dir,"ready.json",&json!({"schemaVersion":1,"status":"ready","observer":self.own.value(),"target":self.target(Some(r)),"enableCollection":{"Path":true,"SndCong":true},"sampleCount":1,"monotonicMs":self.elapsed(),"utcUnixMs":self.utc_ms+self.elapsed()})) }
    }
    pub(super) fn run(config:Config)->Result<()> {
        let start=Instant::now();
        let utc_ms=SystemTime::now().duration_since(UNIX_EPOCH).map_err(|e|e.to_string())?.as_millis() as u64;
        if !config.dir.is_dir() {return Err("控制目录必须已存在".into());}
        // owner刚创建的目录允许启动身份/停止请求竞态；不复用任何旧观察结果。
        for entry in fs::read_dir(&config.dir).map_err(|e|e.to_string())? {
            let name=entry.map_err(|e|e.to_string())?.file_name();
            if name!="launch.json" && name!="stop.request" {return Err(format!("控制目录已有文件:{name:?}"));}
        }
        let log=OpenOptions::new().write(true).create_new(true).open(config.dir.join("samples.jsonl")).map_err(|e|e.to_string())?;
        let own=process(std::process::id())?.ok_or("观察器进程已退出")?;
        let mut b=Native {config:config.clone(),own,start,utc_ms,log};
        b.emit(json!({"event":"start","observer":b.own.value(),"target":b.target(None),"durationMs":config.duration_ms,"intervalMs":INTERVAL_MS,"boundary":"Get/Set tuple API is not atomic with OWNER_MODULE/process identity checks; no reconnect following"}))?;
        let outcome=observe(&mut b,&config);
        let ok=outcome.error.is_none() && outcome.restoration=="restored";
        publish(&config.dir,"result.json",&json!({"schemaVersion":1,"status":if ok {"completed"} else {"failed"},"restoration":outcome.restoration,"restorationErrors":outcome.restoration_errors,"error":outcome.error,"observer":b.own.value(),"target":b.target(outcome.row.as_ref()),"sampleCount":outcome.samples,"firstSnapshots":outcome.first,"lastSnapshots":outcome.last,"monotonicMs":b.elapsed(),"utcUnixMs":utc_ms+b.elapsed()}))?;
        if ok {Ok(())} else {Err(format!("观察结束:{},error={:?}",outcome.restoration,outcome.error))}
    }
}
