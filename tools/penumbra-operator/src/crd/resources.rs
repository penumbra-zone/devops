//! Declarations for static k8s resources, that don't draw input
//! from the CRD spec's configuration.
use std::collections::BTreeMap;

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
