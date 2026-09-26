//! Engine 独占的 `utun` 设备：PF_SYSTEM connect 创建、读接口名、独占 FD。
//!
//! FD 不跨进程；drop FD 即销毁本次创建的接口（OS 语义），不需要显式删除。
//!
//! # Panics
//!
//! `duplicate_utun_fds` 类路径的 dup 失败会在 pump 侧断言（root 正常路径不会发生）。

// 中文文档中的技术术语不逐个加反引号；kctl 常量宽度由内核接口定义保证。
#![allow(clippy::doc_markdown)]
#![allow(clippy::missing_panics_doc)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::explicit_auto_deref)]

use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};

use super::PlatformError;

/// `<sys/kern_control.h>`：utun kext 的控制名与常量。
const UTUN_CONTROL_NAME: &[u8] = b"com.apple.net.utun_control";
const AF_SYSTEM: u8 = 32;
const AF_SYS_CONTROL: u16 = 2;
const SYSPROTO_CONTROL: i32 = 2;
const UTUN_OPT_IFNAME: i32 = 2;
/// `_IOWR('N', 3, struct ctl_info)`，ctl_info = 4 + 96 = 100 字节。
const CTLIOCGINFO: u64 = 0xC064_4E03;

/// 一次连接内 Engine 独占的 utun 设备。
pub struct Utun {
    fd: OwnedFd,
    /// 接口名（如 `utun8`，'static 因 name 来自固定长度内核缓冲的泄漏复制）。
    name: &'static str,
    ifindex: u32,
}

// SAFETY: fd 由本进程 socket/connect 创建，只在 Engine 内使用。
unsafe impl Send for Utun {}
// SAFETY: 同上；Sync 仅用于跨 await 持有引用。
unsafe impl Sync for Utun {}

impl Utun {
    /// 创建一个新 utun：迭代 unit 直到内核接受 connect。
    ///
    /// # Errors
    ///
    /// control socket 创建、`CTLIOCGINFO` 或全部 unit 被占用时返回 typed 错误。
    pub fn acquire() -> Result<Self, PlatformError> {
        // SAFETY: socket(2) 调用，返回新 fd 或 -1。
        let fd = unsafe { libc::socket(libc::PF_SYSTEM, libc::SOCK_DGRAM, SYSPROTO_CONTROL) };
        if fd < 0 {
            return Err(PlatformError::Utun("SOCKET", io::Error::last_os_error()));
        }
        // SAFETY: fd 归 OwnedFd 所有。
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };

        let mut info: libc::ctl_info = unsafe { std::mem::zeroed() };
        let name_len = UTUN_CONTROL_NAME.len();
        assert!(name_len < info.ctl_name.len(), "utun control name length");
        info.ctl_name[..name_len].copy_from_slice(
            UTUN_CONTROL_NAME
                .iter()
                .map(|&b| libc::c_char::try_from(b).unwrap_or(0))
                .collect::<Vec<_>>()
                .as_slice(),
        );
        // SAFETY: fd 有效，info 是完整的 ctl_info 输出缓冲。
        if unsafe { libc::ioctl(fd.as_raw_fd(), CTLIOCGINFO, &mut info) } != 0 {
            return Err(PlatformError::Utun(
                "CTLIOCGINFO",
                io::Error::last_os_error(),
            ));
        }

        let mut last_error = io::Error::from_raw_os_error(libc::EBUSY);
        for unit in 0_u32..64 {
            let address = libc::sockaddr_ctl {
                sc_len: std::mem::size_of::<libc::sockaddr_ctl>() as u8,
                sc_family: AF_SYSTEM,
                ss_sysaddr: AF_SYS_CONTROL,
                sc_id: info.ctl_id,
                sc_unit: unit + 1,
                sc_reserved: [0; 5],
            };
            // SAFETY: connect(2) with a valid sockaddr_ctl.
            if unsafe {
                libc::connect(
                    fd.as_raw_fd(),
                    std::ptr::from_ref(&address).cast::<libc::sockaddr>(),
                    u32::try_from(std::mem::size_of::<libc::sockaddr_ctl>())
                        .expect("size fits u32"),
                )
            } == 0
            {
                let name = read_interface_name(&fd)?;
                let c_name = std::ffi::CString::new(name.as_str()).map_err(|_| {
                    PlatformError::Utun("IFNAME", io::Error::from_raw_os_error(libc::EINVAL))
                })?;
                // SAFETY: c_name is a valid NUL-terminated string.
                let ifindex = unsafe { libc::if_nametoindex(c_name.as_ptr()) };
                if ifindex == 0 {
                    return Err(PlatformError::Utun(
                        "IFINDEX",
                        io::Error::from_raw_os_error(libc::ENODEV),
                    ));
                }
                return Ok(Self {
                    fd,
                    // 名字只含 [utun0-9]，泄漏复制换 'static 以简化所有权。
                    name: Box::leak(name.into_boxed_str()),
                    ifindex,
                });
            }
            last_error = io::Error::last_os_error();
            if last_error.raw_os_error() != Some(libc::EBUSY) {
                return Err(PlatformError::Utun("CONNECT", last_error));
            }
        }
        Err(PlatformError::Utun("NO_FREE_UNIT", last_error))
    }

    /// 接口名。
    #[must_use]
    pub fn name(&self) -> &str {
        self.name
    }

    /// 接口索引（路由消息用）。
    #[must_use]
    pub const fn ifindex(&self) -> u32 {
        self.ifindex
    }

    /// 设备 fd（数据泵 dup 后各自持有；本 fd 保持所有权）。
    #[must_use]
    pub const fn fd(&self) -> &OwnedFd {
        &self.fd
    }
}

impl AsRawFd for Utun {
    fn as_raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }
}

fn read_interface_name(fd: &OwnedFd) -> Result<String, PlatformError> {
    let mut buffer = [0_u8; 256];
    let mut length = u32::try_from(buffer.len()).expect("buffer fits u32");
    // SAFETY: getsockopt(2) with a valid buffer/length out-pair.
    if unsafe {
        libc::getsockopt(
            fd.as_raw_fd(),
            SYSPROTO_CONTROL,
            UTUN_OPT_IFNAME,
            std::ptr::addr_of_mut!(buffer).cast::<libc::c_void>(),
            std::ptr::addr_of_mut!(length),
        )
    } != 0
    {
        return Err(PlatformError::Utun("IFNAME", io::Error::last_os_error()));
    }
    let end = buffer
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(length as usize);
    String::from_utf8(buffer[..end].to_vec())
        .map_err(|_| PlatformError::Utun("IFNAME", io::Error::from_raw_os_error(libc::EILSEQ)))
}
