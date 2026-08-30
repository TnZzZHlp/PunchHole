use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use PunchHole::{Config, load_json_config, parse_ipv4_endpoint, try_parse_cli};
use clap::error::ErrorKind;

fn args(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_string()).collect()
}

static CONFIG_TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

fn load_test_config(contents: &str) -> Result<Config, String> {
    let path = std::env::temp_dir().join(format!(
        "PunchHole-config-test-{}-{}.json",
        std::process::id(),
        CONFIG_TEST_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&path, contents).expect("test configuration should be written");
    let result = load_json_config(&path);
    let _ = std::fs::remove_file(path);
    result
}

#[test]
fn parses_json_with_multiple_scripts() {
    let config = load_test_config(
        r#"{
            "http": "198.51.100.10:80",
            "stun": "198.51.100.20:3478",
            "mappings": [
                { "script": "/opt/app1.sh" },
                { "script": "/opt/app2.sh" }
            ]
        }"#,
    )
    .expect("JSON configuration should parse");

    assert_eq!(config.http, "198.51.100.10:80".parse().unwrap());
    assert_eq!(config.stun, "198.51.100.20:3478".parse().unwrap());
    assert_eq!(
        config.mappings,
        vec![
            PunchHole::Mapping {
                script: PathBuf::from("/opt/app1.sh")
            },
            PunchHole::Mapping {
                script: PathBuf::from("/opt/app2.sh")
            }
        ]
    );
}

#[test]
fn resolves_json_hostnames_to_ipv4() {
    let config = load_test_config(
        r#"{
            "http": "localhost:80",
            "stun": "localhost:3478",
            "mappings": [{ "script": "/opt/app.sh" }]
        }"#,
    )
    .expect("hostnames should resolve");

    assert!(config.http.ip().is_loopback());
    assert_eq!(config.http.port(), 80);
    assert!(config.stun.ip().is_loopback());
    assert_eq!(config.stun.port(), 3478);
}

#[test]
fn rejects_removed_mapping_fields() {
    let error = load_test_config(
        r#"{
            "http": "198.51.100.10:80",
            "stun": "198.51.100.20:3478",
            "mappings": [{
                "local_port": 10001,
                "target": "192.168.2.10:0",
                "script": "/opt/app.sh"
            }]
        }"#,
    )
    .expect_err("removed mapping fields must be rejected");

    assert!(error.contains("unknown field"));
}

#[test]
fn rejects_relative_script_path() {
    let error = load_test_config(
        r#"{
            "http": "198.51.100.10:80",
            "stun": "198.51.100.20:3478",
            "mappings": [{ "script": "app.sh" }]
        }"#,
    )
    .expect_err("relative script must be rejected");

    assert!(error.contains("script path must be absolute"));
}

#[test]
fn rejects_empty_mappings() {
    let error = load_test_config(
        r#"{
            "http": "198.51.100.10:80",
            "stun": "198.51.100.20:3478",
            "mappings": []
        }"#,
    )
    .expect_err("at least one mapping must be required");

    assert!(error.contains("at least one mapping is required"));
}

#[test]
fn rejects_malformed_or_unknown_json() {
    let malformed = load_test_config(
        r#"{
            "http": "198.51.100.10:80",
            "stun": "198.51.100.20:3478",
            "mappings": [
        }"#,
    )
    .expect_err("malformed JSON must be rejected");
    assert!(malformed.contains("invalid JSON configuration"));

    let unknown = load_test_config(
        r#"{
            "http": "198.51.100.10:80",
            "stun": "198.51.100.20:3478",
            "mappings": [{ "script": "/opt/app.sh" }],
            "unexpected": true
        }"#,
    )
    .expect_err("unknown JSON fields must be rejected");
    assert!(unknown.contains("unknown field"));
}

#[test]
fn accepts_only_json_cli_configuration() {
    assert!(try_parse_cli(&args(&["--config", "config.json"])).is_ok());

    let missing = try_parse_cli(&[]).expect_err("--config must be required");
    assert_eq!(missing.kind(), ErrorKind::MissingRequiredArgument);

    let inline = try_parse_cli(&args(&["--http", "198.51.100.10:80"]))
        .expect_err("inline configuration must be removed");
    assert_eq!(inline.kind(), ErrorKind::UnknownArgument);
}

#[test]
fn rejects_invalid_or_non_ipv4_endpoint() {
    assert!(parse_ipv4_endpoint("not-an-endpoint", "endpoint").is_err());
    assert!(parse_ipv4_endpoint("[::1]:22", "endpoint").is_err());
    assert!(parse_ipv4_endpoint("127.0.0.1:0", "endpoint").is_err());
}
