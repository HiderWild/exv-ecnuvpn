//! 公共 SystemConfiguration API：配置锁内只替换本次管理的例外或 PAC 字段。

use super::{PacSettings, Result, Settings, Store, Update};
use std::ffi::{CStr, c_char, c_void};
use std::ptr::{addr_of, addr_of_mut, null};
type Ref = *const c_void;
const UTF8: u32 = 0x0800_0100;

#[link(name = "SystemConfiguration", kind = "framework")]
unsafe extern "C" {
    fn SCPreferencesCreate(allocator: Ref, name: Ref, prefs_id: Ref) -> Ref;
    fn SCPreferencesLock(prefs: Ref, wait: u8) -> u8;
    fn SCPreferencesUnlock(prefs: Ref) -> u8;
    fn SCPreferencesCommitChanges(prefs: Ref) -> u8;
    fn SCPreferencesApplyChanges(prefs: Ref) -> u8;
    fn SCNetworkServiceCopy(prefs: Ref, service_id: Ref) -> Ref;
    fn SCNetworkServiceCopyProtocol(service: Ref, protocol_type: Ref) -> Ref;
    fn SCNetworkProtocolGetConfiguration(protocol: Ref) -> Ref;
    fn SCNetworkProtocolSetConfiguration(protocol: Ref, config: Ref) -> u8;
    fn SCDynamicStoreCopyValue(store: Ref, key: Ref) -> Ref;
    fn SCDynamicStoreCopyProxies(store: Ref) -> Ref;
    fn SCError() -> i32;
}
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(value: Ref);
    fn CFGetTypeID(value: Ref) -> usize;
    fn CFStringGetTypeID() -> usize;
    fn CFArrayGetTypeID() -> usize;
    fn CFDictionaryGetTypeID() -> usize;
    fn CFNumberGetTypeID() -> usize;
    fn CFStringCreateWithBytes(
        allocator: Ref,
        bytes: *const u8,
        length: isize,
        encoding: u32,
        external: u8,
    ) -> Ref;
    fn CFStringGetLength(value: Ref) -> isize;
    fn CFStringGetMaximumSizeForEncoding(length: isize, encoding: u32) -> isize;
    fn CFStringGetCString(value: Ref, buffer: *mut c_char, size: isize, encoding: u32) -> u8;
    fn CFDictionaryGetValue(dictionary: Ref, key: Ref) -> Ref;
    fn CFDictionaryCreateMutableCopy(allocator: Ref, capacity: isize, dictionary: Ref) -> Ref;
    fn CFDictionarySetValue(dictionary: Ref, key: Ref, value: Ref);
    fn CFDictionaryRemoveValue(dictionary: Ref, key: Ref);
    fn CFNumberGetValue(value: Ref, kind: isize, output: *mut c_void) -> u8;
    fn CFNumberCreate(allocator: Ref, kind: isize, value: *const c_void) -> Ref;
    fn CFArrayGetCount(value: Ref) -> isize;
    fn CFArrayGetValueAtIndex(value: Ref, index: isize) -> Ref;
    fn CFArrayCreateMutable(allocator: Ref, capacity: isize, callbacks: Ref) -> Ref;
    fn CFArrayAppendValue(array: Ref, value: Ref);
    static kCFTypeArrayCallBacks: [usize; 5];
}

