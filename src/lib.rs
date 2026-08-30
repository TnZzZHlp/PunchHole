#![allow(non_snake_case)]

use std::sync::Once;

use tracing_subscriber::{EnvFilter, filter::LevelFilter};

mod cli;
mod config;
mod http;
mod mapping;
mod net;
mod notify;
mod stun;

use clap::{CommandFactory, Parser};

pub use config::{Config, Mapping, load_json_config, parse_ipv4_endpoint};
#[doc(hidden)]
pub use http::{connect_http, validate_http_response};
#[doc(hidden)]
pub use net::new_bound_socket;
#[doc(hidden)]
pub use notify::script_arguments;
#[doc(hidden)]
pub use stun::{
    STUN_BINDING_SUCCESS, STUN_MAGIC_COOKIE, XOR_MAPPED_ADDRESS, parse_xor_mapped_address,
};

fn init_tracing() {
    static INIT: Once = Once::new();

    INIT.call_once(|| {
        let filter = EnvFilter::builder()
            .with_default_directive(LevelFilter::INFO.into())
            .try_from_env()
            .unwrap_or_else(|_| EnvFilter::new("info"));
        let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
    });
}

/// Parses CLI configuration and runs all configured mapping workers.
pub fn run() {
    init_tracing();
    let cli = cli::Cli::parse();
    let config = match config::from_cli(&cli) {
        Ok(config) => config,
        Err(error) => cli::Cli::command()
            .error(clap::error::ErrorKind::ValueValidation, error)
            .exit(),
    };

    mapping::run(config);
}

/// Parses CLI arguments without starting mapping workers.
#[doc(hidden)]
pub fn try_parse_cli(args: &[String]) -> Result<(), clap::Error> {
    let mut argv = Vec::with_capacity(args.len() + 1);
    argv.push("PunchHole".to_string());
    argv.extend_from_slice(args);
    cli::Cli::try_parse_from(argv).map(|_| ())
}
