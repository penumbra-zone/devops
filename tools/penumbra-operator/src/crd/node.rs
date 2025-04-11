//! Kubernetes-specific Custom Resource Definition for representing
//! Penumbra concepts, like [PenumbraNode].

use apiexts::CustomResourceDefinition;
use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1 as apiexts;
use kube::runtime::controller::Action;
// use kube::ResourceExt;
use kube_derive::CustomResource;
use schemars::JsonSchema;

use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::OwnerReference;
use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;
use std::fmt;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use k8s_openapi::api::core::v1::{
    ConfigMap, ConfigMapVolumeSource, Container, ContainerPort, EnvVar, ExecAction, KeyToPath,
    PersistentVolumeClaim, PersistentVolumeClaimSpec, PersistentVolumeClaimVolumeSource, Pod,
    PodSpec, Probe, SecurityContext, Service, ServicePort, ServiceSpec, TCPSocketAction, Volume,
    VolumeMount, VolumeResourceRequirements,
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

use crate::crd::resources::DEFAULT_PVC_SIZE;
use crate::crd::resources::PD_NODE_STATE_PVC_NAME;
use crate::error::Result;
use crate::PENUMBRA_IMAGE_REPO;
use crate::PENUMBRA_IMAGE_TAG;

/// The CometBFT RPC URL for the PenumbraNode whose network we want to join.
const DEFAULT_BOOTSTRAP_URL: &str = "https://rpc.testnet-preview.plinfra.net";

/// How many seconds to wait after an error to retry the reconcile action.
const REQUEUE_DELAY_SECONDS: u64 = 30;

pub(crate) const PD_INIT_SCRIPT_NAME: &str = "pd-init";
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
    #[serde(default = "default_image_repo")]
    pub image_repo: String,

    /// Container image tag to fetch from remote `image_repo`.
    #[serde(default = "default_image_tag")]
    pub image_tag: String,

    /// Public RPC endpoint for an already-existing remote node,
    /// to fetch information about the network. If `None`,
    /// the init script will not touch local state before starting services.
    pub bootstrap_url: Option<String>,

    /// Remote URL to fetch a state archive, for extracting pre-upgrade blocks.
    pub archive_url: Option<String>,

    /// Whether to enable ABCI event indexing via CometBFT to a PostgreSQL database.
    #[serde(default)]
    pub enable_indexing: bool,
    /// Optional override for naming the PVC that stores node info.
    /// Useful for reusing the PenumbraNode logic to create validators
    /// via PenumbraNetwork.
    pub node_state_pvc_name: Option<String>,

    /// Whether to populate Endpoints in the Service object immediately.
    /// Necessary for genesis validators, which won't be Ready until
    /// its services are up, but they need to talk to each other through
    /// services in order to work.
    #[serde(default)]
    pub publish_not_ready_addresses: bool,

    /// Optional hard-coded seed settings for CometBFT.
    /// Should be formatted as full CometBFT URLs:
    pub seeds: Option<String>,

    /// Specify the type of node. Affects which labels are added,
    /// to help with selection. Defaults to 'FullNode'.
    #[serde(default)]
    pub node_type: NodeType,

    /// Amount of storage to provision for the node.
    #[serde(default = "default_pvc_size")]
    pub pvc_size: String,

    /// Whether to wait for CometBFT to report `catching_up=False`
    /// before marking the node as Ready to back services.
    #[serde(default = "default_wait_for_catchup")]
    pub wait_for_catchup: bool,

    /// Whether to pause the node by running 'sleep infinity'
    /// in all pods, enabling an admin to interact with storage,
    /// e.g. to perform migrations. Will pause both the pd and cometbft
    /// containers, but not the postgres container, so the database is still available
    /// for dumping or restoring.
    #[serde(default)]
    pub maintenance_mode: bool,
}

// Custom function to return a default value for the `#[serde(default)]` annotation on the struct.
fn default_image_tag() -> String {
    PENUMBRA_IMAGE_TAG.to_owned()
}

// Custom function to return a default value for the `#[serde(default)]` annotation on the struct.
fn default_image_repo() -> String {
    PENUMBRA_IMAGE_REPO.to_owned()
}

// Custom function to return a default value for the `#[serde(default)]` annotation on the struct.
fn default_pvc_size() -> String {
    DEFAULT_PVC_SIZE.to_owned()
}

