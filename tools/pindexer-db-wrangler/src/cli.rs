//! Logic for parsing CLI options, and prompting interactively for input.
use clap::{ColorChoice, Parser};
use inquire::{MultiSelect, Select};
use std::path::PathBuf;
use std::str::FromStr;
use url::Url;
use which::which;

use crate::PenumbraEnvironment;

// Static declarations for "actions" that the tool can perform.
pub const ACTION_DUMP: &str = "dump from cloud";
pub const ACTION_IMPORT: &str = "import locally";
pub const ACTION_REINDEX: &str = "reindex locally";
pub const ACTION_RESTORE: &str = "restore to cloud";

#[derive(Parser)]
#[command(author, version, about, color = ColorChoice::Always)]
/// pindexer-db-wrangler is an admin utility to facilitate managing `pindexer`
/// event databases for the Penumbra ecosystem.
///
/// It's intended to be a one-stop shop for common operations pertaining to lifecycle
/// management of a Penumbra event database. In practice, this usually means
/// "a new version of pindexer was released, and now I want to regenerate my pindexer
/// database to take advantage of the latest schema." This tool can help with that.
///
/// By default, when invoked without any CLI args, the tool will prompt the user
/// for interactive input about which environments should be managed.
pub(crate) struct Cli {
    /// Which network to manage, "testnet" or "mainnet"
    // #[arg(long, value_parser = PenumbraEnvironment::from_str, default_value_t)]
    #[arg(long)]
    pub penumbra_environment: Option<String>,

    /// Database url for the CometBFT events database,
    /// used for dumping and importing locally, so the pindexer
    /// run communicates over local sockets.
    #[arg(long)]
    pub cometbft_src_database_url: Option<String>,

    /// URL for a database dump of a CometBFT events database,
    /// used importing locally, serving as the src db for the
    /// pindexer run.
    #[arg(long)]
    pub cometbft_src_dump_url: Option<Url>,

    /// Database url for the remote pindexer database,
    /// to which the local reindex will be restored.
    #[arg(long)]
    pub pindexer_dst_database_url: Option<String>,

    /// Filepath to write pgdump for cometbft db.
    #[arg(long)]
    pub cometbft_dump_filepath: Option<PathBuf>,

    /// Filepath to write pgdump for pindexer db.
    #[arg(long)]
    pub pindexer_dump_filepath: Option<PathBuf>,

    /// Directory for storing local databases and dump files.
    /// By default, a temporary directory will be used, ensuring
    /// all artifacts are cleaned up after the run.
    #[arg(long)]
    pub working_directory: Option<PathBuf>,
}

impl Cli {
    /// Confirm that required programs are available on `PATH`.
    pub fn check_deps(&self) -> anyhow::Result<()> {
        // let wanted_programs = vec!["pindexer", "pg_dump", "psql", "kubectl"];
        let wanted_programs = vec![
            "pindexer",
            "pg_dump",
            "pg_restore",
            "psql",
            "pindexer-testnet",
            "pindexer-mainnet",
        ];
        let mut found_programs = Vec::<&str>::new();
        for p in wanted_programs.iter() {
            if which(p).is_ok() {
                found_programs.push(p)
            } else {
                tracing::error!("program not found on PATH: {}", p);
            }
        }

        if found_programs == wanted_programs {
            Ok(())
        } else {
            anyhow::bail!("not all programs found on PATH")
        }
    }
    /// Prompt interactively to ask which actions to perform.
    pub fn get_intended_actions(&self) -> anyhow::Result<Vec<String>> {
        // Order is important: the first option is selected by default.
        let options: Vec<&str> = vec![ACTION_DUMP, ACTION_IMPORT, ACTION_REINDEX, ACTION_RESTORE];
        let choices =
            MultiSelect::new("Which actions do you want to perform?", options).prompt()?;
        Ok(choices.into_iter().map(|s| s.to_owned()).collect())
    }
    /// Prompt interactively to ask which environment
    pub fn get_penumbra_environment(&self) -> anyhow::Result<PenumbraEnvironment> {
        // Order is important: the first option is selected by default.
        let options: Vec<&str> = vec!["testnet", "mainnet"];
        let choice = Select::new("Which environment do you want to manage?", options).prompt()?;
        println!("Got it, considering only '{}' databases", choice);
        let penumbra_env = PenumbraEnvironment::from_str(choice)?;
        Ok(penumbra_env)
    }
}
