//! Declarations for each individual k8s spec in a given deployment,
//! all the way down to a [Container].
use k8s_openapi::api::core::v1::{
    ConfigMap, ConfigMapVolumeSource, Container, ContainerPort, EnvVar, KeyToPath,
    PersistentVolumeClaim, PersistentVolumeClaimSpec, Probe, SecurityContext, TCPSocketAction,
    Volume, VolumeMount, VolumeResourceRequirements,
};
use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;
use std::collections::BTreeMap;

use crate::COMETBFT_IMAGE_REPO;
use crate::COMETBFT_IMAGE_TAG;
use crate::PENUMBRA_IMAGE_REPO;
use crate::PENUMBRA_IMAGE_TAG;
use crate::POSTGRES_IMAGE_REPO;
use crate::POSTGRES_IMAGE_TAG;

// Total size for PVC for node, including pd & cometbft state.
// Must provide enough space for archives to be extracted.
const DEFAULT_PVC_SIZE: &str = "200G";

/// Generate map of labels, for use in object metadata.
pub fn labels() -> BTreeMap<String, String> {
    BTreeMap::from([
        (
            "app.kubernetes.io/name".to_owned(),
            "penumbra-node-via-operator".to_owned(),
        ),
        (
            "app.kubernetes.io/managed-by".to_owned(),
            crate::OPERATOR_NAME.to_owned(),
        ),
        (
            "app.kubernetes.io/version".to_owned(),
            env!("CARGO_PKG_VERSION").to_owned(),
        ),
    ])
}

/// Create [PersistentVolumeClaim]s for StatefulSet spec.
pub fn volume_claim_templates() -> Vec<PersistentVolumeClaim> {
    vec![
        PersistentVolumeClaim {
            metadata: ObjectMeta {
                name: Some("penumbra-config".to_owned()),
                labels: Some(labels()),
                ..Default::default()
            },
            spec: Some(PersistentVolumeClaimSpec {
                access_modes: Some(vec!["ReadWriteOnce".to_owned()]),
                resources: Some(VolumeResourceRequirements {
                    requests: Some(BTreeMap::<String, Quantity>::from([(
                        "storage".to_owned(),
                        Quantity(DEFAULT_PVC_SIZE.to_owned()),
                    )])),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        },
        PersistentVolumeClaim {
            metadata: ObjectMeta {
                name: Some("penumbra-db".to_owned()),
                labels: Some(labels()),
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
        },
    ]
}

/// Expose bash script for pd-init as ConfigMap, so it's volume-mountable.
pub fn pd_init_script_configmap() -> ConfigMap {
    ConfigMap {
        metadata: ObjectMeta {
            name: Some("pd-init".to_owned()),
            labels: Some(labels()),
            ..Default::default()
        },
        data: Some(BTreeMap::from([(
            "pd-init".to_owned(),
            include_str!("../files/pd-init").to_string(),
        )])),
        ..Default::default()
    }
}

/// Define additional [Volume]s for the pod, beyond the [PersistentVolumeClaim]s.
pub fn volumes() -> Vec<Volume> {
    vec![
        Volume {
            name: "penumbra-init".to_owned(),
            config_map: Some(ConfigMapVolumeSource {
                name: "pd-init".to_owned(),
                items: Some(vec![KeyToPath {
                    key: "pd-init".to_owned(),
                    path: "pd-init".to_owned(),
                    ..Default::default()
                }]),
                ..Default::default()
            }),
            ..Default::default()
        },
        Volume {
            name: "postgres-schema".to_owned(),
            config_map: Some(ConfigMapVolumeSource {
                name: "penumbra-cometbft-postgres-schema".to_owned(),
                items: Some(vec![KeyToPath {
                    key: "postgres-cometbft-schema.sql".to_owned(),
                    path: "postgres-cometbft-schema.sql".to_owned(),
                    ..Default::default()
                }]),
                ..Default::default()
            }),
            ..Default::default()
        },
    ]
}

/// Create [Container] spec for `pd-init`, for bootstrapping configuration
/// from a remote node.
pub fn pd_init_container(bootstrap_url: String, archive_url: Option<String>) -> Container {
    let container_name = "pd-init".to_owned();
    // Bootstrap URL is required, since we need to talk to another node to join its network.
    let mut env: Vec<EnvVar> = vec![EnvVar {
        name: "PENUMBRA_BOOTSTRAP_URL".to_owned(),
        value: Some(bootstrap_url),
        value_from: None,
    }];
    // Archive URL is optional.
    if let Some(a) = archive_url {
        env.push(EnvVar {
            name: "PENUMBRA_CUSTOM_ARCHIVE_URL".to_owned(),
            value: Some(a),
            value_from: None,
        });
    }

    Container {
        name: container_name,
        image: Some(format!("{PENUMBRA_IMAGE_REPO}:{PENUMBRA_IMAGE_TAG}")),
        command: Some(
            // TODO support opt-in cometbft indexing
            vec!["bash", "/opt/penumbra/pd-init"]
                .into_iter()
                .map(|x| x.to_owned())
                .collect(),
        ),
        env: Some(env),
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
                name: "penumbra-init".to_owned(),
                mount_path: "/opt/penumbra".to_owned(),
                ..Default::default()
            },
            VolumeMount {
                name: "penumbra-config".to_owned(),
                mount_path: "/home/penumbra/.penumbra/".to_owned(),
                ..Default::default()
            },
        ]),
        ..Default::default()
    }
}

/// Create [Container] spec for `pd`, the Penumbra daemon.
pub fn pd_container() -> Container {
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
            name: "penumbra-config".to_owned(),
            mount_path: "/home/penumbra/.penumbra".to_owned(),
            ..Default::default()
        }]),

        ..Default::default()
    }
}

/// Create [Container] spec for `cometbft`, the CometBFT consensus sidecar for Penumbra.
pub fn cometbft_container() -> Container {
    Container {
        name: "cometbft".to_owned(),
        image: Some(format!("{COMETBFT_IMAGE_REPO}:{COMETBFT_IMAGE_TAG}")),
        command: Some(
            vec!["cometbft", "start", "--proxy_app=tcp://127.0.0.1:26658"]
                .into_iter()
                .map(|x| x.to_owned())
                .collect(),
        ),
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
        readiness_probe: Some(Probe {
            tcp_socket: Some(TCPSocketAction {
                port: IntOrString::String("cmt-rpc".to_owned()),
                ..Default::default()
            }),
            ..Default::default()
        }),
        security_context: Some(SecurityContext {
            run_as_user: Some(100),
            ..Default::default()
        }),
        volume_mounts: Some(vec![VolumeMount {
            name: "penumbra-config".to_owned(),
            mount_path: "/cometbft".to_owned(),
            sub_path: Some("network_data/node0/cometbft".to_owned()),
            ..Default::default()
        }]),

        ..Default::default()
    }
}

/// Create [Container] spec for `postgres`, for optional ABCI event indexing
/// via CometBFT.
pub fn postgres_container() -> Container {
    let container_name = "postgres".to_owned();
    Container {
        name: container_name.clone(),
        image: Some(format!("{POSTGRES_IMAGE_REPO}:{POSTGRES_IMAGE_TAG}")),
        // TODO support ssl args
        ports: Some(vec![ContainerPort {
            name: Some(container_name),
            container_port: 5432,
            ..Default::default()
        }]),
        ..Default::default()
    }
}
