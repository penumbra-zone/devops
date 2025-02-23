use anyhow::Context;
use clap::Parser;
use inquire::{MultiSelect, Select};
use std::fs::canonicalize;
use std::io::Write;
use std::io::{stderr, IsTerminal as _};
use std::path::PathBuf;
use std::process::Command;
use std::str::FromStr;
use tempfile::TempDir;
use tokio_stream::StreamExt;
use tracing_subscriber::EnvFilter;
use url::Url;
use which::which;

// Static declarations for "actions" that the tool can perform.
const ACTION_DUMP: &str = "dump from cloud";
const ACTION_IMPORT: &str = "import locally";
const ACTION_REINDEX: &str = "reindex locally";
const ACTION_RESTORE: &str = "restore to cloud";

/// Which network should be used, "testnet" or "mainnet".
/// Shorthand for using chain-ids.
#[derive(Debug, Default, Clone)]
enum PenumbraEnvironment {
    #[default]
    /// The PL-run public testnet, identified by chain-id `penumbra-testnet-phobos-2`.
    Testnet,
    /// The primary public network, identified by chain-id `penumbra-testnet-phobos-2`.
    Mainnet,
}

// Implement FromStr for your enum
impl FromStr for PenumbraEnvironment {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> anyhow::Result<Self> {
        match s.to_lowercase().as_str() {
            "testnet" => Ok(Self::Testnet),
            "mainnet" => Ok(Self::Mainnet),
            _ => Err(anyhow::anyhow!(format!(
                "Unrecognized Penumbra environment: {}",
                s
            ))),
        }
    }
}

use std::fmt::Display;
impl Display for PenumbraEnvironment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PenumbraEnvironment::Testnet => write!(f, "testnet"),
            PenumbraEnvironment::Mainnet => write!(f, "mainnet"),
        }
    }
}

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    /// Which network to manage, "testnet" or "mainnet"
    #[arg(long, value_parser = PenumbraEnvironment::from_str, default_value_t)]
    penumbra_environment: PenumbraEnvironment,

    /// Database url for the CometBFT events database,
    /// used for dumping and importing locally, so the pindexer
    /// run communicates over local sockets.
    #[arg(long)]
    cometbft_src_database_url: Option<String>,

    /// URL for a database dump of a CometBFT events database,
    /// used importing locally, serving as the src db for the
    /// pindexer run.
    #[arg(long)]
    cometbft_src_dump_url: Option<Url>,

    /// Database url for the remote pindexer database,
    /// to which the local reindex will be restored.
    #[arg(long)]
    pindexer_dst_database_url: Option<String>,

    /// Filepath to write pgdump for cometbft db.
    #[arg(long)]
    cometbft_dump_filepath: Option<PathBuf>,

    /// Filepath to write pgdump for pindexer db.
    #[arg(long)]
    pindexer_dump_filepath: Option<PathBuf>,

    /// Directory for storing local databases and dump files.
    /// By default, a temporary directory will be used, ensuring
    /// all artifacts are cleaned up after the run.
    #[arg(long)]
    working_directory: Option<PathBuf>,

    /// Enable verbose mode
    #[arg(short, long)]
    verbose: bool,
}

/// All the options that are network-specific
#[allow(dead_code)]
struct NetworkConfig {
    /// URL for the original (i.e. block 0) genesis json.
    genesis_json_url: Url,
}

