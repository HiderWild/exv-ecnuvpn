//! Startup classification and idempotent repair for `config.json`.

use std::path::Path;

use serde_json::{Map, Value};

use crate::{
    ConfigError, ExvConfig, Protocol,
    config::{DEFAULT_ATRUST_SERVER, DEFAULT_SERVER, write_config_json_atomically},
    paths::config_path,
};

/// The configuration made available to startup callers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupConfig {
    /// Fully hydrated configuration with every known field populated.
    pub config: ExvConfig,
    /// Whether startup had to replace a missing or unusable config with defaults.
    pub requires_quick_start: bool,
}

/// Load, classify, and repair configuration for application startup.
///
/// Missing, blank, malformed, non-object, and known-schema-invalid JSON are
/// replaced with a complete default configuration and require Quick Start.
/// Every valid JSON object is hydrated with known defaults while retaining its
/// unknown fields. The repaired representation is atomically persisted.
pub fn load_for_startup(dir: &Path) -> Result<StartupConfig, ConfigError> {
    let path = config_path(dir);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return bootstrap_default(dir);
        }
        Err(error) => return Err(error.into()),
    };

    let object = match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(object)) => object,
        Ok(_) | Err(_) => return bootstrap_default(dir),
    };
    let mut object = object;
    hydrate_legacy_server_fields(&mut object);
    let config = match serde_json::from_value::<ExvConfig>(Value::Object(object.clone())) {
        Ok(config) => config,
        Err(_) => return bootstrap_default(dir),
    };

    let hydrated = hydrate_known_fields(object, &config)?;
    write_config_json_atomically(dir, &serde_json::to_string_pretty(&hydrated)?)?;
    Ok(StartupConfig {
        config,
        requires_quick_start: false,
    })
}

/// Persist an explicit user submission after removing unknown fields.
///
/// Unlike startup hydration, an explicit submission is the hygiene boundary:
/// the resulting document is exactly the current known [`ExvConfig`] schema.
pub fn save_after_user_submission(dir: &Path, config: &ExvConfig) -> Result<(), ConfigError> {
    config.save_to_dir(dir)
}

fn bootstrap_default(dir: &Path) -> Result<StartupConfig, ConfigError> {
    let config = ExvConfig::default();
    save_after_user_submission(dir, &config)?;
    Ok(StartupConfig {
        config,
        requires_quick_start: true,
    })
}

/// 将旧活动 server 迁移为协议专属服务器字段，与 `load_from_dir` 保持一致。
fn hydrate_legacy_server_fields(object: &mut Map<String, Value>) {
    let protocol = object
        .get("protocol")
        .and_then(Value::as_str)
        .and_then(|raw| raw.parse::<Protocol>().ok())
        .unwrap_or(Protocol::Cisco);
    if !object.contains_key("protocol") {
        object.insert("protocol".into(), Value::String(Protocol::Cisco.as_str().into()));
    }
    if !object.contains_key("server") {
        let default = match protocol {
            Protocol::Cisco => DEFAULT_SERVER,
            Protocol::Atrust => DEFAULT_ATRUST_SERVER,
        };
        object.insert("server".into(), Value::String(default.into()));
    }
    let legacy_server = object
        .get("server")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if !object.contains_key("cisco_server") {
        let value = if protocol == Protocol::Cisco {
            legacy_server.as_str()
        } else {
            DEFAULT_SERVER
        };
        object.insert("cisco_server".into(), Value::String(value.into()));
    }
    if !object.contains_key("atrust_server") {
        let value = if protocol == Protocol::Atrust {
            legacy_server.as_str()
        } else {
            DEFAULT_ATRUST_SERVER
        };
        object.insert("atrust_server".into(), Value::String(value.into()));
    }
}

fn hydrate_known_fields(
    mut original: Map<String, Value>,
    config: &ExvConfig,
) -> Result<Value, ConfigError> {
    let Value::Object(known_fields) = serde_json::to_value(config)? else {
        unreachable!("ExvConfig always serializes to an object");
    };
    original.extend(known_fields);
    Ok(Value::Object(original))
}
