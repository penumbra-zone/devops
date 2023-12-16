//! Kubernetes-specific Custom Resource Definition for representing
//! Penumbra concepts, like [PenumbraNode].

use kube::runtime::controller::Action;
use kube_derive::CustomResource;
use schemars::JsonSchema;

use k8s_openapi::apimachinery::pkg::apis::meta::v1::LabelSelector;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use std::time::Duration;

use serde::{Deserialize, Serialize};

// Declare a StatefulSet for Penumbra node.
use k8s_openapi::api::apps::v1::{StatefulSet, StatefulSetSpec};
use k8s_openapi::api::core::v1::{
    ConfigMap, PodSpec, PodTemplateSpec, Service, ServicePort, ServiceSpec,
};

use rand::Rng;
use rand_core::OsRng;

use kube::{
    api::{Api, DeleteParams, Patch, PatchParams, PostParams},
    Client,
};

use crate::error::Result;
use crate::resources;
use crate::DEFAULT_NAMESPACE;

/// The CometBFT RPC URL for the PenumbraNode whose network we want to join.
const DEFAULT_BOOTSTRAP_URL: &str = "https://rpc.testnet-preview.plinfra.net";

/// K8s CRD specification for a [`PenumbraNode`] resource.
///
/// Constitutes the `spec` field of a [PenumbraNode].
#[derive(CustomResource, Serialize, Deserialize, Debug, PartialEq, Clone, JsonSchema)]
#[kube(
    group = "penumbra.zone",
    version = "v1alpha1",
    kind = "PenumbraNode",
    plural = "penumbranodes",
    derive = "PartialEq",
    namespaced,
    printcolumn = r#"{"name":"NetworkName", "type":"string", "jsonPath":".spec.network_name"}"#
)]
pub struct PenumbraNodeSpec {
    /// Human-readable name for node.
    moniker: String,
    /// Human-readable name for the network, as an arbitrary label,
    /// e.g. "penumbra-preview", rather than a chain-id.
    /// This value is used solely to distinguish the k8s resources,
    /// marking them explicitly as part of a greater whole.
    network_name: Option<String>,
    /// Container image spec, without tag suffix.
    image_repo: Option<String>,
    /// Container image tag to fetch from remote `image_repo`.
    image_tag: Option<String>,
    /// Public RPC endpoint for an already-existing remote node,
    /// to fetch information about the network.
    bootstrap_url: String,
    /// Remote URL to fetch a state archive, for extracting pre-upgrade blocks.
    archive_url: Option<String>,
}

impl Default for PenumbraNodeSpec {
    fn default() -> Self {
        Self {
            moniker: crate::crd::moniker(),
            image_repo: Some(crate::PENUMBRA_IMAGE_REPO.to_owned()),
            image_tag: Some(crate::PENUMBRA_IMAGE_TAG.to_owned()),
            bootstrap_url: String::from(DEFAULT_BOOTSTRAP_URL),
            archive_url: None,
            network_name: None,
        }
    }
}

/// Generate likely-unique moniker in the form of `node-xxxxx`, where `xxxxx`
/// is a random string.
pub fn moniker() -> String {
    format!("node-{}", hex::encode(OsRng.gen::<u32>().to_le_bytes()))
}

impl PenumbraNode {
    /// Creates a likely-unique name for this node's resources.
    pub fn release_name(&self) -> String {
        format!("penumbra-node-{}", self.spec.moniker)
    }