/// Confirm that required programs are available on `PATH`.
fn check_deps() -> anyhow::Result<()> {
    // let wanted_programs = vec!["pindexer", "pg_dump", "psql", "kubectl"];
    let wanted_programs = vec!["pindexer", "pg_dump", "pg_restore", "psql"];
    let mut found_programs = Vec::<&str>::new();
    // let mut result = false;
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

/// Prompt interactively to ask which environment
fn get_penumbra_environment() -> anyhow::Result<String> {
    // Order is important: the first option is selected by default.
    let options: Vec<&str> = vec!["testnet", "mainnet"];
    let choice = Select::new("Which environment do you want to manage?", options).prompt()?;
    println!("Got it, considering only '{}' databases", choice);
    Ok(choice.to_owned())
}

/// Prompt interactively to ask which actions to perform.
fn get_intended_actions() -> anyhow::Result<Vec<String>> {
    // Order is important: the first option is selected by default.
    let options: Vec<&str> = vec![ACTION_DUMP, ACTION_IMPORT, ACTION_REINDEX, ACTION_RESTORE];
    let choices = MultiSelect::new("Which actions do you want to perform?", options).prompt()?;
    Ok(choices.into_iter().map(|s| s.to_owned()).collect())
}

/// Restore a PostgreSQL database dump to the target db, as defined by the `database_url`
/// connection string.
#[tracing::instrument]
fn restore_database(database_url: &str, dump_file: &PathBuf) -> anyhow::Result<()> {
    // pg_restore --exit-on-error --clean --if-exists \
    // --role penumbra --jobs "$(nproc)" --no-owner --no-acl \
    // -d "$DB_WRANGLER_LOCAL_SRC_DB_URL" "$DB_WRANGLER_COMETBFT_DUMP_LOCAL_FILEPATH"
    tracing::warn!(?database_url, "beginning restore");
    let status = Command::new("pg_restore")
        .args([
            "--exit-on-error",
            "--clean",
            "--if-exists",
            "--jobs",
            "10",
            "--no-owner",
            "--no-acl",
            "-d",
            database_url,
            dump_file
                .to_str()
                .expect("failed to convert PathBuf to str"),
        ])
        .status()?;
    if status.success() {
        Ok(())
    } else {
        anyhow::bail!("failed to restore database");
    }
}

/// Create a PostgreSQL database dump of the target db, as defined by the `database_url`
/// connection string. Will be formated as a "custom" pg dump, and saved to the local
/// filepath `dest_file`.
fn dump_database(database_url: &str, dest_file: &PathBuf) -> anyhow::Result<()> {
    let status = Command::new("pg_dump")
        .args([
            "-d",
            database_url,
            "-Fc",
            "-f",
            dest_file
                .to_str()
                .expect("failed to convert dump filepath to str"),
        ])
        .status()?;
    if status.success() {
        Ok(())
    } else {
        anyhow::bail!(format!(
            "failed to dump database to file: {}",
            dest_file.display()
        ));
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();
    check_deps()?;
    let args = Cli::parse();

    // let penumbra_environment = get_penumbra_environment()?;
    let actions = get_intended_actions()?;
    tracing::info!(?actions, "received list of actions");

    // Use a specific directory if requested, otherwise generate a tempdir.
    // Paths must be canonicalized in order to generate UDS paths.
    let d: TempDir;
    let project_dir = match args.working_directory {
        Some(d) => canonicalize(d)?,
        None => {
            d = TempDir::new()?;
            canonicalize(PathBuf::from(d.path()))?
        }
    };

    // Filepath for saving the dumped database.
    let cometbft_dump_file = project_dir.join("cometbft.dump");

    // The logic here is a bit gnarly: we want to support specifying options up-front via CLI
    // flags, but there's also the legacy interactive menus that should be honored.
    match args.cometbft_src_dump_url {
        Some(dump_url) => {
            download_db_dump(&dump_url, &cometbft_dump_file).await?;
        }
        None => {
            if actions.contains(&ACTION_DUMP.to_string()) {
                tracing::info!("dumping src cometbft database to localhost");
                match args.cometbft_src_database_url {
                    Some(db_url) => {
                        dump_database(&db_url, &cometbft_dump_file)?;
                    }
                    None => {
                        tracing::warn!(
                            "neither dump url nor db url were given; trying to load from k8s secrets"
                        );
                        let src_db_url = get_default_src_db_url(&args.penumbra_environment)?;
                        dump_database(&src_db_url, &cometbft_dump_file)?;
                    }
                }
                tracing::info!("done dumping");
            }
        }
    }

    if actions.contains(&ACTION_IMPORT.to_string()) {
        tracing::info!("importing src cometbft database to local postgres instance");
        // It's very important to use an absolute path, otherwise the
        // unix socket syntax for postgres doesn't work. Using a tempdir
        // gives us an absolute path easily, which is nice.
        let mut pg_dir_for_src_db = project_dir.join("postgres-src-db");
        std::fs::create_dir_all(pg_dir_for_src_db.clone())?;
        pg_dir_for_src_db = canonicalize(pg_dir_for_src_db)?;

        let local_src_db_url = format!(
            // Oddly this format isn't working although docs say it should
            // "postgresql://dbname=penumbra_raw?host={}/postgres/sock",
            "postgresql://?dbname=penumbra_raw&host={}/postgres/sock",
            pg_dir_for_src_db.as_os_str().to_str().unwrap()
        );
        tracing::debug!("running postgres for src db import...");
        let _pg = tokio::spawn(picturesque::postgres::run(pg_dir_for_src_db));

        tracing::warn!("sleeping to block on pg jawn");
        tracing::info!(?local_src_db_url, "try connecting manually");
        let _foo = tokio::time::sleep(std::time::Duration::from_secs(300)).await;

        tracing::debug!("restoring cometbft dump to local db...");
        restore_database(&local_src_db_url, &cometbft_dump_file)
            .context("failed to import cometbft db locally")?;
        tracing::info!("cometbft event database imported");
    }

    if actions.contains(&ACTION_REINDEX.to_string()) {
        if !actions.contains(&ACTION_IMPORT.to_string()) {
            anyhow::bail!("cannot reindex without also importing");
        }
        tracing::info!("reindexing via pindexer");
        unimplemented!("still need to hook up pindexer");
    }

    Ok(())
}

/// Fetch a database dump from a remote URL and store locally.
pub async fn download_db_dump(dbdump_url: &Url, dest_file: &PathBuf) -> anyhow::Result<()> {
    // Check whether URL points to a local file
    if dbdump_url.scheme() == "file" {
        tracing::error!(%dbdump_url, "file URLs not supported");
        anyhow::bail!("failed to download file URL");
    } else {
        // Download.
        // TODO: Perhaps we should do some sanity-checking on the pardir existing.
        let response = reqwest::get(dbdump_url.clone()).await?;
        tracing::info!(%dbdump_url, dest_file = ?dest_file, "downloading dbdump");
        let mut download_opts = std::fs::OpenOptions::new();
        download_opts.create(true).truncate(true).write(true);
        let mut dbdump = download_opts
            .open(dest_file)
            .context("failed to get a handle on the dbdump dest file")?;

        // Download via stream, in case file is too large to shove into RAM.
        let mut stream = response.bytes_stream();
        while let Some(chunk_result) = stream.next().await {
            let chunk = chunk_result?;
            dbdump.write_all(&chunk)?;
        }
        dbdump.flush()?;
        tracing::info!("download complete: {}", dest_file.display());
    }

    Ok(())
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

/// Look up a database URL for an online CometBFT event database.
/// Requires k8s access to look up the information from a k8s Secret.
/// This is a fallback, intended for use in PL infra tooling contexts.
/// Non-admin users can simply provide a `--src-db-url` on the CLI.
fn get_default_src_db_url(penumbra_environment: &PenumbraEnvironment) -> anyhow::Result<String> {
    use k8s_openapi::api::core::v1::Secret;
    let secret_name = format!("postgres-creds-{}-pindexer", penumbra_environment);
    let secret_field_name = "pindexer_src_database_url";

    tracing::debug!("looking up secret from k8s");
    let output = std::process::Command::new("kubectl")
        .args([
            "-n",
            penumbra_environment.to_string().as_str(),
            "get",
            "secret",
            &secret_name,
            "-o",
            "json",
        ])
        .output()?;

    if !output.status.success() {
        tracing::error!(?secret_name, "failed to find k8s secret");
        anyhow::bail!("secret lookup via k8s failed");
    }

    tracing::debug!("converting json output to k8s Secret");
    let secret: Secret = serde_json::from_slice(&output.stdout)?;
    let secret_data = secret.data.expect("k8s Secret must contain 'data' field");
    let database_url_bytes = secret_data
        .get(secret_field_name)
        .expect("secret lacks expected field name");
    let database_url = String::from_utf8_lossy(&database_url_bytes.0).to_string();

    Ok(database_url)
}
