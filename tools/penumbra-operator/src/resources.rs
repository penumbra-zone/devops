//! Declarations for static k8s resources, that don't draw input
//! from the CRD spec's configuration.
use k8s_openapi::api::core::v1::{
    ConfigMap, ConfigMapVolumeSource, Container, ContainerPort, EnvVar, KeyToPath, Probe,
    SecurityContext, TCPSocketAction, Volume, VolumeMount,
};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;
use std::collections::BTreeMap;

use crate::COMETBFT_IMAGE_REPO;
use crate::COMETBFT_IMAGE_TAG;
// use crate::PENUMBRA_IMAGE_REPO;
// use crate::PENUMBRA_IMAGE_TAG;
use crate::POSTGRES_IMAGE_REPO;
use crate::POSTGRES_IMAGE_TAG;

pub(crate) const PD_INIT_CONFIG_MAP_NAME: &str = "pd-init";
pub(crate) const CMT_SCHEMA_CONFIG_MAP_NAME: &str = "penumbra-cometbft-postgres-schema";
pub(crate) const DB_PVC_NAME: &str = "penumbra-db";

// Total size for PVC for node, including pd & cometbft state.
// Must provide enough space for archives to be extracted.
pub(crate) const DEFAULT_PVC_SIZE: &str = "200G";

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

/// Expose bash script for pd-init as ConfigMap, so it's volume-mountable.
pub fn pd_init_script_configmap() -> ConfigMap {
    ConfigMap {
        metadata: ObjectMeta {
            name: Some(PD_INIT_CONFIG_MAP_NAME.to_owned()),
            labels: Some(labels()),
            ..Default::default()
        },
        data: Some(BTreeMap::from([(
            PD_INIT_CONFIG_MAP_NAME.to_owned(),
            include_str!("../files/pd-init").to_string(),
        )])),
        ..Default::default()
    }
}

/// Expose PostgreSQL default schema for CometBFT, for initializing the event-indexing
/// database.
pub fn postgres_schema_configmap() -> ConfigMap {
    ConfigMap {
        metadata: ObjectMeta {
            name: Some(CMT_SCHEMA_CONFIG_MAP_NAME.to_owned()),
            labels: Some(labels()),
            ..Default::default()
        },
        data: Some(BTreeMap::from([(
            "postgres-cometbft-schema.sql".to_owned(),
            include_str!("../files/postgres-cometbft-schema.sql").to_string(),
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
                name: PD_INIT_CONFIG_MAP_NAME.to_owned(),
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
                name: CMT_SCHEMA_CONFIG_MAP_NAME.to_owned(),
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

/// Generate map of annotations, for use in object metadata.
/// For now, only applies to StatefulSet.
pub fn annotations() -> BTreeMap<String, String> {
    BTreeMap::from([
        // Opt in to reload functionality via https://github.com/stakater/Reloader
        // Won't do anything unless the "reloader" operator is already running in cluster.
        (
            "configmap.reloader.stakater.com/reload".to_owned(),
            PD_INIT_CONFIG_MAP_NAME.to_owned(),
        ),
        ("reloader.stakater.com/auto".to_owned(), "true".to_owned()),
    ])
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
                name: DB_PVC_NAME.to_owned(),
                mount_path: "/var/lib/postgresql".to_owned(),
                ..Default::default()
            },
        ]),

        ..Default::default()
    }
}
