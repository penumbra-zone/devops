use anyhow::Context;
use clap::Parser;
use std::fs::canonicalize;
// use std::io::Write;
use std::fs::create_dir_all;
use std::io::{stderr, IsTerminal as _};
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;
use tempfile::{Builder, TempDir};
use tokio::time::sleep;
use tracing_subscriber::EnvFilter;
use url::Url;

mod cli;
// mod config;
mod pindexer;
mod postgres;

use crate::cli::Cli;
use crate::cli::{ACTION_DUMP, ACTION_IMPORT, ACTION_REINDEX};
use pindexer_db_wrangler::config::default_home;
use pindexer_db_wrangler::{download_file, PenumbraEnvironment};

/// All the options that are network-specific
#[allow(dead_code)]
struct NetworkConfig {
    /// URL for the original (i.e. block 0) genesis json.
    genesis_json_url: Url,
}

/// Initialize tracing for the console.
fn init_tracing() {
    tracing_subscriber::fmt()
        .with_ansi(stderr().is_terminal())
        .with_target(true)
        .with_env_filter(
            EnvFilter::try_from_default_env()
                // Default to "info"-level logging.
                .or_else(|_| EnvFilter::try_new("info"))
                .expect("failed to initialize logging")
                // Without explicitly disabling the `r1cs` target, the ZK proof implementations
                // will spend an enormous amount of CPU and memory building useless tracing output.
                .add_directive(
                    "r1cs=off"
                        .parse()
                        .expect("rics=off is a valid filter directive"),
                ),
        )
        .with_writer(stderr)
        .init();
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();
    let args = Cli::parse();
    args.check_deps()?;

    let penumbra_environment: PenumbraEnvironment = match args.penumbra_environment.clone() {
        Some(e) => PenumbraEnvironment::from_str(&e)?,
        None => args.get_penumbra_environment()?,
    };
    let actions = args.get_intended_actions()?;
    tracing::info!(?actions, "received list of actions");

    // Use a specific directory if requested, otherwise generate a tempdir.
    // Paths must be canonicalized in order to generate UDS paths.
    let d: TempDir;
    let project_dir = match args.working_directory {
        Some(d) => {
            create_dir_all(&d)?;
            canonicalize(d)?
        }
        None => {
            // Nest the temp dir within homedir, as default system tmpdirs are likely too small
            // for importing ~30GB of postgres data.
            let home = default_home();
            create_dir_all(&home)?;
            d = Builder::new().tempdir_in(home)?;
            canonicalize(PathBuf::from(d.path()))?
        }
    };

    // Filepath for saving the dumped database.
    let cometbft_dump_file = project_dir.join("cometbft.dump");

    // The logic here is a bit gnarly: we want to support specifying options up-front via CLI
    // flags, but there's also the legacy interactive menus that should be honored.
    match args.cometbft_src_dump_url {
        Some(dump_url) => {
            pindexer_db_wrangler::download_file(&dump_url, &cometbft_dump_file).await?;
        }
        None => {
            if actions.contains(&ACTION_DUMP.to_string()) {
                tracing::info!("dumping src cometbft database to localhost");
                match args.cometbft_src_database_url {
                    Some(db_url) => {
                        postgres::dump_database(&db_url, &cometbft_dump_file)?;
                    }
                    None => {
                        tracing::warn!(
                            "neither dump url nor db url were given; trying to load from k8s secrets"
                        );
                        let src_db_url = postgres::get_default_src_db_url(&penumbra_environment)?;
                        postgres::dump_database(&src_db_url, &cometbft_dump_file)?;
                    }
                }
                tracing::info!("done dumping");
            }
        }
    }

    let local_src_db_url: String;
    let mut pg_src_dir: PathBuf;

    let local_dst_db_url: String;
    let mut pg_dst_dir: PathBuf;

    if actions.contains(&ACTION_IMPORT.to_string()) {
        tracing::info!("importing src cometbft database to local postgres instance");
        // It's very important to use an absolute path, otherwise the
        // unix socket syntax for postgres doesn't work. Using a tempdir
        // gives us an absolute path easily, which is nice.
        pg_src_dir = project_dir.join("postgres-src-db");
        std::fs::create_dir_all(pg_src_dir.clone())?;
        pg_src_dir = canonicalize(pg_src_dir)?;

        local_src_db_url = format!(
            // Oddly this format isn't working although docs say it should
            // "postgresql://dbname=penumbra_raw?host={}/postgres/sock",
            "postgresql://?dbname=penumbra_raw&host={}/postgres/sock",
            pg_src_dir.as_os_str().to_str().unwrap()
        );
        tracing::debug!("running postgres for src db import...");
        let _pg = tokio::spawn(picturesque::postgres::run(pg_src_dir));

        // tracing::warn!("sleeping to block on pg jawn");
        // tracing::info!(?local_src_db_url, "try connecting manually");
        // let _foo = sleep(Duration::from_secs(300)).await;
        //
        // tiny sleep: TODO we should instead check for the socket to be listening
        tracing::debug!("sleeping a bit to wait for pg to start");
        let _foo = sleep(Duration::from_secs(2)).await;

        tracing::debug!("restoring cometbft dump to local db...");
        postgres::restore_database(&local_src_db_url, &cometbft_dump_file)
            .context("failed to import cometbft db locally")?;
        tracing::info!("cometbft event database imported");

        // Only reindex if we've already imported.
        if actions.contains(&ACTION_REINDEX.to_string()) {
            // dead code
            pg_dst_dir = project_dir.join("postgres-dst-db");
            std::fs::create_dir_all(pg_dst_dir.clone())?;
            pg_dst_dir = canonicalize(pg_dst_dir)?;

            local_dst_db_url = format!(
                // Oddly this format isn't working although docs say it should
                // "postgresql://dbname=penumbra_raw?host={}/postgres/sock",
                "postgresql://?dbname=penumbra_raw&host={}/postgres/sock",
                pg_dst_dir.as_os_str().to_str().unwrap()
            );
            tracing::debug!("running postgres for src db import...");
            let _pg = tokio::spawn(picturesque::postgres::run(pg_dst_dir));

            // tracing::warn!("sleeping to block on pg jawn");
            // tracing::info!(?local_src_db_url, "try connecting manually");
            // let _foo = sleep(Duration::from_secs(300)).await;
            //
            // tiny sleep: TODO we should instead check for the socket to be listening
            tracing::debug!("sleeping a bit to wait for pg to start");
            let _foo = sleep(Duration::from_secs(5)).await;
            pindexer::run_pindexer(local_src_db_url, local_dst_db_url, &penumbra_environment)
                .await?;
        }
    }

    // Backstop on arg-parsing.
    if actions.contains(&ACTION_REINDEX.to_string())
        && !actions.contains(&ACTION_IMPORT.to_string())
    {
        anyhow::bail!("cannot reindex without also importing");
    }

    tracing::info!("all actions complete!");

    Ok(())
}
