use std::path::PathBuf;

use clap::{ArgGroup, Parser};

#[derive(Debug, Parser)]
#[command(
    name = "PunchHole",
    version,
    about = "Maintains direct IPv4 TCP mappings.",
    group(
        ArgGroup::new("configuration-source")
            .required(true)
            .multiple(true)
            .args(["config", "http", "stun", "mapping"])
    )
)]
pub struct Cli {
    /// Load configuration from a JSON file.
    #[arg(
        short = 'c',
        long,
        value_name = "PATH",
        conflicts_with_all = ["http", "stun", "mapping"]
    )]
    pub(crate) config: Option<PathBuf>,

    /// HTTP service endpoint.
    #[arg(long, value_name = "HOST:PORT", conflicts_with = "config")]
    pub(crate) http: Option<String>,

    /// STUN service endpoint.
    #[arg(long, value_name = "HOST:PORT", conflicts_with = "config")]
    pub(crate) stun: Option<String>,

    /// Mapping specification; repeat for multiple mappings.
    #[arg(long, value_name = "SPEC", conflicts_with = "config")]
    pub(crate) mapping: Vec<String>,
}
