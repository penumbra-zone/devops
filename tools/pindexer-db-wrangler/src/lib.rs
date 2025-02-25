use anyhow::Context;
use std::str::FromStr;
use url::Url;

use std::io::Write;
use std::path::PathBuf;
// use std::process::Command;
use tokio_stream::StreamExt;

pub mod postgres;

/// Which network should be used, "testnet" or "mainnet".
/// Shorthand for using chain-ids.
#[derive(Debug, Default, Clone)]
pub enum PenumbraEnvironment {
    #[default]
    /// The PL-run public testnet, identified by chain-id `penumbra-testnet-phobos-2`.
    Testnet,
    /// The primary public network, identified by chain-id `penumbra-testnet-phobos-2`.
    Mainnet,
}

impl PenumbraEnvironment {
    pub fn genesis_url(&self) -> Url {
        match self {
            PenumbraEnvironment::Testnet => {
                Url::parse("https://artifacts.plinfra.net/penumbra-testnet-phobos-2/genesis-0.json")
                    .unwrap()
            }
            PenumbraEnvironment::Mainnet => {
                Url::parse("https://artifacts.plinfra.net/penumbra-1/genesis-0.json").unwrap()
            }
        }
    }
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

/// Generic function for downloading files
pub async fn download_file(download_url: &Url, dest_path: &PathBuf) -> anyhow::Result<()> {
    // Check whether URL points to a local file
    if download_url.scheme() == "file" {
        tracing::error!(%download_url, "file URLs not supported");
        anyhow::bail!("failed to download file URL");
    } else {
        // Download.
        // TODO: Perhaps we should do some sanity-checking on the pardir existing.
        let response = reqwest::get(download_url.clone()).await?;
        tracing::debug!(%download_url, dest_path = ?dest_path, "downloading");
        let mut download_opts = std::fs::OpenOptions::new();
        download_opts.create(true).truncate(true).write(true);
        let mut dbdump = download_opts
            .open(dest_path)
            .context("failed to get a handle on the dbdump dest file")?;

        // Download via stream, in case file is too large to shove into RAM.
        let mut stream = response.bytes_stream();
        while let Some(chunk_result) = stream.next().await {
            let chunk = chunk_result?;
            dbdump.write_all(&chunk)?;
        }
        dbdump.flush()?;
        tracing::debug!("download complete: {}", dest_path.display());
    }

    Ok(())
}
