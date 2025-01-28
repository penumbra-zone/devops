//! Kubernetes-specific Custom Resource Definition for representing
//! Penumbra concepts, like [PenumbraNode].

use apiexts::CustomResourceDefinition;
use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1 as apiexts;
use kube::runtime::controller::Action;
// use kube::ResourceExt;
use kube_derive::CustomResource;
use schemars::JsonSchema;

use k8s_openapi::apimachinery::pkg::apis::meta::v1::LabelSelector;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::OwnerReference;
use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;
use std::fmt;
use std::time::Duration;

use serde::{Deserialize, Serialize};

// Declare a StatefulSet for Penumbra node.
use k8s_openapi::api::apps::v1::{StatefulSet, StatefulSetSpec};
use k8s_openapi::api::core::v1::{
    ConfigMap, ConfigMapVolumeSource, Container, ContainerPort, EnvVar, KeyToPath,
    PersistentVolumeClaim, PersistentVolumeClaimSpec, PodSpec, PodTemplateSpec, Probe,
    SecurityContext, Service, ServicePort, ServiceSpec, TCPSocketAction, Volume, VolumeMount,
    VolumeResourceRequirements,
};

use rand::Rng;
use rand_core::OsRng;

use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use kube::{
    api::{Api, DeleteParams, Patch, PatchParams, PostParams, PropagationPolicy},
    runtime::wait::{await_condition, conditions},
    Client, CustomResourceExt,
};
use std::collections::BTreeMap;

use crate::crd::resources;
use crate::crd::resources::PD_NODE_STATE_PVC_NAME;
use crate::error::Result;
use crate::DEFAULT_NAMESPACE;
use crate::PENUMBRA_IMAGE_REPO;
use crate::PENUMBRA_IMAGE_TAG;

/// The CometBFT RPC URL for the PenumbraNode whose network we want to join.
const DEFAULT_BOOTSTRAP_URL: &str = "https://rpc.testnet-preview.plinfra.net";

/// How many seconds to wait after an error to retry the reconcile action.
const REQUEUE_DELAY_SECONDS: u64 = 60;

pub(crate) const PD_INIT_SCRIPT_NAME: &str = "pd-init";
pub(crate) const PD_INIT_VOLUME_MOUNT_NAME: &str = "penumbra-init";
pub(crate) const CMT_SCHEMA_CONFIG_MAP_NAME: &str = "penumbra-cometbft-postgres-schema";

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
    printcolumn = r#"{"name":"NetworkName", "type":"string", "jsonPath":".spec.network_name"}"#,
    printcolumn = r#"{"name":"Age","type":"date","jsonPath":".metadata.creationTimestamp"}"#
)]
pub struct PenumbraNodeSpec {
    /// Human-readable name for node.
    pub moniker: String,
    /// Human-readable name for the network, as an arbitrary label,
    /// e.g. "penumbra-preview", rather than a chain-id.
    /// This value is used solely to distinguish the k8s resources,
    /// marking them explicitly as part of a greater whole.
    pub network_name: Option<String>,
    /// Container image spec, without tag suffix.
    pub image_repo: Option<String>,
    /// Container image tag to fetch from remote `image_repo`.
    pub image_tag: Option<String>,
    /// Public RPC endpoint for an already-existing remote node,
    /// to fetch information about the network. If `None`,
    /// the init script will not touch local state before starting services.
    pub bootstrap_url: Option<String>,
    /// Remote URL to fetch a state archive, for extracting pre-upgrade blocks.
    pub archive_url: Option<String>,
    /// Whether to enable ABCI event indexing via CometBFT to a PostgreSQL database.
    pub enable_indexing: Option<bool>,
    /// Optional override for naming the PVC that stores node info.
    /// Useful for reusing the PenumbraNode logic to create validators
    /// via PenumbraNetwork.
    pub node_state_pvc_name: Option<String>,

