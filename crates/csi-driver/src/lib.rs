//! CSI controller-side service and policy validation.

use nas_csi_proto::csi::v1 as csi;
use nas_csi_truenas_client::{JsonRpcClient, TrueNasWebSocketConfig, TrueNasWebSocketTransport};
use nas_csi_types::AccessMode as PolicyAccessMode;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::net::UnixListener;
use tokio_stream::wrappers::UnixListenerStream;
use tonic::{Request, Response, Status, transport::Server};

pub const DEFAULT_DRIVER_NAME: &str = "nas-csi.dev";

#[derive(Copy, Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum VolumeMode {
    Filesystem,
    Block,
}

#[derive(Copy, Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum CsiAccessMode {
    SingleNodeWriter,
    SingleNodeReaderOnly,
    MultiNodeReaderOnly,
    MultiNodeSingleWriter,
    MultiNodeMultiWriter,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct VolumePolicy {
    pub name: String,
    pub access: PolicyAccessMode,
    pub allow_multi_node_writer: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapabilityRequest {
    pub volume_mode: VolumeMode,
    pub access_mode: CsiAccessMode,
    pub policy: VolumePolicy,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CapabilityValidation {
    Valid { warnings: Vec<String> },
    Invalid { reason: String },
}

pub fn validate_capability(request: &CapabilityRequest) -> CapabilityValidation {
    if request.volume_mode == VolumeMode::Block {
        return CapabilityValidation::Invalid {
            reason: "block volume mode is not supported for same-dataset storage".to_string(),
        };
    }

    match request.policy.access {
        PolicyAccessMode::ReadOnly => validate_read_only_policy(request),
        PolicyAccessMode::ReadWrite => validate_read_write_policy(request),
    }
}

fn validate_read_only_policy(request: &CapabilityRequest) -> CapabilityValidation {
    match request.access_mode {
        CsiAccessMode::SingleNodeReaderOnly | CsiAccessMode::MultiNodeReaderOnly => {
            CapabilityValidation::Valid {
                warnings: Vec::new(),
            }
        }
        _ => CapabilityValidation::Invalid {
            reason: format!("policy {} is read-only", request.policy.name),
        },
    }
}

fn validate_read_write_policy(request: &CapabilityRequest) -> CapabilityValidation {
    match request.access_mode {
        CsiAccessMode::SingleNodeWriter | CsiAccessMode::MultiNodeSingleWriter => {
            CapabilityValidation::Valid {
                warnings: Vec::new(),
            }
        }
        CsiAccessMode::MultiNodeMultiWriter if request.policy.allow_multi_node_writer => {
            CapabilityValidation::Valid {
                warnings: vec![
                    "multi-node writer uses shared filesystem semantics; application-level write conflicts remain possible".to_string(),
                ],
            }
        }
        CsiAccessMode::MultiNodeMultiWriter => CapabilityValidation::Invalid {
            reason: format!(
                "policy {} does not allow multi-node writer",
                request.policy.name
            ),
        },
        CsiAccessMode::SingleNodeReaderOnly | CsiAccessMode::MultiNodeReaderOnly => {
            CapabilityValidation::Valid {
                warnings: Vec::new(),
            }
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ControllerConfig {
    pub driver_name: String,
    pub vendor_version: String,
    pub default_capacity_bytes: i64,
    pub metadata_path: Option<PathBuf>,
    pub allow_dynamic_dataset_creation: bool,
    pub allow_authoritative_dataset_delete: bool,
    pub allow_snapshot_delete: bool,
}

impl Default for ControllerConfig {
    fn default() -> Self {
        Self {
            driver_name: DEFAULT_DRIVER_NAME.to_string(),
            vendor_version: env!("CARGO_PKG_VERSION").to_string(),
            default_capacity_bytes: 0,
            metadata_path: None,
            allow_dynamic_dataset_creation: false,
            allow_authoritative_dataset_delete: false,
            allow_snapshot_delete: true,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ControllerRuntimeConfig {
    #[serde(rename = "apiVersion", default)]
    pub api_version: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub driver_name: Option<String>,
    #[serde(default)]
    pub vendor_version: Option<String>,
    #[serde(default)]
    pub default_capacity_bytes: Option<i64>,
    #[serde(default)]
    pub metadata_path: Option<PathBuf>,
    #[serde(default)]
    pub allow_dynamic_dataset_creation: bool,
    #[serde(default)]
    pub allow_authoritative_dataset_delete: bool,
    #[serde(default = "default_allow_snapshot_delete")]
    pub allow_snapshot_delete: bool,
    #[serde(default)]
    pub truenas: Option<ControllerTrueNasConfig>,
    #[serde(default)]
    pub existing_volumes: Vec<ExistingVolumeConfig>,
}

impl Default for ControllerRuntimeConfig {
    fn default() -> Self {
        Self {
            api_version: String::new(),
            kind: String::new(),
            driver_name: None,
            vendor_version: None,
            default_capacity_bytes: None,
            metadata_path: None,
            allow_dynamic_dataset_creation: false,
            allow_authoritative_dataset_delete: false,
            allow_snapshot_delete: true,
            truenas: None,
            existing_volumes: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ControllerTrueNasConfig {
    pub url: String,
    pub api_key_file: PathBuf,
    #[serde(default = "default_connect_timeout_seconds")]
    pub connect_timeout_seconds: u64,
    #[serde(default = "default_request_timeout_seconds")]
    pub request_timeout_seconds: u64,
    #[serde(default = "default_max_retries")]
    pub max_retries: u8,
    #[serde(default)]
    pub accept_invalid_certs: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExistingVolumeConfig {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    pub dataset: String,
    #[serde(default)]
    pub source_path: Option<String>,
    pub policy: String,
    pub access: PolicyAccessMode,
    #[serde(default)]
    pub allow_multi_node_writer: bool,
}

impl ControllerRuntimeConfig {
    pub fn to_controller_config(&self) -> ControllerConfig {
        ControllerConfig {
            driver_name: self
                .driver_name
                .clone()
                .unwrap_or_else(|| DEFAULT_DRIVER_NAME.to_string()),
            vendor_version: self
                .vendor_version
                .clone()
                .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string()),
            default_capacity_bytes: self.default_capacity_bytes.unwrap_or(0),
            metadata_path: self.metadata_path.clone(),
            allow_dynamic_dataset_creation: self.allow_dynamic_dataset_creation,
            allow_authoritative_dataset_delete: self.allow_authoritative_dataset_delete,
            allow_snapshot_delete: self.allow_snapshot_delete,
        }
    }
}

fn default_allow_snapshot_delete() -> bool {
    true
}

fn default_connect_timeout_seconds() -> u64 {
    10
}

fn default_request_timeout_seconds() -> u64 {
    30
}

fn default_max_retries() -> u8 {
    2
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct ControllerState {
    pub volumes: BTreeMap<String, DatasetVolume>,
    pub snapshots: BTreeMap<String, SnapshotMetadata>,
    pub assignments: BTreeMap<String, PublishAssignment>,
}

impl ControllerState {
    pub fn with_volume(mut self, volume: DatasetVolume) -> Self {
        self.volumes.insert(volume.id.clone(), volume);
        self
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DatasetVolume {
    pub id: String,
    pub name: String,
    pub dataset: String,
    pub source_path: String,
    pub tag: String,
    pub policy: VolumePolicy,
    pub capacity_bytes: i64,
    pub dynamically_created: bool,
    pub delete_dataset_on_delete: bool,
    pub smb_shares: Vec<SmbShareMetadata>,
    pub snapshots: Vec<SnapshotMetadata>,
    pub retention: Option<RetentionReplicationMetadata>,
}

impl DatasetVolume {
    pub fn existing_filesystem(
        id: impl Into<String>,
        dataset: impl Into<String>,
        source_path: impl Into<String>,
        policy: VolumePolicy,
    ) -> Self {
        let id = id.into();
        let dataset = dataset.into();
        Self {
            name: id.clone(),
            tag: format!("nascsi_{}", safe_identifier(&id)),
            id,
            dataset,
            source_path: source_path.into(),
            policy,
            capacity_bytes: 0,
            dynamically_created: false,
            delete_dataset_on_delete: false,
            smb_shares: Vec::new(),
            snapshots: Vec::new(),
            retention: None,
        }
    }

    fn to_csi_volume(&self) -> csi::Volume {
        let mut context = HashMap::new();
        context.insert("nas-csi.dev/dataset".to_string(), self.dataset.clone());
        context.insert(
            "nas-csi.dev/sourcePath".to_string(),
            self.source_path.clone(),
        );
        context.insert("nas-csi.dev/tag".to_string(), self.tag.clone());
        context.insert("nas-csi.dev/policy".to_string(), self.policy.name.clone());
        context.insert(
            "nas-csi.dev/access".to_string(),
            match self.policy.access {
                PolicyAccessMode::ReadWrite => "read-write",
                PolicyAccessMode::ReadOnly => "read-only",
            }
            .to_string(),
        );
        if !self.smb_shares.is_empty() {
            context.insert(
                "nas-csi.dev/smbShares".to_string(),
                self.smb_shares
                    .iter()
                    .map(|share| share.name.as_str())
                    .collect::<Vec<_>>()
                    .join(","),
            );
        }
        if let Some(retention) = &self.retention {
            context.insert(
                "nas-csi.dev/retentionPolicy".to_string(),
                retention.retention_policy.clone(),
            );
            context.insert(
                "nas-csi.dev/replicationPolicy".to_string(),
                retention.replication_policy.clone(),
            );
        }
        csi::Volume {
            volume_id: self.id.clone(),
            capacity_bytes: self.capacity_bytes,
            volume_context: context,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct SmbShareMetadata {
    pub name: String,
    pub path: String,
    pub enabled: bool,
    pub managed: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct SnapshotMetadata {
    pub id: String,
    pub source_volume_id: String,
    pub name: String,
    pub size_bytes: i64,
    pub creation_time_unix_seconds: i64,
    pub ready_to_use: bool,
}

impl SnapshotMetadata {
    fn to_csi_snapshot(&self) -> csi::Snapshot {
        csi::Snapshot {
            snapshot_id: self.id.clone(),
            source_volume_id: self.source_volume_id.clone(),
            size_bytes: self.size_bytes,
            creation_time_unix_seconds: self.creation_time_unix_seconds,
            ready_to_use: self.ready_to_use,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct RetentionReplicationMetadata {
    pub retention_policy: String,
    pub replication_policy: String,
    pub snapshot_prefix: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct PublishAssignment {
    pub volume_id: String,
    pub node_id: String,
    pub read_only: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DatasetBackendMetadata {
    pub dataset: String,
    pub source_path: String,
    pub smb_shares: Vec<SmbShareMetadata>,
    pub snapshots: Vec<SnapshotMetadata>,
    pub retention: Option<RetentionReplicationMetadata>,
}

pub trait ControllerBackend: Send + Sync {
    fn lookup_filesystem_dataset(
        &self,
        dataset: &str,
    ) -> Result<Option<DatasetBackendMetadata>, ControllerBackendError>;
    fn create_filesystem_dataset(&self, dataset: &str) -> Result<(), ControllerBackendError>;
    fn delete_filesystem_dataset(&self, dataset: &str) -> Result<(), ControllerBackendError>;
    fn create_snapshot(
        &self,
        dataset: &str,
        snapshot_name: &str,
    ) -> Result<String, ControllerBackendError>;
    fn delete_snapshot(&self, snapshot_id: &str) -> Result<(), ControllerBackendError>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControllerBackendError {
    pub message: String,
}

impl fmt::Display for ControllerBackendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ControllerBackendError {}

#[derive(Default)]
pub struct NoopControllerBackend;

impl ControllerBackend for NoopControllerBackend {
    fn lookup_filesystem_dataset(
        &self,
        _dataset: &str,
    ) -> Result<Option<DatasetBackendMetadata>, ControllerBackendError> {
        Ok(None)
    }

    fn create_filesystem_dataset(&self, _dataset: &str) -> Result<(), ControllerBackendError> {
        Ok(())
    }

    fn delete_filesystem_dataset(&self, _dataset: &str) -> Result<(), ControllerBackendError> {
        Ok(())
    }

    fn create_snapshot(
        &self,
        dataset: &str,
        snapshot_name: &str,
    ) -> Result<String, ControllerBackendError> {
        Ok(format!("{dataset}@{snapshot_name}"))
    }

    fn delete_snapshot(&self, _snapshot_id: &str) -> Result<(), ControllerBackendError> {
        Ok(())
    }
}

pub struct TrueNasApiBackend {
    client: Mutex<JsonRpcClient<TrueNasWebSocketTransport>>,
}

impl TrueNasApiBackend {
    pub fn from_config(config: &ControllerTrueNasConfig) -> Result<Self, ControllerConfigError> {
        let api_key = fs::read_to_string(&config.api_key_file)
            .map_err(|error| ControllerConfigError::Io {
                path: config.api_key_file.clone(),
                message: error.to_string(),
            })?
            .trim()
            .to_string();
        if api_key.is_empty() {
            return Err(ControllerConfigError::Invalid(
                "TrueNAS apiKeyFile is empty".to_string(),
            ));
        }

        let mut transport_config = TrueNasWebSocketConfig::new(config.url.clone(), api_key);
        transport_config.connect_timeout = Duration::from_secs(config.connect_timeout_seconds);
        transport_config.request_timeout = Duration::from_secs(config.request_timeout_seconds);
        transport_config.max_retries = config.max_retries;
        transport_config.accept_invalid_certs = config.accept_invalid_certs;
        Ok(Self {
            client: Mutex::new(JsonRpcClient::new(TrueNasWebSocketTransport::new(
                transport_config,
            ))),
        })
    }

    fn with_client<R>(
        &self,
        f: impl FnOnce(
            &mut JsonRpcClient<TrueNasWebSocketTransport>,
        ) -> Result<R, nas_csi_truenas_client::ClientError>,
    ) -> Result<R, ControllerBackendError> {
        let mut client = self.client.lock().expect("TrueNAS client mutex poisoned");
        f(&mut client).map_err(|error| ControllerBackendError {
            message: error.to_string(),
        })
    }
}

impl ControllerBackend for TrueNasApiBackend {
    fn lookup_filesystem_dataset(
        &self,
        dataset: &str,
    ) -> Result<Option<DatasetBackendMetadata>, ControllerBackendError> {
        self.with_client(|client| {
            let datasets = client.pool_dataset_query()?;
            let Some(record) = datasets.into_iter().find(|record| {
                (record.name == dataset || record.id == dataset)
                    && record.kind_value().is_none_or(|kind| kind == "FILESYSTEM")
            }) else {
                return Ok(None);
            };
            let source_path = record
                .mountpoint_value()
                .map(str::to_string)
                .unwrap_or_else(|| format!("/mnt/{dataset}"));
            let shares = client
                .sharing_smb_query()?
                .into_iter()
                .filter(|share| share.path == source_path)
                .map(|share| SmbShareMetadata {
                    name: share.name,
                    path: share.path,
                    enabled: share.enabled.unwrap_or(false),
                    managed: false,
                })
                .collect::<Vec<_>>();
            let snapshots = client
                .pool_snapshot_query()?
                .into_iter()
                .filter(|snapshot| {
                    snapshot.dataset.as_deref() == Some(dataset)
                        || snapshot.id.starts_with(&format!("{dataset}@"))
                })
                .map(|snapshot| SnapshotMetadata {
                    id: snapshot.id,
                    source_volume_id: dataset.to_string(),
                    name: snapshot.name,
                    size_bytes: 0,
                    creation_time_unix_seconds: 0,
                    ready_to_use: true,
                })
                .collect();
            Ok(Some(DatasetBackendMetadata {
                dataset: record.name,
                source_path,
                smb_shares: shares,
                snapshots,
                retention: None,
            }))
        })
    }

    fn create_filesystem_dataset(&self, dataset: &str) -> Result<(), ControllerBackendError> {
        self.with_client(|client| client.pool_dataset_create(dataset).map(|_| ()))
    }

    fn delete_filesystem_dataset(&self, dataset: &str) -> Result<(), ControllerBackendError> {
        self.with_client(|client| client.pool_dataset_delete(dataset, false).map(|_| ()))
    }

    fn create_snapshot(
        &self,
        dataset: &str,
        snapshot_name: &str,
    ) -> Result<String, ControllerBackendError> {
        self.with_client(|client| {
            client
                .pool_snapshot_create(dataset, snapshot_name, false)
                .map(|snapshot| snapshot.id)
        })
    }

    fn delete_snapshot(&self, snapshot_id: &str) -> Result<(), ControllerBackendError> {
        self.with_client(|client| client.pool_snapshot_delete(snapshot_id).map(|_| ()))
    }
}

#[derive(Clone)]
pub struct NasCsiControllerService {
    config: ControllerConfig,
    state: Arc<Mutex<ControllerState>>,
    backend: Arc<dyn ControllerBackend>,
}

impl NasCsiControllerService {
    pub fn new(config: ControllerConfig, state: ControllerState) -> Self {
        Self {
            config,
            state: Arc::new(Mutex::new(state)),
            backend: Arc::new(NoopControllerBackend),
        }
    }

    pub fn with_backend(
        config: ControllerConfig,
        state: ControllerState,
        backend: Arc<dyn ControllerBackend>,
    ) -> Self {
        Self {
            config,
            state: Arc::new(Mutex::new(state)),
            backend,
        }
    }

    pub fn from_runtime_config(
        config: ControllerRuntimeConfig,
    ) -> Result<Self, ControllerConfigError> {
        let controller_config = config.to_controller_config();
        let backend: Arc<dyn ControllerBackend> = match &config.truenas {
            Some(truenas) => Arc::new(TrueNasApiBackend::from_config(truenas)?),
            None => Arc::new(NoopControllerBackend),
        };
        let state = load_controller_state(controller_config.metadata_path.as_deref())?;
        let service = Self::with_backend(controller_config, state, backend);
        service.register_existing_volumes_from_config(&config.existing_volumes)?;
        Ok(service)
    }

    pub fn snapshot_state(&self) -> ControllerState {
        self.state
            .lock()
            .expect("controller state poisoned")
            .clone()
    }

    pub fn register_existing_volumes_from_config(
        &self,
        volumes: &[ExistingVolumeConfig],
    ) -> Result<(), ControllerConfigError> {
        for volume in volumes {
            self.register_existing_volume(volume)
                .map_err(|error| ControllerConfigError::Invalid(error.to_string()))?;
        }
        Ok(())
    }

    fn register_existing_volume(&self, config: &ExistingVolumeConfig) -> Result<(), Status> {
        if config.id.trim().is_empty() {
            return Err(Status::invalid_argument(
                "existing volume id must not be empty",
            ));
        }
        let metadata = self
            .backend
            .lookup_filesystem_dataset(&config.dataset)
            .map_err(|error| Status::internal(error.to_string()))?;
        let source_path = config
            .source_path
            .clone()
            .or_else(|| {
                metadata
                    .as_ref()
                    .map(|metadata| metadata.source_path.clone())
            })
            .unwrap_or_else(|| format!("/mnt/{}", config.dataset));
        let policy = VolumePolicy {
            name: config.policy.clone(),
            access: config.access,
            allow_multi_node_writer: config.allow_multi_node_writer,
        };
        let mut volume = DatasetVolume::existing_filesystem(
            config.id.clone(),
            metadata
                .as_ref()
                .map(|metadata| metadata.dataset.clone())
                .unwrap_or_else(|| config.dataset.clone()),
            source_path,
            policy,
        );
        volume.name = config.name.clone().unwrap_or_else(|| config.id.clone());
        if let Some(metadata) = metadata {
            volume.smb_shares = metadata.smb_shares;
            volume.snapshots = metadata.snapshots;
            volume.retention = metadata.retention;
        }
        let mut state = self.state.lock().expect("controller state poisoned");
        state.volumes.insert(volume.id.clone(), volume);
        self.persist_state_locked(&state)?;
        Ok(())
    }

    fn find_volume_for_create(
        state: &ControllerState,
        name: &str,
        dataset: Option<&str>,
    ) -> Option<DatasetVolume> {
        state
            .volumes
            .values()
            .find(|volume| {
                volume.name == name
                    || volume.id == name
                    || dataset.is_some_and(|dataset| dataset == volume.dataset)
            })
            .cloned()
    }

    fn persist_state_locked(&self, state: &ControllerState) -> Result<(), Status> {
        persist_controller_state(self.config.metadata_path.as_deref(), state)
            .map_err(|error| Status::internal(error.to_string()))
    }

    fn create_volume_inner(
        &self,
        request: csi::CreateVolumeRequest,
    ) -> Result<csi::Volume, Status> {
        if request.name.trim().is_empty() {
            return Err(Status::invalid_argument("volume name must not be empty"));
        }
        let dataset = request
            .parameters
            .get("nas-csi.dev/dataset")
            .or_else(|| request.parameters.get("dataset"))
            .map(String::as_str);

        {
            let state = self.state.lock().expect("controller state poisoned");
            if let Some(volume) = Self::find_volume_for_create(&state, &request.name, dataset) {
                validate_csi_capabilities(&volume, &request.volume_capabilities)?;
                return Ok(volume.to_csi_volume());
            }
        }

        if is_existing_dataset_request(&request.parameters) {
            let dataset = dataset.ok_or_else(|| {
                Status::invalid_argument(
                    "existing dataset registration requires nas-csi.dev/dataset",
                )
            })?;
            let volume = self.register_existing_volume_from_request(&request, dataset)?;
            validate_csi_capabilities(&volume, &request.volume_capabilities)?;
            return Ok(volume.to_csi_volume());
        }

        if !self.config.allow_dynamic_dataset_creation
            || !is_dynamic_dataset_request(&request.parameters)
        {
            return Err(Status::not_found(
                "volume is not mapped to an existing TrueNAS filesystem dataset",
            ));
        }

        let dataset = dataset.ok_or_else(|| {
            Status::invalid_argument("dynamic dataset creation requires nas-csi.dev/dataset")
        })?;
        self.backend
            .create_filesystem_dataset(dataset)
            .map_err(|error| Status::internal(error.to_string()))?;

        let policy = VolumePolicy {
            name: request
                .parameters
                .get("nas-csi.dev/policy")
                .cloned()
                .unwrap_or_else(|| "dynamic".to_string()),
            access: PolicyAccessMode::ReadWrite,
            allow_multi_node_writer: request
                .parameters
                .get("nas-csi.dev/allowMultiNodeWriter")
                .is_some_and(|value| value == "true"),
        };
        let source_path = request
            .parameters
            .get("nas-csi.dev/sourcePath")
            .cloned()
            .unwrap_or_else(|| format!("/mnt/{dataset}"));
        let mut volume = DatasetVolume::existing_filesystem(
            request.name.clone(),
            dataset.to_string(),
            source_path,
            policy,
        );
        volume.dynamically_created = true;
        volume.delete_dataset_on_delete = self.config.allow_authoritative_dataset_delete
            && request
                .parameters
                .get("nas-csi.dev/deleteDatasetOnDelete")
                .is_some_and(|value| value == "true");
        volume.capacity_bytes = requested_capacity(request.capacity_range.as_ref())
            .unwrap_or(self.config.default_capacity_bytes);
        validate_csi_capabilities(&volume, &request.volume_capabilities)?;

        let csi_volume = volume.to_csi_volume();
        let mut state = self.state.lock().expect("controller state poisoned");
        state.volumes.insert(volume.id.clone(), volume);
        self.persist_state_locked(&state)?;
        Ok(csi_volume)
    }

    fn register_existing_volume_from_request(
        &self,
        request: &csi::CreateVolumeRequest,
        dataset: &str,
    ) -> Result<DatasetVolume, Status> {
        let metadata = self
            .backend
            .lookup_filesystem_dataset(dataset)
            .map_err(|error| Status::internal(error.to_string()))?
            .ok_or_else(|| Status::not_found("TrueNAS filesystem dataset not found"))?;
        let policy = policy_from_parameters(&request.parameters, PolicyAccessMode::ReadWrite);
        let mut volume = DatasetVolume::existing_filesystem(
            request.name.clone(),
            metadata.dataset,
            metadata.source_path,
            policy,
        );
        volume.smb_shares = metadata.smb_shares;
        volume.snapshots = metadata.snapshots;
        volume.retention = metadata.retention;
        volume.capacity_bytes = requested_capacity(request.capacity_range.as_ref())
            .unwrap_or(self.config.default_capacity_bytes);
        volume.delete_dataset_on_delete = false;

        let mut state = self.state.lock().expect("controller state poisoned");
        state.volumes.insert(volume.id.clone(), volume.clone());
        self.persist_state_locked(&state)?;
        Ok(volume)
    }

    fn delete_volume_inner(&self, volume_id: &str) -> Result<(), Status> {
        if volume_id.trim().is_empty() {
            return Ok(());
        }

        let mut state = self.state.lock().expect("controller state poisoned");
        let Some(volume) = state.volumes.remove(volume_id) else {
            return Ok(());
        };

        state
            .assignments
            .retain(|_, assignment| assignment.volume_id != volume_id);

        if volume.delete_dataset_on_delete && self.config.allow_authoritative_dataset_delete {
            self.backend
                .delete_filesystem_dataset(&volume.dataset)
                .map_err(|error| Status::internal(error.to_string()))?;
        }
        self.persist_state_locked(&state)?;
        Ok(())
    }
}

#[tonic::async_trait]
impl csi::identity_server::Identity for NasCsiControllerService {
    async fn get_plugin_info(
        &self,
        _request: Request<csi::GetPluginInfoRequest>,
    ) -> Result<Response<csi::GetPluginInfoResponse>, Status> {
        Ok(Response::new(csi::GetPluginInfoResponse {
            name: self.config.driver_name.clone(),
            vendor_version: self.config.vendor_version.clone(),
            manifest: HashMap::from([(
                "nas-csi.dev/storage".to_string(),
                "truenas-virtiofs".to_string(),
            )]),
        }))
    }

    async fn get_plugin_capabilities(
        &self,
        _request: Request<csi::GetPluginCapabilitiesRequest>,
    ) -> Result<Response<csi::GetPluginCapabilitiesResponse>, Status> {
        Ok(Response::new(csi::GetPluginCapabilitiesResponse {
            capabilities: vec![csi::PluginCapability {
                service: csi::plugin_capability::ServiceType::ControllerService as i32,
            }],
        }))
    }

    async fn probe(
        &self,
        _request: Request<csi::ProbeRequest>,
    ) -> Result<Response<csi::ProbeResponse>, Status> {
        Ok(Response::new(csi::ProbeResponse { ready: true }))
    }
}

fn log_controller_operation_start(operation: &str, fields: Vec<(&'static str, serde_json::Value)>) {
    log_controller_operation(operation, "start", fields, None);
}

fn log_controller_operation_success(
    operation: &str,
    fields: Vec<(&'static str, serde_json::Value)>,
) {
    log_controller_operation(operation, "success", fields, None);
}

fn log_controller_operation_failure(
    operation: &str,
    fields: Vec<(&'static str, serde_json::Value)>,
    status: &Status,
) {
    log_controller_operation(operation, "failure", fields, Some(status));
}

fn log_controller_operation(
    operation: &str,
    result: &str,
    fields: Vec<(&'static str, serde_json::Value)>,
    status: Option<&Status>,
) {
    let mut log = serde_json::Map::new();
    log.insert("event".to_string(), "csi_controller_operation".into());
    log.insert("component".to_string(), "nas-csi-controller".into());
    log.insert("operation".to_string(), operation.into());
    log.insert("result".to_string(), result.into());
    for (key, value) in fields {
        log.insert(key.to_string(), value);
    }
    if let Some(status) = status {
        log.insert(
            "statusCode".to_string(),
            format!("{:?}", status.code()).into(),
        );
        log.insert("error".to_string(), status.message().into());
    }
    eprintln!("{}", serde_json::Value::Object(log));
}

#[tonic::async_trait]
impl csi::controller_server::Controller for NasCsiControllerService {
    async fn create_volume(
        &self,
        request: Request<csi::CreateVolumeRequest>,
    ) -> Result<Response<csi::CreateVolumeResponse>, Status> {
        let request = request.into_inner();
        let volume_name = request.name.clone();
        let requested_dataset = request
            .parameters
            .get("nas-csi.dev/dataset")
            .or_else(|| request.parameters.get("dataset"))
            .cloned()
            .unwrap_or_default();
        log_controller_operation_start(
            "create_volume",
            vec![
                ("volumeName", volume_name.clone().into()),
                ("dataset", requested_dataset.clone().into()),
            ],
        );
        match self.create_volume_inner(request) {
            Ok(volume) => {
                log_controller_operation_success(
                    "create_volume",
                    vec![
                        ("volumeName", volume_name.into()),
                        ("dataset", requested_dataset.into()),
                        ("volumeId", volume.volume_id.clone().into()),
                    ],
                );
                Ok(Response::new(csi::CreateVolumeResponse {
                    volume: Some(volume),
                }))
            }
            Err(status) => {
                log_controller_operation_failure(
                    "create_volume",
                    vec![
                        ("volumeName", volume_name.into()),
                        ("dataset", requested_dataset.into()),
                    ],
                    &status,
                );
                Err(status)
            }
        }
    }

    async fn delete_volume(
        &self,
        request: Request<csi::DeleteVolumeRequest>,
    ) -> Result<Response<csi::DeleteVolumeResponse>, Status> {
        let volume_id = request.into_inner().volume_id;
        log_controller_operation_start(
            "delete_volume",
            vec![("volumeId", volume_id.clone().into())],
        );
        match self.delete_volume_inner(&volume_id) {
            Ok(()) => {
                log_controller_operation_success(
                    "delete_volume",
                    vec![("volumeId", volume_id.into())],
                );
                Ok(Response::new(csi::DeleteVolumeResponse {}))
            }
            Err(status) => {
                log_controller_operation_failure(
                    "delete_volume",
                    vec![("volumeId", volume_id.into())],
                    &status,
                );
                Err(status)
            }
        }
    }

    async fn controller_publish_volume(
        &self,
        request: Request<csi::ControllerPublishVolumeRequest>,
    ) -> Result<Response<csi::ControllerPublishVolumeResponse>, Status> {
        let request = request.into_inner();
        let volume_id = request.volume_id.clone();
        let node_id = request.node_id.clone();
        let requested_read_only = request.readonly;
        let identifiers = || {
            vec![
                ("volumeId", volume_id.clone().into()),
                ("nodeId", node_id.clone().into()),
                ("readOnly", requested_read_only.into()),
            ]
        };
        log_controller_operation_start("controller_publish_volume", identifiers());
        let result = (|| -> Result<HashMap<String, String>, Status> {
            if request.node_id.trim().is_empty() {
                return Err(Status::invalid_argument("node_id must not be empty"));
            }
            let mut state = self.state.lock().expect("controller state poisoned");
            let volume = state
                .volumes
                .get(&request.volume_id)
                .ok_or_else(|| Status::not_found("volume not found"))?
                .clone();
            validate_csi_capabilities(&volume, &request.volume_capability)?;
            state.assignments.insert(
                assignment_key(&request.volume_id, &request.node_id),
                PublishAssignment {
                    volume_id: request.volume_id.clone(),
                    node_id: request.node_id.clone(),
                    read_only: request.readonly
                        || volume.policy.access == PolicyAccessMode::ReadOnly,
                },
            );
            self.persist_state_locked(&state)?;

            Ok(HashMap::from([
                ("nas-csi.dev/exportId".to_string(), volume.id.clone()),
                ("nas-csi.dev/dataset".to_string(), volume.dataset.clone()),
                (
                    "nas-csi.dev/sourcePath".to_string(),
                    volume.source_path.clone(),
                ),
                ("nas-csi.dev/tag".to_string(), volume.tag.clone()),
                (
                    "nas-csi.dev/readOnly".to_string(),
                    (request.readonly || volume.policy.access == PolicyAccessMode::ReadOnly)
                        .to_string(),
                ),
            ]))
        })();
        match result {
            Ok(publish_context) => {
                let effective_read_only = publish_context
                    .get("nas-csi.dev/readOnly")
                    .is_some_and(|value| value == "true");
                log_controller_operation_success(
                    "controller_publish_volume",
                    vec![
                        ("volumeId", volume_id.into()),
                        ("nodeId", node_id.into()),
                        ("readOnly", requested_read_only.into()),
                        ("effectiveReadOnly", effective_read_only.into()),
                    ],
                );
                Ok(Response::new(csi::ControllerPublishVolumeResponse {
                    publish_context,
                }))
            }
            Err(status) => {
                log_controller_operation_failure(
                    "controller_publish_volume",
                    identifiers(),
                    &status,
                );
                Err(status)
            }
        }
    }

    async fn controller_unpublish_volume(
        &self,
        request: Request<csi::ControllerUnpublishVolumeRequest>,
    ) -> Result<Response<csi::ControllerUnpublishVolumeResponse>, Status> {
        let request = request.into_inner();
        let volume_id = request.volume_id;
        let node_id = request.node_id;
        let identifiers = || {
            vec![
                ("volumeId", volume_id.clone().into()),
                ("nodeId", node_id.clone().into()),
            ]
        };
        log_controller_operation_start("controller_unpublish_volume", identifiers());
        let result = (|| -> Result<(), Status> {
            let mut state = self.state.lock().expect("controller state poisoned");
            state
                .assignments
                .remove(&assignment_key(&volume_id, &node_id));
            self.persist_state_locked(&state)
        })();
        match result {
            Ok(()) => {
                log_controller_operation_success("controller_unpublish_volume", identifiers());
                Ok(Response::new(csi::ControllerUnpublishVolumeResponse {}))
            }
            Err(status) => {
                log_controller_operation_failure(
                    "controller_unpublish_volume",
                    identifiers(),
                    &status,
                );
                Err(status)
            }
        }
    }

    async fn validate_volume_capabilities(
        &self,
        request: Request<csi::ValidateVolumeCapabilitiesRequest>,
    ) -> Result<Response<csi::ValidateVolumeCapabilitiesResponse>, Status> {
        let request = request.into_inner();
        let volume_id = request.volume_id.clone();
        log_controller_operation_start(
            "validate_volume_capabilities",
            vec![("volumeId", volume_id.clone().into())],
        );
        let result = (|| -> Result<(bool, csi::ValidateVolumeCapabilitiesResponse), Status> {
            let state = self.state.lock().expect("controller state poisoned");
            let volume = state
                .volumes
                .get(&request.volume_id)
                .ok_or_else(|| Status::not_found("volume not found"))?;
            match validate_csi_capabilities(volume, &request.volume_capabilities) {
                Ok(()) => Ok((
                    true,
                    csi::ValidateVolumeCapabilitiesResponse {
                        confirmed: Some(csi::validate_volume_capabilities_response::Confirmed {
                            volume_context_id: volume.id.clone(),
                            volume_capabilities: request.volume_capabilities,
                        }),
                        message: String::new(),
                    },
                )),
                Err(status) => Ok((
                    false,
                    csi::ValidateVolumeCapabilitiesResponse {
                        confirmed: None,
                        message: status.message().to_string(),
                    },
                )),
            }
        })();
        match result {
            Ok((confirmed, response)) => {
                log_controller_operation_success(
                    "validate_volume_capabilities",
                    vec![
                        ("volumeId", volume_id.into()),
                        ("confirmed", confirmed.into()),
                    ],
                );
                Ok(Response::new(response))
            }
            Err(status) => {
                log_controller_operation_failure(
                    "validate_volume_capabilities",
                    vec![("volumeId", volume_id.into())],
                    &status,
                );
                Err(status)
            }
        }
    }

    async fn list_volumes(
        &self,
        _request: Request<csi::ListVolumesRequest>,
    ) -> Result<Response<csi::ListVolumesResponse>, Status> {
        log_controller_operation_start("list_volumes", Vec::new());
        let state = self.state.lock().expect("controller state poisoned");
        let entries = state
            .volumes
            .values()
            .map(|volume| csi::list_volumes_response::Entry {
                volume: Some(volume.to_csi_volume()),
            })
            .collect::<Vec<_>>();
        log_controller_operation_success(
            "list_volumes",
            vec![("count", (entries.len() as u64).into())],
        );
        Ok(Response::new(csi::ListVolumesResponse {
            entries,
            next_token: String::new(),
        }))
    }

    async fn get_capacity(
        &self,
        _request: Request<csi::GetCapacityRequest>,
    ) -> Result<Response<csi::GetCapacityResponse>, Status> {
        Ok(Response::new(csi::GetCapacityResponse {
            available_capacity: self.config.default_capacity_bytes,
        }))
    }

    async fn controller_get_capabilities(
        &self,
        _request: Request<csi::ControllerGetCapabilitiesRequest>,
    ) -> Result<Response<csi::ControllerGetCapabilitiesResponse>, Status> {
        use csi::controller_service_capability::Type;
        Ok(Response::new(csi::ControllerGetCapabilitiesResponse {
            capabilities: vec![
                Type::CreateDeleteVolume,
                Type::PublishUnpublishVolume,
                Type::ListVolumes,
                Type::GetCapacity,
                Type::CreateDeleteSnapshot,
                Type::ListSnapshots,
            ]
            .into_iter()
            .map(|r#type| csi::ControllerServiceCapability {
                r#type: r#type as i32,
            })
            .collect(),
        }))
    }

    async fn create_snapshot(
        &self,
        request: Request<csi::CreateSnapshotRequest>,
    ) -> Result<Response<csi::CreateSnapshotResponse>, Status> {
        let request = request.into_inner();
        let source_volume_id = request.source_volume_id.clone();
        let snapshot_name = request.name.clone();
        let identifiers = || {
            vec![
                ("sourceVolumeId", source_volume_id.clone().into()),
                ("snapshotName", snapshot_name.clone().into()),
            ]
        };
        log_controller_operation_start("create_snapshot", identifiers());
        let result = (|| -> Result<SnapshotMetadata, Status> {
            let mut state = self.state.lock().expect("controller state poisoned");
            if let Some(snapshot) = state.snapshots.values().find(|snapshot| {
                snapshot.source_volume_id == request.source_volume_id
                    && snapshot.name == request.name
            }) {
                return Ok(snapshot.clone());
            }
            let volume = state
                .volumes
                .get(&request.source_volume_id)
                .ok_or_else(|| Status::not_found("source volume not found"))?
                .clone();
            let snapshot_id = self
                .backend
                .create_snapshot(&volume.dataset, &request.name)
                .map_err(|error| Status::internal(error.to_string()))?;
            let snapshot = SnapshotMetadata {
                id: snapshot_id,
                source_volume_id: volume.id,
                name: request.name,
                size_bytes: volume.capacity_bytes,
                creation_time_unix_seconds: 0,
                ready_to_use: true,
            };
            state
                .snapshots
                .insert(snapshot.id.clone(), snapshot.clone());
            self.persist_state_locked(&state)?;
            Ok(snapshot)
        })();
        match result {
            Ok(snapshot) => {
                log_controller_operation_success(
                    "create_snapshot",
                    vec![
                        ("sourceVolumeId", source_volume_id.into()),
                        ("snapshotName", snapshot_name.into()),
                        ("snapshotId", snapshot.id.clone().into()),
                    ],
                );
                Ok(Response::new(csi::CreateSnapshotResponse {
                    snapshot: Some(snapshot.to_csi_snapshot()),
                }))
            }
            Err(status) => {
                log_controller_operation_failure("create_snapshot", identifiers(), &status);
                Err(status)
            }
        }
    }

    async fn delete_snapshot(
        &self,
        request: Request<csi::DeleteSnapshotRequest>,
    ) -> Result<Response<csi::DeleteSnapshotResponse>, Status> {
        let snapshot_id = request.into_inner().snapshot_id;
        log_controller_operation_start(
            "delete_snapshot",
            vec![("snapshotId", snapshot_id.clone().into())],
        );
        let result = (|| -> Result<(), Status> {
            if snapshot_id.trim().is_empty() {
                return Ok(());
            }
            if self.config.allow_snapshot_delete {
                self.backend
                    .delete_snapshot(&snapshot_id)
                    .map_err(|error| Status::internal(error.to_string()))?;
            }
            let mut state = self.state.lock().expect("controller state poisoned");
            state.snapshots.remove(&snapshot_id);
            self.persist_state_locked(&state)
        })();
        match result {
            Ok(()) => {
                log_controller_operation_success(
                    "delete_snapshot",
                    vec![("snapshotId", snapshot_id.into())],
                );
                Ok(Response::new(csi::DeleteSnapshotResponse {}))
            }
            Err(status) => {
                log_controller_operation_failure(
                    "delete_snapshot",
                    vec![("snapshotId", snapshot_id.into())],
                    &status,
                );
                Err(status)
            }
        }
    }

    async fn list_snapshots(
        &self,
        request: Request<csi::ListSnapshotsRequest>,
    ) -> Result<Response<csi::ListSnapshotsResponse>, Status> {
        let source_volume_id = request.into_inner().source_volume_id;
        log_controller_operation_start(
            "list_snapshots",
            vec![("sourceVolumeId", source_volume_id.clone().into())],
        );
        let state = self.state.lock().expect("controller state poisoned");
        let entries = state
            .snapshots
            .values()
            .filter(|snapshot| {
                source_volume_id.is_empty() || snapshot.source_volume_id == source_volume_id
            })
            .map(|snapshot| csi::list_snapshots_response::Entry {
                snapshot: Some(snapshot.to_csi_snapshot()),
            })
            .collect::<Vec<_>>();
        log_controller_operation_success(
            "list_snapshots",
            vec![
                ("sourceVolumeId", source_volume_id.into()),
                ("count", (entries.len() as u64).into()),
            ],
        );
        Ok(Response::new(csi::ListSnapshotsResponse { entries }))
    }
}

pub fn controller_servers(
    service: NasCsiControllerService,
) -> (
    csi::identity_server::IdentityServer<NasCsiControllerService>,
    csi::controller_server::ControllerServer<NasCsiControllerService>,
) {
    (
        csi::identity_server::IdentityServer::new(service.clone()),
        csi::controller_server::ControllerServer::new(service),
    )
}

#[derive(Debug)]
pub enum ControllerServeError {
    Io(std::io::Error),
    Transport(tonic::transport::Error),
}

impl fmt::Display for ControllerServeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "I/O error: {error}"),
            Self::Transport(error) => write!(f, "transport error: {error}"),
        }
    }
}

impl std::error::Error for ControllerServeError {}

pub async fn serve_controller_uds(
    socket_path: &Path,
    service: NasCsiControllerService,
) -> Result<(), ControllerServeError> {
    match std::fs::remove_file(socket_path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(ControllerServeError::Io(error)),
    }
    let listener = UnixListener::bind(socket_path).map_err(ControllerServeError::Io)?;
    let (identity, controller) = controller_servers(service);
    Server::builder()
        .add_service(identity)
        .add_service(controller)
        .serve_with_incoming(UnixListenerStream::new(listener))
        .await
        .map_err(ControllerServeError::Transport)
}

#[derive(Debug)]
pub enum ControllerConfigError {
    Io { path: PathBuf, message: String },
    Parse { path: PathBuf, message: String },
    Invalid(String),
}

impl fmt::Display for ControllerConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, message } => write!(f, "{}: {message}", path.display()),
            Self::Parse { path, message } => write!(f, "{}: {message}", path.display()),
            Self::Invalid(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for ControllerConfigError {}

pub fn load_controller_runtime_config(
    path: &Path,
) -> Result<ControllerRuntimeConfig, ControllerConfigError> {
    let content = fs::read_to_string(path).map_err(|error| ControllerConfigError::Io {
        path: path.to_path_buf(),
        message: error.to_string(),
    })?;
    let config: ControllerRuntimeConfig =
        serde_yml::from_str(&content).map_err(|error| ControllerConfigError::Parse {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
    validate_controller_runtime_config(&config)?;
    Ok(config)
}

pub fn controller_service_from_config_path(
    path: &Path,
) -> Result<NasCsiControllerService, ControllerConfigError> {
    NasCsiControllerService::from_runtime_config(load_controller_runtime_config(path)?)
}

pub fn validate_controller_runtime_config(
    config: &ControllerRuntimeConfig,
) -> Result<(), ControllerConfigError> {
    if !config.kind.is_empty() && config.kind != "ControllerConfig" {
        return Err(ControllerConfigError::Invalid(
            "kind must be ControllerConfig".to_string(),
        ));
    }
    if config.allow_dynamic_dataset_creation && config.truenas.is_none() {
        return Err(ControllerConfigError::Invalid(
            "dynamic dataset creation requires truenas backend config".to_string(),
        ));
    }
    if config.allow_authoritative_dataset_delete && !config.allow_dynamic_dataset_creation {
        return Err(ControllerConfigError::Invalid(
            "authoritative dataset delete requires dynamic dataset creation".to_string(),
        ));
    }
    for volume in &config.existing_volumes {
        if volume.id.trim().is_empty() {
            return Err(ControllerConfigError::Invalid(
                "existing volume id must not be empty".to_string(),
            ));
        }
        if volume.dataset.trim().is_empty() {
            return Err(ControllerConfigError::Invalid(format!(
                "existing volume {} dataset must not be empty",
                volume.id
            )));
        }
        if volume.policy.trim().is_empty() {
            return Err(ControllerConfigError::Invalid(format!(
                "existing volume {} policy must not be empty",
                volume.id
            )));
        }
    }
    Ok(())
}

fn load_controller_state(path: Option<&Path>) -> Result<ControllerState, ControllerConfigError> {
    let Some(path) = path else {
        return Ok(ControllerState::default());
    };
    match fs::read_to_string(path) {
        Ok(content) => {
            serde_json::from_str(&content).map_err(|error| ControllerConfigError::Parse {
                path: path.to_path_buf(),
                message: error.to_string(),
            })
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(ControllerState::default()),
        Err(error) => Err(ControllerConfigError::Io {
            path: path.to_path_buf(),
            message: error.to_string(),
        }),
    }
}

fn persist_controller_state(
    path: Option<&Path>,
    state: &ControllerState,
) -> Result<(), std::io::Error> {
    let Some(path) = path else {
        return Ok(());
    };
    let contents = serde_json::to_vec_pretty(state)
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    atomic_write(path, &contents)
}

fn atomic_write(path: &Path, contents: &[u8]) -> Result<(), std::io::Error> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }
    if fs::read(path).ok().as_deref() == Some(contents) {
        return Ok(());
    }
    let temp_path = temp_path_for(path);
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temp_path)?;
    let write_result = (|| -> Result<(), std::io::Error> {
        file.write_all(contents)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp_path, path)?;
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            File::open(parent)?.sync_all()?;
        }
        Ok(())
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    write_result
}

fn temp_path_for(path: &Path) -> PathBuf {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("controller-state");
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    parent.join(format!(".{file_name}.nas-csi.tmp.{}", now))
}

fn validate_csi_capabilities(
    volume: &DatasetVolume,
    capabilities: &[csi::VolumeCapability],
) -> Result<(), Status> {
    if capabilities.is_empty() {
        return Err(Status::invalid_argument(
            "at least one volume capability is required",
        ));
    }

    for capability in capabilities {
        let request = capability_request_from_csi(volume, capability)?;
        if let CapabilityValidation::Invalid { reason } = validate_capability(&request) {
            return Err(Status::invalid_argument(reason));
        }
    }
    Ok(())
}

fn capability_request_from_csi(
    volume: &DatasetVolume,
    capability: &csi::VolumeCapability,
) -> Result<CapabilityRequest, Status> {
    let volume_mode = match capability.access_type {
        Some(csi::volume_capability::AccessType::Mount(_)) => VolumeMode::Filesystem,
        Some(csi::volume_capability::AccessType::Block(_)) => VolumeMode::Block,
        None => {
            return Err(Status::invalid_argument(
                "volume capability access_type is required",
            ));
        }
    };
    let access_mode = match capability
        .access_mode
        .as_ref()
        .map(|mode| csi::volume_capability::access_mode::Mode::try_from(mode.mode))
    {
        Some(Ok(csi::volume_capability::access_mode::Mode::SingleNodeWriter)) => {
            CsiAccessMode::SingleNodeWriter
        }
        Some(Ok(csi::volume_capability::access_mode::Mode::SingleNodeReaderOnly)) => {
            CsiAccessMode::SingleNodeReaderOnly
        }
        Some(Ok(csi::volume_capability::access_mode::Mode::MultiNodeReaderOnly)) => {
            CsiAccessMode::MultiNodeReaderOnly
        }
        Some(Ok(csi::volume_capability::access_mode::Mode::MultiNodeSingleWriter)) => {
            CsiAccessMode::MultiNodeSingleWriter
        }
        Some(Ok(csi::volume_capability::access_mode::Mode::MultiNodeMultiWriter)) => {
            CsiAccessMode::MultiNodeMultiWriter
        }
        _ => {
            return Err(Status::invalid_argument(
                "supported volume capability access_mode is required",
            ));
        }
    };

    Ok(CapabilityRequest {
        volume_mode,
        access_mode,
        policy: volume.policy.clone(),
    })
}

fn requested_capacity(range: Option<&csi::CapacityRange>) -> Option<i64> {
    let range = range?;
    if range.required_bytes > 0 {
        Some(range.required_bytes)
    } else if range.limit_bytes > 0 {
        Some(range.limit_bytes)
    } else {
        None
    }
}

fn is_existing_dataset_request(parameters: &HashMap<String, String>) -> bool {
    parameters
        .get("nas-csi.dev/mode")
        .or_else(|| parameters.get("mode"))
        .is_some_and(|mode| mode == "existing-dataset")
}

fn is_dynamic_dataset_request(parameters: &HashMap<String, String>) -> bool {
    parameters
        .get("nas-csi.dev/mode")
        .or_else(|| parameters.get("mode"))
        .is_some_and(|mode| mode == "dynamic-filesystem-dataset")
        || parameters
            .get("nas-csi.dev/createDataset")
            .is_some_and(|value| value == "true")
}

fn policy_from_parameters(
    parameters: &HashMap<String, String>,
    default_access: PolicyAccessMode,
) -> VolumePolicy {
    let access = match parameters.get("nas-csi.dev/access").map(String::as_str) {
        Some("read-only") => PolicyAccessMode::ReadOnly,
        Some("read-write") => PolicyAccessMode::ReadWrite,
        _ => default_access,
    };
    VolumePolicy {
        name: parameters
            .get("nas-csi.dev/policy")
            .cloned()
            .unwrap_or_else(|| "existing-dataset".to_string()),
        access,
        allow_multi_node_writer: parameters
            .get("nas-csi.dev/allowMultiNodeWriter")
            .is_some_and(|value| value == "true"),
    }
}

fn assignment_key(volume_id: &str, node_id: &str) -> String {
    format!("{volume_id}\0{node_id}")
}

fn safe_identifier(value: &str) -> String {
    value
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use csi::controller_server::Controller;
    use std::sync::Mutex;

    fn read_write_policy() -> VolumePolicy {
        VolumePolicy {
            name: "repos-dev".to_string(),
            access: PolicyAccessMode::ReadWrite,
            allow_multi_node_writer: true,
        }
    }

    fn read_only_policy() -> VolumePolicy {
        VolumePolicy {
            name: "samples-ro".to_string(),
            access: PolicyAccessMode::ReadOnly,
            allow_multi_node_writer: false,
        }
    }

    #[derive(Default)]
    struct FakeTrueNasBackend {
        datasets: Mutex<BTreeMap<String, DatasetBackendMetadata>>,
        created_datasets: Mutex<Vec<String>>,
        deleted_datasets: Mutex<Vec<String>>,
        created_snapshots: Mutex<Vec<String>>,
        deleted_snapshots: Mutex<Vec<String>>,
    }

    impl FakeTrueNasBackend {
        fn with_dataset(self, metadata: DatasetBackendMetadata) -> Self {
            self.datasets
                .lock()
                .expect("datasets mutex")
                .insert(metadata.dataset.clone(), metadata);
            self
        }

        fn deleted_datasets(&self) -> Vec<String> {
            self.deleted_datasets
                .lock()
                .expect("deleted datasets mutex")
                .clone()
        }

        fn created_snapshots(&self) -> Vec<String> {
            self.created_snapshots
                .lock()
                .expect("created snapshots mutex")
                .clone()
        }

        fn deleted_snapshots(&self) -> Vec<String> {
            self.deleted_snapshots
                .lock()
                .expect("deleted snapshots mutex")
                .clone()
        }
    }

    impl ControllerBackend for FakeTrueNasBackend {
        fn lookup_filesystem_dataset(
            &self,
            dataset: &str,
        ) -> Result<Option<DatasetBackendMetadata>, ControllerBackendError> {
            Ok(self
                .datasets
                .lock()
                .expect("datasets mutex")
                .get(dataset)
                .cloned())
        }

        fn create_filesystem_dataset(&self, dataset: &str) -> Result<(), ControllerBackendError> {
            self.created_datasets
                .lock()
                .expect("created datasets mutex")
                .push(dataset.to_string());
            self.datasets
                .lock()
                .expect("datasets mutex")
                .entry(dataset.to_string())
                .or_insert_with(|| DatasetBackendMetadata {
                    dataset: dataset.to_string(),
                    source_path: format!("/mnt/{dataset}"),
                    smb_shares: Vec::new(),
                    snapshots: Vec::new(),
                    retention: None,
                });
            Ok(())
        }

        fn delete_filesystem_dataset(&self, dataset: &str) -> Result<(), ControllerBackendError> {
            self.deleted_datasets
                .lock()
                .expect("deleted datasets mutex")
                .push(dataset.to_string());
            self.datasets
                .lock()
                .expect("datasets mutex")
                .remove(dataset);
            Ok(())
        }

        fn create_snapshot(
            &self,
            dataset: &str,
            snapshot_name: &str,
        ) -> Result<String, ControllerBackendError> {
            let id = format!("{dataset}@{snapshot_name}");
            self.created_snapshots
                .lock()
                .expect("created snapshots mutex")
                .push(id.clone());
            Ok(id)
        }

        fn delete_snapshot(&self, snapshot_id: &str) -> Result<(), ControllerBackendError> {
            self.deleted_snapshots
                .lock()
                .expect("deleted snapshots mutex")
                .push(snapshot_id.to_string());
            Ok(())
        }
    }

    fn mount_capability(mode: csi::volume_capability::access_mode::Mode) -> csi::VolumeCapability {
        csi::VolumeCapability {
            access_type: Some(csi::volume_capability::AccessType::Mount(
                csi::volume_capability::MountVolume {
                    fs_type: String::new(),
                    mount_flags: Vec::new(),
                },
            )),
            access_mode: Some(csi::volume_capability::AccessMode { mode: mode as i32 }),
        }
    }

    #[test]
    fn rejects_block_volume_mode() {
        let result = validate_capability(&CapabilityRequest {
            volume_mode: VolumeMode::Block,
            access_mode: CsiAccessMode::SingleNodeWriter,
            policy: read_write_policy(),
        });

        assert!(matches!(result, CapabilityValidation::Invalid { .. }));
    }

    #[test]
    fn rejects_writer_for_read_only_policy() {
        let result = validate_capability(&CapabilityRequest {
            volume_mode: VolumeMode::Filesystem,
            access_mode: CsiAccessMode::SingleNodeWriter,
            policy: read_only_policy(),
        });

        assert_eq!(
            result,
            CapabilityValidation::Invalid {
                reason: "policy samples-ro is read-only".to_string()
            }
        );
    }

    #[test]
    fn allows_multi_node_reader_for_read_only_policy() {
        let result = validate_capability(&CapabilityRequest {
            volume_mode: VolumeMode::Filesystem,
            access_mode: CsiAccessMode::MultiNodeReaderOnly,
            policy: read_only_policy(),
        });

        assert_eq!(
            result,
            CapabilityValidation::Valid {
                warnings: Vec::new()
            }
        );
    }

    #[test]
    fn warns_for_allowed_multi_node_writer() {
        let result = validate_capability(&CapabilityRequest {
            volume_mode: VolumeMode::Filesystem,
            access_mode: CsiAccessMode::MultiNodeMultiWriter,
            policy: read_write_policy(),
        });

        match result {
            CapabilityValidation::Valid { warnings } => assert_eq!(warnings.len(), 1),
            other => panic!("expected valid capability, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn create_volume_returns_existing_dataset_volume() {
        let state = ControllerState::default().with_volume(DatasetVolume::existing_filesystem(
            "repos",
            "tank/repos",
            "/mnt/tank/repos",
            read_write_policy(),
        ));
        let service = NasCsiControllerService::new(ControllerConfig::default(), state);

        let response = service
            .create_volume(Request::new(csi::CreateVolumeRequest {
                name: "repos".to_string(),
                capacity_range: None,
                volume_capabilities: vec![mount_capability(
                    csi::volume_capability::access_mode::Mode::MultiNodeMultiWriter,
                )],
                parameters: HashMap::from([(
                    "nas-csi.dev/dataset".to_string(),
                    "tank/repos".to_string(),
                )]),
            }))
            .await
            .expect("create volume")
            .into_inner();

        let volume = response.volume.expect("volume");
        assert_eq!(volume.volume_id, "repos");
        assert_eq!(
            volume.volume_context["nas-csi.dev/sourcePath"],
            "/mnt/tank/repos"
        );
    }

    #[tokio::test]
    async fn create_volume_can_create_dynamic_dataset_when_enabled() {
        let service = NasCsiControllerService::new(
            ControllerConfig {
                allow_dynamic_dataset_creation: true,
                default_capacity_bytes: 1024,
                ..ControllerConfig::default()
            },
            ControllerState::default(),
        );

        let response = service
            .create_volume(Request::new(csi::CreateVolumeRequest {
                name: "scratch".to_string(),
                capacity_range: None,
                volume_capabilities: vec![mount_capability(
                    csi::volume_capability::access_mode::Mode::SingleNodeWriter,
                )],
                parameters: HashMap::from([
                    (
                        "nas-csi.dev/dataset".to_string(),
                        "tank/scratch".to_string(),
                    ),
                    (
                        "nas-csi.dev/mode".to_string(),
                        "dynamic-filesystem-dataset".to_string(),
                    ),
                ]),
            }))
            .await
            .expect("create dynamic")
            .into_inner();

        assert_eq!(response.volume.expect("volume").volume_id, "scratch");
        assert!(
            service
                .snapshot_state()
                .volumes
                .get("scratch")
                .expect("state volume")
                .dynamically_created
        );
        assert!(
            !service
                .snapshot_state()
                .volumes
                .get("scratch")
                .expect("state volume")
                .delete_dataset_on_delete
        );
    }

    #[tokio::test]
    async fn dynamic_dataset_creation_requires_explicit_mode() {
        let service = NasCsiControllerService::new(
            ControllerConfig {
                allow_dynamic_dataset_creation: true,
                ..ControllerConfig::default()
            },
            ControllerState::default(),
        );

        let error = service
            .create_volume(Request::new(csi::CreateVolumeRequest {
                name: "scratch".to_string(),
                capacity_range: None,
                volume_capabilities: vec![mount_capability(
                    csi::volume_capability::access_mode::Mode::SingleNodeWriter,
                )],
                parameters: HashMap::from([(
                    "nas-csi.dev/dataset".to_string(),
                    "tank/scratch".to_string(),
                )]),
            }))
            .await
            .expect_err("dynamic mode required");

        assert_eq!(error.code(), tonic::Code::NotFound);
    }

    #[tokio::test]
    async fn existing_dataset_registration_uses_backend_dataset_and_smb_metadata() {
        let backend = Arc::new(FakeTrueNasBackend::default().with_dataset(
            DatasetBackendMetadata {
                dataset: "tank/repos".to_string(),
                source_path: "/mnt/tank/repos".to_string(),
                smb_shares: vec![SmbShareMetadata {
                    name: "repos".to_string(),
                    path: "/mnt/tank/repos".to_string(),
                    enabled: true,
                    managed: false,
                }],
                snapshots: vec![SnapshotMetadata {
                    id: "tank/repos@manual".to_string(),
                    source_volume_id: "repos".to_string(),
                    name: "manual".to_string(),
                    size_bytes: 0,
                    creation_time_unix_seconds: 0,
                    ready_to_use: true,
                }],
                retention: Some(RetentionReplicationMetadata {
                    retention_policy: "daily".to_string(),
                    replication_policy: "offsite".to_string(),
                    snapshot_prefix: "auto".to_string(),
                }),
            },
        ));
        let service = NasCsiControllerService::with_backend(
            ControllerConfig::default(),
            ControllerState::default(),
            backend,
        );

        let response = service
            .create_volume(Request::new(csi::CreateVolumeRequest {
                name: "repos".to_string(),
                capacity_range: None,
                volume_capabilities: vec![mount_capability(
                    csi::volume_capability::access_mode::Mode::MultiNodeMultiWriter,
                )],
                parameters: HashMap::from([
                    (
                        "nas-csi.dev/mode".to_string(),
                        "existing-dataset".to_string(),
                    ),
                    ("nas-csi.dev/dataset".to_string(), "tank/repos".to_string()),
                    ("nas-csi.dev/policy".to_string(), "repos-dev".to_string()),
                    (
                        "nas-csi.dev/allowMultiNodeWriter".to_string(),
                        "true".to_string(),
                    ),
                ]),
            }))
            .await
            .expect("register existing")
            .into_inner();

        let volume = response.volume.expect("volume");
        assert_eq!(volume.volume_id, "repos");
        assert_eq!(volume.volume_context["nas-csi.dev/smbShares"], "repos");
        assert_eq!(
            volume.volume_context["nas-csi.dev/retentionPolicy"],
            "daily"
        );
        let state = service.snapshot_state();
        let state_volume = state.volumes.get("repos").expect("state volume");
        assert_eq!(state_volume.smb_shares.len(), 1);
        assert_eq!(state_volume.snapshots.len(), 1);
    }

    #[tokio::test]
    async fn metadata_file_preserves_volume_identity_across_restart() {
        let root = unique_test_dir("controller-state");
        let state_path = root.join("state.json");
        let config = ControllerConfig {
            metadata_path: Some(state_path.clone()),
            ..ControllerConfig::default()
        };
        let backend = Arc::new(FakeTrueNasBackend::default().with_dataset(
            DatasetBackendMetadata {
                dataset: "tank/repos".to_string(),
                source_path: "/mnt/tank/repos".to_string(),
                smb_shares: Vec::new(),
                snapshots: Vec::new(),
                retention: None,
            },
        ));
        let service = NasCsiControllerService::with_backend(
            config.clone(),
            ControllerState::default(),
            backend,
        );
        service
            .create_volume(Request::new(csi::CreateVolumeRequest {
                name: "repos".to_string(),
                capacity_range: None,
                volume_capabilities: vec![mount_capability(
                    csi::volume_capability::access_mode::Mode::MultiNodeMultiWriter,
                )],
                parameters: HashMap::from([
                    (
                        "nas-csi.dev/mode".to_string(),
                        "existing-dataset".to_string(),
                    ),
                    ("nas-csi.dev/dataset".to_string(), "tank/repos".to_string()),
                    ("nas-csi.dev/policy".to_string(), "repos-dev".to_string()),
                    (
                        "nas-csi.dev/allowMultiNodeWriter".to_string(),
                        "true".to_string(),
                    ),
                ]),
            }))
            .await
            .expect("register existing");

        let restarted = NasCsiControllerService::new(
            config,
            load_controller_state(Some(&state_path)).expect("load state"),
        );
        let response = restarted
            .create_volume(Request::new(csi::CreateVolumeRequest {
                name: "repos".to_string(),
                capacity_range: None,
                volume_capabilities: vec![mount_capability(
                    csi::volume_capability::access_mode::Mode::MultiNodeMultiWriter,
                )],
                parameters: HashMap::from([(
                    "nas-csi.dev/dataset".to_string(),
                    "tank/repos".to_string(),
                )]),
            }))
            .await
            .expect("idempotent after restart")
            .into_inner();

        assert_eq!(response.volume.expect("volume").volume_id, "repos");
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn delete_volume_does_not_delete_authoritative_dataset_by_default() {
        let mut volume = DatasetVolume::existing_filesystem(
            "repos",
            "tank/repos",
            "/mnt/tank/repos",
            read_write_policy(),
        );
        volume.delete_dataset_on_delete = true;
        let service = NasCsiControllerService::new(
            ControllerConfig::default(),
            ControllerState::default().with_volume(volume),
        );

        service
            .delete_volume(Request::new(csi::DeleteVolumeRequest {
                volume_id: "repos".to_string(),
            }))
            .await
            .expect("delete volume");

        assert!(!service.snapshot_state().volumes.contains_key("repos"));
    }

    #[tokio::test]
    async fn dynamic_dataset_delete_requires_config_and_parameter_opt_in() {
        let backend = Arc::new(FakeTrueNasBackend::default());
        let service = NasCsiControllerService::with_backend(
            ControllerConfig {
                allow_dynamic_dataset_creation: true,
                allow_authoritative_dataset_delete: true,
                ..ControllerConfig::default()
            },
            ControllerState::default(),
            backend.clone(),
        );

        service
            .create_volume(Request::new(csi::CreateVolumeRequest {
                name: "scratch".to_string(),
                capacity_range: None,
                volume_capabilities: vec![mount_capability(
                    csi::volume_capability::access_mode::Mode::SingleNodeWriter,
                )],
                parameters: HashMap::from([
                    (
                        "nas-csi.dev/mode".to_string(),
                        "dynamic-filesystem-dataset".to_string(),
                    ),
                    (
                        "nas-csi.dev/dataset".to_string(),
                        "tank/scratch".to_string(),
                    ),
                    (
                        "nas-csi.dev/deleteDatasetOnDelete".to_string(),
                        "true".to_string(),
                    ),
                ]),
            }))
            .await
            .expect("create dynamic");
        service
            .delete_volume(Request::new(csi::DeleteVolumeRequest {
                volume_id: "scratch".to_string(),
            }))
            .await
            .expect("delete dynamic");

        assert_eq!(backend.deleted_datasets(), vec!["tank/scratch".to_string()]);
    }

    #[tokio::test]
    async fn controller_publish_records_node_assignment() {
        let state = ControllerState::default().with_volume(DatasetVolume::existing_filesystem(
            "repos",
            "tank/repos",
            "/mnt/tank/repos",
            read_write_policy(),
        ));
        let service = NasCsiControllerService::new(ControllerConfig::default(), state);

        let response = service
            .controller_publish_volume(Request::new(csi::ControllerPublishVolumeRequest {
                volume_id: "repos".to_string(),
                node_id: "agent-1".to_string(),
                readonly: false,
                volume_capability: vec![mount_capability(
                    csi::volume_capability::access_mode::Mode::MultiNodeMultiWriter,
                )],
            }))
            .await
            .expect("publish")
            .into_inner();

        assert_eq!(response.publish_context["nas-csi.dev/exportId"], "repos");
        assert!(
            service
                .snapshot_state()
                .assignments
                .contains_key(&assignment_key("repos", "agent-1"))
        );
    }

    #[tokio::test]
    async fn snapshot_lifecycle_uses_dataset_snapshot_identity() {
        let backend = Arc::new(FakeTrueNasBackend::default());
        let state = ControllerState::default().with_volume(DatasetVolume::existing_filesystem(
            "repos",
            "tank/repos",
            "/mnt/tank/repos",
            read_write_policy(),
        ));
        let service = NasCsiControllerService::with_backend(
            ControllerConfig::default(),
            state,
            backend.clone(),
        );

        let response = service
            .create_snapshot(Request::new(csi::CreateSnapshotRequest {
                source_volume_id: "repos".to_string(),
                name: "manual-1".to_string(),
                parameters: HashMap::new(),
            }))
            .await
            .expect("create snapshot")
            .into_inner();

        assert_eq!(
            response.snapshot.expect("snapshot").snapshot_id,
            "tank/repos@manual-1"
        );
        let list = service
            .list_snapshots(Request::new(csi::ListSnapshotsRequest {
                source_volume_id: "repos".to_string(),
            }))
            .await
            .expect("list snapshots")
            .into_inner();
        assert_eq!(list.entries.len(), 1);
        assert_eq!(
            backend.created_snapshots(),
            vec!["tank/repos@manual-1".to_string()]
        );
        service
            .delete_snapshot(Request::new(csi::DeleteSnapshotRequest {
                snapshot_id: "tank/repos@manual-1".to_string(),
            }))
            .await
            .expect("delete snapshot");
        assert_eq!(
            backend.deleted_snapshots(),
            vec!["tank/repos@manual-1".to_string()]
        );
    }

    #[test]
    fn loads_controller_runtime_config_from_yaml() {
        let root = unique_test_dir("controller-config");
        let config_path = root.join("controller.yaml");
        fs::write(
            &config_path,
            r#"
apiVersion: nas-csi.dev/v1alpha1
kind: ControllerConfig
driverName: nas-csi.dev
metadataPath: /var/lib/nas-csi/controller/state.json
defaultCapacityBytes: 4096
allowDynamicDatasetCreation: false
allowAuthoritativeDatasetDelete: false
allowSnapshotDelete: true
existingVolumes:
  - id: repos
    dataset: tank/repos
    sourcePath: /mnt/tank/repos
    policy: repos-dev
    access: read-write
    allowMultiNodeWriter: true
"#,
        )
        .expect("write config");

        let config = load_controller_runtime_config(&config_path).expect("load config");

        assert_eq!(config.default_capacity_bytes, Some(4096));
        assert_eq!(config.existing_volumes.len(), 1);
        assert_eq!(config.existing_volumes[0].id, "repos");
        let _ = fs::remove_dir_all(root);
    }

    fn unique_test_dir(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("nas-csi-driver-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create temp test dir");
        path
    }
}
