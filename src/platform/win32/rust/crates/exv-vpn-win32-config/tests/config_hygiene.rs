//! Startup config classification and persistence hygiene contracts.

use exv_vpn_win32_config::{
    ExvConfig, config_path, load_for_startup, save_after_user_submission,
};

fn assert_default_config_file(dir: &std::path::Path) {
    let text = std::fs::read_to_string(config_path(dir)).expect("default config written");
    let parsed: ExvConfig = serde_json::from_str(&text).expect("default config parses");
    assert_eq!(parsed, ExvConfig::default());
}

#[test]
fn compatibility_load_is_non_mutating_while_startup_bootstraps_missing_config() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = config_path(dir.path());

    let compatibility_config = ExvConfig::load_from_dir(dir.path()).expect("compatibility load");
    assert_eq!(compatibility_config, ExvConfig::default());
    assert!(
        !path.exists(),
        "the compatibility API must not create config.json or decide Quick Start"
    );

    let startup = load_for_startup(dir.path()).expect("startup load");
    assert!(startup.requires_quick_start);
    assert_eq!(startup.config, ExvConfig::default());
    assert_default_config_file(dir.path());
}

#[test]
fn startup_invalid_or_unusable_config_bootstraps_default_and_requires_quick_start() {
    let cases = [
        ("missing", None),
        ("blank", Some(" \n\t ")),
        ("array_root", Some("[]")),
        ("bad_json", Some("{not json")),
        ("known_field_wrong_type", Some(r#"{"mtu": "not-a-number"}"#)),
    ];

    for (name, initial) in cases {
        let dir = tempfile::tempdir().expect("tempdir");
        if let Some(initial) = initial {
            std::fs::write(config_path(dir.path()), initial).expect("write initial config");
        }

        let startup = load_for_startup(dir.path()).expect(name);
        assert!(startup.requires_quick_start, "{name}");
        assert_eq!(startup.config, ExvConfig::default(), "{name}");
        assert_default_config_file(dir.path());
    }
}

#[test]
fn startup_valid_json_objects_do_not_require_quick_start() {
    let cases = [
        ("empty_object", "{}"),
        ("partial_known_fields", r#"{"server": "vpn.example.test"}"#),
        ("unknown_fields_only", r#"{"future_option": true}"#),
        ("blank_credentials", r#"{"username": "", "password": ""}"#),
    ];

    for (name, initial) in cases {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(config_path(dir.path()), initial).expect("write initial config");

        let startup = load_for_startup(dir.path()).expect(name);
        assert!(!startup.requires_quick_start, "{name}");
    }
}

#[test]
fn startup_hydrates_known_fields_preserves_unknown_fields_and_submit_removes_them() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        config_path(dir.path()),
        r#"{"server": "vpn.example.test", "future_option": {"enabled": true}}"#,
    )
    .expect("write partial config");

    let startup = load_for_startup(dir.path()).expect("startup");
    assert!(!startup.requires_quick_start);
    assert_eq!(startup.config.server, "vpn.example.test");
    assert_eq!(startup.config.mtu, ExvConfig::default().mtu);

    let hydrated: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(config_path(dir.path())).expect("read hydrated config"),
    )
    .expect("parse hydrated config");
    assert_eq!(hydrated["future_option"]["enabled"], true);
    assert!(hydrated.get("mtu").is_some());

    save_after_user_submission(dir.path(), &startup.config).expect("submit config");
    let submitted: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(config_path(dir.path())).expect("read submitted config"),
    )
    .expect("parse submitted config");
    assert!(submitted.get("future_option").is_none());
    assert_eq!(
        serde_json::from_value::<ExvConfig>(submitted).expect("known config"),
        startup.config
    );
}

#[test]
fn startup_hydration_is_idempotent_and_byte_stable() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        config_path(dir.path()),
        r#"{"username": "student", "future_option": 42}"#,
    )
    .expect("write partial config");

    let first = load_for_startup(dir.path()).expect("first startup");
    assert!(!first.requires_quick_start);
    let after_first = std::fs::read(config_path(dir.path())).expect("read first result");

    let second = load_for_startup(dir.path()).expect("second startup");
    assert!(!second.requires_quick_start);
    let after_second = std::fs::read(config_path(dir.path())).expect("read second result");

    assert_eq!(first.config, second.config);
    assert_eq!(after_first, after_second);
}
