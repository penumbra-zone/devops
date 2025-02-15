#![cfg(feature = "network-integration")]

// use penumbra_operator::configure_tracing;
use std::process::{Child, Command};
// use std::thread;
// use std::time::Duration;
use test_context::AsyncTestContext;

/// The k8s namespace in which all test resources will be created.
/// Must be unique to the test suite, because all resources in this
/// namespace will be destroyed!
pub const TEST_NAMESPACE: &str = "penumbra-operator-testing";

/// Manager for `cargo run` PID, allows creation of TestContext.
pub struct TestProcesses {
    operator: Child,
}

impl AsyncTestContext for TestProcesses {
    async fn setup() -> TestProcesses {
        eprintln!("Setting up test processes...");

        // Start minikube
        eprintln!("Starting minikube...");
        let minikube_output = Command::new("minikube")
            .arg("start")
            .output()
            .expect("Failed to start minikube");

        if !minikube_output.status.success() {
            panic!(
                "Failed to start minikube: {}",
                String::from_utf8_lossy(&minikube_output.stderr)
            );
        }
        eprintln!("Minikube started successfully");

        // Start operator
        eprintln!("Starting operator...");
        let operator = Command::new("cargo")
            .arg("run")
            .spawn()
            .expect("Failed to start operator");

        eprintln!("Started operator process with PID: {}", operator.id());

        TestProcesses { operator }
    }
}

impl Drop for TestProcesses {
    fn drop(&mut self) {
        eprintln!("Cleaning up test processes...");

        // Kill operator
        if let Err(e) = self.operator.kill() {
            eprintln!("Error killing operator: {}", e);
        }
        if let Ok(status) = self.operator.wait() {
            eprintln!("Operator exited with status: {}", status);
        }

        // Stop minikube
        if let Err(e) = Command::new("minikube").arg("stop").output() {
            eprintln!("Error stopping minikube: {}", e);
        }

        eprintln!("Cleanup complete");
    }
}

/// Reusable function to handle deleting all instances of a resource.
///
/// Optionally can take additional args, to support filtering, e.g.
/// by label.
pub fn cleanup_resource(resource_type: &str, args: Vec<String>) -> anyhow::Result<()> {
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
    if resources.is_empty() {
        return Ok(());
    }
    let status = Command::new("kubectl")
        .args(["-n", TEST_NAMESPACE, "delete", "--wait"])
        .args(&resources)
        .status()?;
    assert!(status.success(), "failed to delete {}", resource_type);

    Ok(())
}
