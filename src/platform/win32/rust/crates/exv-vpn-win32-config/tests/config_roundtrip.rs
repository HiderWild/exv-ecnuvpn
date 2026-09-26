
//! Integration tests: config load/save round-trip, credential round-trip, and
//! 八条校园默认路由与历史配置保留行为。

use exv_vpn_win32_config::{
    config_dir, config_path, key_path, ConfigError, ExvConfig,
};

// ---------------------------------------------------------------------------
// 新建配置的默认值
// ---------------------------------------------------------------------------

#[test]
fn connection_mode_defaults_and_roundtrips_without_losing_credentials() {
    let old: ExvConfig = serde_json::from_str(r#"{"username":"student","password":"sealed"}"#).unwrap();
    assert_eq!(serde_json::to_value(&old).unwrap()["connection_mode"], "standard");
    for mode in ["standard", "compatibility"] {
        let input = serde_json::json!({"username":"student","password":"sealed","connection_mode":mode});
        let config: ExvConfig = serde_json::from_value(input).unwrap();
        let output = serde_json::to_value(config).unwrap();
        assert_eq!(output["connection_mode"], mode);
        assert_eq!(output["password"], "sealed");
    }
    let unknown: ExvConfig = serde_json::from_str(r#"{"username":"student","connection_mode":"future"}"#).unwrap();
    assert_eq!(unknown.username, "student");
    assert_eq!(serde_json::to_value(unknown).unwrap()["connection_mode"], "standard");
}

#[test]
fn defaults_contain_only_the_eight_campus_routes() {
    let cfg = ExvConfig::default();

    assert_eq!(cfg.server, "vpn-cn.ecnu.edu.cn");
    assert_eq!(cfg.username, "");
    assert_eq!(cfg.password, "");
    assert!(!cfg.remember_password);
    assert_eq!(cfg.user_agent, "AnyConnect Win_x86_64 4.10.05095");
    assert_eq!(cfg.mtu, 1290);

    let expected_routes = [
        "49.52.4.0/25",
        "59.78.176.0/20",
        "59.78.199.0/21",
        "58.198.176.128/25",
        "59.78.189.128/25",
        "219.228.63.0/21",
        "202.120.80.0/20",
        "219.228.144.0/22",
    ];
    assert_eq!(cfg.routes, expected_routes);
    // C1: auto-reconnect off by default, unlimited attempts (0 = unlimited).
    assert!(!cfg.auto_reconnect);
    assert_eq!(cfg.auto_reconnect_max_attempts, 0);
    // reconnect-backoff：默认关闭（关闭 = 维持现状立即重连）。
    assert!(!cfg.auto_reconnect_backoff);
}

// ---------------------------------------------------------------------------
// Load/save round-trip
// ---------------------------------------------------------------------------

#[test]
fn save_then_load_roundtrips() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cfg = ExvConfig {
        server: "vpn-ct.ecnu.edu.cn".to_string(),
        username: "student".to_string(),
        mtu: 1400,
        ..ExvConfig::default()
    };

    cfg.save_to_dir(dir.path()).expect("save");

    // Files created in the right layout.
    assert!(config_path(dir.path()).exists());
    assert!(dir.path().join("config.json").exists());

    let loaded = ExvConfig::load_from_dir(dir.path()).expect("load");
    assert_eq!(loaded, cfg);
}

#[test]
fn historical_routes_survive_load_and_save_without_migration() {
    let dir = tempfile::tempdir().expect("tempdir");
    let json = r#"{"routes":["219.228.60.69","222.66.117.0/24","10.0.0.0/8"]}"#;
    std::fs::write(config_path(dir.path()), json).expect("write historical config");
    let cfg = ExvConfig::load_from_dir(dir.path()).expect("load historical config");
    assert_eq!(cfg.routes, ["219.228.60.69", "222.66.117.0/24", "10.0.0.0/8"]);
    assert_eq!(std::fs::read_to_string(config_path(dir.path())).unwrap(), json);
    cfg.save_to_dir(dir.path()).expect("save historical config");
    let loaded = ExvConfig::load_from_dir(dir.path()).expect("reload");
    assert_eq!(loaded.routes, ["219.228.60.69", "222.66.117.0/24", "10.0.0.0/8"]);
}

/// 2026-09-08 计划 I6：历史 `server_bypass_ips`（手填网关 IP）字段已退役——
/// 旧配置携带该字段时解析不报错且值被丢弃（serde 未知字段容忍 = 忽略不读、
/// 读到就丢弃）；任何保存路径都写不出它（字段不存在于结构体 = 结构性禁止写入）。
#[test]
fn legacy_server_bypass_ips_is_dropped_on_load_and_never_written() {
    let dir = tempfile::tempdir().expect("tempdir");
    let json = r#"{
        "server": "vpn-cn.ecnu.edu.cn",
        "username": "student",
        "server_bypass_ips": ["222.66.117.109", "202.120.88.66"]
    }"#;
    std::fs::write(config_path(dir.path()), json).expect("write legacy config");

    // 读到就丢弃：解析成功，字段值不进入任何状态（结构体无该字段）。
    let cfg = ExvConfig::load_from_dir(dir.path()).expect("load legacy");
    assert_eq!(cfg.server, "vpn-cn.ecnu.edu.cn");

    // 不准写入：保存后的落盘 JSON 不得再含该键（一次性保存即清洗旧文件）。
    cfg.save_to_dir(dir.path()).expect("save");
    let saved = std::fs::read_to_string(dir.path().join("config.json")).expect("read saved");
    assert!(
        !saved.contains("server_bypass_ips"),
        "保存输出不得包含 server_bypass_ips：{saved}"
    );
    assert!(
        !saved.contains("222.66.117.109") && !saved.contains("202.120.88.66"),
        "保存输出不得包含任何网关 IP：{saved}"
    );
}

