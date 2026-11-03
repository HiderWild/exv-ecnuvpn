//! 只读诊断已安装服务：不取得 owner、不连接 VPN、不修改网卡或服务。
use std::time::Duration;

use exv_core::grpc_transport::connect_engine_service_channel;
use exv_vpn_win32_ipc::peer_auth::SYSTEM_SID;
use exv_vpn_win32_ipc::pipe_names::control_pipe_name_for_current_user;
use exv_vpn_win32_ipc::service_key::read_service_psk;
use exv_vpn_wire::generated::{
    KeepAliveRequest, ServiceManageRequest, ServiceSelfQuery,
    helper_control_client::HelperControlClient, service_manage_request,
};

#[tokio::main]
async fn main() {
    let result = tokio::time::timeout(Duration::from_secs(12), probe()).await;
    match result {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
        Err(_) => {
            eprintln!("service probe: overall timeout");
            std::process::exit(2);
        }
    }
}

async fn probe() -> Result<(), String> {
    let mut psk = read_service_psk().map_err(|_| "PSK unreadable or invalid".to_owned())?;
    // D12：服务管道名按安装用户（= 当前用户）SID 派生的常量名（ipc 单一事实源）。
    let pipe = control_pipe_name_for_current_user()
        .ok_or_else(|| "current user SID unavailable".to_string())?;
    eprintln!("PSK readable; connecting authenticated service channel at {pipe}");
    let connection = connect_engine_service_channel(&pipe, SYSTEM_SID, &psk).await;
    zeroize::Zeroize::zeroize(&mut psk);
    let (channel, peer) = connection.map_err(|e| format!("channel: {e:?}"))?;
    eprintln!("authenticated service peer: {peer:?}");
    let mut client = HelperControlClient::new(channel);
    client
        .keep_alive(KeepAliveRequest { monotonic_tick: 0 })
        .await
        .map_err(|e| format!("keepalive: {:?}", e.code()))?;
    eprintln!("keepalive replied");
    let reply = client
        .service_manage(ServiceManageRequest {
            action: Some(service_manage_request::Action::Query(ServiceSelfQuery {})),
        })
        .await
        .map_err(|e| format!("self report: {:?}", e.code()))?;
    let report = reply
        .into_inner()
        .self_report
        .ok_or("self report missing")?;
    println!(
        "{}",
        serde_json::json!({
            "control_plane_ready": report.control_plane_ready,
            "psk_present": report.psk_present,
            "connection_mode": report.connection_mode,
            "vpn_connect_requested": false,
            "owner_acquired": false
        })
    );
    Ok(())
}
