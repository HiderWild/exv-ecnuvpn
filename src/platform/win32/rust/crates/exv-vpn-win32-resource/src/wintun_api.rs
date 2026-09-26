

use std::path::Path;
use std::io::Read;
use std::os::windows::fs::OpenOptionsExt;

use sha2::{Digest, Sha256};

use windows::core::{GUID, HSTRING, PCSTR, PCWSTR};
use windows::Win32::Foundation::{FreeLibrary, HANDLE, HMODULE};
use windows::Win32::NetworkManagement::Ndis::NET_LUID_LH;
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

use crate::native_error::NativeError;

/// Frozen SHA-256 of the amd64 wintun-0.14.1 DLL (native-wintun-facts.md §1).
pub const FROZEN_WINTUN_DLL_SHA256: &str =
    "e5da8447dc2c320edc0fc52fa01885c103de8c118481f683643cacc3220dafce";

/// Typed signature of `WintunCreateAdapter` (wintun.h 0.14.1).
pub type WintunCreateAdapterFn = unsafe extern "system" fn(PCWSTR, PCWSTR, *const GUID) -> HANDLE;
/// Typed signature of `WintunOpenAdapter`.
pub type WintunOpenAdapterFn = unsafe extern "system" fn(PCWSTR) -> HANDLE;
/// Typed signature of `WintunGetAdapterLUID`.
pub type WintunGetAdapterLuidFn = unsafe extern "system" fn(HANDLE, *mut NET_LUID_LH) -> i32;
/// Typed signature of `WintunStartSession`.
pub type WintunStartSessionFn = unsafe extern "system" fn(HANDLE, u32) -> HANDLE;
/// Typed signature of `WintunEndSession`.
pub type WintunEndSessionFn = unsafe extern "system" fn(HANDLE);
/// Typed signature of `WintunAllocateSendPacket`.
pub type WintunAllocateSendPacketFn = unsafe extern "system" fn(HANDLE, u32) -> *mut u8;
/// Typed signature of `WintunSendPacket`.
pub type WintunSendPacketFn = unsafe extern "system" fn(HANDLE, *const u8);
/// Typed signature of `WintunReceivePacket`.
pub type WintunReceivePacketFn = unsafe extern "system" fn(HANDLE, *mut u32) -> *mut u8;
/// Typed signature of `WintunReleaseReceivePacket`.
pub type WintunReleaseReceivePacketFn = unsafe extern "system" fn(HANDLE, *const u8);
/// Typed signature of `WintunGetReadWaitEvent`.
pub type WintunGetReadWaitEventFn = unsafe extern "system" fn(HANDLE) -> HANDLE;
/// Typed signature of `WintunCloseAdapter`.
pub type WintunCloseAdapterFn = unsafe extern "system" fn(HANDLE) -> i32;
/// Typed signature of `WintunDeleteDriver` (takes `WINTUN_API_VERSION`).
pub type WintunDeleteDriverFn = unsafe extern "system" fn(u32) -> i32;
/// Typed signature of `WintunGetRunningDriverVersion`.
pub type WintunGetRunningDriverVersionFn = unsafe extern "system" fn() -> u32;
/// Logger level of the `WintunSetLogger` callback (wintun.h `_WINTUN_LOGGER_LEVEL`).
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WintunLoggerLevel {
    Info = 0,
    Warn = 1,
    Err = 2,
}
/// `WINTUN_LOGGER_CALLBACK` (wintun.h): level / timestamp / wide message.
pub type WintunLoggerCallback = unsafe extern "system" fn(WintunLoggerLevel, u64, PCWSTR);
/// Typed signature of `WintunSetLogger`.
pub type WintunSetLoggerFn = unsafe extern "system" fn(Option<WintunLoggerCallback>);

