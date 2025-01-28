//! Primary entrypoint for the controller runtime.
use std::io::IsTerminal as _;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{prelude::*, EnvFilter};

/// Initialize the [tracing] library via [tracing_subscriber].
fn configure_tracing() -> anyhow::Result<()> {
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

#[tokio::main]
/// Entrypoint. So far, just prints some debugging info, validating
/// a connection to the k8s API, and dumping the StatefulSet config.
async fn main() -> anyhow::Result<()> {
    // Set up logging.
    configure_tracing()?;

    // Run the controller reconciliation loop.
    tracing::info!("starting penumbra operator");
    penumbra_operator::controller::run().await?;
    tracing::info!("shutting down");
    Ok(())
}
