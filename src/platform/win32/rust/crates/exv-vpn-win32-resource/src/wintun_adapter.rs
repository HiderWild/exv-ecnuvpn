

use windows::Win32::Foundation::{GetLastError, HANDLE};
use windows::Win32::NetworkManagement::IpHelper::{
    ConvertInterfaceLuidToAlias, ConvertInterfaceLuidToIndex,
};
use windows::Win32::NetworkManagement::Ndis::NET_LUID_LH;
use windows::core::{HSTRING, PCWSTR};

use crate::native_error::NativeError;
use crate::wintun_api::{WintunCloseAdapterFn, WintunExports, WintunLibrary};

/// How this handle was obtained: the creator owns the adapter (its close
/// removes the adapter); an opened second handle does not (its close only
/// releases the handle).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdapterOpen {
    /// This handle created the adapter (owned).
    Created,
    /// This handle opened an existing adapter by name (not owned).
    Opened,
}

/// An open Wintun adapter handle with its resolved identity
/// (LUID / ifindex / alias).
///
/// Dropping the adapter closes the handle: a creator close removes the adapter
/// (the real cleanup predicate of 0.14.1), a non-creator close only releases
/// the second handle.
/// [`WintunLibrary`] 必须比本句柄长寿：保存函数指针不会保持模块加载。
pub struct WintunAdapter {
    /// The adapter handle from `WintunCreateAdapter`/`WintunOpenAdapter`.
    handle: HANDLE,
    /// The 64-bit LUID reported by `WintunGetAdapterLUID`.
    luid: u64,
    /// The interface index from `ConvertInterfaceLuidToIndex`.
    ifindex: u32,
    /// The adapter alias (friendly name) from `ConvertInterfaceLuidToAlias`.
    alias: String,
    /// Whether this handle created the adapter (creator close removes it).
    owned: bool,
    /// `WintunCloseAdapter` export, kept for `Drop`.
    close: WintunCloseAdapterFn,
}

impl WintunAdapter {
    /// 按精确名称领用，只有原生 Open 未找到才创建；身份错误不触发创建。
    /// 调用者必须让 lib 比返回的 adapter 长寿。
    pub fn open_or_create(
        lib: &WintunLibrary,
        name: &str,
        tunnel_type: &str,
    ) -> Result<(Self, AdapterOpen), NativeError> {
        let exports = *lib.exports();
        let name_w = HSTRING::from(name);
        let tunnel_w = HSTRING::from(tunnel_type);
        open_or_create_with(
            name,
            || {
                // SAFETY: 宽字符串在调用期间存活；仅 NULL 后立即读原始错误。
                let handle = unsafe { (exports.open_adapter)(PCWSTR::from_raw(name_w.as_ptr())) };
                if handle.0.is_null() {
                    Err(NativeError::from_win32(
                        unsafe { GetLastError().0 },
                        "WintunOpenAdapter 失败",
                    ))
                } else {
                    Ok(handle)
                }
            },
            || {
                // SAFETY: 两宽字符串有效；NULL GUID 由系统分配，NULL 句柄才读错误。
                let handle = unsafe {
                    (exports.create_adapter)(
                        PCWSTR::from_raw(name_w.as_ptr()),
                        PCWSTR::from_raw(tunnel_w.as_ptr()),
                        std::ptr::null(),
                    )
                };
                if handle.0.is_null() {
                    Err(NativeError::from_win32(
                        unsafe { GetLastError().0 },
                        "WintunCreateAdapter 失败",
                    ))
                } else {
                    Ok(handle)
                }
            },
            |handle| adapter_identity(&exports, handle),
            exports.close_adapter,
        )
    }
    /// Create a new Wintun adapter (creator owned; dropping it removes the
    /// adapter).
    ///
    /// # Errors
    ///
    /// Returns a [`NativeError`] if `WintunCreateAdapter` fails (non-elevated
    /// hosts get `ERROR_ACCESS_DENIED` — creating a Wintun adapter needs
    /// administrator rights) or the LUID/ifindex/alias identity cannot be
    /// resolved (in which case the just-created adapter is closed again, which
    /// removes it).
    pub fn create(
        lib: &WintunLibrary,
        name: &str,
        tunnel_type: &str,
    ) -> Result<(Self, AdapterOpen), NativeError> {
        let _t = crate::timing::Timed::new("resource.wintun_adapter.create");
        let exports = *lib.exports();
        let name_w = HSTRING::from(name);
        let tunnel_w = HSTRING::from(tunnel_type);
        // SAFETY: name_w/tunnel_w 在调用期间存活且是合法宽字符串；RequestedGUID=NULL
        // 由系统分配 GUID；失败时返回 NULL 句柄（wintun.h 契约）。
        let handle = unsafe {
            (exports.create_adapter)(
                PCWSTR::from_raw(name_w.as_ptr()),
                PCWSTR::from_raw(tunnel_w.as_ptr()),
                std::ptr::null(),
            )
        };
        if handle.0.is_null() {
            return Err(NativeError::from_win32(
                unsafe { GetLastError().0 },
                "WintunCreateAdapter 失败（创建 adapter 需要管理员权限）",
            ));
        }
        let identity = match adapter_identity(&exports, handle) {
            Ok(v) => v,
            Err(e) => {
                // 回滚：创建者 close 即移除 adapter（frozen fact），不留残留。
                // SAFETY: handle 是本函数刚创建的 adapter 句柄。
                unsafe { (exports.close_adapter)(handle) };
                return Err(e);
            }
        };
        Ok((
            Self {
                handle,
                luid: identity.luid,
                ifindex: identity.ifindex,
                alias: identity.alias,
                owned: true,
                close: exports.close_adapter,
            },
            AdapterOpen::Created,
        ))
    }

