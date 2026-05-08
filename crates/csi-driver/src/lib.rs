//! CSI controller-side service and policy validation.

use nas_csi_proto::csi::v1 as csi;
use nas_csi_types::AccessMode as PolicyAccessMode;
use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tokio::net::UnixListener;
use tokio_stream::wrappers::UnixListenerStream;
use tonic::{Request, Response, Status, transport::Server};

pub const DEFAULT_DRIVER_NAME: &str = "nas-csi.dev";

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum VolumeMode {
    Filesystem,
    Block,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CsiAccessMode {
    SingleNodeWriter,
    SingleNodeReaderOnly,
    MultiNodeReaderOnly,
    MultiNodeSingleWriter,
    MultiNodeMultiWriter,
}

#[derive(Clone, Debug, PartialEq, Eq)]
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControllerConfig {
    pub driver_name: String,
    pub vendor_version: String,
    pub default_capacity_bytes: i64,
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
            allow_dynamic_dataset_creation: false,
            allow_authoritative_dataset_delete: false,
            allow_snapshot_delete: true,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
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

#[derive(Clone, Debug, PartialEq, Eq)]
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SmbShareMetadata {
    pub name: String,
    pub path: String,
    pub enabled: bool,
    pub managed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RetentionReplicationMetadata {
    pub retention_policy: String,
    pub replication_policy: String,
    pub snapshot_prefix: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishAssignment {
    pub volume_id: String,
    pub node_id: String,
    pub read_only: bool,
}

pub trait ControllerBackend: Send + Sync {
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

    pub fn snapshot_state(&self) -> ControllerState {
        self.state
            .lock()
            .expect("controller state poisoned")
            .clone()
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

        if !self.config.allow_dynamic_dataset_creation {
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
        volume.delete_dataset_on_delete = true;
        volume.capacity_bytes = requested_capacity(request.capacity_range.as_ref())
            .unwrap_or(self.config.default_capacity_bytes);
        validate_csi_capabilities(&volume, &request.volume_capabilities)?;

        let csi_volume = volume.to_csi_volume();
        self.state
            .lock()
            .expect("controller state poisoned")
            .volumes
            .insert(volume.id.clone(), volume);
        Ok(csi_volume)
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

#[tonic::async_trait]
impl csi::controller_server::Controller for NasCsiControllerService {
    async fn create_volume(
        &self,
        request: Request<csi::CreateVolumeRequest>,
    ) -> Result<Response<csi::CreateVolumeResponse>, Status> {
        let volume = self.create_volume_inner(request.into_inner())?;
        Ok(Response::new(csi::CreateVolumeResponse {
            volume: Some(volume),
        }))
    }

    async fn delete_volume(
        &self,
        request: Request<csi::DeleteVolumeRequest>,
    ) -> Result<Response<csi::DeleteVolumeResponse>, Status> {
        self.delete_volume_inner(&request.into_inner().volume_id)?;
        Ok(Response::new(csi::DeleteVolumeResponse {}))
    }

    async fn controller_publish_volume(
        &self,
        request: Request<csi::ControllerPublishVolumeRequest>,
    ) -> Result<Response<csi::ControllerPublishVolumeResponse>, Status> {
        let request = request.into_inner();
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
                read_only: request.readonly || volume.policy.access == PolicyAccessMode::ReadOnly,
            },
        );

        Ok(Response::new(csi::ControllerPublishVolumeResponse {
            publish_context: HashMap::from([
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
            ]),
        }))
    }

    async fn controller_unpublish_volume(
        &self,
        request: Request<csi::ControllerUnpublishVolumeRequest>,
    ) -> Result<Response<csi::ControllerUnpublishVolumeResponse>, Status> {
        let request = request.into_inner();
        self.state
            .lock()
            .expect("controller state poisoned")
            .assignments
            .remove(&assignment_key(&request.volume_id, &request.node_id));
        Ok(Response::new(csi::ControllerUnpublishVolumeResponse {}))
    }

    async fn validate_volume_capabilities(
        &self,
        request: Request<csi::ValidateVolumeCapabilitiesRequest>,
    ) -> Result<Response<csi::ValidateVolumeCapabilitiesResponse>, Status> {
        let request = request.into_inner();
        let state = self.state.lock().expect("controller state poisoned");
        let volume = state
            .volumes
            .get(&request.volume_id)
            .ok_or_else(|| Status::not_found("volume not found"))?;
        match validate_csi_capabilities(volume, &request.volume_capabilities) {
            Ok(()) => Ok(Response::new(csi::ValidateVolumeCapabilitiesResponse {
                confirmed: Some(csi::validate_volume_capabilities_response::Confirmed {
                    volume_context_id: volume.id.clone(),
                    volume_capabilities: request.volume_capabilities,
                }),
                message: String::new(),
            })),
            Err(status) => Ok(Response::new(csi::ValidateVolumeCapabilitiesResponse {
                confirmed: None,
                message: status.message().to_string(),
            })),
        }
    }

    async fn list_volumes(
        &self,
        _request: Request<csi::ListVolumesRequest>,
    ) -> Result<Response<csi::ListVolumesResponse>, Status> {
        let state = self.state.lock().expect("controller state poisoned");
        Ok(Response::new(csi::ListVolumesResponse {
            entries: state
                .volumes
                .values()
                .map(|volume| csi::list_volumes_response::Entry {
                    volume: Some(volume.to_csi_volume()),
                })
                .collect(),
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
        let mut state = self.state.lock().expect("controller state poisoned");
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
        Ok(Response::new(csi::CreateSnapshotResponse {
            snapshot: Some(snapshot.to_csi_snapshot()),
        }))
    }

    async fn delete_snapshot(
        &self,
        request: Request<csi::DeleteSnapshotRequest>,
    ) -> Result<Response<csi::DeleteSnapshotResponse>, Status> {
        let snapshot_id = request.into_inner().snapshot_id;
        if snapshot_id.trim().is_empty() {
            return Ok(Response::new(csi::DeleteSnapshotResponse {}));
        }
        if self.config.allow_snapshot_delete {
            self.backend
                .delete_snapshot(&snapshot_id)
                .map_err(|error| Status::internal(error.to_string()))?;
        }
        self.state
            .lock()
            .expect("controller state poisoned")
            .snapshots
            .remove(&snapshot_id);
        Ok(Response::new(csi::DeleteSnapshotResponse {}))
    }

    async fn list_snapshots(
        &self,
        request: Request<csi::ListSnapshotsRequest>,
    ) -> Result<Response<csi::ListSnapshotsResponse>, Status> {
        let source_volume_id = request.into_inner().source_volume_id;
        let state = self.state.lock().expect("controller state poisoned");
        Ok(Response::new(csi::ListSnapshotsResponse {
            entries: state
                .snapshots
                .values()
                .filter(|snapshot| {
                    source_volume_id.is_empty() || snapshot.source_volume_id == source_volume_id
                })
                .map(|snapshot| csi::list_snapshots_response::Entry {
                    snapshot: Some(snapshot.to_csi_snapshot()),
                })
                .collect(),
        }))
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
                parameters: HashMap::from([(
                    "nas-csi.dev/dataset".to_string(),
                    "tank/scratch".to_string(),
                )]),
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
        let state = ControllerState::default().with_volume(DatasetVolume::existing_filesystem(
            "repos",
            "tank/repos",
            "/mnt/tank/repos",
            read_write_policy(),
        ));
        let service = NasCsiControllerService::new(ControllerConfig::default(), state);

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
    }
}