#[test]
fn load_missing_config_yields_default() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cfg = ExvConfig::load_from_dir(dir.path()).expect("load missing");
    assert_eq!(cfg, ExvConfig::default());
}

#[test]
fn load_invalid_config_yields_default() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(config_path(dir.path()), "not json{").expect("write invalid");
    let cfg = ExvConfig::load_from_dir(dir.path()).expect("load invalid");
    assert_eq!(cfg, ExvConfig::default());
}

#[test]
fn load_unknown_fields_ignored_and_partial_defaulted() {
    let dir = tempfile::tempdir().expect("tempdir");
    let json = r#"{
        "server": "vpn-lt.ecnu.edu.cn",
        "username": "student",
        "some_future_field": 42
    }"#;
    std::fs::write(config_path(dir.path()), json).expect("write partial");

    let cfg = ExvConfig::load_from_dir(dir.path()).expect("load partial");
    assert_eq!(cfg.server, "vpn-lt.ecnu.edu.cn");
    assert_eq!(cfg.username, "student");
    // Missing fields fall back to defaults.
    assert_eq!(cfg.mtu, 1290);
    assert_eq!(cfg.user_agent, "AnyConnect Win_x86_64 4.10.05095");
    // 路由同样回落默认集合：与默认构造逐项一致（不写死条数或字面清单——默认集合的
    // 精确内容由 defaults_contain_only_the_eight_campus_routes 单独钉住）。
    assert_eq!(cfg.routes, ExvConfig::default().routes);
    assert!(!cfg.remember_password);
}

#[test]
fn load_old_config_without_auto_reconnect_fields_yields_defaults() {
    // 旧 config.json（新字段未写入）→ 结构体级 `#[serde(default)]` 用 Default 补齐。
    let dir = tempfile::tempdir().expect("tempdir");
    let json = r#"{
        "server": "vpn-cn.ecnu.edu.cn",
        "username": "student",
        "mtu": 1290
    }"#;
    std::fs::write(config_path(dir.path()), json).expect("write old config");

    let cfg = ExvConfig::load_from_dir(dir.path()).expect("load old config");
    assert_eq!(cfg.server, "vpn-cn.ecnu.edu.cn");
    // 缺省字段回退默认：auto_reconnect=false，auto_reconnect_max_attempts=0，
    // auto_reconnect_backoff=false。
    assert!(!cfg.auto_reconnect);
    assert_eq!(cfg.auto_reconnect_max_attempts, 0);
    assert!(!cfg.auto_reconnect_backoff);
}