struct Owned(Ref);
impl Drop for Owned {
    fn drop(&mut self) {
        unsafe { CFRelease(self.0) };
    }
}
fn own(value: Ref, action: &str) -> Result<Owned> {
    if value.is_null() {
        Err(error(action))
    } else {
        Ok(Owned(value))
    }
}
fn error(action: &str) -> String {
    format!("{action}: SCError={}", unsafe { SCError() })
}
fn check(value: u8, action: &str) -> Result<()> {
    if value == 0 {
        Err(error(action))
    } else {
        Ok(())
    }
}
fn string(value: &str) -> Result<Owned> {
    own(
        unsafe { CFStringCreateWithBytes(null(), value.as_ptr(), value.len() as isize, UTF8, 0) },
        "CFStringCreate",
    )
}
fn text(value: Ref) -> Result<String> {
    if value.is_null() || unsafe { CFGetTypeID(value) != CFStringGetTypeID() } {
        return Err("expected CFString".into());
    }
    let size = unsafe { CFStringGetMaximumSizeForEncoding(CFStringGetLength(value), UTF8) } + 1;
    let mut bytes = vec![0_i8; size as usize];
    check(
        unsafe { CFStringGetCString(value, bytes.as_mut_ptr(), size, UTF8) },
        "CFStringGetCString",
    )?;
    unsafe { CStr::from_ptr(bytes.as_ptr()) }
        .to_str()
        .map(str::to_owned)
        .map_err(|e| e.to_string())
}
fn field(dictionary: Ref, key: &str) -> Result<Ref> {
    if dictionary.is_null() || unsafe { CFGetTypeID(dictionary) != CFDictionaryGetTypeID() } {
        return Err("expected CFDictionary".into());
    }
    Ok(unsafe { CFDictionaryGetValue(dictionary, string(key)?.0) })
}
fn number(dictionary: Ref, key: &str) -> Result<Option<i32>> {
    let value = field(dictionary, key)?;
    if value.is_null() {
        return Ok(None);
    }
    if unsafe { CFGetTypeID(value) != CFNumberGetTypeID() } {
        return Err(format!("{key}: expected CFNumber"));
    }
    let mut number = 0_i32;
    check(
        unsafe { CFNumberGetValue(value, 3, addr_of_mut!(number).cast()) },
        "CFNumberGetValue",
    )?;
    Ok(Some(number))
}
fn enabled(dictionary: Ref, key: &str) -> Result<bool> {
    Ok(number(dictionary, key)?.unwrap_or(0) != 0)
}
fn pac(dictionary: Ref) -> Result<PacSettings> {
    let url = field(dictionary, "ProxyAutoConfigURLString")?;
    Ok(PacSettings {
        enabled: number(dictionary, "ProxyAutoConfigEnable")?,
        url: if url.is_null() {
            None
        } else {
            Some(text(url)?)
        },
    })
}
fn set_pac(dictionary: Ref, value: &PacSettings) -> Result<()> {
    let key = string("ProxyAutoConfigEnable")?;
    if let Some(number) = value.enabled {
        let number = own(
            unsafe { CFNumberCreate(null(), 3, addr_of!(number).cast()) },
            "CFNumberCreate",
        )?;
        unsafe { CFDictionarySetValue(dictionary, key.0, number.0) };
    } else {
        unsafe { CFDictionaryRemoveValue(dictionary, key.0) };
    }
    let key = string("ProxyAutoConfigURLString")?;
    if let Some(url) = &value.url {
        unsafe { CFDictionarySetValue(dictionary, key.0, string(url)?.0) };
    } else {
        unsafe { CFDictionaryRemoveValue(dictionary, key.0) };
    }
    Ok(())
}
fn exceptions(dictionary: Ref) -> Result<Option<Vec<String>>> {
    let value = field(dictionary, "ExceptionsList")?;
    if value.is_null() {
        return Ok(None);
    }
    if unsafe { CFGetTypeID(value) != CFArrayGetTypeID() } {
        return Err("ExceptionsList: expected CFArray".into());
    }
    let mut result = Vec::new();
    for index in 0..unsafe { CFArrayGetCount(value) } {
        result.push(text(unsafe { CFArrayGetValueAtIndex(value, index) })?);
    }
    Ok(Some(result))
}
struct Preferences(Owned);
impl Preferences {
    fn locked() -> Result<Self> {
        let prefs = own(
            unsafe { SCPreferencesCreate(null(), string("EXV campus proxy policy")?.0, null()) },
            "SCPreferencesCreate",
        )?;
        check(
            unsafe { SCPreferencesLock(prefs.0, 0) },
            "SCPreferencesLock",
        )?;
        Ok(Self(prefs))
    }
}
impl Drop for Preferences {
    fn drop(&mut self) {
        unsafe { SCPreferencesUnlock(self.0.0) };
    }
}

