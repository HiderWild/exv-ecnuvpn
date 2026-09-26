//! Darwin 服务内部修订号（单列于 app 营销版本之外的自增计数）。
//!
//! ## 为什么需要
//!
//! 同一个 app 版本内可能发生多次服务侧（engine）内部更新；调试「服务覆盖安装」时
//! 需要随时回答「装在机器上的 engine 是第几版」。营销版本不承载这个语义。
//!
//! ## 方案：修订号作为二进制**元数据**嵌入，读文件即可查询（不依赖进程运行/请求）
//!
//! 修订号在编译期写入可执行文件的一个自定义 Mach-O section
//! `(__TEXT,__exv_revision)`，内容是修订号的 u32 little-endian 4 字节。查询侧
//! **不运行进程**、不发任何请求——直接解析 Mach-O 头找到该 section 读 4 字节。
//! 这是 macOS 可执行文件原生自带的元数据机制（与 Windows PE 的
//! Version Resource、代码段常量同一思路；Rust 用 `#[link_section]` 落进二进制）。
//!
//! 因为 launchd 总是从**固定安装路径**启动 engine（`KeepAlive` re-exec），「已装
//! engine 文件的嵌入式修订号」同时就是「下一次/正在运行的 engine 修订号」——
//! 文件与运行两个问题坍缩成一个，不再需要运行期写叶、也不需要 socket 门控。
//!
//! 开发 debug 与发布裁剪（release+strip）都保留该 section：这是二进制最简
//! 元数据，strip="symbols" 不删自定义 `#[used]` 数据段（构建验证已用 release
//! 产物实测 `otool` 可读）。
//!
//! ## bump 纪律
//!
//! 触及 engine 运行逻辑的提交应 +1 本常量（`scripts/bump-darwin-service-revision.sh`）。
//! 忘 bump 会让已装/随附比对显示「旧版」，是人工可见的信号而非静默错误。
//!
//! ## 边界（不做什么）
//!
//! * **不是升级决策器**：只读观测，不触发安装/覆盖副作用。
//! * **非协议协商**：wire 冻结，不进任何协议字段。
//! * **不含 win32 侧**。
//! * **不参与健康五态判定**：`ServiceStatus` 五态词汇不受影响（本模块与
//!   `service_status` 正交）。

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Darwin 服务内部修订号（engine 与 Core 共同编译，进 engine 二进制的元数据段）。
pub const DARWIN_SERVICE_REVISION: u32 = 1;

/// 修订号元数据所在的 Mach-O segment（`link_section` 写作 `"{segment},{section}"`）。
pub const REVISION_SEGMENT: &str = "__TEXT";
/// 修订号元数据所在的 Mach-O section 名（≤15 字节 + NUL，Mach-O 名字上限）。
pub const REVISION_SECTION: &str = "__exv_revision";

/// `otool`/`llvm-objdump` 可直接读的人写 section 名（`otool -s __TEXT __exv_revision <bin>`）。
#[must_use]
pub const fn revision_section_specifier() -> &'static str {
    "__TEXT,__exv_revision"
}

/// 把修订号作为 4 字节 u32 little-endian 数据落进 `__TEXT,__exv_revision`。
///
/// 供需要携带修订号的二进制（engine；Core 自测）在 crate 根部调用一次：
/// `exv_vpn_darwin_ipc::embed_service_revision!()`。`#[used]` 阻止链接器回收，
/// 纯数据 section 不参与代码执行。
#[macro_export]
macro_rules! embed_service_revision {
    () => {
        // `link_section` 在 Rust 2024 是 unsafe attribute，须 `unsafe(...)` 包裹。
        #[used]
        #[unsafe(link_section = "__TEXT,__exv_revision")]
        static EXV_ENGINE_SERVICE_REVISION: [u8; 4] =
            $crate::service_revision::DARWIN_SERVICE_REVISION.to_le_bytes();
    };
}

