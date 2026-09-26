//! 出站 socket 的物理出口绑定：connect 前对未连接 socket 设置 `IP_BOUND_IF`。
//!
//! 绑定失败必须返回错误，不得无声降级为系统默认路由。

// 中文文档中的技术术语不逐个加反引号；socket 选项宽度转换由常量保证。
#![allow(clippy::doc_markdown)]
#![allow(clippy::cast_possible_truncation)]

use std::sync::Arc;

use exv_vpn_cstp::connector::SocketBinder;

/// `IP_BOUND_IF` 的 Darwin 常量（`<netinet/in.h>`；IPPROTO_IP 级别）。
const IP_BOUND_IF: i32 = 25;

/// 构造把出站 TCP socket 钉在 `ifindex` 物理出口上的 binder。
#[must_use]
pub fn bound_socket_binder(ifindex: u32) -> Arc<SocketBinder> {
    Arc::new(move |socket: &tokio::net::TcpSocket| {
        use std::os::fd::AsRawFd;
        let index: libc::c_uint = ifindex;
        // SAFETY: socket is a live socket; the option writes 4 bytes of c_uint.
        let result = unsafe {
            libc::setsockopt(
                socket.as_raw_fd(),
                libc::IPPROTO_IP,
                IP_BOUND_IF,
                std::ptr::addr_of!(index).cast(),
                std::mem::size_of::<libc::c_uint>() as libc::socklen_t,
            )
        };
        if result != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    })
}
