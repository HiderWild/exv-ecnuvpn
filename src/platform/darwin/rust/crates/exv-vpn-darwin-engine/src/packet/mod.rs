//! Engine 内 utun ↔ CSTP 数据通路（帧头、双向泵）。
//!
//! 帧 AF 头为 `AF_INET`（大端，实测）。

// 中文文档中的技术术语不逐个加反引号。
#![allow(clippy::doc_markdown)]

pub mod probe;
pub mod pump;

/// utun 4 字节协议头（AF_INET=2；实测 macOS 内核按大端投放与接收）。
pub const AF_INET_HEADER: [u8; 4] = 2_u32.to_be_bytes();
