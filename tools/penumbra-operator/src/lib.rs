//! The Penumbra operator, for running Penumbra nodes on Kubernetes.

use std::io::IsTerminal as _;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{prelude::*, EnvFilter};

/// The name of the operator, for reuse in resource names and labels.
pub const OPERATOR_NAME: &str = "penumbra-operator";
/// The FQDN for namespacing the CRDs within the Kubernetes API.
pub const OPERATOR_GROUP: &str = "penumbra.zone";

/// The container image repository for the Penumbra images.
pub const PENUMBRA_IMAGE_REPO: &str = "ghcr.io/penumbra-zone/penumbra";
/// The container tag used for Penumbra images.
pub const PENUMBRA_IMAGE_TAG: &str = "v1.0.2";

/// The container image repository for CometBFT images.
pub const COMETBFT_IMAGE_REPO: &str = "docker.io/cometbft/cometbft";
/// The container tag used for CometBFT images.
pub const COMETBFT_IMAGE_TAG: &str = "v0.37.15";

/// The container image repository for Postgres images.
pub const POSTGRES_IMAGE_REPO: &str = "docker.io/library/postgres";
/// The container tag used for Postgres images.
pub const POSTGRES_IMAGE_TAG: &str = "latest";

pub const PENUMBRA_FINALIZER: &str = "latest";

pub mod controller;
pub mod crd;
pub mod error;

/// Initialize the [tracing] library via [tracing_subscriber].
pub fn configure_tracing() -> anyhow::Result<()> {
    // Lifted from pd main.rs.
    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_ansi(std::io::stdout().is_terminal())
        .with_target(true);
    // The `EnvFilter` layer is used to filter events based on `RUST_LOG`.
    let filter_layer = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new("info,penumbra_operator=debug"))?;
    let registry = tracing_subscriber::registry()
        .with(filter_layer)
        .with(fmt_layer);
    registry.init();
    Ok(())
}
