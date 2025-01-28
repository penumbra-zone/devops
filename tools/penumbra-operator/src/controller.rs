//! Kubernetes controller logic for managing Penumbra CRDs.

use futures_util::StreamExt;
use kube::runtime::finalizer::{finalizer, Event as Finalizer};
use kube::Config;

use kube::runtime::{
    controller::{Action, Controller},
    watcher,
};
use kube::{api::Api, Client};

use crate::crd::{PenumbraNetwork, PenumbraNode};
use std::sync::Arc;
use std::time::Duration;

use crate::error::{Error, Result};
use crate::DEFAULT_NAMESPACE;

pub const PENUMBRA_NODE_FINALIZER: &str = "penumbranodes.penumbra.zone";
pub const PENUMBRA_NETWORK_FINALIZER: &str = "penumbranetworks.penumbra.zone";

/// How many seconds to wait after an error to retry the reconcile action.
const REQUEUE_DELAY_SECONDS: u64 = 60;
// const REQUEUE_DELAY_SECONDS: u64 = 5;

/// Ensures that the relevant CRDs for the operator are recognized
/// by the cluster. Idempotent, so it's OK to run this command multiple times.
///
/// Most convenient when running the operator interactively, via an admin's workstation,
/// as it needs sufficient RBAC privileges to create CRDs, which it may not have
/// while running inside the cluster.
#[tracing::instrument(skip_all)]
pub async fn install_crds(client: &Client) -> anyhow::Result<()> {
    PenumbraNode::install(client).await?;
    PenumbraNetwork::install(client).await?;
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
async fn reconcile_penumbra_node(node: Arc<PenumbraNode>, ctx: Arc<Context>) -> Result<Action> {
    tracing::debug!("reconciling {node}");
    let client = ctx.client.clone();
    let nodes: Api<PenumbraNode> = Api::namespaced(client.clone(), DEFAULT_NAMESPACE);
    finalizer(&nodes, PENUMBRA_NODE_FINALIZER, node, |event| async {
        match event {
            Finalizer::Apply(node) => node.reconcile(&client.clone()).await,
            Finalizer::Cleanup(node) => node.cleanup(&client.clone()).await,
        }
    })
    .await
    .map_err(|e| Error::FinalizerError(Box::new(e)))
}

/// Ensures that CRDs are adequately represented in terms of cluster resources.
#[tracing::instrument(skip_all)]
async fn reconcile_penumbra_network(
    network: Arc<PenumbraNetwork>,
    ctx: Arc<Context>,
) -> Result<Action> {
    tracing::debug!("reconciling {network}'");
    let client = ctx.client.clone();
    let networks: Api<PenumbraNetwork> = Api::namespaced(client.clone(), DEFAULT_NAMESPACE);
    finalizer(
        &networks,
        PENUMBRA_NETWORK_FINALIZER,
        network,
        |event| async {
            match event {
                Finalizer::Apply(n) => n.reconcile(&client.clone()).await,
                Finalizer::Cleanup(n) => n.cleanup(&client.clone()).await,
            }
        },
    )
    .await
    .map_err(|e| Error::FinalizerError(Box::new(e)))
}

/// Custom error handler for reconciliation loop. Logs error, requeues object.
fn error_policy_penumbra_node(n: Arc<PenumbraNode>, e: &Error, _ctx: Arc<Context>) -> Action {
    tracing::error!(?n, ?e, "encountered error while reconciling, requeuing");
    // tracing::error!("encountered error while reconciling {}, requeuing", n);
    Action::requeue(Duration::from_secs(REQUEUE_DELAY_SECONDS))
}

/// Custom error handler for reconciliation loop. Logs error, requeues object.
fn error_policy_penumbra_network(n: Arc<PenumbraNetwork>, e: &Error, _ctx: Arc<Context>) -> Action {
    tracing::error!(?n, ?e, "encountered error while reconciling, requeuing");
    // tracing::error!("encountered error while reconciling {}, requeuing", n);
    Action::requeue(Duration::from_secs(REQUEUE_DELAY_SECONDS))
}

/// Main controller loop. Manages two separate [Controller]s,
/// one for [PenumbraNode] and another for [PenumbraNetwork].
#[tracing::instrument(skip_all)]
pub async fn run() -> anyhow::Result<()> {
    tracing::debug!("entering run loop for controller");
    // Ensure the operator only manages resources in a specific namespace.
    let mut k8s_config = Config::infer().await?;
    k8s_config.default_namespace = DEFAULT_NAMESPACE.to_string();
    let client = Client::try_from(k8s_config)?;

    // Useful for running via cli interactively.
    install_crds(&client.clone()).await?;

    // Create APIs for both resources
    let penumbra_nodes = Api::<PenumbraNode>::all(client.clone());
    let penumbra_networks = Api::<PenumbraNetwork>::all(client.clone());

    let context = Arc::new(Context {
        client: client.clone(),
    });

    // Create controllers for both resources
    let penumbra_node_controller = Controller::new(penumbra_nodes, watcher::Config::default())
        .run(
            reconcile_penumbra_node,
            error_policy_penumbra_node,
            context.clone(),
        )
        .filter_map(|x| async move { Result::ok(x) })
        .for_each(|_| futures::future::ready(()));

    let penumbra_network_controller =
        Controller::new(penumbra_networks, watcher::Config::default())
            .run(
                reconcile_penumbra_network,
                error_policy_penumbra_network,
                context.clone(),
            )
            .filter_map(|x| async move { Result::ok(x) })
            .for_each(|_| futures::future::ready(()));

    // Run both controllers concurrently
    futures::join!(penumbra_node_controller, penumbra_network_controller);

    // Set up watchers for PenumbraNode.
    let nodes = Api::<PenumbraNode>::all(client.clone());
    Controller::new(nodes, watcher::Config::default())
        .run(reconcile_penumbra_node, error_policy_penumbra_node, context)
        .for_each(|res| async move {
            match res {
                Ok(o) => tracing::info!("reconciled {:?}", o),
                Err(e) => tracing::error!("reconcile failed: {:?}", e),
            }
        })
        .await;
    Ok(())
}
