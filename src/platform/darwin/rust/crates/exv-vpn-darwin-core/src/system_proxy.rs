//! 读取 macOS 当前有效系统代理设置；只上报模式与数量，不修改设置或暴露地址。

use exv_vpn_wire::generated as wire;

#[derive(Default, Debug)]
struct ProxySettings {
    manual: bool,
    automatic: bool,
    endpoints: u32,
}

impl ProxySettings {
    fn into_wire(self, tunnel_present: bool) -> wire::SystemProxyDetection {
        let present = self.manual || self.automatic;
        wire::SystemProxyDetection {
            mode: match (self.manual, self.automatic) {
                (false, false) => "disabled",
                (true, false) => "manual",
                (false, true) => "automatic",
                (true, true) => "mixed",
            }
            .to_owned(),
            endpoint_count: self.endpoints,
            // EXV 没有修改 macOS 系统代理豁免；系统原有 ExceptionsList 不是 EXV 已合并的证据。
            bypass_merged: false,
            topology: match (present, tunnel_present) {
                (false, false) => "t0",
                (true, false) => "t1",
                (false, true) => "t2",
                (true, true) => "t3",
            }
            .to_owned(),
        }
    }
}

pub(crate) fn detect(tunnel_present: bool) -> Option<wire::SystemProxyDetection> {
    native::read()
        .ok()
        .map(|settings| settings.into_wire(tunnel_present))
}

/// 当前 IPv4/IPv6 主接口及地址族。没有该族的主路由或读取失败时不猜测备用接口。
pub(crate) fn primary_network_interfaces() -> Vec<(String, bool)> {
    native::primary_network_interfaces()
}

#[cfg(target_os = "macos")]
mod native {
    use super::ProxySettings;
    use std::ffi::c_void;
    type Ref = *const c_void;

    #[link(name = "SystemConfiguration", kind = "framework")]
    unsafe extern "C" {
        fn SCDynamicStoreCopyProxies(store: Ref) -> Ref;
        fn SCDynamicStoreCopyValue(store: Ref, key: Ref) -> Ref;
        fn SCError() -> i32;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFRelease(value: Ref);
        fn CFGetTypeID(value: Ref) -> usize;
        fn CFDictionaryGetTypeID() -> usize;
        fn CFDictionaryGetValue(dictionary: Ref, key: Ref) -> Ref;
        fn CFStringCreateWithBytes(
            allocator: Ref,
            bytes: *const u8,
            len: isize,
            encoding: u32,
            external: u8,
        ) -> Ref;
        fn CFStringGetTypeID() -> usize;
        fn CFStringGetLength(string: Ref) -> isize;
        fn CFStringGetCString(
            string: Ref,
            buffer: *mut std::ffi::c_char,
            capacity: isize,
            encoding: u32,
        ) -> u8;
        fn CFNumberGetTypeID() -> usize;
        fn CFNumberGetValue(number: Ref, kind: isize, output: *mut c_void) -> u8;
    }
    struct Owned(Ref);
    impl Drop for Owned {
        fn drop(&mut self) {
            // SAFETY: 本类型只接收 Copy/Create 返回的非空所有权引用。
            unsafe { CFRelease(self.0) };
        }
    }
    fn string(key: &str) -> Result<Owned, ()> {
        // SAFETY: UTF-8 字节在调用期存活，Create 返回的引用由 Owned 释放。
        let key = unsafe {
            CFStringCreateWithBytes(
                std::ptr::null(),
                key.as_ptr(),
                isize::try_from(key.len()).map_err(|_| ())?,
                0x0800_0100,
                0,
            )
        };
        if key.is_null() {
            return Err(());
        }
        Ok(Owned(key))
    }
    fn value(dictionary: Ref, key: &str) -> Result<Ref, ()> {
        let key = string(key)?;
        // SAFETY: 字典和 key 均为存活的正确类型；返回值借用自字典。
        Ok(unsafe { CFDictionaryGetValue(dictionary, key.0) })
    }
    fn number(dictionary: Ref, key: &str) -> Result<i32, ()> {
        let value = value(dictionary, key)?;
        if value.is_null() {
            return Ok(0);
        }
        // SAFETY: value 来自仍存活的字典；先检查 CFNumber 类型再读取 i32。
        unsafe {
            if CFGetTypeID(value) != CFNumberGetTypeID() {
                return Err(());
            }
            let mut result = 0_i32;
            if CFNumberGetValue(value, 3, std::ptr::addr_of_mut!(result).cast()) == 0 {
                return Err(());
            }
            Ok(result)
        }
    }
    fn has_string(dictionary: Ref, key: &str) -> Result<bool, ()> {
        let value = value(dictionary, key)?;
        if value.is_null() {
            return Ok(false);
        }
        // SAFETY: value 借用自仍存活的字典；先核对类型。
        unsafe {
            if CFGetTypeID(value) != CFStringGetTypeID() {
                return Err(());
            }
            Ok(CFStringGetLength(value) > 0)
        }
    }
    fn primary_interface(ipv6: bool) -> Result<Option<String>, ()> {
        let key = string(if ipv6 {
            "State:/Network/Global/IPv6"
        } else {
            "State:/Network/Global/IPv4"
        })?;
        // SAFETY: key 为存活的 CFString；NULL store 使用系统临时会话，只读动态存储。
        let dictionary = unsafe { SCDynamicStoreCopyValue(std::ptr::null(), key.0) };
        if dictionary.is_null() {
            return Ok(None);
        }
        let dictionary = Owned(dictionary);
        // SAFETY: Copy 返回的对象在 Owned 释放前有效；先验证字典类型。
        if unsafe { CFGetTypeID(dictionary.0) != CFDictionaryGetTypeID() } {
            return Err(());
        }
        let value = value(dictionary.0, "PrimaryInterface")?;
        if value.is_null() {
            return Ok(None);
        }
        // SAFETY: value 借用自仍存活的字典；先验证字符串类型再读取，缓冲区长度准确。
        unsafe {
            if CFGetTypeID(value) != CFStringGetTypeID() {
                return Err(());
            }
            let mut bytes = [0_i8; 256];
            if CFStringGetCString(value, bytes.as_mut_ptr(), 256, 0x0800_0100) == 0 {
                return Err(());
            }
            let name = std::ffi::CStr::from_ptr(bytes.as_ptr())
                .to_str()
                .map_err(|_| ())?;
            Ok((!name.is_empty()).then(|| name.to_owned()))
        }
    }