/// The 14 typed exports of wintun-0.14.1, resolved at load time
/// (native-wintun-facts.md §1: all 14 resolve; `WintunGetAdapterName` does not
/// exist and is never required). A copy is kept by [`WintunAdapter`](crate::wintun_adapter::WintunAdapter)
/// so closing the adapter works even after the library is dropped.
#[derive(Clone, Copy)]
pub struct WintunExports {
    /// `WintunCreateAdapter(Name, TunnelType, RequestedGUID)`.
    pub create_adapter: WintunCreateAdapterFn,
    /// `WintunOpenAdapter(Name)`.
    pub open_adapter: WintunOpenAdapterFn,
    /// `WintunGetAdapterLUID(Adapter, Luid)`.
    pub get_adapter_luid: WintunGetAdapterLuidFn,
    /// `WintunStartSession(Adapter, Capacity)`.
    pub start_session: WintunStartSessionFn,
    /// `WintunEndSession(Session)`.
    pub end_session: WintunEndSessionFn,
    /// `WintunAllocateSendPacket(Session, PacketSize)`.
    pub allocate_send_packet: WintunAllocateSendPacketFn,
    /// `WintunSendPacket(Session, Packet)`.
    pub send_packet: WintunSendPacketFn,
    /// `WintunReceivePacket(Session, PacketSize)`.
    pub receive_packet: WintunReceivePacketFn,
    /// `WintunReleaseReceivePacket(Session, Packet)`.
    pub release_receive_packet: WintunReleaseReceivePacketFn,
    /// `WintunGetReadWaitEvent(Session)`.
    pub get_read_wait_event: WintunGetReadWaitEventFn,
    /// `WintunCloseAdapter(Adapter)`.
    pub close_adapter: WintunCloseAdapterFn,
    /// `WintunDeleteDriver(Version)`.
    pub delete_driver: WintunDeleteDriverFn,
    /// `WintunGetRunningDriverVersion()`.
    pub get_running_driver_version: WintunGetRunningDriverVersionFn,
    /// `WintunSetLogger(Logger)`.
    pub set_logger: WintunSetLoggerFn,
}

/// A loaded `wintun.dll` module with all 14 typed exports resolved.
///
/// Loading contract (frozen): exact absolute path only (never PATH search),
/// SHA-256 equals [`FROZEN_WINTUN_DLL_SHA256`], and all 14 exports resolve.
/// The module reference is released with `FreeLibrary` on drop.
pub struct WintunLibrary {
    /// The module handle from `LoadLibraryW`, released by `FreeLibrary` in `Drop`.
    module: HMODULE,
    /// The resolved typed exports of this module.
    exports: WintunExports,
    /// 校验到卸载期间禁止写入/替换同一 DLL，防止校验后再按路径加载的竞态。
    _validated_file: std::fs::File,
}

impl WintunLibrary {
    /// Load `wintun.dll` from `dll_path` under the frozen loading contract.
    ///
    /// # Errors
    ///
    /// Returns a [`NativeError`] if the path is not absolute, the file cannot be
    /// read, its SHA-256 does not match [`FROZEN_WINTUN_DLL_SHA256`],
    /// `LoadLibraryW` fails, or any of the 14 exports cannot be resolved.
    pub fn load(dll_path: &Path) -> Result<Self, NativeError> {
        if !dll_path.is_absolute() {
            return Err(NativeError::from_win32(
                2,
                "wintun DLL 必须按精确绝对路径加载（绝不 PATH 搜索）",
            ));
        }
        let read_error = |error: std::io::Error| {
            NativeError::from_win32(
                error.raw_os_error().unwrap_or(0) as u32,
                &format!("无法读取 wintun.dll：{}：{error}", dll_path.display()),
            )
        };
        // FILE_SHARE_READ：允许 Windows 加载器读取，但拒绝其他写入和删除句柄。
        let mut validated_file = std::fs::OpenOptions::new().read(true).share_mode(1)
            .open(dll_path).map_err(read_error)?;
        let mut bytes = Vec::new();
        validated_file.read_to_end(&mut bytes).map_err(read_error)?;
        let digest = Sha256::digest(&bytes);
        let hash_hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        if !hash_hex.eq_ignore_ascii_case(FROZEN_WINTUN_DLL_SHA256) {
            return Err(NativeError::from_win32(
                13,
                "wintun.dll SHA-256 与冻结值不一致（mutant 拒绝）",
            ));
        }
        let hstring = HSTRING::from(dll_path.as_os_str());
        // SAFETY: hstring 是合法宽字符串绝对路径；返回的模块句柄由本结构持有并在
        // Drop 中经 FreeLibrary 释放（LoadLibraryW 引用计数配对）。
        let module = unsafe { LoadLibraryW(&hstring) }.map_err(|e| {
            let hresult = e.code().0 as u32;
            NativeError::from_win32(
                // 只解包 HRESULT_FROM_WIN32；未知 HRESULT 保留全部位用于诊断。
                if hresult & 0xffff_0000 == 0x8007_0000 { hresult & 0xffff } else { hresult },
                &format!("LoadLibraryW 失败：{}：{e}", dll_path.display()),
            )
        })?;
        let exports = match resolve_all_exports(module) {
            Ok(e) => e,
            Err(e) => {
                // SAFETY: module 已由本函数加载但 WintunLibrary 未构造成功，此处
                // 释放该引用避免泄漏。
                unsafe { let _ = FreeLibrary(module); }
                return Err(e);
            }
        };
        Ok(Self { module, exports, _validated_file: validated_file })
    }

