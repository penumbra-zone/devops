//! The Penumbra operator, for running Penumbra nodes on Kubernetes.

/// The name of the operator, for reuse in resource names and labels.
pub const OPERATOR_NAME: &str = "penumbra-operator";
/// The FQDN for namespacing the CRDs within the Kubernetes API.
pub const OPERATOR_GROUP: &str = "penumbra.zone";

/// The Kubernetes namespace in which resources will be monitored.
pub const DEFAULT_NAMESPACE: &str = "penumbra";

/// The container image repository for the Penumbra images.
pub const PENUMBRA_IMAGE_REPO: &str = "ghcr.io/penumbra-zone/penumbra";
/// The container tag used for Penumbra images.
pub const PENUMBRA_IMAGE_TAG: &str = "v0.81.3";

/// The container image repository for CometBFT images.
pub const COMETBFT_IMAGE_REPO: &str = "docker.io/cometbft/cometbft";
/// The container tag used for CometBFT images.
pub const COMETBFT_IMAGE_TAG: &str = "v0.37.11";

/// The container image repository for Postgres images.
pub const POSTGRES_IMAGE_REPO: &str = "docker.io/library/postgres";
/// The container tag used for Postgres images.
pub const POSTGRES_IMAGE_TAG: &str = "latest";

pub const PENUMBRA_FINALIZER: &str = "latest";

pub mod controller;
pub mod crd;
pub mod error;