    pub(super) fn primary_network_interfaces() -> Vec<(String, bool)> {
        [false, true]
            .into_iter()
            .filter_map(|ipv6| {
                // 任一地址族的真实主接口可用即可；另一族不存在或读取失败不伪造主接口。
                primary_interface(ipv6)
                    .ok()
                    .flatten()
                    .map(|name| (name, ipv6))
            })
            .collect()
    }

    pub(super) fn read() -> Result<ProxySettings, ()> {
        // SAFETY: NULL 让 SystemConfiguration 使用临时 session；此 API 只读。
        let dictionary = unsafe { SCDynamicStoreCopyProxies(std::ptr::null()) };
        if dictionary.is_null() {
            // kSCStatusNoKey=1004 表示没有设置；其他错误不得伪造为关闭。
            return if unsafe { SCError() } == 1004 {
                Ok(ProxySettings::default())
            } else {
                Err(())
            };
        }
        let dictionary = Owned(dictionary);
        // SAFETY: Copy 返回的 CF 对象在 Owned 释放前有效。
        if unsafe { CFGetTypeID(dictionary.0) != CFDictionaryGetTypeID() } {
            return Err(());
        }
        let mut settings = ProxySettings::default();
        for protocol in ["HTTP", "HTTPS", "SOCKS", "FTP", "Gopher", "RTSP"] {
            if number(dictionary.0, &format!("{protocol}Enable"))? != 0 {
                settings.manual = true;
                let port = number(dictionary.0, &format!("{protocol}Port"))?;
                if has_string(dictionary.0, &format!("{protocol}Proxy"))?
                    && (1..=65535).contains(&port)
                {
                    settings.endpoints += 1;
                }
            }
        }
        settings.automatic = number(dictionary.0, "ProxyAutoConfigEnable")? != 0
            || number(dictionary.0, "ProxyAutoDiscoveryEnable")? != 0;
        Ok(settings)
    }
}