    /// Optional override for naming the StatefulSet created by the controller.
    /// Useful for reusing the PenumbraNode logic to create validators
    /// via PenumbraNetwork.
    pub release_name: Option<String>,

    /// Whether to populate Endpoints in the Service object immediately.
    /// Necessary for genesis validators, which won't be Ready until
    /// its services are up, but they need to talk to each other through
    /// services in order to work.
    pub publish_not_ready_addresses: Option<bool>,
}

impl Default for PenumbraNodeSpec {
    fn default() -> Self {
        Self {
            moniker: moniker(),
            image_repo: Some(crate::PENUMBRA_IMAGE_REPO.to_owned()),
            image_tag: Some(crate::PENUMBRA_IMAGE_TAG.to_owned()),
            bootstrap_url: Some(DEFAULT_BOOTSTRAP_URL.to_owned()),
            archive_url: None,
            network_name: None,
            enable_indexing: Some(false),
            node_state_pvc_name: None,
            release_name: None,
            publish_not_ready_addresses: Some(false),
        }
    }
}

impl fmt::Display for PenumbraNode {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "PenumbraNode<{}>", self.spec.moniker)
    }
}

/// Generate likely-unique moniker in the form of `node-xxxxx`, where `xxxxx`
/// is a random string.
pub fn moniker() -> String {
    format!("node-{}", hex::encode(OsRng.gen::<u32>().to_le_bytes()))
}

impl PenumbraNode {
    /// Creates a likely-unique name for this node's resources.
    ///
    /// Can be overridden. Kubernetes will create volumes for the StatefulSet
    /// based on the following pattern:
    ///
    ///    <volumeClaimTemplateName>-<statefulSetName>-<ordinal>
    ///
    /// For full-node resources, this is fine, but for creating validators via PenumbraNetwork,
    /// we'll need to customize it.
    pub fn release_name(&self) -> String {
        match &self.spec.release_name {
            Some(n) => n.to_owned(),
            None => format!("penumbra-node-{}", self.spec.moniker),
        }
    }

    /// Emit an [OwnerResource] suitable for inclusion in a resource's metadata,
    /// so that deletion of the parent CRD will propagate to cleanup of the dependent
    /// resources.
    ///
    /// TODO: figure out how this applies to PVCs and retention.
    pub fn oref(&self) -> OwnerReference {
        OwnerReference {
            // TODO: figure out how to access the kube-derive fields for api_version and kind.
            api_version: "v1alpha1".to_owned(),
            kind: "PenumbraNode".to_owned(),
            name: self
                .metadata
                .name
                .clone()
                .unwrap_or_else(|| panic!("PenumbraNode<{}> lacks a name", &self.release_name())),
            uid: self
                .metadata
                .uid
                .clone()
                .unwrap_or_else(|| panic!("PenumbraNode<{}> lacks a uid", self.release_name())),
            controller: Some(true),
            block_owner_deletion: Some(true),
        }
    }

    /// Generate map of labels, for use in object metadata.
    /// These labels will be appended to the baseline labels common across all Penumnbra CRDs.
    pub fn labels(&self) -> BTreeMap<String, String> {
        let mut l = crate::crd::resources::labels();
        l.extend(BTreeMap::from([
            (
                "app.kubernetes.io/component".to_owned(),
                "penumbra-node".to_owned(),
            ),
            ("app.kubernetes.io/name".to_owned(), self.release_name()),
            ("app.kubernetes.io/part-of".to_owned(), self.release_name()),
        ]));
        l
    }

