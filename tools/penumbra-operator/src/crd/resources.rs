//! Declarations for static k8s resources, that don't draw input
//! from the CRD spec's configuration.
use k8s_openapi::api::core::v1::{
    Container, ContainerPort, EnvVar, Probe, SecurityContext, TCPSocketAction, VolumeMount,
};
use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;
use std::collections::BTreeMap;

use crate::COMETBFT_IMAGE_REPO;
use crate::COMETBFT_IMAGE_TAG;
// use crate::PENUMBRA_IMAGE_REPO;
// use crate::PENUMBRA_IMAGE_TAG;
use crate::POSTGRES_IMAGE_REPO;
use crate::POSTGRES_IMAGE_TAG;

pub(crate) const DB_PVC_NAME: &str = "penumbra-db";
pub(crate) const PD_NODE_STATE_PVC_NAME: &str = "penumbra-config";

// Total size for PVC for node, including pd & cometbft state.
// Must provide enough space for archives to be extracted.
pub(crate) const DEFAULT_PVC_SIZE: &str = "200G";

/// Generate map of labels, for use in object metadata.
/// These are the common baseline across all resources;
/// individual CRDs will likely add more, like `component`.
pub fn labels() -> BTreeMap<String, String> {
    BTreeMap::from([
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

/// Generate map of annotations, for use in object metadata.
/// For now, only applies to StatefulSet.
pub fn annotations() -> BTreeMap<String, String> {
    BTreeMap::from([
        // Opt in to reload functionality via https://github.com/stakater/Reloader
        // Won't do anything unless the "reloader" operator is already running in cluster.
        (
            "configmap.reloader.stakater.com/reload".to_owned(),
            crate::crd::node::PD_INIT_SCRIPT_NAME.to_owned(),
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
            name: PD_NODE_STATE_PVC_NAME.to_owned(),
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