/// Mach-O 64 头魔数（thin，little-endian）。
const MH_MAGIC_64: u32 = 0xfeed_facf;
/// fat/universal 头魔数（big-endian）。
const FAT_MAGIC: u32 = 0xcafe_babe;
/// `LC_SEGMENT_64` load command。
const LC_SEGMENT_64: u32 = 0x19;
/// arm64 CPU type（fat 中优先选它）。
const CPU_TYPE_ARM64: u32 = 0x0100_000c;
/// fat 单个 slice 头的字节数（`fat_arch` × 20）。
const FAT_ARCH_BYTES: usize = 20;
/// thin 头 32 字节 + 首个 load command 起始。
const MACHO_HEADER_BYTES: usize = 32;
/// load commands 区读取的防御上限（畸形文件声明再大也只读此量）。
const CMDS_READ_CAP: usize = 4 * 1024 * 1024;

/// 从 thin Mach-O 头解析 load command 总量信息：`(ncmds, sizeofcmds)`。
fn thin_header_counts(bytes: &[u8]) -> Option<(usize, usize)> {
    if bytes.len() < MACHO_HEADER_BYTES {
        return None;
    }
    let ncmds = u32::from_le_bytes(bytes[16..20].try_into().ok()?) as usize;
    let sizeofcmds = u32::from_le_bytes(bytes[20..24].try_into().ok()?) as usize;
    Some((ncmds, sizeofcmds))
}

/// 在 load commands 字节区（不含 32 字节头）里找目标 section 的**内容文件偏移**
/// （Mach-O section `offset` 字段；thin 中相对文件头，fat 中相对 slice 头）。
/// 找不到/结构非法 → `None`。
fn find_revision_content_offset_in_cmds(cmds: &[u8], ncmds: usize) -> Option<usize> {
    let mut cursor = 0usize;
    for _ in 0..ncmds {
        let cmd_bytes = cmds.get(cursor..cursor + 8)?;
        let cmd = u32::from_le_bytes(cmd_bytes[0..4].try_into().ok()?);
        let cmdsize = u32::from_le_bytes(cmd_bytes[4..8].try_into().ok()?) as usize;
        if cmdsize < 8 {
            return None;
        }
        if cmd == LC_SEGMENT_64 && cmdsize >= 72 {
            let seg = &cmds[cursor..cursor + cmdsize];
            if cstring_name(&seg[8..24]) == REVISION_SEGMENT.as_bytes() {
                let nsects = u32::from_le_bytes(seg[64..68].try_into().ok()?) as usize;
                let mut sect_cursor = cursor + 72;
                for _ in 0..nsects {
                    let sect = cmds.get(sect_cursor..sect_cursor + 80)?;
                    if cstring_name(&sect[16..32]) == REVISION_SEGMENT.as_bytes()
                        && cstring_name(&sect[0..16]) == REVISION_SECTION.as_bytes()
                    {
                        return u32::from_le_bytes(sect[48..52].try_into().ok()?)
                            .try_into()
                            .ok();
                    }
                    sect_cursor += 80;
                }
            }
        }
        cursor += cmdsize;
    }
    None
}

/// 从完整 Mach-O（thin 或 fat）字节解析修订号：合成/内存 buffer 用（单测）；
/// 内容须在同一 buffer 内（含 load commands 与 section 内容）。非 Mach-O/缺失
/// section → `None`。
#[must_use]
pub fn parse_revision_from_macho(bytes: &[u8]) -> Option<u32> {
    if bytes.len() < 4 {
        return None;
    }
    let magic = u32::from_le_bytes(bytes[0..4].try_into().ok()?);
    if magic == FAT_MAGIC {
        // fat 头全部 big-endian；取 arm64 slice（缺省取第一个）。
        let nfat = u32::from_be_bytes(bytes[4..8].try_into().ok()?) as usize;
        let mut chosen: Option<(usize, usize)> = None; // (offset, size)
        let mut cursor = 8usize;
        for _ in 0..nfat {
            let arch = bytes.get(cursor..cursor + FAT_ARCH_BYTES)?;
            let cputype = u32::from_be_bytes(arch[0..4].try_into().ok()?);
            let offset = u32::from_be_bytes(arch[8..12].try_into().ok()?) as usize;
            let size = u32::from_be_bytes(arch[12..16].try_into().ok()?) as usize;
            if cputype == CPU_TYPE_ARM64 {
                chosen = Some((offset, size));
                break;
            }
            chosen.get_or_insert((offset, size));
            cursor += FAT_ARCH_BYTES;
        }
        let (offset, size) = chosen?;
        let slice = bytes.get(offset..offset + size)?;
        parse_revision_from_macho(slice)
    } else if magic == MH_MAGIC_64 {
        let (ncmds, sizeofcmds) = thin_header_counts(bytes)?;
        let cmds = bytes.get(MACHO_HEADER_BYTES..MACHO_HEADER_BYTES + sizeofcmds)?;
        let content_offset = find_revision_content_offset_in_cmds(cmds, ncmds)?;
        let content = bytes.get(content_offset..content_offset + 4)?;
        Some(u32::from_le_bytes(content.try_into().ok()?))
    } else {
        None
    }
}

