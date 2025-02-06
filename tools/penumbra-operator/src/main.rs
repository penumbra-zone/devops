//! Primary entrypoint for the controller runtime.
use penumbra_operator::configure_tracing;

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
