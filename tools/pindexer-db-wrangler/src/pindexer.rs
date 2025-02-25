use crate::postgres::{get_latest_cometbft_block_height, get_latest_pindexer_block_height};
use crate::{download_file, PenumbraEnvironment};
use std::process::Command;
use std::time::Duration;
use tokio::time::sleep;

/// Run `pindexer`, as found on PATH, reading from one database and writing to another.
/// We use full Strings for the db URLs, so that we can spawn background tasks to do
/// progress reporting.
pub async fn run_pindexer(
    src_db_url: String,
    dest_db_url: String,
    penumbra_environment: &PenumbraEnvironment,
) -> anyhow::Result<()> {
    // let genesis_file = tempfile::tempfile()?;
    let genesis_url = penumbra_environment.genesis_url();
    let genesis_file = tempfile::Builder::new()
        .prefix("genesis-0")
        .suffix(".json")
        .tempfile()?;
    let g = genesis_file.path().to_path_buf();
    download_file(&genesis_url, &g).await?;

    let dest_db_url_2 = dest_db_url.clone();
    let target_height = get_latest_cometbft_block_height(&src_db_url).await?;
    let progress_handle = tokio::spawn(async move {
        loop {
            // Sleep first, otherwise db tables may not exist yet.
            sleep(Duration::from_secs(180)).await;
            match get_latest_pindexer_block_height(&dest_db_url_2).await {
                Ok(height) => tracing::info!(
                    "current progress: indexed {}/{} blocks",
                    height,
                    target_height
                ),
                Err(e) => tracing::warn!(error = %e, "failed to get current height"),
            }
        }
    });

    // Use either `pindexer-mainnet` or `pindexer-testnet` from nix env.
    // Temporary during LQT support push.
    let pindexer_bin = format!("pindexer-{}", penumbra_environment);
    tracing::info!("running pindexer to height {}", target_height);
    let status = Command::new(pindexer_bin)
        .args(vec![
            "-g",
            g.as_os_str()
                .to_str()
                .expect("failed to convert genesis filepath to str"),
            "-s",
            &src_db_url,
            "-d",
            &dest_db_url,
            "--exit-on-catchup",
        ])
        .status()?;
    if !status.success() {
        anyhow::bail!("failed during pindexer run");
    }

    // Explicitly drop the handle on the backgrounded reporting task
    drop(progress_handle);
    Ok(())
}
