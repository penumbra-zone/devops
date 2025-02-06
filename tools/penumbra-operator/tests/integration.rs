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
//
use penumbra_operator::configure_tracing;
use std::process::Command;

/// The k8s namespace in which all test resources will be created.
/// Must be unique to the test suite, because all resources in this
/// namespace will be destroyed!
//
// TODO: figure out how to make this unique
// const TEST_NAMESPACE: &str = "penumbra-operator-testing";
const TEST_NAMESPACE: &str = "penumbra";

/// Remove all test resources. This is a VERY DESTRUCTIVE action.
async fn cleanup() -> anyhow::Result<()> {
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

/// Reusable function to handle deleting all instances of a resource.
/// Optionally can take additional args, to support filtering, e.g.
/// by label.
fn cleanup_resource(resource_type: &str, args: Vec<String>) -> anyhow::Result<()> {
    // Get list of all requested resource types.
    let output = Command::new("kubectl")
        .args(["-n", TEST_NAMESPACE, "get", resource_type, "-o", "name"])
        .args(args)
        .output()?;
    assert!(
        output.status.success(),
        "failed to get existing {}s: {}",
        resource_type,
        String::from_utf8_lossy(&output.stderr)
    );
    let s = String::from_utf8_lossy(&output.stdout);
    let resources: Vec<&str> = s
        .trim_end_matches('\n')
        .split('\n')
        .filter(|x| !x.is_empty())
        .collect();
    tracing::debug!("found {} {}", resources.len(), resource_type);

    // Remove any PenumbraNetworks..
    let status = Command::new("kubectl")
        .args(["-n", TEST_NAMESPACE, "delete", "--wait"])
        .args(&resources)
        .status()?;
    assert!(status.success(), "failed to delete {}", resource_type);

    Ok(())
}

#[tokio::test]
async fn cleanup_passes() -> anyhow::Result<()> {
    cleanup().await?;
    Ok(())
}

#[tokio::test]
/// Apply an example CRD for network.
async fn create_network_via_crd() -> anyhow::Result<()> {
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