/// 从 16 字节定长 Mach-O 名字字段取 NUL 截断的有效名。
fn cstring_name(field: &[u8]) -> &[u8] {
    match field.iter().position(|&byte| byte == 0) {
        Some(end) => &field[..end],
        None => field,
    }
}

/// 从磁盘可执行文件读嵌入式修订号（有界：只读文件头 + load commands 区 + 目标
/// section 的 4 字节，**绝不读全文件**——debug 构建可达百 MB 也不受影响）。
/// 文件缺失/不可读/非 Mach-O/无目标 section → `None`（fail-soft，只读观测不猜测）。
#[must_use]
pub fn read_embedded_revision(path: &Path) -> Option<u32> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut head = [0u8; MACHO_HEADER_BYTES];
    let got = file.read(&mut head).ok()?;
    if got < MACHO_HEADER_BYTES {
        return None;
    }
    let magic = u32::from_le_bytes(head[0..4].try_into().ok()?);
    if magic != MH_MAGIC_64 && magic != FAT_MAGIC {
        return None;
    }
    // fat 时先定位 arm64 slice 基址；thin 基址为 0。slice 内的 Mach-O 偏移相对
    // slice 头，因此内容绝对位置 = slice_base + 解析出的 section offset。
    let slice_base: u64 = if magic == FAT_MAGIC {
        // 读足 fat 头（8 + nfat*20）。
        let mut fat_head = [0u8; 8 + 16 * FAT_ARCH_BYTES];
        file.seek(SeekFrom::Start(0)).ok()?;
        let got = file.read(&mut fat_head).ok()?;
        let nfat = u32::from_be_bytes(fat_head.get(4..8)?.try_into().ok()?) as usize;
        if 8 + nfat * FAT_ARCH_BYTES > got {
            return None;
        }
        let mut base: Option<u64> = None;
        for i in 0..nfat {
            let arch = &fat_head[8 + i * FAT_ARCH_BYTES..8 + (i + 1) * FAT_ARCH_BYTES];
            let cputype = u32::from_be_bytes(arch[0..4].try_into().ok()?);
            let offset = u32::from_be_bytes(arch[8..12].try_into().ok()?);
            if cputype == CPU_TYPE_ARM64 {
                base = Some(u64::from(offset));
                break;
            }
            base.get_or_insert(u64::from(offset));
        }
        base?
    } else {
        0
    };
    // 读 slice 头。
    file.seek(SeekFrom::Start(slice_base)).ok()?;
    let mut slice_head = [0u8; MACHO_HEADER_BYTES];
    let got = file.read(&mut slice_head).ok()?;
    if got < MACHO_HEADER_BYTES {
        return None;
    }
    let (ncmds, sizeofcmds) = thin_header_counts(&slice_head)?;
    let to_read = sizeofcmds.min(CMDS_READ_CAP);
    let mut cmds_buf = vec![0u8; to_read];
    // 关键：load commands 紧跟在 **slice 头之后**（thin 的 32 字节头）。上面读完
    // slice 头后文件指针恰在 32 字节处，直接续读即得 commands 区。
    let got = file.read(&mut cmds_buf).ok()?;
    cmds_buf.truncate(got);
    let content_rel = find_revision_content_offset_in_cmds(&cmds_buf, ncmds)?;
    file.seek(SeekFrom::Start(slice_base + content_rel as u64)).ok()?;
    let mut content = [0u8; 4];
    let got = file.read(&mut content).ok()?;
    if got < 4 {
        return None;
    }
    Some(u32::from_le_bytes(content))
}

