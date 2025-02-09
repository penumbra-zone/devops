//! Kubernetes-specific Custom Resource Definition for representing
//! a [PenumbraNetwork], a full chain which [PenumbraNode]s can join.
//!
//! The creation of resources is strictly ordered:
//!
//!   1. Create PVCs based on `num_validators` (PVCs must match names in StatefulSet VCTs).
//!   2. Create PVC for shared-config, to store network genesis.
//!   3. Create Job for `pd network generate`, copy outputs to relevant volumes.
//!      The job will not be recreated if the PVCs had already exists.
//!   4. Create `StatefulSet` for each validator, using the [PenumbraNode] CRD.
//!

use apiexts::CustomResourceDefinition;
use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1 as apiexts;
use kube::runtime::controller::Action;
use kube_derive::CustomResource;
use schemars::JsonSchema;

use k8s_openapi::apimachinery::pkg::apis::meta::v1::{ObjectMeta, OwnerReference};
use std::fmt;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use k8s_openapi::api::batch::v1::{Job, JobSpec};
use k8s_openapi::api::core::v1::{
    ConfigMap, ConfigMapVolumeSource, Container, EnvVar, KeyToPath, PersistentVolumeClaim,
    PersistentVolumeClaimSpec, PersistentVolumeClaimVolumeSource, PodSpec, PodTemplateSpec,
    SecurityContext, Volume, VolumeMount, VolumeResourceRequirements,
};

use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use kube::{
    api::{Api, DeleteParams, Patch, PatchParams, PostParams, PropagationPolicy},
    runtime::wait::{await_condition, conditions},
    Client, CustomResourceExt,
};
use std::collections::BTreeMap;

use crate::crd::node::{PenumbraNode, PenumbraNodeSpec};
use crate::crd::resources::PD_NODE_STATE_PVC_NAME;
use crate::error::Result;
use crate::DEFAULT_NAMESPACE;
use crate::PENUMBRA_IMAGE_REPO;
use crate::PENUMBRA_IMAGE_TAG;

use super::node::PD_INIT_SCRIPT_NAME;

const PD_NETWORK_GENERATE_CONFIG_MAP_NAME: &str = "pd-network-generate";
const PD_NETWORK_MOUNT_POINT: &str = "/opt/penumbra";
const PD_NETWORK_VAL_CONFIGS_MOUNT_POINT: &str = "/penumbra-config";

const DEFAULT_NUM_VALIDATORS: u64 = 2;
const DEFAULT_EPOCH_DURATION: u64 = 2000;
const DEFAULT_PROPOSAL_VOTING_BLOCKS: u64 = 100;

/// K8s CRD specification for a [`PenumbraNetwork`] resource.
///
/// Constitutes the `spec` field of a [PenumbraNetwork].
#[derive(CustomResource, Serialize, Deserialize, Debug, PartialEq, Clone, JsonSchema)]
#[kube(
    group = "penumbra.zone",
    version = "v1alpha1",
    kind = "PenumbraNetwork",
    plural = "penumbranetworks",
    derive = "PartialEq",
    namespaced,
    printcolumn = r#"{"name":"ChainId", "type":"string", "jsonPath":".spec.chain_id"}"#,
    printcolumn = r#"{"name":"Age","type":"date","jsonPath":".metadata.creationTimestamp"}"#
)]
pub struct PenumbraNetworkSpec {
    /// The unique identifier for the chain.
    chain_id: String,
    /// How many validators will be configured at genesis.
    num_validators: Option<u64>,
    /// Whether to allow local, intra-cluster addresses.
    /// Defaults to true.
    allow_local_addresses: Option<bool>,

    // Chain params
    epoch_duration: Option<u64>,
    proposal_voting_blocks: Option<u64>,
}

impl Default for PenumbraNetworkSpec {
    fn default() -> Self {
        Self {
            chain_id: "i-didnt-edit-the-config-1".to_owned(),
            num_validators: Some(DEFAULT_NUM_VALIDATORS),
            allow_local_addresses: Some(true),
            epoch_duration: Some(DEFAULT_EPOCH_DURATION),
            proposal_voting_blocks: Some(DEFAULT_PROPOSAL_VOTING_BLOCKS),
        }
    }
}