    /// Open an existing Wintun adapter by name (not owned; dropping this handle
    /// does not remove the adapter).
    ///
    /// # Errors
    ///
    /// Returns a [`NativeError`] with `code == 1168` (`ERROR_NOT_FOUND`) when no
    /// adapter with `name` exists; other codes for unexpected failures.
    pub fn open(lib: &WintunLibrary, name: &str) -> Result<(Self, AdapterOpen), NativeError> {
        let _t = crate::timing::Timed::new("resource.wintun_adapter.open");
        let exports = *lib.exports();
        let name_w = HSTRING::from(name);
        // SAFETY: name_w 是合法宽字符串；adapter 不存在时返回 NULL 句柄。
        let handle = unsafe { (exports.open_adapter)(PCWSTR::from_raw(name_w.as_ptr())) };
        if handle.0.is_null() {
            return Err(NativeError::from_win32(
                unsafe { GetLastError().0 },
                "WintunOpenAdapter 失败（adapter 不存在）",
            ));
        }
        let identity = match adapter_identity(&exports, handle) {
            Ok(v) => v,
            Err(e) => {
                // 非创建者 close 不删除 adapter（frozen fact），仅释放句柄。
                // SAFETY: handle 是本次 open 得到的句柄。
                unsafe { (exports.close_adapter)(handle) };
                return Err(e);
            }
        };
        Ok((
            Self {
                handle,
                luid: identity.luid,
                ifindex: identity.ifindex,
                alias: identity.alias,
                owned: false,
                close: exports.close_adapter,
            },
            AdapterOpen::Opened,
        ))
    }

    /// The adapter's 64-bit LUID (`WintunGetAdapterLUID`).
    #[must_use]
    pub fn luid(&self) -> u64 {
        self.luid
    }

    /// The interface index (`ConvertInterfaceLuidToIndex`).
    #[must_use]
    pub fn ifindex(&self) -> u32 {
        self.ifindex
    }

    /// The adapter alias (friendly name, `ConvertInterfaceLuidToAlias`).
    #[must_use]
    pub fn alias(&self) -> &str {
        &self.alias
    }

    /// Whether this handle created the adapter (only a creator close removes
    /// the adapter; `AdapterOpen::Created`/`Opened` carry the same fact).
    #[must_use]
    pub fn owned(&self) -> bool {
        self.owned
    }

