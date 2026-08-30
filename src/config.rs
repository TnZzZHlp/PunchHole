use std::fs;
use std::net::{SocketAddr, SocketAddrV4, ToSocketAddrs};
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::cli::Cli;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct JsonConfig {
    http: String,
    stun: String,
    mappings: Vec<JsonMapping>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct JsonMapping {
    script: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mapping {
    pub script: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub http: SocketAddrV4,
    pub stun: SocketAddrV4,
    pub mappings: Vec<Mapping>,
}

pub fn from_cli(cli: &Cli) -> Result<Config, String> {
    load_json_config(&cli.config)
}

/// Loads and strictly validates a JSON configuration file.
pub fn load_json_config(path: &Path) -> Result<Config, String> {
    let contents = fs::read_to_string(path).map_err(|error| {
        format!(
            "could not read JSON configuration {}: {error}",
            path.display()
        )
    })?;
    parse_json_config(&contents).map_err(|error| {
        format!(
            "could not load JSON configuration {}: {error}",
            path.display()
        )
    })
}

fn parse_json_config(contents: &str) -> Result<Config, String> {
    let json: JsonConfig = serde_json::from_str(contents)
        .map_err(|error| format!("invalid JSON configuration: {error}"))?;
    let http = parse_ipv4_endpoint(&json.http, "HTTP endpoint")?;
    let stun = parse_ipv4_endpoint(&json.stun, "STUN endpoint")?;
    let mappings = json
        .mappings
        .into_iter()
        .enumerate()
        .map(|(index, mapping)| {
            mapping_from_script(&mapping.script)
                .map_err(|error| format!("mapping {}: {error}", index + 1))
        })
        .collect::<Result<Vec<_>, _>>()?;

    if mappings.is_empty() {
        return Err("at least one mapping is required".to_string());
    }

    Ok(Config {
        http,
        stun,
        mappings,
    })
}

fn mapping_from_script(script: &str) -> Result<Mapping, String> {
    if !Path::new(script).is_absolute() {
        return Err(format!("script path must be absolute: {script}"));
    }
    Ok(Mapping {
        script: PathBuf::from(script),
    })
}

fn resolve_ipv4_endpoint(value: &str, label: &str) -> Result<SocketAddrV4, String> {
    value
        .to_socket_addrs()
        .map_err(|error| format!("invalid {label}: {value}: {error}"))?
        .find_map(|address| match address {
            SocketAddr::V4(address) => Some(address),
            SocketAddr::V6(_) => None,
        })
        .ok_or_else(|| format!("{label} must resolve to an IPv4 address: {value}"))
}

/// Resolves an IPv4 address or hostname endpoint and rejects port zero.
pub fn parse_ipv4_endpoint(value: &str, label: &str) -> Result<SocketAddrV4, String> {
    let address = resolve_ipv4_endpoint(value, label)?;
    if address.port() == 0 {
        return Err(format!("{label} port must be between 1 and 65535"));
    }
    Ok(address)
}
