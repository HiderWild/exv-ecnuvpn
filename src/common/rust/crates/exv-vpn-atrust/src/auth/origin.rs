//! 协议 HTTPS 网关端点（origin）解释：Common 内唯一的规范化与同源比较权威。
//!
//! 宿主负责把配置/用户文本补全为端点（补 `https://`、去尾斜杠），端点本身的解释、规范化与
//! 比较只由本模块给出：`AuthClient` 构造、`BrowserSession` 校验和宿主网关比较都委托
//! [`GatewayOrigin::parse`]，不再保留各自的解析副本。规范化只用于同源比较与端点文本，
//! 不回写旧消息的 `gateway` 字段，也不改变 TLS 主机名与 HTTP authority 的既有区别。

/// 协议 HTTPS 网关端点；内部字段不公开，只以访问器暴露规范化结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayOrigin {
    endpoint: String,
    host: String,
    port: u16,
}

/// 端点错误为固定分类，`Debug`/`Display` 都不携带输入材料；由消费者映射到各自既有错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("invalid HTTPS gateway endpoint")]
pub struct GatewayOriginError;

impl GatewayOrigin {
    /// 解释端点并规范化主机与端口：主机小写、IPv6 字面量去掉方括号、缺省端口按 443。
    ///
    /// 接受 `https` 方案（`http` crate 已把方案字符串小写）、非空主机、可选 1..=65535 端口、
    /// 空路径或 `/`；拒绝其它方案、空主机、端口 0、非法端口、非根路径、authority 中的 `@`、
    /// query 与 fragment。
    ///
    /// # Errors
    /// 输入不是上述形状时返回 [`GatewayOriginError`]，错误不包含输入文本。
    pub fn parse(endpoint: &str) -> Result<Self, GatewayOriginError> {
        let uri: http::Uri = endpoint.parse().map_err(|_| GatewayOriginError)?;
        let authority = uri.authority().ok_or(GatewayOriginError)?;
        if uri.scheme_str() != Some("https")
            || authority.as_str().contains('@')
            || uri.query().is_some()
            || !matches!(uri.path(), "" | "/")
            || endpoint.contains('#')
        {
            return Err(GatewayOriginError);
        }
        // `http::Uri` 不校验端口数字：`https://host:abc/` 会被解析成“没有端口”。
        // 因此主机之后出现 ':' 时（IPv6 字面量看 `]:`）必须给出合法端口，否则拒绝。
        let authority_text = authority.as_str();
        let has_port_suffix = match authority_text.strip_prefix('[') {
            Some(rest) => rest.contains("]:"),
            None => authority_text.contains(':'),
        };
        let explicit_port = uri.port_u16();
        if (has_port_suffix && explicit_port.is_none()) || explicit_port == Some(0) {
            return Err(GatewayOriginError);
        }
        let host = uri
            .host()
            .ok_or(GatewayOriginError)?
            .trim_matches(['[', ']'])
            .to_ascii_lowercase();
        if host.is_empty() {
            return Err(GatewayOriginError);
        }
        let port = explicit_port.unwrap_or(443);
        let endpoint = if host.contains(':') {
            format!("https://[{host}]:{port}")
        } else {
            format!("https://{host}:{port}")
        };
        Ok(Self {
            endpoint,
            host,
            port,
        })
    }

    /// 规范化端点文本 `https://{小写主机}:{端口}`；IPv6 字面量加方括号。
    #[must_use]
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// 规范化主机：小写、IPv6 字面量不含方括号；可直接作为 TLS `ServerName` 与解析目标。
    #[must_use]
    pub fn host(&self) -> &str {
        &self.host
    }

    /// 规范化端口：显式端口或缺省 443。
    #[must_use]
    pub fn port(&self) -> u16 {
        self.port
    }

    /// 同源比较只使用规范化后的主机与端口，不受输入大小写、显式 443 或尾斜杠影响。
    #[must_use]
    pub fn same_origin(&self, other: &Self) -> bool {
        self.host == other.host && self.port == other.port
    }
}
