use std::path::PathBuf;

use clap::Parser;

#[derive(Debug, Parser)]
#[command(
    name = "PunchHole",
    version,
    about = "Maintains direct IPv4 TCP mappings."
)]
pub struct Cli {
    /// Load configuration from a JSON file.
    #[arg(short = 'c', long, value_name = "PATH")]
    pub(crate) config: PathBuf,
}