#[test]
fn auto_reconnect_fields_save_load_roundtrip() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cfg = ExvConfig {
        auto_reconnect: true,
        auto_reconnect_max_attempts: 5,
        auto_reconnect_backoff: true,
        ..ExvConfig::default()
    };

    cfg.save_to_dir(dir.path()).expect("save");
    let loaded = ExvConfig::load_from_dir(dir.path()).expect("load");
    assert_eq!(loaded, cfg);
    assert!(loaded.auto_reconnect);
    assert_eq!(loaded.auto_reconnect_max_attempts, 5);
    assert!(loaded.auto_reconnect_backoff, "退避开关随配置往返");
}

// ---------------------------------------------------------------------------
// Password encrypt/decrypt round-trip via key.bin
// ---------------------------------------------------------------------------

#[test]
fn password_roundtrip_via_key_file() {
    let dir = tempfile::tempdir().expect("tempdir");

    let key = ExvConfig::ensure_key(dir.path()).expect("ensure key");
    assert_eq!(key.len(), 32);
    assert!(key_path(dir.path()).exists());

    let mut cfg = ExvConfig::default();
    cfg.set_password_encrypted("s3cret!", &key).expect("encrypt");
    assert!(cfg.remember_password);
    assert_ne!(cfg.password, "s3cret!");
    assert!(!cfg.password.is_empty());

    let decrypted = cfg.decrypt_password(&key).expect("decrypt");
    assert_eq!(decrypted, "s3cret!");
}

#[test]
fn password_roundtrip_after_save_load() {
    let dir = tempfile::tempdir().expect("tempdir");
    let key = ExvConfig::ensure_key(dir.path()).expect("ensure key");

    let mut cfg = ExvConfig {
        username: "student".to_string(),
        ..ExvConfig::default()
    };
    cfg.set_password_encrypted("hunter2", &key).expect("encrypt");
    cfg.save_to_dir(dir.path()).expect("save");

    // Simulate a fresh load: ciphertext persists, key file persists.
    let loaded = ExvConfig::load_from_dir(dir.path()).expect("load");
    assert_ne!(loaded.password, "hunter2");
    let loaded_key = ExvConfig::load_key(dir.path())
        .expect("load key")
        .expect("key present");
    assert_eq!(loaded_key, key);
    assert_eq!(loaded.decrypt_password(&loaded_key).expect("decrypt"), "hunter2");
}

#[test]
fn password_decrypt_wrong_key_fails() {
    let dir = tempfile::tempdir().expect("tempdir");
    let key = ExvConfig::ensure_key(dir.path()).expect("ensure key");

    let mut cfg = ExvConfig::default();
    cfg.set_password_encrypted("secret", &key).expect("encrypt");

    let wrong_key = [7u8; 32];
    let result = cfg.decrypt_password(&wrong_key);
    assert!(matches!(result, Err(ConfigError::Crypto(_))));
}

#[test]
fn ensure_key_preserves_existing_key() {
    let dir = tempfile::tempdir().expect("tempdir");
    let key = ExvConfig::ensure_key(dir.path()).expect("first");
    let again = ExvConfig::ensure_key(dir.path()).expect("second");
    assert_eq!(key, again);
}

#[test]
fn load_key_rejects_wrong_length() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(key_path(dir.path()), [1u8; 16]).expect("write short key");
    let result = ExvConfig::load_key(dir.path());
    assert!(matches!(result, Err(ConfigError::Malformed(_))));
}

// ---------------------------------------------------------------------------
// config_dir resolution
// ---------------------------------------------------------------------------

#[test]
fn config_dir_defaults_to_dot_exv() {
    // EXV_CONFIG_DIR override is not set in CI; the default path must end in
    // `.exv` (via USERPROFILE or HOME) rather than be empty.
    let dir = config_dir();
    assert!(!dir.as_os_str().is_empty());
    assert!(dir.file_name().is_some_and(|n| n == ".exv"));
}

