#![cfg(feature = "network-integration")]
//! Basic integration testing for penumbra-operator.
//!
//! Will perform destructive operations against an actual k8s cluster!
//! Right now, it just destroys and creates a devnet. That's about it:
//! but the scaffolding is in place to test anything in the required resources.
//!
// # Things to test:
// #
// # - [ ] services have endpoints
// # - [ ] dns resolves in a multi-validator setup
// # - [ ]

use penumbra_operator::configure_tracing;
use std::process::Command;
mod common;
use common::cleanup_resource;
use common::TEST_NAMESPACE;

use crate::common::TestProcesses;
use test_context::test_context;

/// Remove all test resources. This is a VERY DESTRUCTIVE action.
async fn cleanup_networks() -> anyhow::Result<()> {
    configure_tracing()?;

    // Delete all PenumbraNetworks.
    cleanup_resource("penumbranetwork", vec![])?;

    // Delete all PVCs.
    // Filter for genesis-validator PVCs, i.e. ones that were created via the PenumbraNetwork CRD.
    let pvc_args: Vec<String> = vec![
        "-l".to_owned(),
        "app.kubernetes.io/component=genesis-validator".to_owned(),
    ];
    cleanup_resource("pvc", pvc_args)?;

    Ok(())
}

#[test_context(TestProcesses)]
#[tokio::test]
async fn cleanup_networks_passes(_ctx: &TestProcesses) -> anyhow::Result<()> {
    cleanup_networks().await?;
    Ok(())
}

#[test_context(TestProcesses)]
#[tokio::test]
/// Apply an example CRD for network.
async fn create_network_via_crd(_ctx: &TestProcesses) -> anyhow::Result<()> {
    let status = Command::new("kubectl")
        .args([
            "-n",
            TEST_NAMESPACE,
            "apply",
            "-f",
            "files/crd-network-example.yaml",
        ])
        .status()
        .map_err(|e| anyhow::anyhow!("Failed to execute kubectl apply {}", e))?;

    if !status.success() {
        anyhow::bail!(
            "Command failed with exit code: {}",
            status.code().unwrap_or(-1)
        );
    }
    Ok(())
}