impl fmt::Display for PenumbraNetwork {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "PenumbraNetwork<{}>", self.spec.chain_id)
    }
}

impl PenumbraNetwork {
    /// Creates a likely-unique name for this network.
    pub fn release_name(&self) -> String {
        // Don't prefix the name, it gets redundant and hard to read when
        // embedded in the names of so many resources.
        // format!("penumbra-network-{}", self.spec.chain_id)
        self.spec.chain_id.clone()
    }

    /// Emit an [OwnerResource] suitable for inclusion in a resource's metadata,
    /// so that deletion of the parent CRD will propagate to cleanup of the dependent
    /// resources.
    pub fn oref(&self) -> OwnerReference {
        OwnerReference {
            // TODO: figure out how to access the kube-derive fields for api_version and kind.
            api_version: "v1alpha1".to_owned(),
            kind: "PenumbraNetwork".to_owned(),
            name: self.metadata.name.clone().unwrap_or_else(|| {
                panic!("PenumbraNetwork<{}> lacks a name", &self.release_name())
            }),
            uid: self
                .metadata
                .uid
                .clone()
                .unwrap_or_else(|| panic!("PenumbraNetwork<{}> lacks a uid", self.release_name())),
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
                "genesis-validator".to_owned(),
            ),
            ("app.kubernetes.io/name".to_owned(), self.release_name()),
            ("app.kubernetes.io/part-of".to_owned(), self.release_name()),
        ]));
        l
    }

    /// Install the CRD to the cluster.
    pub async fn install(client: &Client) -> Result<()> {
        // Lifted from the kube.rs examples directory
        let params = PatchParams::apply(crate::OPERATOR_NAME).force();
        let crds: Api<CustomResourceDefinition> = Api::all(client.clone());

        // Create `PenumbraNetwork` CRD.
        let crd_fqdn = format!("penumbranetworks.{}", crate::OPERATOR_GROUP);
        tracing::debug!("creating crd: {}", crd_fqdn,);
        crds.patch(&crd_fqdn, &params, &Patch::Apply(PenumbraNetwork::crd()))
            .await?;

        // Block until ready.
        tracing::trace!("waiting for the api-server to accept the CRD");
        let establish = await_condition(crds.clone(), &crd_fqdn, conditions::is_crd_established());
        let _ = tokio::time::timeout(std::time::Duration::from_secs(10), establish)
            .await
            .map_err(|_e| crate::error::Error::InstallFailure);
        Ok(())
    }

    /// Emit a [Job] that runs `pd network generate`
    /// on CRD creation, to create validator identities and genesis
    /// material.
    pub fn generate_network_job(&self) -> Job {
        Job {
            metadata: ObjectMeta {
                name: Some(format!("{}-generate-network", self.release_name())),
                // TODO: Does this clobber annotations automatically generated by `kube`?
                annotations: Some(crate::crd::resources::annotations()),
                owner_references: Some(vec![self.oref()]),
                ..Default::default()
            },
            spec: Some(JobSpec {
                // Don't retry on failure
                backoff_limit: Some(0),
                template: PodTemplateSpec {
                    metadata: Some(ObjectMeta {
                        name: Some(self.release_name()),
                        labels: Some(self.labels()),
                        // Don't set owner references on the Pod; by default, the Pod will have
                        // orefs set to the parent Job, which is enough to ensure cleanup.
                        // owner_references: Some(vec![self.oref()]),
                        ..Default::default()
                    }),
                    spec: Some(PodSpec {
                        containers: vec![self.pd_network_generate_container()],
                        volumes: Some(self.volumes()),
                        restart_policy: Some("Never".to_owned()),
                        ..Default::default()
                    }),
                },
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    /// Expose bash script for pd-network-generate as ConfigMap, so it's volume-mountable.
    pub fn pd_network_generate_script_configmap(&self) -> ConfigMap {
        ConfigMap {
            metadata: ObjectMeta {
                name: Some(format!(
                    "{}-{}",
                    self.release_name(),
                    PD_NETWORK_GENERATE_CONFIG_MAP_NAME,
                )),
                labels: Some(self.labels()),
                // owner_references: Some(vec![self.oref()]),
                ..Default::default()
            },
            data: Some(BTreeMap::from([
                // Write the script that generates network info.
                (
                    PD_NETWORK_GENERATE_CONFIG_MAP_NAME.to_owned(),
                    include_str!("../../files/pd-network-generate").to_string(),
                ),
                // Write validator metadata to a JSON file, for input into the script.
                (
                    "validators.json".to_owned(),
                    serde_json::to_string(&self.validator_metadata())
                        .expect("failed to serialize validator metadata"),
                ),
            ])),
            ..Default::default()
        }
    }

    /// Define [Volume]s for the Job pod, mounting in scripts.
    pub fn volumes(&self) -> Vec<Volume> {
        let mut volumes: Vec<Volume> = vec![Volume {
            name: crate::crd::node::PD_INIT_SCRIPT_NAME.to_owned(),
            config_map: Some(ConfigMapVolumeSource {
                name: self
                    .pd_network_generate_script_configmap()
                    .metadata
                    .name
                    .expect("pd network generate script must have name"),
                items: Some(vec![
                    KeyToPath {
                        key: PD_NETWORK_GENERATE_CONFIG_MAP_NAME.to_owned(),
                        path: PD_NETWORK_GENERATE_CONFIG_MAP_NAME.to_owned(),
                        mode: Some(0o0755),
                    },
                    KeyToPath {
                        key: "validators.json".to_owned(),
                        path: "validators.json".to_owned(),
                        mode: Some(0o0644),
                    },
                ]),
                ..Default::default()
            }),
            ..Default::default()
        }];

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

        volumes.extend(pvc_volumes);
        volumes
    }

    /// Emits structured data required for the `validators-input-file` config.
    #[tracing::instrument]
    fn validator_metadata(&self) -> Vec<ValidatorMetadata> {
        let mut results = Vec::<ValidatorMetadata>::new();
        for i in 0..self.spec.num_validators.unwrap_or(DEFAULT_NUM_VALIDATORS) {
            let v = ValidatorMetadata {
                name: format!("Penumbra Labs CI {}", i),
                ..Default::default()
            };
            results.push(v);
        }
        assert_eq!(
            results.len() as u64,
            self.spec.num_validators.unwrap_or(DEFAULT_NUM_VALIDATORS)
        );
        results
    }

    /// Create [PersistentVolumeClaim]s for Job spec.
    pub fn pvcs(&self) -> Vec<PersistentVolumeClaim> {
        let mut pvcs = vec![PersistentVolumeClaim {
            // Create PVC for initializing the network. Validator configs
            // will be copied out of this PVC to others.
            metadata: ObjectMeta {
                name: self
                    .pd_network_generate_script_configmap()
                    .clone()
                    .metadata
                    .name,
                labels: Some(self.labels()),
                owner_references: Some(vec![self.oref()]),
                ..Default::default()
            },
            spec: Some(PersistentVolumeClaimSpec {
                access_modes: Some(vec!["ReadWriteOnce".to_owned()]),
                resources: Some(VolumeResourceRequirements {
                    requests: Some(BTreeMap::<String, Quantity>::from([(
                        "storage".to_owned(),
                        // Quantity(crate::crd::resources::DEFAULT_PVC_SIZE.to_owned()),
                        // We don't need much space for initial configs.
                        Quantity("100M".to_owned()),
                    )])),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        }];
        // Create PVC for each validator.
        // It's important that we name the volumes the same way that the PenumbraNode CRD
        // will create them. Here's an example:
        //
        //   persistentvolumeclaim/penumbra-config-penumbra-node-penumbra-devnet-55-val-0-0
        //   persistentvolumeclaim/penumbra-config-penumbra-node-penumbra-devnet-55-val-1-0
        //   persistentvolumeclaim/penumbra-network-penumbra-devnet-55-val-0
        //   persistentvolumeclaim/penumbra-network-penumbra-devnet-55-val-1
        //
        // The first two are created by PenumbraNode, the second by PenumbraNetwork. The second two
        // should match the first two.

        for i in 0..self.spec.num_validators.unwrap_or(DEFAULT_NUM_VALIDATORS) {
            let pvc_name = self.val_pvc_name(i);
            let pvc = PersistentVolumeClaim {
                metadata: ObjectMeta {
                    name: Some(pvc_name),
                    labels: Some(self.labels()),
                    owner_references: Some(vec![self.oref()]),
                    ..Default::default()
                },
                spec: Some(PersistentVolumeClaimSpec {
                    access_modes: Some(vec!["ReadWriteOnce".to_owned()]),
                    resources: Some(VolumeResourceRequirements {
                        requests: Some(BTreeMap::<String, Quantity>::from([(
                            "storage".to_owned(),
                            // TODO make volume size customizable via CRD spec.
                            Quantity(crate::crd::resources::DEFAULT_PVC_SIZE.to_owned()),
                        )])),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            };
            pvcs.push(pvc);
        }
        pvcs
    }

    /// Generate environment variables for pd container that generates the network.
    pub fn pd_env(&self) -> Vec<EnvVar> {
        // let mut env = Vec::<EnvVar>::new();
        let mut env = vec![
            EnvVar {
                name: "PENUMBRA_NETWORK_CHAIN_ID".to_owned(),
                value: Some(self.spec.chain_id.clone()),
                value_from: None,
            },
            EnvVar {
                name: "PENUMBRA_NETWORK_NUM_VALIDATORS".to_owned(),
                value: Some(
                    self.spec
                        .num_validators
                        .unwrap_or(DEFAULT_NUM_VALIDATORS)
                        .to_string(),
                ),
                value_from: None,
            },
            EnvVar {
                name: "PENUMBRA_NETWORK_EPOCH_DURATION".to_owned(),
                value: Some(
                    self.spec
                        .epoch_duration
                        .unwrap_or(DEFAULT_EPOCH_DURATION)
                        .to_string(),
                ),
                value_from: None,
            },
            EnvVar {
                name: "PENUMBRA_NETWORK_PROPOSAL_VOTING_BLOCKS".to_owned(),
                value: Some(
                    self.spec
                        .proposal_voting_blocks
                        .unwrap_or(DEFAULT_PROPOSAL_VOTING_BLOCKS)
                        .to_string(),
                ),
                value_from: None,
            },
            EnvVar {
                name: "PENUMBRA_NETWORK_VALIDATORS_INPUT_FILEPATH".to_owned(),
                value: Some(format!("{PD_NETWORK_MOUNT_POINT}/validators.json")),
                value_from: None,
            },
            EnvVar {
                // TODO: how to get external addrs?
                name: "PENUMBRA_NETWORK_EXTERNAL_ADDRESSES".to_owned(),
                value: Some(self.release_name()),
                value_from: None,
            },
            EnvVar {
                name: "PD_NETWORK_MOUNT_POINT".to_owned(),
                value: Some(PD_NETWORK_MOUNT_POINT.to_owned()),
                value_from: None,
            },
            EnvVar {
                name: "PD_NETWORK_VAL_CONFIGS_MOUNT_POINT".to_owned(),
                value: Some(PD_NETWORK_VAL_CONFIGS_MOUNT_POINT.to_owned()),
                value_from: None,
            },
        ];

        // Format the `--peer-address-template`, so that the `pd network generate`
        // command can include address info (bundled with corresponding pubkey)
        // in the generated CometBFT configs.
        if self.spec.num_validators.unwrap_or(DEFAULT_NUM_VALIDATORS) > 1 {
            // There's no trailing zero on the service name, so strip

            // l
            let peer_address_template = format!(
                "penumbra-node-{}",
                self.val_name(0)
                    .strip_suffix("-0")
                    .expect("failed to strip basic suffix"),
            );

            env.push(EnvVar {
                name: "PENUMBRA_NETWORK_PEER_ADDRESS_TEMPLATE".to_owned(),
                // The initial CometBFT configs must contain the
                value: Some(peer_address_template),
                value_from: None,
            });
        }
        env
    }

    /// Reusable function to make declaring the validator name DRY.
    fn val_name(&self, index: u64) -> String {
        format!("{}-val-{}", self.release_name(), index)
    }

    /// Generate the precise PVC name.
    fn val_pvc_name(&self, index: u64) -> String {
        // This name must match exactly what's set in the PenumbraNode's Pod, so it gets reused.
        format!(
            "penumbra-node-{}-val-{}-{}",
            self.spec.chain_id, index, PD_NODE_STATE_PVC_NAME
        )
    }

    /// Create [Container] spec for `pd-network-generate`, for creating chain info.
    pub fn pd_network_generate_container(&self) -> Container {
        // Prepare VolumeMounts separately, since we need to iterate
        let mut volume_mounts = Vec::<VolumeMount>::new();

        volume_mounts.push(VolumeMount {
            name: PD_INIT_SCRIPT_NAME.to_owned(),
            mount_path: PD_NETWORK_MOUNT_POINT.to_owned(),
            ..Default::default()
        });

        for i in 0..self.spec.num_validators.unwrap_or(DEFAULT_NUM_VALIDATORS) {
            let val_name = self.val_name(i);
            let val_pvc_name = self.val_pvc_name(i);
            volume_mounts.push(VolumeMount {
                name: val_pvc_name.clone(),
                mount_path: format!("{PD_NETWORK_VAL_CONFIGS_MOUNT_POINT}/{val_name}"),
                ..Default::default()
            });
        }

        Container {
            name: "pd".to_owned(),
            image: Some(format!("{PENUMBRA_IMAGE_REPO}:{PENUMBRA_IMAGE_TAG}")),
            command: Some(vec![format!(
                "{PD_NETWORK_MOUNT_POINT}/{PD_NETWORK_GENERATE_CONFIG_MAP_NAME}"
            )]),
            env: Some(self.pd_env()),
            // Run as root during init, so we can shown to penumbra & cometbft users.
            // The application itself will run as a normal user.
            security_context: Some(SecurityContext {
                run_as_user: Some(0),
                run_as_group: Some(0),
                allow_privilege_escalation: Some(true),
                ..Default::default()
            }),
            volume_mounts: Some(volume_mounts),
            ..Default::default()
        }
    }

    /// Ensure that the cluster resources representing the CRD are removed.
    #[tracing::instrument(skip_all)]
    pub async fn cleanup(&self, client: &Client) -> Result<Action> {
        tracing::warn!("cleanup functionality only partially implmented");

        // Delete the Job that created genesis.
        let job_api: Api<Job> = Api::namespaced(client.clone(), DEFAULT_NAMESPACE);
        let job_name = self
            .generate_network_job()
            .metadata
            .name
            .expect("Job should have name");
        match job_api.get(&job_name).await {
            Ok(_n) => {
                tracing::debug!("deleting Job<{}>", job_name);
                // Ensure cleanup of the "Completed" Pod by setting background deletion.
                let delete_params = DeleteParams {
                    propagation_policy: Some(PropagationPolicy::Background),
                    ..Default::default()
                };
                job_api.delete(&job_name, &delete_params).await?;
            }
            Err(_e) => {
                // Log a warning because this shouldn't happen.
                tracing::warn!("Job<{}> not found, skipping deletion", job_name);
            }
        }

        // Delete the validators.
        let node_api: Api<PenumbraNode> = Api::namespaced(client.clone(), DEFAULT_NAMESPACE);
        for (i, _v) in self.validators().await.iter().enumerate() {
            let val_name = self.val_name(i as u64);
            match node_api.get(&val_name).await {
                Ok(_n) => {
                    let delete_params = DeleteParams::default();
                    node_api.delete(&val_name, &delete_params).await?;
                }
                Err(_e) => {
                    // Log a warning because this shouldn't happen.
                    tracing::warn!(
                        "PenumbraNode<{}> for validator not found, skipping deletion",
                        val_name
                    );
                }
            }
        }

        Ok(Action::await_change())
    }

    /// Ensure that the CRD is adequately expressed in cluster resources.
    pub async fn reconcile(&self, client: &Client) -> Result<Action> {
        // We need a ConfigMap in order for the initContainer to run.
        let cm = self.pd_network_generate_script_configmap();
        let cm_api: Api<ConfigMap> = Api::namespaced(client.clone(), DEFAULT_NAMESPACE);

        // Create ConfigMap for storing the `pd-network-generate` script.
        let cm_name = self
            .pd_network_generate_script_configmap()
            .metadata
            .name
            .expect("pd-network-generate ConfigMap must have name");
        match cm_api.get(&cm_name).await {
            Ok(_) => {
                let patch = Patch::Merge(&cm);
                let params = PatchParams::default();
                tracing::debug!("patching ConfigMap<{}>", &cm_name);
                cm_api.patch(&cm_name, &params, &patch).await?;
            }
            Err(_e) => {
                tracing::info!("creating ConfigMap<{}>", &cm_name);
                cm_api.create(&PostParams::default(), &cm).await?;
            }
        }

        // Create PVCs: one for genesis, one for each of `num_validators`.
        let pvcs = self.pvcs();
        let pvc_api: Api<PersistentVolumeClaim> =
            Api::namespaced(client.clone(), DEFAULT_NAMESPACE);

        let expected_pvcs: usize = pvcs.len();
        let mut already_created = 0;

        // TODO: support resizing after creation.
        for pvc in pvcs {
            let pvc_name = pvc.metadata.name.as_ref().expect("pvc has name");
            match pvc_api.get(pvc_name).await {
                Ok(_) => {
                    let patch = Patch::Merge(&pvc);
                    let params = PatchParams::default();
                    pvc_api.patch(pvc_name, &params, &patch).await?;
                    already_created += 1;
                }
                Err(_e) => {
                    tracing::info!("creating PVC<{}>", &pvc_name);
                    pvc_api.create(&PostParams::default(), &pvc).await?;
                }
            }
        }

        // If all the PVCs already created before we entered this loop,
        // then we likely don't need to run the Job again.
        // TODO: annotate the pvcs after successful creation.
        let already_initialized = expected_pvcs == already_created;

        // Create Job for `pd network generate`, copy outputs to relevant volumes.
        let job_api: Api<Job> = Api::namespaced(client.clone(), DEFAULT_NAMESPACE);
        let job = self.generate_network_job();

        let job_name = self
            .generate_network_job()
            .metadata
            .name
            .expect("Job needs name");
        match job_api.get(&job_name).await {
            Ok(_) => {
                // The job should only run once, so don't bother patching it.
                tracing::debug!("Job<{}> already exists", job_name);
                // Debugging: do patch the job!
                // tracing::debug!("Job<{}> already exists, patching it", job_name);
                // let patch = Patch::Merge(&job);
                // let params = PatchParams::default();
                // job_api.patch(job_name, &params, &patch).await?;
            }
            Err(_e) => {
                if already_initialized {
                    tracing::warn!(
                        "Job<{}> doesn't exist, but PVCs appear initialized",
                        &job_name
                    );
                } else {
                    tracing::warn!("creating Job<{}>", &job_name);
                    match job_api.create(&PostParams::default(), &job).await {
                        Ok(_) => {}
                        Err(e) => {
                            tracing::error!("Job<{}> failed: {}", job_name, e,);
                            return Err(crate::error::Error::KubeError(e));
                        }
                    }
                }
            }
        }
        // Job exists, now block until it's done.
        let timeout = 60;
        let interval = 5;
        let mut elapsed = 0;

        while elapsed < timeout {
            match job_api.get(&job_name).await {
                Ok(j) => {
                    let status = j
                        .status
                        .clone()
                        .unwrap_or_else(|| panic!("Job<{}> should have status", &job_name));
                    if status.failed.unwrap_or_default() > 0 {
                        let msg = format!(
                            "network genesis creation failed for {}",
                            self.release_name()
                        );
                        tracing::error!(msg);
                        return Err(crate::error::Error::GenesisFailure);
                    }
                    if status.succeeded.unwrap_or_default() > 0 {
                        tracing::debug!("Job<{}> completed successfully", job_name);
                        break;
                    } else {
                        tracing::debug!(
                            "waiting for network-genesis Job<{}> to finish, {}/{}s",
                            job_name,
                            elapsed,
                            timeout
                        );
                        tokio::time::sleep(tokio::time::Duration::from_secs(interval)).await;
                        elapsed += interval;
                    }
                }
                Err(_e) => {
                    tracing::warn!("Job<{}> does not exist, but it should", &job_name);
                    break;
                    // job_api.create(&PostParams::default(), &job).await?;
                }
            }
        }

        // Create a PenumbraNode for each validator.
        let node_api: Api<PenumbraNode> = Api::namespaced(client.clone(), DEFAULT_NAMESPACE);
        for (i, n) in self.validators().await.into_iter().enumerate() {
            let node_name = self.val_name(i as u64);
            match node_api.get(&node_name).await {
                Ok(_) => {
                    let patch = Patch::Merge(&n);
                    let params = PatchParams::default();
                    node_api.patch(&node_name, &params, &patch).await?;
                }
                Err(_e) => {
                    tracing::info!("creating PenumbraNode<{}>", &node_name);
                    node_api.create(&PostParams::default(), &n).await?;
                }
            }
        }

        // If no events were received, check back every 1 minute
        Ok(Action::requeue(Duration::from_secs(60)))
    }

    /// Emit `PenumbraNode` objects, one per validator, to handle the configuring
    /// of genesis validator key material and network info.
    pub async fn validators(&self) -> Vec<PenumbraNode> {
        let mut vals = Vec::<PenumbraNode>::new();

        for (i, _metadata) in self.validator_metadata().iter().enumerate() {
            let val_name = self.val_name(i as u64);
            let v = PenumbraNode {
                metadata: ObjectMeta {
                    name: Some(val_name),
                    owner_references: Some(vec![self.oref()]),
                    ..Default::default()
                },
                spec: PenumbraNodeSpec {
                    moniker: format!("{}-val-{}", self.spec.chain_id, i),
                    // Disable bootstrap URL, to debug what's in the local volumes
                    bootstrap_url: None,
                    // Force early publication of Endpoints to Services,
                    // so the validators can communicate immediately.
                    publish_not_ready_addresses: Some(true),
                    ..Default::default()
                },
            };
            vals.push(v)
        }
        vals
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_specs() {
        let network = PenumbraNetwork::new(
            "foo-chain-1",
            PenumbraNetworkSpec {
                chain_id: "foo-chain-1".into(),
                ..Default::default()
            },
        );
        assert_eq!(
            &network.metadata.name.as_ref().unwrap().to_owned(),
            &network.release_name()
        );
    }
}

/// Represents the human-readable info describing a genesis validator.
/// Used during network genesis, to include initial validator state.
/// TODO: just pull in the upstream resource from pd (which isn't published to crates.io yet).
#[derive(Serialize)]
struct ValidatorMetadata {
    name: String,
    website: String,
    description: String,
    funding_streams: Vec<String>,
    sequence_number: u64,
}

impl Default for ValidatorMetadata {
    fn default() -> Self {
        Self {
            name: "Penumbra Labs CI".to_owned(),
            website: "https://penumbra.zone".to_owned(),
            description: "This is a validator run by Penumbra Labs, using testnets as a public CI"
                .to_owned(),
            funding_streams: vec![],
            sequence_number: 0,
        }
    }
}