    /// The resolved typed exports of the loaded module.
    #[must_use]
    pub fn exports(&self) -> &WintunExports {
        &self.exports
    }
}

impl Drop for WintunLibrary {
    fn drop(&mut self) {
        // SAFETY: self.module 是本实例 LoadLibraryW 持有的模块引用；Drop 是最后一次
        // 使用（FreeLibrary 与 LoadLibraryW 引用计数配对）。
        unsafe { let _ = FreeLibrary(self.module); }
    }
}

/// Resolve all 14 frozen exports of wintun-0.14.1 into a [`WintunExports`].
fn resolve_all_exports(module: HMODULE) -> Result<WintunExports, NativeError> {
    Ok(WintunExports {
        create_adapter: resolve_typed(module, "WintunCreateAdapter")?,
        open_adapter: resolve_typed(module, "WintunOpenAdapter")?,
        get_adapter_luid: resolve_typed(module, "WintunGetAdapterLUID")?,
        start_session: resolve_typed(module, "WintunStartSession")?,
        end_session: resolve_typed(module, "WintunEndSession")?,
        allocate_send_packet: resolve_typed(module, "WintunAllocateSendPacket")?,
        send_packet: resolve_typed(module, "WintunSendPacket")?,
        receive_packet: resolve_typed(module, "WintunReceivePacket")?,
        release_receive_packet: resolve_typed(module, "WintunReleaseReceivePacket")?,
        get_read_wait_event: resolve_typed(module, "WintunGetReadWaitEvent")?,
        close_adapter: resolve_typed(module, "WintunCloseAdapter")?,
        delete_driver: resolve_typed(module, "WintunDeleteDriver")?,
        get_running_driver_version: resolve_typed(module, "WintunGetRunningDriverVersion")?,
        set_logger: resolve_typed(module, "WintunSetLogger")?,
    })
}

/// Resolve one export by name and transmute it to the typed signature `T`.
fn resolve_typed<T>(module: HMODULE, name: &str) -> Result<T, NativeError> {
    let cname = std::ffi::CString::new(name)
        .map_err(|_| NativeError::from_win32(0, "导出名包含 NUL 字节"))?;
    // SAFETY: module 是有效已加载模块句柄；cname 是 NUL 结尾 ANSI 导出名。
    let fp = unsafe { GetProcAddress(module, PCSTR::from_raw(cname.as_ptr().cast::<u8>())) }
        .ok_or_else(|| NativeError::from_win32(127, &format!("wintun.dll 缺少导出 {name}")))?;
    // SAFETY: 调用方以该导出的真实签名实例化 T（FARPROC 到类型化函数指针，同址转换）。
    Ok(unsafe { std::mem::transmute_copy(&fp) })
}