    /// The raw adapter handle (crate-internal; used by
    /// [`WintunSession::start`](crate::wintun_session::WintunSession::start) to
    /// call `WintunStartSession` on this adapter).
    #[must_use]
    pub(crate) fn handle(&self) -> HANDLE {
        self.handle
    }
}

impl Drop for WintunAdapter {
    fn drop(&mut self) {
        // SAFETY: self.handle 是本实例持有的有效 adapter 句柄；创建者 close 即移除
        // adapter（0.14.1 真实清理谓词），非创建者 close 仅释放该句柄。
        unsafe { (self.close)(self.handle) };
    }
}

fn open_or_create_with(
    name: &str,
    open: impl FnOnce() -> Result<HANDLE, NativeError>,
    create: impl FnOnce() -> Result<HANDLE, NativeError>,
    identity: impl FnOnce(HANDLE) -> Result<WintunIdentity, NativeError>,
    close: WintunCloseAdapterFn,
) -> Result<(WintunAdapter, AdapterOpen), NativeError> {
    let (handle, kind) = match open() {
        Ok(handle) => (handle, AdapterOpen::Opened),
        Err(error) if error.code == 1168 => (create()?, AdapterOpen::Created),
        Err(error) => return Err(error),
    };
    let identity = identity(handle).and_then(|identity| {
        if identity.alias == name {
            Ok(identity)
        } else {
            Err(NativeError::from_win32(
                13,
                "Wintun adapter alias 与配置精确名称不匹配",
            ))
        }
    });
    match identity {
        Ok(identity) => Ok((
            WintunAdapter {
                handle,
                luid: identity.luid,
                ifindex: identity.ifindex,
                alias: identity.alias,
                owned: kind == AdapterOpen::Created,
                close,
            },
            kind,
        )),
        Err(error) => {
            // SAFETY: 仅关闭本轮刚取得的有效句柄；lib 仍保持加载。
            unsafe { close(handle) };
            Err(error)
        }
    }
}

/// Resolved identity of an adapter handle.
struct WintunIdentity {
    luid: u64,
    ifindex: u32,
    alias: String,
}

/// Resolve LUID / ifindex / alias for an open adapter handle
/// (`WintunGetAdapterLUID` + iphlpapi conversions).
fn adapter_identity(
    exports: &WintunExports,
    handle: HANDLE,
) -> Result<WintunIdentity, NativeError> {
    let mut luid = NET_LUID_LH::default();
    // SAFETY: luid 是有效输出参数（8 字节 union）；返回 BOOL 非零表示成功。
    let ok = unsafe { (exports.get_adapter_luid)(handle, &raw mut luid) };
    if ok == 0 {
        return Err(NativeError::from_win32(
            unsafe { GetLastError().0 },
            "WintunGetAdapterLUID 失败",
        ));
    }
    // SAFETY: NET_LUID_LH 是 Rust union，字段读取必须位于 unsafe 块中。
    let luid_value = unsafe { luid.Value };
    let mut ifindex = 0u32;
    // SAFETY: ifindex 是有效输出参数；返回 WIN32_ERROR，0 表示成功。
    let rc_index = unsafe { ConvertInterfaceLuidToIndex(&raw const luid, &raw mut ifindex) };
    if rc_index.0 != 0 {
        return Err(NativeError::from_win32(
            rc_index.0,
            "ConvertInterfaceLuidToIndex 失败",
        ));
    }
    let mut alias_buf = vec![0u16; 256];
    // SAFETY: alias_buf 是有效可变宽字符缓冲（长度以 u16 计，由 &mut [u16] 自带）。
    let rc_alias = unsafe { ConvertInterfaceLuidToAlias(&raw const luid, &mut alias_buf) };
    if rc_alias.0 != 0 {
        return Err(NativeError::from_win32(
            rc_alias.0,
            "ConvertInterfaceLuidToAlias 失败",
        ));
    }
    let end = alias_buf
        .iter()
        .position(|&c| c == 0)
        .unwrap_or(alias_buf.len());
    let alias = String::from_utf16_lossy(&alias_buf[..end]);
    Ok(WintunIdentity {
        luid: luid_value,
        ifindex,
        alias,
    })
}

