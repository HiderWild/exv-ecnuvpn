//! Startup classification and idempotent repair for `config.json`.

use std::path::Path;

use serde_json::{Map, Value};

use crate::{ConfigError, ExvConfig, config::write_config_json_atomically, paths::config_path};

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
