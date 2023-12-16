//! Kubernetes controller logic for managing Penumbra CRDs.

use apiexts::CustomResourceDefinition;
use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1 as apiexts;
use kube::Config;
use kube::ResourceExt;
// use futures::StreamExt;
use futures_util::StreamExt;
use kube::runtime::finalizer::{finalizer, Event as Finalizer};

use kube::runtime::{
    controller::{Action, Controller},
    watcher,
};
use kube::{
    api::{Api, Patch, PatchParams},
    runtime::wait::{await_condition, conditions},
    Client, CustomResourceExt,
};

use crate::crd::PenumbraNode;
use std::sync::Arc;
use std::time::Duration;

use crate::error::{Error, Result};
use crate::DEFAULT_NAMESPACE;
use crate::OPERATOR_GROUP;
use crate::OPERATOR_NAME;

pub const PENUMBRA_FINALIZER: &str = "penumbranodes.penumbra.zone";

/// Ensures that the relevant CRDs for the operator are recognized
/// by the cluster. Idempotent, so it's OK to run this command multiple times.
///
/// Most convenient when running the operator interactively, via an admin's workstation,
/// as it needs sufficient RBAC privileges to create CRDs, which it may not have
/// while running inside the cluster.
#[tracing::instrument(skip_all)]
pub async fn install_crds(client: &Client) -> anyhow::Result<()> {
    // Lifted from the kube.rs examples directory
    let params = PatchParams::apply(OPERATOR_NAME).force();
    let crds: Api<CustomResourceDefinition> = Api::all(client.clone());
    let crd_fqdn = format!("penumbranodes.{}", OPERATOR_GROUP);
    tracing::info!("creating crd: {}", crd_fqdn,);
    crds.patch(&crd_fqdn, &params, &Patch::Apply(PenumbraNode::crd()))
        .await?;

    // Block until ready.
    tracing::info!("waiting for the api-server to accept the CRD");
    let establish = await_condition(crds, &crd_fqdn, conditions::is_crd_established());
    let _ = tokio::time::timeout(std::time::Duration::from_secs(10), establish).await?;
    tracing::info!("done!");
    Ok(())
}

/// Wrapper struct for k8s [Client].
///
/// Allows the cloneable [Client] to be wrapped in a higher-level Arc
struct Context {
    client: Client,
}

/// Ensures that CRDs are adequately represented in terms of cluster resources.
#[tracing::instrument(skip_all)]
async fn reconcile(node: Arc<PenumbraNode>, ctx: Arc<Context>) -> Result<Action> {
    tracing::info!("reconciling PenumbraNode '{}'", node.name_any());
    let client = ctx.client.clone();
    let nodes: Api<PenumbraNode> = Api::namespaced(client.clone(), DEFAULT_NAMESPACE);
    finalizer(&nodes, PENUMBRA_FINALIZER, node, |event| async {
        match event {
            Finalizer::Apply(node) => node.reconcile(&client.clone()).await,
            Finalizer::Cleanup(node) => node.cleanup(&client.clone()).await,
        }
    })
    .await
    .map_err(|e| Error::FinalizerError(Box::new(e)))
}

/// Custom error handler for reconciliation loop. Logs error, requeues object.
#[tracing::instrument(skip_all)]
fn error_policy(n: Arc<PenumbraNode>, e: &Error, _ctx: Arc<Context>) -> Action {
    tracing::warn!(
        ?n,
        ?e,
        "encountered error while reconciling node, requeuing"
    );
    Action::requeue(Duration::from_secs(60))
}

/// Main controller loop.
#[tracing::instrument(skip_all)]
pub async fn run() -> anyhow::Result<()> {
    tracing::debug!("entering run loop for controller");
    // Ensure the operator only manages resources in a specific namespace.
    let mut k8s_config = Config::infer().await?;
    k8s_config.default_namespace = DEFAULT_NAMESPACE.to_string();
    let client = Client::try_from(k8s_config)?;

    // Useful for running via cli interactively.
    install_crds(&client.clone()).await?;

    // Set up watchers.
    let nodes = Api::<PenumbraNode>::all(client.clone());
    let context = Arc::new(Context { client });
    Controller::new(nodes, watcher::Config::default())
        .run(reconcile, error_policy, context)
        .for_each(|res| async move {
            match res {
                Ok(o) => tracing::info!("reconciled {:?}", o),
                Err(e) => tracing::error!("reconcile failed: {:?}", e),
            }
        })
        .await;
    Ok(())
}