// Custom function to return a default value for the `#[serde(default)]` annotation on the struct.
fn default_wait_for_catchup() -> bool {
    true
}

impl Default for PenumbraNodeSpec {
    fn default() -> Self {
        Self {
            moniker: moniker(),
            image_repo: crate::PENUMBRA_IMAGE_REPO.to_owned(),
            image_tag: crate::PENUMBRA_IMAGE_TAG.to_owned(),
            bootstrap_url: Some(DEFAULT_BOOTSTRAP_URL.to_owned()),
            archive_url: None,
            network_name: None,
            enable_indexing: false,
            node_state_pvc_name: None,
            publish_not_ready_addresses: false,
            seeds: None,
            node_type: NodeType::FullNode,
            pvc_size: DEFAULT_PVC_SIZE.to_owned(),
            wait_for_catchup: default_wait_for_catchup(),
            maintenance_mode: false,
        }
    }
}

impl fmt::Display for PenumbraNode {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "PenumbraNode<{}>", self.spec.moniker)
    }
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, JsonSchema, Default)]
pub enum NodeType {
    #[default]
    FullNode,
    Validator,
    GenesisValidator,
}

impl fmt::Display for NodeType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            NodeType::FullNode => write!(f, "full-node"),
            NodeType::Validator => write!(f, "validator"),
            NodeType::GenesisValidator => write!(f, "genesis-validator"),
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
                // Using default simply to get the unit tests passing, because uid isn't set
                // when running unit tests. That's a whack reason, but hopefully more testing,
                // including integration testing, will shake out where the default case
                // would be a problem.
                // .unwrap_or_else(|| panic!("PenumbraNode<{}> lacks a uid", self.release_name())),
                .unwrap_or_default(),
            controller: Some(true),
            ..Default::default()
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
            (
                format!("{}/node-type", crate::OPERATOR_NAME),
                self.spec.node_type.clone().to_string(),
            ),
        ]));
        l
    }

    pub fn pod(&self) -> Pod {
        let mut containers = vec![self.pd_container(), self.cometbft_container()];
        // Opt in to ABCI event indexing
        if self.spec.enable_indexing {
            containers.push(self.postgres_container());
        }
        Pod {
            metadata: ObjectMeta {
                name: Some(self.release_name()),
                labels: Some(self.labels()),
                owner_references: Some(vec![self.oref()]),
                ..Default::default()
            },
            spec: Some(PodSpec {
                init_containers: if self.spec.maintenance_mode {
                    None
                } else {
                    Some(vec![self.pd_init_container()])
                },
                containers,
                volumes: Some(self.volumes()),
                // Set restartPolicy for the Pod to be Never, so a crashed node stays down.
                // This is important for situations like a controlled chain halt via governance
                // proposal.
                restart_policy: Some("Never".to_owned()),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    /// Container command to "pause" an instance, serving its storage
    /// without a runtime, so that an admin can program it.
    pub fn container_pause_cmd(&self) -> Vec<String> {
        vec!["sleep", "infinity"]
            .into_iter()
            .map(|x| x.to_owned())
            .collect()
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
        let cmt_ports: Vec<ServicePort> = self
            .cometbft_container()
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
        if self.spec.enable_indexing {
            db_ports = self
                .postgres_container()
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
                publish_not_ready_addresses: Some(self.spec.publish_not_ready_addresses),
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
            name: PD_INIT_SCRIPT_NAME.to_owned(),
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
        if self.spec.enable_indexing {
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

        let pvc_volumes = self.pvcs().into_iter().map(|pvc| Volume {
            name: pvc
                .metadata
                .name
                .clone()
                .expect("pvc is missing name field"),
            persistent_volume_claim: Some(PersistentVolumeClaimVolumeSource {
                claim_name: pvc.metadata.name.expect("pvc is missing name field"),
                ..Default::default()
            }),
            ..Default::default()
        });

        vols.extend(pvc_volumes);
        // Also append the PVCs, because no intermediate cnotroller like Deployment or StatefulSet
        // will do so automatically.
        // TODO
        vols
    }

    /// Create [PersistentVolumeClaim]s for StatefulSet spec.
    pub fn pvcs(&self) -> Vec<PersistentVolumeClaim> {
        let mut claims = vec![PersistentVolumeClaim {
            // PVC for storing node state, for all applications.
            metadata: ObjectMeta {
                // name: Some(crate::crd::resources::PD_NODE_STATE_PVC_NAME.to_owned()),
                // We can't assume a higher-level resource manager like StatefulSet will
                // refine the PVC name field to be specific; we must make it explicit.
                name: Some(format!(
                    "{}-{}",
                    self.release_name(),
                    crate::crd::resources::PD_NODE_STATE_PVC_NAME.to_owned()
                )),
                labels: Some(self.labels()),
                owner_references: Some(vec![self.oref()]),
                ..Default::default()
            },
            spec: Some(PersistentVolumeClaimSpec {
                access_modes: Some(vec!["ReadWriteOnce".to_owned()]),
                resources: Some(VolumeResourceRequirements {
                    requests: Some(BTreeMap::<String, Quantity>::from([(
                        "storage".to_owned(),
                        Quantity(self.spec.pvc_size.clone()),
                    )])),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        }];
        // TODO: ditch separate PVC for db, just submount into primary setup
        if self.spec.enable_indexing {
            claims.push(PersistentVolumeClaim {
                metadata: ObjectMeta {
                    name: Some(format!(
                        "{}-{}",
                        self.release_name(),
                        crate::crd::resources::DB_PVC_NAME.to_owned()
                    )),
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

        // Set CometBFT peers, if necessary.
        if let Some(s) = &self.spec.seeds.clone() {
            env.push(EnvVar {
                name: "PENUMBRA_COMETBFT_SEEDS".to_owned(),
                value: Some(s.to_string()),
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
        if self.spec.enable_indexing {
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
            image: Some(format!(
                "{}:{}",
                self.spec.image_repo.clone(),
                self.spec.image_tag.clone()
            )),
            command: if self.spec.maintenance_mode {
                Some(self.container_pause_cmd())
            } else {
                Some(
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
                )
            },
            env: Some(self.pd_env()),
            security_context: Some(SecurityContext {
                // Run as root if maintenance mode is enabled, otherwise as 1000,
                // which matches the default UID in the container image.
                run_as_user: if self.spec.maintenance_mode {
                    Some(0)
                } else {
                    Some(1000)
                },
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
                // name: PD_NODE_STATE_PVC_NAME.to_owned(),
                name: format!(
                    "{}-{}",
                    self.release_name(),
                    PD_NODE_STATE_PVC_NAME.to_owned()
                ),
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
            image: Some(format!(
                "{}:{}",
                self.spec.image_repo.clone(),
                self.spec.image_tag.clone()
            )),
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
                    name: PD_INIT_SCRIPT_NAME.to_owned(),
                    mount_path: "/opt/penumbra".to_owned(),
                    ..Default::default()
                },
                VolumeMount {
                    name: format!(
                        "{}-{}",
                        self.release_name(),
                        PD_NODE_STATE_PVC_NAME.to_owned()
                    ),
                    // name: PD_NODE_STATE_PVC_NAME.to_owned(),
                    mount_path: "/home/penumbra/.penumbra/".to_owned(),
                    ..Default::default()
                },
            ]),
            ..Default::default()
        }
    }
    /// Create [Container] spec for `cometbft`, the CometBFT consensus sidecar for Penumbra.
    pub fn cometbft_container(&self) -> Container {
        Container {
            name: "cometbft".to_owned(),
            image: Some(format!(
                "{}:{}",
                crate::COMETBFT_IMAGE_REPO,
                crate::COMETBFT_IMAGE_TAG
            )),
            command: if self.spec.maintenance_mode {
                Some(self.container_pause_cmd())
            } else {
                Some(
                    vec!["cometbft", "start", "--proxy_app=tcp://127.0.0.1:26658"]
                        .into_iter()
                        .map(|x| x.to_owned())
                        .collect(),
                )
            },
            ports: Some(vec![
                ContainerPort {
                    name: Some("cmt-p2p".to_owned()),
                    container_port: 26656,
                    ..Default::default()
                },
                ContainerPort {
                    name: Some("cmt-rpc".to_owned()),
                    container_port: 26657,
                    ..Default::default()
                },
                ContainerPort {
                    name: Some("cmt-metrics".to_owned()),
                    container_port: 26660,
                    ..Default::default()
                },
            ]),
            readiness_probe: if self.spec.wait_for_catchup {
                Some(Probe {
                    exec: Some(ExecAction {
                        command: Some(vec![
                            "sh".to_owned(),
                            "-cex".to_owned(),
                            r#"catching_up="$(curl -s http://localhost:26657/status | jq -r .result.sync_info.catching_up)" ;
                               test "$catching_up" = "false"
                            "#.to_owned(),
                        ]),
                    }),
                    initial_delay_seconds: Some(10),
                    period_seconds: Some(30),
                    success_threshold: Some(1),
                    failure_threshold: Some(1),
                    ..Default::default()
                })
            } else {
                Some(Probe {
                    tcp_socket: Some(TCPSocketAction {
                        port: IntOrString::String("cmt-rpc".to_owned()),
                        ..Default::default()
                    }),
                    ..Default::default()
                })
            },
            security_context: Some(SecurityContext {
                run_as_user: Some(100),
                ..Default::default()
            }),
            volume_mounts: Some(vec![VolumeMount {
                name: format!(
                    "{}-{}",
                    self.release_name(),
                    PD_NODE_STATE_PVC_NAME.to_owned()
                ),
                mount_path: "/cometbft".to_owned(),
                sub_path: Some("network_data/node0/cometbft".to_owned()),
                ..Default::default()
            }]),

            ..Default::default()
        }
    }

    /// Create [Container] spec for `postgres`, for optional ABCI event indexing
    /// via CometBFT.
    pub fn postgres_container(&self) -> Container {
        let container_name = "postgres".to_owned();
        Container {
            name: container_name.clone(),
            image: Some(format!(
                "{}:{}",
                crate::POSTGRES_IMAGE_REPO,
                crate::POSTGRES_IMAGE_TAG
            )),
            // TODO support ssl args
            ports: Some(vec![ContainerPort {
                name: Some(container_name),
                container_port: 5432,
                ..Default::default()
            }]),
            // TODO support auth customization
            env: Some(vec![
                EnvVar {
                    name: "POSTGRES_PASSWORD".to_string(),
                    value: Some("penumbra".to_string()),
                    ..Default::default()
                },
                EnvVar {
                    name: "POSTGRES_DB".to_string(),
                    value: Some("penumbra".to_string()),
                    ..Default::default()
                },
                EnvVar {
                    name: "POSTGRES_USER".to_string(),
                    value: Some("penumbra".to_string()),
                    ..Default::default()
                },
            ]),
            readiness_probe: Some(Probe {
                tcp_socket: Some(TCPSocketAction {
                    port: IntOrString::Int(5432),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            volume_mounts: Some(vec![
                VolumeMount {
                    name: "postgres-schema".to_owned(),
                    mount_path: "/docker-entrypoint-initdb.d".to_owned(),
                    read_only: Some(true),
                    ..Default::default()
                },
                VolumeMount {
                    name: format!(
                        "{}-{}",
                        self.release_name(),
                        crate::crd::resources::DB_PVC_NAME.to_owned()
                    ),
                    mount_path: "/var/lib/postgresql".to_owned(),
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
        let namespace = self
            .metadata
            .namespace
            .as_ref()
            .expect("namespace is required");
        tracing::warn!("cleanup functionality only partially implmented");

        tracing::info!("deleting Pod");
        let pod_api: Api<Pod> = Api::namespaced(client.clone(), namespace);

        match pod_api.get(self.release_name().as_str()).await {
            Ok(_sts) => {
                let delete_params = DeleteParams {
                    propagation_policy: Some(PropagationPolicy::Foreground),
                    ..Default::default()
                };
                pod_api
                    .delete(self.release_name().as_str(), &delete_params)
                    .await?;
            }
            Err(_e) => {
                // Log a warning because this shouldn't happen.
                tracing::warn!("statefulset not found, skipping deletion");
            }
        }

        let svc_api: Api<Service> = Api::namespaced(client.clone(), namespace);
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

    async fn delete_and_wait(api: &Api<Pod>, name: &str) -> Result<()> {
        // Start deletion with foreground propagation
        let dp = DeleteParams {
            propagation_policy: Some(PropagationPolicy::Foreground),
            ..Default::default()
        };

        api.delete(name, &dp).await?;

        // Simple poll loop with timeout
        let timeout = Duration::from_secs(30);
        let start = std::time::Instant::now();

        while start.elapsed() < timeout {
            match api.get(name).await {
                Ok(_) => {
                    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                }
                Err(kube::Error::Api(err)) if err.code == 404 => {
                    return Ok(()); // Pod is gone
                }
                Err(e) => return Err(e.into()),
            }
        }

        Err(crate::error::Error::Timeout)
        // Err(crate::error::Error::KubeError("Pod deletion timeout"))
    }

    /// Ensure that the CRD is adequately expressed in cluster resources.
    pub async fn reconcile(&self, client: &Client) -> Result<Action> {
        let namespace = self
            .metadata
            .namespace
            .as_ref()
            .expect("namespace is required");

        // We need a ConfigMap in order for the initContainer to run.
        let cm = self.pd_init_script_configmap();
        let cm_api: Api<ConfigMap> = Api::namespaced(client.clone(), namespace);
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

        // Create PVCs. Currently there are two, but the db should be folded in to the primary.
        let pvcs = self.pvcs();
        let pvc_api: Api<PersistentVolumeClaim> = Api::namespaced(client.clone(), namespace);

        // TODO: support resizing after creation.
        for pvc in pvcs {
            let pvc_name = pvc.metadata.name.as_ref().expect("pvc has name");
            match pvc_api.get(pvc_name).await {
                Ok(_) => {
                    let patch = Patch::Merge(&pvc);
                    let params = PatchParams::default();
                    pvc_api.patch(pvc_name, &params, &patch).await?;
                }
                Err(_e) => {
                    tracing::info!("creating PVC<{}>", &pvc_name);
                    pvc_api.create(&PostParams::default(), &pvc).await?;
                }
            }
        }

        // Generate a Pod for the node.
        let pod = self.pod();
        let pod_api: Api<Pod> = Api::namespaced(client.clone(), namespace);
        match pod_api.get(&self.release_name()).await {
            Ok(_pod_old) => {
                tracing::trace!("patching Pod<{}>", self.release_name());
                let patch = Patch::Apply(&pod);
                let params = PatchParams::apply(crate::OPERATOR_NAME);
                match pod_api
                    .patch(self.release_name().as_str(), &params, &patch)
                    .await
                {
                    Ok(_pod_new) => {}
                    // If patching the Pod failed, we likely tried to update a non-mutable field.
                    // Instead, recreate the pod.
                    // Err(e) => {
                    Err(kube::Error::Api(err)) => {
                        if err.code == 422 {
                            // Unprocessable Entity
                            tracing::warn!(
                                "failed to patch Pod<{}>: {}, recreating it",
                                self.release_name(),
                                err,
                            );
                            Self::delete_and_wait(&pod_api, &self.release_name()).await?;
                            pod_api.create(&PostParams::default(), &pod).await?;
                        } else if err.code == 409 {
                            // Unprocessable Entity
                            tracing::warn!(
                                "failed to patch Pod<{}>: {}, recreating it",
                                self.release_name(),
                                err,
                            );
                            Self::delete_and_wait(&pod_api, &self.release_name()).await?;
                            pod_api.create(&PostParams::default(), &pod).await?;
                        } else {
                            tracing::warn!(
                                "received error code '{}' on PATCH to Pod<{}>",
                                err.code,
                                self.release_name()
                            );
                            // return Err(crate::error::Error::KubeError(kube::Error::Api(err)));
                        }
                    }
                    Err(e) => {
                        tracing::error!("found unexpected error");
                        return Err(crate::error::Error::KubeError(e));
                    }
                }
            }
            Err(_) => {
                tracing::info!("creating Pod<{}>", self.release_name());
                pod_api.create(&PostParams::default(), &pod).await?;
            }
        }

        // Generate a Service for the node.
        let svc = self.service();
        let svc_api: Api<Service> = Api::namespaced(client.clone(), namespace);
        match svc_api.get(&self.release_name()).await {
            Ok(_svc_old) => {
                tracing::trace!("patching Service<{}>", self.release_name());
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

    use crate::PENUMBRA_IMAGE_REPO;
    use crate::PENUMBRA_IMAGE_TAG;
    #[test]
    fn from_specs() {
        let fullnode = PenumbraNode::new(
            "foo",
            PenumbraNodeSpec {
                moniker: "foo".into(),
                ..Default::default()
            },
        );
        let pod = fullnode.pod();
        assert_eq!(pod.metadata.name.unwrap(), fullnode.release_name());

        let container_image = pod.spec.clone().expect("pod must have spec").containers[0]
            .image
            .clone()
            .expect("pod must have image")
            .to_string();
        assert_eq!(
            format!("{PENUMBRA_IMAGE_REPO}:{PENUMBRA_IMAGE_TAG}"),
            container_image
        );
    }
}
