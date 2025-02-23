use clap::Parser;
use inquire::{MultiSelect, Select};
use std::io::Write;
use std::io::{stderr, IsTerminal as _};
use std::path::PathBuf;
use tokio_stream::StreamExt;
use tracing_subscriber::EnvFilter;
use url::Url;
use which::which;

// Static declarations for "actions" that the tool can perform.
const ACTION_DUMP: &str = "dump from cloud";
const ACTION_IMPORT: &str = "import locally";
const ACTION_REINDEX: &str = "reindex locally";
const ACTION_RESTORE: &str = "restore to cloud";

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    /// Which network to manage, "testnet" or "mainnet"
    #[arg(long)]
    penumbra_environment: String,

    /// Filepath to write pgdump for cometbft db.
    #[arg(long)]
    cometbft_dump_filepath: PathBuf,

    /// Filepath to write pgdump for pindexer db.
    #[arg(long)]
    pindexer_dump_filepath: PathBuf,

    /// Enable verbose mode
    #[arg(short, long)]
    verbose: bool,

    /// Optional configuration file
    #[arg(short, long)]
    config: Option<String>,
}

/// All the options that are network-specific
#[allow(dead_code)]
struct NetworkConfig {
    /// URL for the original (i.e. block 0) genesis json.
    genesis_json_url: Url,
}

/// Confirm that required programs are available on `PATH`.
fn check_deps() -> anyhow::Result<()> {
    let wanted_programs = vec!["pindexer", "pg_dump", "psql", "kubectl"];
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

/// Create a PostgreSQL database dump of the target db, as defined by the `database_url`
/// connection string. Will be formated as a "custom" pg dump.
fn dump_database(database_url: &str, dest_file: &PathBuf) -> anyhow::Result<()> {
    let status = std::process::Command::new("pg_dump")
        .args([
            "-d",
            database_url,
            "-Fc",
            "-f",
            dest_file
                .to_str()
                .expect("failed to convert PathBuf to str"),
        ])
        .status()?;
    if status.success() {
        Ok(())
    } else {
        anyhow::bail!("failed to dump src database");
    }
}

fn main() -> anyhow::Result<()> {
    init_tracing();
    check_deps()?;
    let penumbra_environment = get_penumbra_environment()?;
    let actions = get_intended_actions()?;
    tracing::info!(?actions, "received list of actions");

    let src_db_url = get_default_src_db_url(&penumbra_environment)?;

    if actions.contains(&ACTION_DUMP.to_string()) {
        tracing::info!("dumping database");
        let dump = PathBuf::from("cometbft.dump");
        dump_database(&src_db_url, &dump)?;
        tracing::info!("done dumping");
    }

    if actions.contains(&ACTION_IMPORT.to_string()) {
        tracing::info!("importing database");
        let _dump = PathBuf::from("cometbft.dump");
        unimplemented!("database import is not implemented yet");
    }

    Ok(())
}

/// Fetch a database dump from a remote URL and store locally.
pub async fn download_db_dump(dbdump_url: Url, dest_dir: PathBuf) -> anyhow::Result<PathBuf> {
    let dbdump_filepath: std::path::PathBuf;
    // Check whether URL points to a local file
    if dbdump_url.scheme() == "file" {
        tracing::info!(%dbdump_url, "extracting compressed node state from local file");
        dbdump_filepath = dbdump_url.to_file_path().map_err(|e| {
            tracing::error!(?e);
            anyhow::anyhow!("failed to convert archive url to filepath")
        })?;
    } else {
        // Download.
        // Here we inspect HEAD so we can infer filename.
        tracing::info!(%dbdump_url, "downloading dbdump");
        let response = reqwest::get(dbdump_url).await?;
        let fname = response
            .url()
            .path_segments()
            .and_then(|segments| segments.last())
            .and_then(|name| if name.is_empty() { None } else { Some(name) })
            .unwrap_or("dbdump.dump");

        dbdump_filepath = dest_dir.join(fname);
        let mut download_opts = std::fs::OpenOptions::new();
        download_opts.create_new(true).write(true);
        let mut dbdump_file = download_opts.open(&dbdump_filepath)?;

        // Download via stream, in case file is too large to shove into RAM.
        let mut stream = response.bytes_stream();
        while let Some(chunk_result) = stream.next().await {
            let chunk = chunk_result?;
            dbdump_file.write_all(&chunk)?;
        }
        dbdump_file.flush()?;
        tracing::info!("download complete: {}", dbdump_filepath.display());
    }

    Ok(dbdump_filepath)
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
fn get_default_src_db_url(penumbra_environment: &str) -> anyhow::Result<String> {
    use k8s_openapi::api::core::v1::Secret;
    let secret_name = format!("postgres-creds-{}-pindexer", penumbra_environment);
    let secret_field_name = "pindexer_src_database_url";

    tracing::debug!("looking up secret from k8s");
    let output = std::process::Command::new("kubectl")
        .args([
            "-n",
            penumbra_environment,
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
