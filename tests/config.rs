use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use PunchHole::{
    Config, load_json_config, parse_config, parse_ipv4_endpoint, parse_target_endpoint,
    try_parse_cli,
};
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
fn parses_preserved_inline_cli_form() {
    let config = parse_config(&args(&[
        "--http",
        "198.51.100.10:80",
        "--stun",
        "198.51.100.20:3478",
        "--mapping",
        "local=10001,target=192.168.1.20:22,script=/opt/app1.sh",
        "--mapping",
        "local=10002,target=127.0.0.1:8080,script=/opt/app2.sh",
    ]))
    .expect("configuration should parse");

    assert_eq!(config.http, "198.51.100.10:80".parse().unwrap());
    assert_eq!(config.stun, "198.51.100.20:3478".parse().unwrap());
    assert_eq!(config.mappings.len(), 2);
    assert_eq!(config.mappings[0].local_port, 10001);
    assert_eq!(
        config.mappings[0].target,
        parse_target_endpoint("192.168.1.20:22").unwrap()
    );
    assert_eq!(config.mappings[0].script, PathBuf::from("/opt/app1.sh"));
    assert_eq!(config.mappings[1].local_port, 10002);
    assert_eq!(
        config.mappings[1].target,
        parse_target_endpoint("127.0.0.1:8080").unwrap()
    );
    assert_eq!(config.mappings[1].script, PathBuf::from("/opt/app2.sh"));
}

#[test]
fn parses_json_with_multiple_mappings() {
    let config = load_test_config(
        r#"{
                "http": "198.51.100.10:80",
                "stun": "198.51.100.20:3478",
                "mappings": [
                    {
                        "local_port": 10001,
                        "target": "192.168.2.10:0",
                        "script": "/opt/qbittorrent-set-port.sh"
                    },
                    {
                        "local_port": 10002,
                        "target": "127.0.0.1:8080",
                        "script": "/opt/app2.sh"
                    }
                ]
            }"#,
    )
    .expect("JSON configuration should parse");

    assert_eq!(config.http, "198.51.100.10:80".parse().unwrap());
    assert_eq!(config.stun, "198.51.100.20:3478".parse().unwrap());
    assert_eq!(config.mappings.len(), 2);
    assert_eq!(config.mappings[0].local_port, 10001);
    assert!(config.mappings[0].target.uses_public_port());
    assert_eq!(
        config.mappings[0].script,
        PathBuf::from("/opt/qbittorrent-set-port.sh")
    );
    assert_eq!(config.mappings[1].local_port, 10002);
    assert_eq!(
        config.mappings[1].target,
        parse_target_endpoint("127.0.0.1:8080").unwrap()
    );
}

#[test]
fn resolves_json_hostnames_to_ipv4() {
    let config = load_test_config(
        r#"{
                "http": "localhost:80",
                "stun": "localhost:3478",
                "mappings": [
                    {
                        "local_port": 10001,
                        "target": "localhost:0",
                        "script": "/opt/app.sh"
                    }
                ]
            }"#,
    )
    .expect("hostnames should resolve");

    assert!(config.http.ip().is_loopback());
    assert_eq!(config.http.port(), 80);
    assert!(config.stun.ip().is_loopback());
    assert_eq!(config.stun.port(), 3478);
    assert!(config.mappings[0].target.address.is_loopback());
    assert!(config.mappings[0].target.uses_public_port());
}

#[test]
fn parses_json_local_alias() {
    let config = load_test_config(
        r#"{
                "http": "198.51.100.10:80",
                "stun": "198.51.100.20:3478",
                "mappings": [
                    {
                        "local": 10001,
                        "target": "127.0.0.1:8080",
                        "script": "/opt/app.sh"
                    }
                ]
            }"#,
    )
    .expect("JSON local alias should parse");

    assert_eq!(config.mappings[0].local_port, 10001);
}

#[test]
fn rejects_malformed_json() {
    let error = load_test_config(
        r#"{
                "http": "198.51.100.10:80",
                "stun": "198.51.100.20:3478",
                "mappings": [
            }"#,
    )
    .expect_err("malformed JSON must be rejected");

    assert!(error.contains("invalid JSON configuration"));
}

#[test]
fn rejects_unknown_json_fields() {
    let error = load_test_config(
        r#"{
                "http": "198.51.100.10:80",
                "stun": "198.51.100.20:3478",
                "mappings": [],
                "unexpected": true
            }"#,
    )
    .expect_err("unknown JSON fields must be rejected");

    assert!(error.contains("unknown field"));
}

#[test]
fn rejects_duplicate_json_local_ports() {
    let error = load_test_config(
        r#"{
                "http": "198.51.100.10:80",
                "stun": "198.51.100.20:3478",
                "mappings": [
                    {
                        "local_port": 10001,
                        "target": "127.0.0.1:8080",
                        "script": "/opt/app1.sh"
                    },
                    {
                        "local_port": 10001,
                        "target": "127.0.0.1:8081",
                        "script": "/opt/app2.sh"
                    }
                ]
            }"#,
    )
    .expect_err("duplicate JSON local ports must be rejected");

    assert!(error.contains("duplicate local port"));
}

#[test]
fn rejects_config_and_inline_option_conflict() {
    let error = try_parse_cli(&args(&[
        "--config",
        "config.json",
        "--http",
        "198.51.100.10:80",
    ]))
    .expect_err("config and inline options must conflict");

    assert_eq!(error.kind(), ErrorKind::ArgumentConflict);
}

#[test]
fn rejects_missing_configuration_source() {
    let error = try_parse_cli(&[]).expect_err("a configuration source must be required");

    assert_eq!(error.kind(), ErrorKind::MissingRequiredArgument);
    assert!(error.to_string().contains("--config"));
}

#[test]
fn rejects_duplicate_local_ports() {
    let error = parse_config(&args(&[
        "--http",
        "198.51.100.10:80",
        "--stun",
        "198.51.100.20:3478",
        "--mapping",
        "local=10001,target=192.168.1.20:22,script=/opt/app1.sh",
        "--mapping",
        "local=10001,target=127.0.0.1:8080,script=/opt/app2.sh",
    ]))
    .expect_err("duplicate ports must be rejected");

    assert!(error.contains("duplicate local port"));
}

#[test]
fn rejects_invalid_or_non_ipv4_endpoint() {
    assert!(parse_ipv4_endpoint("not-an-endpoint", "target").is_err());
    assert!(parse_ipv4_endpoint("[::1]:22", "target").is_err());
    assert!(parse_ipv4_endpoint("127.0.0.1:0", "target").is_err());
    assert!(parse_target_endpoint("not-an-endpoint").is_err());
    assert!(parse_target_endpoint("[::1]:22").is_err());
}

#[test]
fn resolves_dynamic_target_to_public_port() {
    let target = parse_target_endpoint("192.168.2.10:0").unwrap();
    assert!(target.uses_public_port());
    assert_eq!(
        target.resolve("203.0.113.7:42424".parse().unwrap()),
        "192.168.2.10:42424".parse().unwrap()
    );
}