pub struct NativeStore;
impl Store for NativeStore {
    fn primary_service(&mut self) -> Result<String> {
        let state = own(
            unsafe { SCDynamicStoreCopyValue(null(), string("State:/Network/Global/IPv4")?.0) },
            "primary IPv4 service",
        )?;
        text(field(state.0, "PrimaryService")?)
    }
    fn transaction<T>(
        &mut self,
        service: &str,
        action: impl FnOnce(Settings) -> Result<(T, Option<Update>)>,
    ) -> Result<T> {
        let prefs = Preferences::locked()?;
        let service_id = string(service)?;
        let service = own(
            unsafe { SCNetworkServiceCopy(prefs.0.0, service_id.0) },
            "SCNetworkServiceCopy",
        )?;
        let protocol = own(
            unsafe { SCNetworkServiceCopyProtocol(service.0, string("Proxies")?.0) },
            "SCNetworkServiceCopyProtocol",
        )?;
        let config = unsafe { SCNetworkProtocolGetConfiguration(protocol.0) };
        let settings = Settings {
            exceptions: exceptions(config)?,
            manual_http: enabled(config, "HTTPEnable")? || enabled(config, "HTTPSEnable")?,
            automatic: enabled(config, "ProxyAutoConfigEnable")?
                || enabled(config, "ProxyAutoDiscoveryEnable")?,
            pac: pac(config)?,
        };
        let (result, update) = action(settings)?;
        if let Some(update) = update {
            let dictionary = own(
                unsafe { CFDictionaryCreateMutableCopy(null(), 0, config) },
                "CFDictionaryCreateMutableCopy",
            )?;
            match &update {
                Update::Pac(value) => set_pac(dictionary.0, value)?,
                Update::Exceptions(entries) => {
                    let key = string("ExceptionsList")?;
                    if let Some(entries) = entries {
                        let array = own(
                            unsafe {
                                CFArrayCreateMutable(
                                    null(),
                                    0,
                                    addr_of!(kCFTypeArrayCallBacks).cast(),
                                )
                            },
                            "CFArrayCreateMutable",
                        )?;
                        for entry in entries {
                            unsafe { CFArrayAppendValue(array.0, string(&entry)?.0) };
                        }
                        unsafe { CFDictionarySetValue(dictionary.0, key.0, array.0) };
                    } else {
                        unsafe { CFDictionaryRemoveValue(dictionary.0, key.0) };
                    }
                }
            }
            apply_configuration(
                || check(unsafe { SCNetworkProtocolSetConfiguration(protocol.0, dictionary.0) }, "SCNetworkProtocolSetConfiguration"),
                || check(unsafe { SCPreferencesCommitChanges(prefs.0.0) }, "SCPreferencesCommitChanges"),
                || check(unsafe { SCPreferencesApplyChanges(prefs.0.0) }, "SCPreferencesApplyChanges"),
            )?;
        }
        Ok(result)
    }
    fn effective_pac(&mut self) -> Result<PacSettings> {
        let dictionary = own(
            unsafe { SCDynamicStoreCopyProxies(null()) },
            "SCDynamicStoreCopyProxies",
        )?;
        pac(dictionary.0)
    }
    fn effective_exceptions(&mut self) -> Result<Option<Vec<String>>> {
        let dictionary = own(
            unsafe { SCDynamicStoreCopyProxies(null()) },
            "SCDynamicStoreCopyProxies",
        )?;
        exceptions(dictionary.0)
    }
}

// 只串联本次变更的三个原 API；成功后不再打开 Preferences 或查询效果。
fn apply_configuration(
    set: impl FnOnce() -> Result<()>,
    commit: impl FnOnce() -> Result<()>,
    apply: impl FnOnce() -> Result<()>,
) -> Result<()> {
    set()?;
    commit()?;
    apply()
}
