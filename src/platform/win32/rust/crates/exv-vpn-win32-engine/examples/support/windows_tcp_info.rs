// 诊断薄模块，由Windows节点实验的同DataNode owner采样；不接入产品默认路径。
// SDK: 10.0.18362.0/shared/mstcpip.h:206,292–312，ws2def.h:377,390。
// 本例程不创建连接；若另行用于 loopback，仅证明该连接的 API 能力。
#![allow(dead_code)]

use serde_json::{Value, json};
use std::{
    mem::{offset_of, size_of},
    os::windows::io::AsRawSocket,
};
use windows::Win32::Networking::WinSock::{SOCKET, SOCKET_ERROR, WSAGetLastError, WSAIoctl};

const SIO_TCP_INFO: u32 = 0xD8000027;

// BOOLEAN 保留为 u8，所有字段都允许任意位模式；不把原生 BOOLEAN 写进 Rust bool。
#[repr(C)]
#[derive(Default)]
struct TcpInfoV0 {
    state: u32,
    mss: u32,
    connection_time_ms: u64,
    timestamps_enabled: u8,
    rtt_us: u32,
    min_rtt_us: u32,
    bytes_in_flight: u32,
    cwnd: u32,
    snd_wnd: u32,
    rcv_wnd: u32,
    rcv_buf: u32,
    bytes_out: u64,
    bytes_in: u64,
    bytes_reordered: u32,
    bytes_retrans: u32,
    fast_retrans: u32,
    dup_acks_in: u32,
    timeout_episodes: u32,
    syn_retrans: u8,
}

pub fn abi() -> Value {
    json!({"api":"SIO_TCP_INFO", "version":0, "struct_bytes":size_of::<TcpInfoV0>(),
        "offsets":{"ConnectionTimeMs":offset_of!(TcpInfoV0,connection_time_ms),
            "TimestampsEnabled":offset_of!(TcpInfoV0,timestamps_enabled),
            "RttUs":offset_of!(TcpInfoV0,rtt_us), "BytesOut":offset_of!(TcpInfoV0,bytes_out),
            "SynRetrans":offset_of!(TcpInfoV0,syn_retrans)},
        "units":{"ConnectionTimeMs":"ms","RttUs":"us","MinRttUs":"us"},
        "source":"Windows SDK 10.0.18362.0/shared/mstcpip.h TCP_INFO_v0"})
}

/// 同步读取借用 socket，不复制、不关闭、不保存句柄、不改变 socket 设置。
pub fn sample(socket: &impl AsRawSocket) -> Value {
    sample_with(|info, returned| {
        let version: u32 = 0;
        // SAFETY: 传入借用期内有效 socket；输入/输出及长度均为本机 v0 ABI。
        // 同步调用且无 OVERLAPPED，返回后内核不再借用这些局部缓冲。
        let rc = unsafe {
            WSAIoctl(
                SOCKET(socket.as_raw_socket() as usize),
                SIO_TCP_INFO,
                Some((&raw const version).cast()),
                size_of::<u32>() as u32,
                Some((info as *mut TcpInfoV0).cast()),
                size_of::<TcpInfoV0>() as u32,
                returned,
                None,
                None,
            )
        };
        if rc == SOCKET_ERROR {
            // SAFETY: 紧随失败读取，不插入另一个 Winsock 调用覆盖错误。
            Err(unsafe { WSAGetLastError().0 })
        } else {
            Ok(())
        }
    })
}

fn sample_with(read: impl FnOnce(&mut TcpInfoV0, &mut u32) -> Result<(), i32>) -> Value {
    let mut info = TcpInfoV0::default();
    let mut returned = 0;
    let result = read(&mut info, &mut returned);
    let valid = result.is_ok() && returned as usize == size_of::<TcpInfoV0>();
    let fields = if valid {
        json!({"State":info.state,"Mss":info.mss,"ConnectionTimeMs":info.connection_time_ms,
            "TimestampsEnabled":info.timestamps_enabled != 0,"RttUs":info.rtt_us,
            "MinRttUs":info.min_rtt_us,"BytesInFlight":info.bytes_in_flight,"Cwnd":info.cwnd,
            "SndWnd":info.snd_wnd,"RcvWnd":info.rcv_wnd,"RcvBuf":info.rcv_buf,
            "BytesOut":info.bytes_out,"BytesIn":info.bytes_in,"BytesReordered":info.bytes_reordered,
            "BytesRetrans":info.bytes_retrans,"FastRetrans":info.fast_retrans,
            "DupAcksIn":info.dup_acks_in,"TimeoutEpisodes":info.timeout_episodes,"SynRetrans":info.syn_retrans})
    } else {
        Value::Null
    };
    json!({"api":"SIO_TCP_INFO","version":0,"struct_bytes":size_of::<TcpInfoV0>(),
        "returned_bytes":returned,"wsa_error":result.err(),"length_valid":valid,
        "fields":fields,"send_queue_bytes":null,"queue_residence_us":null})
}