    /// Emit a [StatefulSet] that matches the configured CRD.
    ///
    /// This function aggregates all the subcomponents of a StatefulSet,
    /// e.g. PodSpec, Pod, Container, VolumeClaimTemplate, etc.,
    /// and bottles them up into a single StatefulSet.
    pub fn stateful_set(&self) -> StatefulSet {
        let init_container = self.pd_init_container();

        let mut containers = vec![self.pd_container(), resources::cometbft_container()];
        // Opt in to ABCI event indexing
        if self.spec.enable_indexing.unwrap_or_default() {
            containers.push(resources::postgres_container());
        }
        StatefulSet {
            metadata: ObjectMeta {
                name: Some(self.release_name()),
                // TODO: Does this clobber annotations automatically generated by `kube`?
                annotations: Some(crate::crd::resources::annotations()),
                // TODO is it ok to clobber orefs here?
                owner_references: Some(vec![self.oref()]),
                ..Default::default()
            },
            spec: Some(StatefulSetSpec {
                replicas: Some(1),
                template: PodTemplateSpec {
                    metadata: Some(ObjectMeta {
                        name: Some(self.release_name()),
                        labels: Some(self.labels()),
                        ..Default::default()
                    }),
                    spec: Some(PodSpec {
                        init_containers: Some(vec![init_container]),
                        containers,
                        volumes: Some(self.volumes()),
                        ..Default::default()
                    }),
                },
                volume_claim_templates: Some(self.pvcs()),
                selector: LabelSelector {
                    match_labels: Some(self.labels()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    /// Expose bash script for pd-init as ConfigMap, so it's volume-mountable.
    pub fn pd_init_script_configmap(&self) -> ConfigMap {
        ConfigMap {
            metadata: ObjectMeta {
                name: Some(format!("{}-{PD_INIT_SCRIPT_NAME}", self.release_name())),
                labels: Some(self.labels()),
                owner_references: Some(vec![self.oref()]),
                ..Default::default()
            },
            data: Some(BTreeMap::from([(
                PD_INIT_SCRIPT_NAME.to_owned(),
                include_str!("../../files/pd-init").to_string(),
            )])),
            ..Default::default()
        }
    }

    /// Emit a [Service] for the configured CRD.
    pub fn service(&self) -> Service {
        let mut ports: Vec<ServicePort> = Vec::new();

        let pd_ports: Vec<ServicePort> = self
            .pd_container()
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

        let mut db_ports: Vec<ServicePort> = Vec::new();
        if self.spec.enable_indexing.unwrap_or_default() {
            db_ports = resources::postgres_container()
                .ports
                .expect("failed to find ports for postgres container")
                .into_iter()
                .map(|p| ServicePort {
                    name: p.name,
                    port: p.container_port,
                    ..Default::default()
                })
                .collect();
        }

        ports.extend(pd_ports);
        ports.extend(cmt_ports);
        ports.extend(db_ports);

        Service {
            metadata: ObjectMeta {
                name: Some(self.release_name()),
                owner_references: Some(vec![self.oref()]),
                ..Default::default()
            },
            spec: Some(ServiceSpec {
                // Setting `None` will result in ClusterIP being automatically set.
                // Instead, we'll set the value to the string "None",
                // which m akes it a headless service, suitable for StatefulSets.
                cluster_ip: Some("None".to_string()),
                ports: Some(ports),
                publish_not_ready_addresses: Some(
                    self.spec.publish_not_ready_addresses.unwrap_or_default(),
                ),
                // Set only one label on the selector, that of the release name,
                // which will be unique across all CRDs.
                selector: Some(BTreeMap::from([(
                    "app.kubernetes.io/name".to_owned(),
                    self.release_name(),
                )])),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    /// Define additional [Volume]s for the pod, beyond the [PersistentVolumeClaim]s.
    pub fn volumes(&self) -> Vec<Volume> {
        let mut vols = vec![Volume {
            name: PD_INIT_VOLUME_MOUNT_NAME.to_owned(),
            config_map: Some(ConfigMapVolumeSource {
                name: self
                    .pd_init_script_configmap()
                    .clone()
                    .metadata
                    .name
                    .expect("pd-init script configmap must have name"),
                items: Some(vec![KeyToPath {
                    key: PD_INIT_SCRIPT_NAME.to_owned(),
                    path: PD_INIT_SCRIPT_NAME.to_owned(),
                    ..Default::default()
                }]),
                default_mode: Some(0o0755),
                ..Default::default()
            }),
            ..Default::default()
        }];
        if self.spec.enable_indexing.unwrap_or_default() {
            vols.push(Volume {
                name: "postgres-schema".to_owned(),
                config_map: Some(ConfigMapVolumeSource {
                    name: CMT_SCHEMA_CONFIG_MAP_NAME.to_owned(),
                    items: Some(vec![KeyToPath {
                        key: "postgres-cometbft-schema.sql".to_owned(),
                        path: "postgres-cometbft-schema.sql".to_owned(),
                        ..Default::default()
                    }]),
                    ..Default::default()
                }),
                ..Default::default()
            });
        }
        vols
    }

    /// Create [PersistentVolumeClaim]s for StatefulSet spec.
    pub fn pvcs(&self) -> Vec<PersistentVolumeClaim> {
        let mut claims = vec![PersistentVolumeClaim {
            // PVC for storing node state, for all applications.
            metadata: ObjectMeta {
                name: Some(crate::crd::resources::PD_NODE_STATE_PVC_NAME.to_owned()),
                labels: Some(self.labels()),
                owner_references: Some(vec![self.oref()]),
                ..Default::default()
            },
            spec: Some(PersistentVolumeClaimSpec {
                access_modes: Some(vec!["ReadWriteOnce".to_owned()]),
                resources: Some(VolumeResourceRequirements {
                    requests: Some(BTreeMap::<String, Quantity>::from([(
                        "storage".to_owned(),
                        Quantity(crate::crd::resources::DEFAULT_PVC_SIZE.to_owned()),
                    )])),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        }];
        if self.spec.enable_indexing.unwrap_or_default() {
            claims.push(PersistentVolumeClaim {
                metadata: ObjectMeta {
                    name: Some(crate::crd::resources::DB_PVC_NAME.to_owned()),
                    labels: Some(self.labels()),
                    owner_references: Some(vec![self.oref()]),
                    ..Default::default()
                },
                spec: Some(PersistentVolumeClaimSpec {
                    access_modes: Some(vec!["ReadWriteOnce".to_owned()]),
                    resources: Some(VolumeResourceRequirements {
                        requests: Some(BTreeMap::<String, Quantity>::from([(
                            "storage".to_owned(),
                            Quantity("1G".to_owned()),
                        )])),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            });
        }
        claims
    }

    /// Generate environment variables for pd container, particularly the initContainer.
    pub fn pd_env(&self) -> Vec<EnvVar> {
        let mut env = Vec::<EnvVar>::new();
        // Bootstrap URL is required if joining another network, but can be disabled,
        // e.g. for genesis validators or prepared state.
        if let Some(u) = &self.spec.bootstrap_url.clone() {
            env.push(EnvVar {
                name: "PENUMBRA_BOOTSTRAP_URL".to_owned(),
                // Must match what's honored by `pd network join --help`
                // TODO: use the pd built-in env vars
                // name: "PENUMBRA_PD_JOIN_URL".to_string(),
                value: Some(u.to_string()),
                value_from: None,
            });
        }

        // Optionally load snapshot data from archive. Only affects init env.
        if let Some(u) = &self.spec.archive_url {
            env.push(EnvVar {
                // We intentionally don't use the built-in pd env var, because the archive
                // format has changed, and largely remains undocumented.
                // name: "PENUMBRA_PD_ARCHIVE_URL".to_string(),
                name: "PENUMBRA_CUSTOM_ARCHIVE_URL".to_owned(),
                value: Some(u.to_string()),
                ..Default::default()
            });
        }
        // Opt in to ABCI event indexing.
        if self.spec.enable_indexing.unwrap_or_default() {
            env.push(EnvVar {
                name: "PENUMBRA_COMETBFT_INDEXER".to_string(),
                value: Some("psql".to_string()),
                ..Default::default()
            });
            // TODO: disable psql indexer if indexing disabled.
            env.push(EnvVar {
                name: "COMETBFT_POSTGRES_CONNECTION_URL".to_string(),
                value: Some(
                    "postgresql://penumbra:penumbra@localhost:5432/penumbra?sslmode=disable"
                        .to_string(),
                ),
                ..Default::default()
            });
        }
        env
    }

    /// Create [Container] spec for `pd`, the Penumbra daemon.
    pub fn pd_container(&self) -> Container {
        let container_name = "pd".to_owned();
        Container {
            name: container_name,
            image: Some(format!("{PENUMBRA_IMAGE_REPO}:{PENUMBRA_IMAGE_TAG}")),
            command: Some(
                // TODO convert these options to env vars,
                // to make them more easily overrideable.
                vec![
                    "pd",
                    "start",
                    "--grpc-bind",
                    "0.0.0.0:8080",
                    "--metrics-bind",
                    "0.0.0.0:9000",
                    "--enable-expensive-rpc",
                ]
                .into_iter()
                .map(|x| x.to_owned())
                .collect(),
            ),
            env: Some(self.pd_env()),
            security_context: Some(SecurityContext {
                run_as_user: Some(1000),
                ..Default::default()
            }),
            ports: Some(vec![
                ContainerPort {
                    name: Some("pd-grpc".to_owned()),
                    container_port: 8080,
                    ..Default::default()
                },
                ContainerPort {
                    name: Some("pd-abci".to_owned()),
                    container_port: 26658,
                    ..Default::default()
                },
                ContainerPort {
                    name: Some("pd-metrics".to_owned()),
                    container_port: 9000,
                    ..Default::default()
                },
            ]),
            readiness_probe: Some(Probe {
                tcp_socket: Some(TCPSocketAction {
                    port: IntOrString::String("pd-grpc".to_owned()),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            volume_mounts: Some(vec![VolumeMount {
                name: PD_NODE_STATE_PVC_NAME.to_owned(),
                mount_path: "/home/penumbra/.penumbra".to_owned(),
                ..Default::default()
            }]),

            ..Default::default()
        }
    }
    /// Create [Container] spec for `pd-init`, for bootstrapping configuration
    /// from a remote node.
    pub fn pd_init_container(&self) -> Container {
        let container_name = "pd-init".to_owned();

        Container {
            name: container_name,
            image: Some(format!("{PENUMBRA_IMAGE_REPO}:{PENUMBRA_IMAGE_TAG}")),
            command: Some(vec![format!("/opt/penumbra/{PD_INIT_SCRIPT_NAME}")]),
            env: Some(self.pd_env()),
            // Run as root during init, so we can shown to penumbra & cometbft users.
            // The application itself will run as a normal user.
            security_context: Some(SecurityContext {
                run_as_user: Some(0),
                run_as_group: Some(0),
                allow_privilege_escalation: Some(true),
                ..Default::default()
            }),
            volume_mounts: Some(vec![
                VolumeMount {
                    name: PD_INIT_VOLUME_MOUNT_NAME.to_owned(),
                    mount_path: "/opt/penumbra".to_owned(),
                    ..Default::default()
                },
                VolumeMount {
                    name: PD_NODE_STATE_PVC_NAME.to_owned(),
                    mount_path: "/home/penumbra/.penumbra/".to_owned(),
                    ..Default::default()
                },
            ]),
            ..Default::default()
        }
    }

    /// Install the CRD to the cluster.
    pub async fn install(client: &Client) -> Result<()> {
        // Lifted from the kube.rs examples directory
        let params = PatchParams::apply(crate::OPERATOR_NAME).force();
        let crds: Api<CustomResourceDefinition> = Api::all(client.clone());

        // Create `PenumbraNode` CRD.
        let node_crd_fqdn = format!("penumbranodes.{}", crate::OPERATOR_GROUP);
        tracing::debug!("creating crd: {}", node_crd_fqdn,);
        crds.patch(&node_crd_fqdn, &params, &Patch::Apply(PenumbraNode::crd()))
            .await?;

        // Block until ready.
        tracing::trace!("waiting for the api-server to accept the CRD");
        let establish = await_condition(
            crds.clone(),
            &node_crd_fqdn,
            conditions::is_crd_established(),
        );
        let _ = tokio::time::timeout(std::time::Duration::from_secs(10), establish)
            .await
            .map_err(|_e| crate::error::Error::InstallFailure);
        Ok(())
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
        let cm = self.pd_init_script_configmap();
        let cm_api: Api<ConfigMap> = Api::namespaced(client.clone(), DEFAULT_NAMESPACE);
        // In lieu of a `get-or-create` method in the kube API, we'll match on a get() call,
        // and create if not found.
        let cm_name = cm
            .clone()
            .metadata
            .name
            .expect("pd init script must have name");
        match cm_api.get(&cm_name).await {
            Ok(_) => {
                let patch = Patch::Merge(&cm);
                let params = PatchParams::default();
                cm_api.patch(&cm_name, &params, &patch).await?;
            }
            Err(_e) => {
                tracing::info!("creating {:?}", &cm);
                cm_api.create(&PostParams::default(), &cm).await?;
            }
        }

        // Generate a StatefulSet for the node.
        let ss = self.stateful_set();
        let ss_api: Api<StatefulSet> = Api::namespaced(client.clone(), DEFAULT_NAMESPACE);
        match ss_api.get(&self.release_name()).await {
            Ok(_ss_old) => {
                tracing::debug!("patching StatefulSet<{}>", self.release_name());
                let patch = Patch::Merge(&ss);
                let params = PatchParams::default();
                match ss_api
                    .patch(self.release_name().as_str(), &params, &patch)
                    .await
                {
                    Ok(_ss_new) => {}
                    // If patching the StatefulSet failed, we likely tried to update a field that
                    // isn't allowed. Let's recreate the StatefulSet, while keeping its pods
                    // running, to correct.
                    Err(e) => {
                        tracing::warn!(
                            "failed to patch StatefulSet<{}>: {}, recreating it",
                            self.release_name(),
                            e,
                        );
                        // Delete StatefulSet but orphan the pods, so the running node is not
                        // affected. Doing so allows us to update
                        let dp = DeleteParams {
                            propagation_policy: Some(PropagationPolicy::Orphan),
                            ..DeleteParams::default()
                        };
                        ss_api.delete(&self.release_name(), &dp).await?;
                        ss_api.create(&PostParams::default(), &ss).await?;
                    }
                }
            }
            Err(_) => {
                tracing::info!("creating StatefulSet<{}>", self.release_name());
                ss_api.create(&PostParams::default(), &ss).await?;
            }
        }

        // Generate a Service for the node.
        let svc = self.service();
        let svc_api: Api<Service> = Api::namespaced(client.clone(), DEFAULT_NAMESPACE);
        match svc_api.get(&self.release_name()).await {
            Ok(_svc_old) => {
                tracing::debug!("patching Service<{}>", self.release_name());
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
        Ok(Action::requeue(Duration::from_secs(REQUEUE_DELAY_SECONDS)))
    }

    /// Expose PostgreSQL default schema for CometBFT, for initializing the event-indexing
    /// database.
    pub fn postgres_schema_configmap(&self) -> ConfigMap {
        ConfigMap {
            metadata: ObjectMeta {
                name: Some(format!(
                    "{}-{CMT_SCHEMA_CONFIG_MAP_NAME}",
                    self.release_name()
                )),
                labels: Some(crate::crd::resources::labels()),
                owner_references: Some(vec![self.oref()]),
                ..Default::default()
            },
            data: Some(BTreeMap::from([(
                "postgres-cometbft-schema.sql".to_owned(),
                include_str!("../../files/postgres-cometbft-schema.sql").to_string(),
            )])),
            ..Default::default()
        }
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
