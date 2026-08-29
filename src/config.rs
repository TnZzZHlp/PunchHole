use std::collections::HashSet;
use std::fs;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, ToSocketAddrs};
use std::path::{Path, PathBuf};

use clap::Parser;
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
    #[serde(alias = "local")]
    local_port: u16,
    target: String,
    script: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetPort {
    Fixed(u16),
    Public,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    pub address: Ipv4Addr,
    pub port: TargetPort,
}

impl Target {
    pub const fn resolve(self, public: SocketAddrV4) -> SocketAddrV4 {
        let port = match self.port {
            TargetPort::Fixed(port) => port,
            TargetPort::Public => public.port(),
        };
        SocketAddrV4::new(self.address, port)
    }

    pub const fn uses_public_port(self) -> bool {
        matches!(self.port, TargetPort::Public)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mapping {
    pub local_port: u16,
    pub target: Target,
    pub script: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub http: SocketAddrV4,
    pub stun: SocketAddrV4,
    pub mappings: Vec<Mapping>,
}

/// Parses the preserved inline command-line form into runtime configuration.
pub fn parse_config(args: &[String]) -> Result<Config, String> {
    let mut argv = Vec::with_capacity(args.len() + 1);
    argv.push("PunchHole".to_string());
    argv.extend_from_slice(args);
    let cli = Cli::try_parse_from(argv).map_err(|error| error.to_string())?;
    from_cli(&cli)
}

pub fn from_cli(cli: &Cli) -> Result<Config, String> {
    if let Some(path) = &cli.config {
        return load_json_config(path);
    }

    let http = cli
        .http
        .as_deref()
        .ok_or_else(|| "inline configuration requires --http".to_string())?;
    let stun = cli
        .stun
        .as_deref()
        .ok_or_else(|| "inline configuration requires --stun".to_string())?;
    let mappings = cli
        .mapping
        .iter()
        .map(|spec| parse_mapping(spec))
        .collect::<Result<Vec<_>, _>>()?;
    let http = parse_ipv4_endpoint(http, "HTTP endpoint")?;
    let stun = parse_ipv4_endpoint(stun, "STUN endpoint")?;
    build_config(http, stun, mappings)
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
            mapping_from_parts(mapping.local_port, &mapping.target, &mapping.script)
                .map_err(|error| format!("mapping {}: {error}", index + 1))
        })
        .collect::<Result<Vec<_>, _>>()?;
    build_config(http, stun, mappings)
}

fn build_config(
    http: SocketAddrV4,
    stun: SocketAddrV4,
    mappings: Vec<Mapping>,
) -> Result<Config, String> {
    if mappings.is_empty() {
        return Err("at least one mapping is required".to_string());
    }

    let mut local_ports = HashSet::with_capacity(mappings.len());
    for mapping in &mappings {
        if !local_ports.insert(mapping.local_port) {
            return Err(format!(
                "duplicate local port in mappings: {}",
                mapping.local_port
            ));
        }
    }

    Ok(Config {
        http,
        stun,
        mappings,
    })
}

fn parse_mapping(spec: &str) -> Result<Mapping, String> {
    let mut local_port = None;
    let mut target = None;
    let mut script = None;

    for field in spec.split(',') {
        let (name, value) = field
            .split_once('=')
            .ok_or_else(|| format!("invalid mapping field {field:?}; expected name=value"))?;
        let name = name.trim();
        let value = value.trim();
        if value.is_empty() {
            return Err(format!("mapping field {name:?} is empty"));
        }

        match name {
            "local" => {
                if local_port.is_some() {
                    return Err("mapping contains local more than once".to_string());
                }
                local_port = Some(
                    value
                        .parse::<u16>()
                        .map_err(|_| format!("invalid local port: {value}"))?,
                );
            }
            "target" => {
                if target.is_some() {
                    return Err("mapping contains target more than once".to_string());
                }
                target = Some(value.to_string());
            }
            "script" => {
                if script.is_some() {
                    return Err("mapping contains script more than once".to_string());
                }
                script = Some(value.to_string());
            }
            _ => return Err(format!("unknown mapping field: {name}")),
        }
    }

    let local_port = local_port.ok_or_else(|| "mapping is missing local".to_string())?;
    let target = target.ok_or_else(|| "mapping is missing target".to_string())?;
    let script = script.ok_or_else(|| "mapping is missing script".to_string())?;
    mapping_from_parts(local_port, &target, &script)
}

fn mapping_from_parts(local_port: u16, target: &str, script: &str) -> Result<Mapping, String> {
    if local_port == 0 {
        return Err("local port must be between 1 and 65535".to_string());
    }
    let target = parse_target_endpoint(target)?;
    if !Path::new(script).is_absolute() {
        return Err(format!("script path must be absolute: {script}"));
    }
    let script = PathBuf::from(script);
    Ok(Mapping {
        local_port,
        target,
        script,
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

/// Resolves an IPv4 target endpoint; port zero selects the public mapped port.
pub fn parse_target_endpoint(value: &str) -> Result<Target, String> {
    let address = resolve_ipv4_endpoint(value, "target endpoint")?;
    Ok(Target {
        address: *address.ip(),
        port: if address.port() == 0 {
            TargetPort::Public
        } else {
            TargetPort::Fixed(address.port())
        },
    })
}