    /// Emit a [StatefulSet] that matches the configured CRD.
    ///
    /// This function aggregates all the subcomponents of a StatefulSet,
    /// e.g. PodSpec, Pod, Container, VolumeClaimTemplate, etc.,
    /// and bottles them up into a single StatefulSet.
    pub fn stateful_set(&self) -> StatefulSet {
        let init_container = resources::pd_init_container(
            self.spec.bootstrap_url.clone(),
            self.spec.archive_url.clone(),
        );
        StatefulSet {
            metadata: ObjectMeta {
                name: Some(self.release_name()),
                ..Default::default()
            },
            spec: Some(StatefulSetSpec {
                replicas: Some(1),
                template: PodTemplateSpec {
                    metadata: Some(ObjectMeta {
                        name: Some(self.release_name()),
                        labels: Some(resources::labels()),
                        ..Default::default()
                    }),
                    spec: Some(PodSpec {
                        init_containers: Some(vec![init_container]),
                        containers: vec![
                            resources::pd_container(),
                            resources::cometbft_container(),
                            // resources::postgres_container(),
                        ],
                        volumes: Some(resources::volumes()),
                        ..Default::default()
                    }),
                },
                volume_claim_templates: Some(resources::volume_claim_templates()),
                selector: LabelSelector {
                    match_labels: Some(resources::labels()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    /// Emit a [Service] for the configured CRD.
    pub fn service(&self) -> Service {
        let mut ports: Vec<ServicePort> = Vec::new();

        let pd_ports: Vec<ServicePort> = resources::pd_container()
            .ports
            .expect("failed to find ports for pd container")
            .into_iter()
            .map(|p| ServicePort {
                name: p.name,
                port: p.container_port,
                ..Default::default()
            })
            .collect();
        let cmt_ports: Vec<ServicePort> = resources::cometbft_container()
            .ports
            .expect("failed to find ports for cometbft container")
            .into_iter()
            .map(|p| ServicePort {
                name: p.name,
                port: p.container_port,
                ..Default::default()
            })
            .collect();
        ports.extend(pd_ports);
        ports.extend(cmt_ports);

        Service {
            metadata: ObjectMeta {
                name: Some(self.release_name()),
                ..Default::default()
            },
            spec: Some(ServiceSpec {
                // Setting `None` will result in ClusterIP being automatically set.
                // Instead, we'll set the value to the string "None",
                // which m akes it a headless service, suitable for StatefulSets.
                cluster_ip: Some("None".to_string()),
                ports: Some(ports),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    /// Ensure that the cluster resources representing the CRD are removed.
    #[tracing::instrument(skip_all)]
    pub async fn cleanup(&self, client: &Client) -> Result<Action> {
        tracing::warn!("cleanup functionality only partially implmented");

        tracing::info!("deleting Statefulset");
        let sts_api: Api<StatefulSet> = Api::namespaced(client.clone(), DEFAULT_NAMESPACE);
        match sts_api.get(self.release_name().as_str()).await {
            Ok(_sts) => {
                let delete_params = DeleteParams::default();
                sts_api
                    .delete(self.release_name().as_str(), &delete_params)
                    .await?;
            }
            Err(_e) => {
                // Log a warning because this shouldn't happen.
                tracing::warn!("statefulset not found, skipping deletion");
            }
        }

        let svc_api: Api<Service> = Api::namespaced(client.clone(), DEFAULT_NAMESPACE);
        match svc_api.get(self.release_name().as_str()).await {
            Ok(_sts) => {
                let delete_params = DeleteParams::default();
                svc_api
                    .delete(self.release_name().as_str(), &delete_params)
                    .await?;
            }
            Err(_e) => {
                // Log a warning because this shouldn't happen.
                tracing::warn!("service not found, skipping deletion");
            }
        }

        Ok(Action::await_change())
    }

    /// Ensure that the CRD is adequately expressed in cluster resources.
    pub async fn reconcile(&self, client: &Client) -> Result<Action> {
        // We need a ConfigMap in order for the initContainer to run.
        let cm = resources::pd_init_script_configmap();
        let cm_api: Api<ConfigMap> = Api::namespaced(client.clone(), DEFAULT_NAMESPACE);
        // In lieu of a `get-or-create` method in the kube API, we'll match on a get() call,
        // and create if not found.
        match cm_api.get("pd-init").await {
            Ok(_) => {
                let patch = Patch::Merge(&cm);
                let params = PatchParams::default();
                cm_api.patch("pd-init", &params, &patch).await?;
            }
            Err(_e) => {
                tracing::info!("creating pd-init ConfigMap");
                cm_api.create(&PostParams::default(), &cm).await?;
            }
        }

        // Generate a StatefulSet for the node.
        let ss = self.stateful_set();
        let ss_api: Api<StatefulSet> = Api::namespaced(client.clone(), DEFAULT_NAMESPACE);
        match ss_api.get(&self.release_name()).await {
            Ok(_ss_old) => {
                tracing::debug!("found existing statefulset, trying to patch it...");
                let patch = Patch::Merge(&ss);
                let params = PatchParams::default();
                let _ss_new = ss_api
                    .patch(self.release_name().as_str(), &params, &patch)
                    .await?;
            }
            Err(_) => {
                tracing::info!("no prior sts found, CREATING one");
                ss_api.create(&PostParams::default(), &ss).await?;
            }
        }

        // Generate a Service for the node.
        let svc = self.service();
        let svc_api: Api<Service> = Api::namespaced(client.clone(), DEFAULT_NAMESPACE);
        match svc_api.get(&self.release_name()).await {
            Ok(_svc_old) => {
                tracing::debug!("found existing service, trying to patch it...");
                let patch = Patch::Merge(&svc);
                let params = PatchParams::default();
                let _svc_new = svc_api
                    .patch(self.release_name().as_str(), &params, &patch)
                    .await?;
            }
            Err(_) => {
                tracing::info!("creating service");
                svc_api.create(&PostParams::default(), &svc).await?;
            }
        }
        // If no events were received, check back every 5 minutes
        Ok(Action::requeue(Duration::from_secs(5 * 60)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_specs() {
        let fullnode = PenumbraNode::new(
            "foo",
            PenumbraNodeSpec {
                moniker: "foo".into(),
                ..Default::default()
            },
        );
        let ss = fullnode.stateful_set();
        assert_eq!(ss.metadata.name.unwrap(), fullnode.release_name());
    }
}
