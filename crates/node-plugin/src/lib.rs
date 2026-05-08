//! CSI node-side mount planning primitives.

use nas_csi_proto::csi::v1 as csi;
use nas_csi_types::{AccessMode, NodeRuntimeConfig};
use std::fmt;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use tokio::net::UnixListener;
use tokio_stream::wrappers::UnixListenerStream;
use tonic::{Request, Response, Status, transport::Server};

#[derive(Debug)]
pub enum NodeRuntimeConfigError {
    Io(String),
    Parse(serde_yml::Error),
    Validate(Vec<String>),
}

impl fmt::Display for NodeRuntimeConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "{error}"),
            Self::Parse(error) => write!(f, "failed to parse node runtime config: {error}"),
            Self::Validate(errors) => {
                write!(
                    f,
                    "node runtime config validation failed with {} error(s)",
                    errors.len()
                )
            }
        }
    }
}

impl std::error::Error for NodeRuntimeConfigError {}

pub fn parse_node_runtime_config(input: &str) -> Result<NodeRuntimeConfig, NodeRuntimeConfigError> {
    let config: NodeRuntimeConfig =
        serde_yml::from_str(input).map_err(NodeRuntimeConfigError::Parse)?;
    let errors = config.validate();
    if errors.is_empty() {
        Ok(config)
    } else {
        Err(NodeRuntimeConfigError::Validate(errors))
    }
}

pub fn load_node_runtime_config(path: &Path) -> Result<NodeRuntimeConfig, NodeRuntimeConfigError> {
    let input = fs::read_to_string(path).map_err(|error| {
        NodeRuntimeConfigError::Io(format!("failed to read {}: {error}", path.display()))
    })?;
    parse_node_runtime_config(&input)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MountInfoEntry {
    pub mount_id: u64,
    pub parent_id: u64,
    pub root: String,
    pub mount_point: String,
    pub filesystem_type: String,
    pub source: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MountInfoParseError {
    MissingSeparator,
    MissingField(&'static str),
    InvalidInteger(&'static str),
}

pub fn parse_mountinfo(input: &str) -> Result<Vec<MountInfoEntry>, MountInfoParseError> {
    input
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(parse_mountinfo_line)
        .collect()
}

pub fn parse_mountinfo_line(line: &str) -> Result<MountInfoEntry, MountInfoParseError> {
    let (left, right) = line
        .split_once(" - ")
        .ok_or(MountInfoParseError::MissingSeparator)?;
    let left_fields = left.split_whitespace().collect::<Vec<_>>();
    let right_fields = right.split_whitespace().collect::<Vec<_>>();

    let mount_id = parse_u64(
        left_fields
            .first()
            .copied()
            .ok_or(MountInfoParseError::MissingField("mount_id"))?,
        "mount_id",
    )?;
    let parent_id = parse_u64(
        left_fields
            .get(1)
            .copied()
            .ok_or(MountInfoParseError::MissingField("parent_id"))?,
        "parent_id",
    )?;
    let root = unescape_mount_field(
        left_fields
            .get(3)
            .copied()
            .ok_or(MountInfoParseError::MissingField("root"))?,
    );
    let mount_point = unescape_mount_field(
        left_fields
            .get(4)
            .copied()
            .ok_or(MountInfoParseError::MissingField("mount_point"))?,
    );
    let filesystem_type = right_fields
        .first()
        .copied()
        .ok_or(MountInfoParseError::MissingField("filesystem_type"))?
        .to_string();
    let source = unescape_mount_field(
        right_fields
            .get(1)
            .copied()
            .ok_or(MountInfoParseError::MissingField("source"))?,
    );

    Ok(MountInfoEntry {
        mount_id,
        parent_id,
        root,
        mount_point,
        filesystem_type,
        source,
    })
}

fn parse_u64(value: &str, field: &'static str) -> Result<u64, MountInfoParseError> {
    value
        .parse()
        .map_err(|_| MountInfoParseError::InvalidInteger(field))
}

fn unescape_mount_field(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\\' {
            let octal = (chars.next(), chars.next(), chars.next());
            if let (Some(a), Some(b), Some(c)) = octal
                && a.is_ascii_digit()
                && b.is_ascii_digit()
                && c.is_ascii_digit()
            {
                let code =
                    u8::from_str_radix(&[a, b, c].iter().collect::<String>(), 8).unwrap_or(b'?');
                output.push(code as char);
                continue;
            }
            output.push(ch);
            if let (Some(a), Some(b), Some(c)) = octal {
                output.push(a);
                output.push(b);
                output.push(c);
            }
        } else {
            output.push(ch);
        }
    }

    output
}

pub fn find_mount<'a>(
    entries: &'a [MountInfoEntry],
    mount_point: &str,
) -> Option<&'a MountInfoEntry> {
    entries
        .iter()
        .find(|entry| entry.mount_point == mount_point)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeStageRequestPlan {
    pub volume_id: String,
    pub staging_path: String,
    pub read_only: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NodeStageAction {
    AlreadyStaged,
    BindMount {
        source: String,
        target: String,
        read_only: bool,
    },
    Error(String),
}

pub fn plan_node_stage(
    request: &NodeStageRequestPlan,
    runtime: &NodeRuntimeConfig,
    mounts: &[MountInfoEntry],
) -> NodeStageAction {
    let Some(export) = runtime
        .exports
        .iter()
        .find(|export| export.id == request.volume_id)
    else {
        return NodeStageAction::Error(format!(
            "volume {} is not configured on node {}",
            request.volume_id, runtime.node_name
        ));
    };

    let Some(source_mount) = find_mount(mounts, &export.guest_mount_path) else {
        return NodeStageAction::Error(format!(
            "guest virtiofs export is not mounted: {}",
            export.guest_mount_path
        ));
    };
    if source_mount.filesystem_type != "virtiofs" {
        return NodeStageAction::Error(format!(
            "guest export {} is {}, expected virtiofs",
            export.guest_mount_path, source_mount.filesystem_type
        ));
    }
    if source_mount.source != export.tag {
        return NodeStageAction::Error(format!(
            "guest export {} is mounted from {}, expected tag {}",
            export.guest_mount_path, source_mount.source, export.tag
        ));
    }

    if let Some(staging_mount) = find_mount(mounts, &request.staging_path) {
        if staging_mount.source == source_mount.source || staging_mount.root == source_mount.root {
            return NodeStageAction::AlreadyStaged;
        }
        return NodeStageAction::Error(format!(
            "staging path is already mounted from unexpected source: {}",
            request.staging_path
        ));
    }

    NodeStageAction::BindMount {
        source: export.guest_mount_path.clone(),
        target: request.staging_path.clone(),
        read_only: request.read_only || export.access == AccessMode::ReadOnly,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodePublishRequestPlan {
    pub volume_id: String,
    pub staging_path: String,
    pub target_path: String,
    pub read_only: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NodePublishAction {
    AlreadyPublished,
    BindMount {
        source: String,
        target: String,
        read_only: bool,
    },
    Error(String),
}

pub fn plan_node_publish(
    request: &NodePublishRequestPlan,
    mounts: &[MountInfoEntry],
) -> NodePublishAction {
    let Some(staging_mount) = find_mount(mounts, &request.staging_path) else {
        return NodePublishAction::Error(format!(
            "staging path is not mounted: {}",
            request.staging_path
        ));
    };

    if staging_mount.filesystem_type != "virtiofs" {
        return NodePublishAction::Error(format!(
            "staging path {} is {}, expected virtiofs",
            request.staging_path, staging_mount.filesystem_type
        ));
    }

    if let Some(target_mount) = find_mount(mounts, &request.target_path) {
        if target_mount.root == staging_mount.root || target_mount.source == staging_mount.source {
            return NodePublishAction::AlreadyPublished;
        }
        return NodePublishAction::Error(format!(
            "target path is already mounted from unexpected source: {}",
            request.target_path
        ));
    }

    NodePublishAction::BindMount {
        source: request.staging_path.clone(),
        target: request.target_path.clone(),
        read_only: request.read_only,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeUnpublishRequestPlan {
    pub target_path: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NodeUnpublishAction {
    AlreadyUnpublished,
    Unmount { target: String },
}

pub fn plan_node_unpublish(
    request: &NodeUnpublishRequestPlan,
    mounts: &[MountInfoEntry],
) -> NodeUnpublishAction {
    if find_mount(mounts, &request.target_path).is_some() {
        NodeUnpublishAction::Unmount {
            target: request.target_path.clone(),
        }
    } else {
        NodeUnpublishAction::AlreadyUnpublished
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeUnstageRequestPlan {
    pub staging_path: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NodeUnstageAction {
    AlreadyUnstaged,
    Unmount { target: String },
}

pub fn plan_node_unstage(
    request: &NodeUnstageRequestPlan,
    mounts: &[MountInfoEntry],
) -> NodeUnstageAction {
    if find_mount(mounts, &request.staging_path).is_some() {
        NodeUnstageAction::Unmount {
            target: request.staging_path.clone(),
        }
    } else {
        NodeUnstageAction::AlreadyUnstaged
    }
}

pub trait NodeMounter: Send + Sync {
    fn read_mountinfo(&self) -> Result<String, NodeMountError>;
    fn ensure_dir(&self, path: &str) -> Result<(), NodeMountError>;
    fn bind_mount(&self, source: &str, target: &str, read_only: bool)
    -> Result<(), NodeMountError>;
    fn unmount(&self, target: &str) -> Result<(), NodeMountError>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeMountError {
    pub message: String,
}

impl fmt::Display for NodeMountError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for NodeMountError {}

pub struct RealNodeMounter;

impl NodeMounter for RealNodeMounter {
    fn read_mountinfo(&self) -> Result<String, NodeMountError> {
        fs::read_to_string("/proc/self/mountinfo").map_err(|error| NodeMountError {
            message: format!("failed to read /proc/self/mountinfo: {error}"),
        })
    }

    fn ensure_dir(&self, path: &str) -> Result<(), NodeMountError> {
        fs::create_dir_all(path).map_err(|error| NodeMountError {
            message: format!("failed to create {path}: {error}"),
        })
    }

    fn bind_mount(
        &self,
        source: &str,
        target: &str,
        read_only: bool,
    ) -> Result<(), NodeMountError> {
        run_mount_command("mount", ["--bind", source, target])?;
        if read_only {
            run_mount_command("mount", ["-o", "remount,bind,ro", target])?;
        }
        Ok(())
    }

    fn unmount(&self, target: &str) -> Result<(), NodeMountError> {
        run_mount_command("umount", [target])
    }
}

fn run_mount_command<const N: usize>(program: &str, args: [&str; N]) -> Result<(), NodeMountError> {
    let status = Command::new(program)
        .args(args)
        .status()
        .map_err(|error| NodeMountError {
            message: format!("failed to execute {program}: {error}"),
        })?;
    if status.success() {
        Ok(())
    } else {
        Err(NodeMountError {
            message: format!("{program} exited with status {status}"),
        })
    }
}

pub struct NasCsiNodeService<M> {
    runtime: Arc<NodeRuntimeConfig>,
    mounter: Arc<M>,
}

impl<M> Clone for NasCsiNodeService<M> {
    fn clone(&self) -> Self {
        Self {
            runtime: Arc::clone(&self.runtime),
            mounter: Arc::clone(&self.mounter),
        }
    }
}

impl<M> NasCsiNodeService<M>
where
    M: NodeMounter + 'static,
{
    pub fn new(runtime: NodeRuntimeConfig, mounter: M) -> Self {
        Self {
            runtime: Arc::new(runtime),
            mounter: Arc::new(mounter),
        }
    }

    fn current_mounts(&self) -> Result<Vec<MountInfoEntry>, Status> {
        let mountinfo = self
            .mounter
            .read_mountinfo()
            .map_err(|error| Status::internal(error.to_string()))?;
        parse_mountinfo(&mountinfo)
            .map_err(|error| Status::internal(format!("failed to parse mountinfo: {error:?}")))
    }
}

fn log_node_operation_start(operation: &str, fields: Vec<(&'static str, serde_json::Value)>) {
    log_node_operation(operation, "start", fields, None);
}

fn log_node_operation_success(operation: &str, fields: Vec<(&'static str, serde_json::Value)>) {
    log_node_operation(operation, "success", fields, None);
}

fn log_node_operation_failure(
    operation: &str,
    fields: Vec<(&'static str, serde_json::Value)>,
    status: &Status,
) {
    log_node_operation(operation, "failure", fields, Some(status));
}

fn log_node_operation(
    operation: &str,
    result: &str,
    fields: Vec<(&'static str, serde_json::Value)>,
    status: Option<&Status>,
) {
    let mut log = serde_json::Map::new();
    log.insert("event".to_string(), "csi_node_operation".into());
    log.insert("component".to_string(), "nas-csi-node".into());
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
impl<M> csi::identity_server::Identity for NasCsiNodeService<M>
where
    M: NodeMounter + 'static,
{
    async fn get_plugin_info(
        &self,
        _request: Request<csi::GetPluginInfoRequest>,
    ) -> Result<Response<csi::GetPluginInfoResponse>, Status> {
        Ok(Response::new(csi::GetPluginInfoResponse {
            name: "nas-csi.dev".to_string(),
            vendor_version: env!("CARGO_PKG_VERSION").to_string(),
            manifest: std::collections::HashMap::from([(
                "nas-csi.dev/node".to_string(),
                self.runtime.node_name.clone(),
            )]),
        }))
    }

    async fn get_plugin_capabilities(
        &self,
        _request: Request<csi::GetPluginCapabilitiesRequest>,
    ) -> Result<Response<csi::GetPluginCapabilitiesResponse>, Status> {
        Ok(Response::new(csi::GetPluginCapabilitiesResponse {
            capabilities: Vec::new(),
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
impl<M> csi::node_server::Node for NasCsiNodeService<M>
where
    M: NodeMounter + 'static,
{
    async fn node_stage_volume(
        &self,
        request: Request<csi::NodeStageVolumeRequest>,
    ) -> Result<Response<csi::NodeStageVolumeResponse>, Status> {
        let request = request.into_inner();
        let volume_id = request.volume_id;
        let staging_path = request.staging_target_path;
        let requested_read_only = request.readonly;
        let identifiers = || {
            vec![
                ("volumeId", volume_id.clone().into()),
                ("exportId", volume_id.clone().into()),
                ("stagingPath", staging_path.clone().into()),
                ("targetPath", staging_path.clone().into()),
                ("readOnly", requested_read_only.into()),
            ]
        };
        log_node_operation_start("node_stage_volume", identifiers());
        let result = (|| -> Result<&'static str, Status> {
            let mounts = self.current_mounts()?;
            match plan_node_stage(
                &NodeStageRequestPlan {
                    volume_id: volume_id.clone(),
                    staging_path: staging_path.clone(),
                    read_only: requested_read_only,
                },
                &self.runtime,
                &mounts,
            ) {
                NodeStageAction::AlreadyStaged => Ok("already_staged"),
                NodeStageAction::BindMount {
                    source,
                    target,
                    read_only,
                } => {
                    self.mounter
                        .ensure_dir(&target)
                        .map_err(|error| Status::internal(error.to_string()))?;
                    self.mounter
                        .bind_mount(&source, &target, read_only)
                        .map_err(|error| Status::internal(error.to_string()))?;
                    Ok("bind_mount")
                }
                NodeStageAction::Error(error) => Err(Status::failed_precondition(error)),
            }
        })();
        match result {
            Ok(action) => {
                let mut fields = identifiers();
                fields.push(("action", action.into()));
                log_node_operation_success("node_stage_volume", fields);
                Ok(Response::new(csi::NodeStageVolumeResponse {}))
            }
            Err(status) => {
                log_node_operation_failure("node_stage_volume", identifiers(), &status);
                Err(status)
            }
        }
    }

    async fn node_unstage_volume(
        &self,
        request: Request<csi::NodeUnstageVolumeRequest>,
    ) -> Result<Response<csi::NodeUnstageVolumeResponse>, Status> {
        let request = request.into_inner();
        let volume_id = request.volume_id;
        let staging_path = request.staging_target_path;
        let identifiers = || {
            vec![
                ("volumeId", volume_id.clone().into()),
                ("exportId", volume_id.clone().into()),
                ("stagingPath", staging_path.clone().into()),
                ("targetPath", staging_path.clone().into()),
                ("readOnly", serde_json::Value::Null),
            ]
        };
        log_node_operation_start("node_unstage_volume", identifiers());
        let result = (|| -> Result<&'static str, Status> {
            let mounts = self.current_mounts()?;
            match plan_node_unstage(
                &NodeUnstageRequestPlan {
                    staging_path: staging_path.clone(),
                },
                &mounts,
            ) {
                NodeUnstageAction::AlreadyUnstaged => Ok("already_unstaged"),
                NodeUnstageAction::Unmount { target } => {
                    self.mounter
                        .unmount(&target)
                        .map_err(|error| Status::internal(error.to_string()))?;
                    Ok("unmount")
                }
            }
        })();
        match result {
            Ok(action) => {
                let mut fields = identifiers();
                fields.push(("action", action.into()));
                log_node_operation_success("node_unstage_volume", fields);
                Ok(Response::new(csi::NodeUnstageVolumeResponse {}))
            }
            Err(status) => {
                log_node_operation_failure("node_unstage_volume", identifiers(), &status);
                Err(status)
            }
        }
    }

    async fn node_publish_volume(
        &self,
        request: Request<csi::NodePublishVolumeRequest>,
    ) -> Result<Response<csi::NodePublishVolumeResponse>, Status> {
        let request = request.into_inner();
        let volume_id = request.volume_id;
        let staging_path = request.staging_target_path;
        let target_path = request.target_path;
        let requested_read_only = request.readonly;
        let identifiers = || {
            vec![
                ("volumeId", volume_id.clone().into()),
                ("exportId", volume_id.clone().into()),
                ("stagingPath", staging_path.clone().into()),
                ("targetPath", target_path.clone().into()),
                ("readOnly", requested_read_only.into()),
            ]
        };
        log_node_operation_start("node_publish_volume", identifiers());
        let result = (|| -> Result<&'static str, Status> {
            let mounts = self.current_mounts()?;
            match plan_node_publish(
                &NodePublishRequestPlan {
                    volume_id: volume_id.clone(),
                    staging_path: staging_path.clone(),
                    target_path: target_path.clone(),
                    read_only: requested_read_only,
                },
                &mounts,
            ) {
                NodePublishAction::AlreadyPublished => Ok("already_published"),
                NodePublishAction::BindMount {
                    source,
                    target,
                    read_only,
                } => {
                    self.mounter
                        .ensure_dir(&target)
                        .map_err(|error| Status::internal(error.to_string()))?;
                    self.mounter
                        .bind_mount(&source, &target, read_only)
                        .map_err(|error| Status::internal(error.to_string()))?;
                    Ok("bind_mount")
                }
                NodePublishAction::Error(error) => Err(Status::failed_precondition(error)),
            }
        })();
        match result {
            Ok(action) => {
                let mut fields = identifiers();
                fields.push(("action", action.into()));
                log_node_operation_success("node_publish_volume", fields);
                Ok(Response::new(csi::NodePublishVolumeResponse {}))
            }
            Err(status) => {
                log_node_operation_failure("node_publish_volume", identifiers(), &status);
                Err(status)
            }
        }
    }

    async fn node_unpublish_volume(
        &self,
        request: Request<csi::NodeUnpublishVolumeRequest>,
    ) -> Result<Response<csi::NodeUnpublishVolumeResponse>, Status> {
        let request = request.into_inner();
        let volume_id = request.volume_id;
        let target_path = request.target_path;
        let identifiers = || {
            vec![
                ("volumeId", volume_id.clone().into()),
                ("exportId", volume_id.clone().into()),
                ("targetPath", target_path.clone().into()),
                ("readOnly", serde_json::Value::Null),
            ]
        };
        log_node_operation_start("node_unpublish_volume", identifiers());
        let result = (|| -> Result<&'static str, Status> {
            let mounts = self.current_mounts()?;
            match plan_node_unpublish(
                &NodeUnpublishRequestPlan {
                    target_path: target_path.clone(),
                },
                &mounts,
            ) {
                NodeUnpublishAction::AlreadyUnpublished => Ok("already_unpublished"),
                NodeUnpublishAction::Unmount { target } => {
                    self.mounter
                        .unmount(&target)
                        .map_err(|error| Status::internal(error.to_string()))?;
                    Ok("unmount")
                }
            }
        })();
        match result {
            Ok(action) => {
                let mut fields = identifiers();
                fields.push(("action", action.into()));
                log_node_operation_success("node_unpublish_volume", fields);
                Ok(Response::new(csi::NodeUnpublishVolumeResponse {}))
            }
            Err(status) => {
                log_node_operation_failure("node_unpublish_volume", identifiers(), &status);
                Err(status)
            }
        }
    }

    async fn node_get_info(
        &self,
        _request: Request<csi::NodeGetInfoRequest>,
    ) -> Result<Response<csi::NodeGetInfoResponse>, Status> {
        Ok(Response::new(csi::NodeGetInfoResponse {
            node_id: self.runtime.node_name.clone(),
        }))
    }

    async fn node_get_capabilities(
        &self,
        _request: Request<csi::NodeGetCapabilitiesRequest>,
    ) -> Result<Response<csi::NodeGetCapabilitiesResponse>, Status> {
        Ok(Response::new(csi::NodeGetCapabilitiesResponse {
            capabilities: vec![csi::NodeServiceCapability {
                r#type: csi::node_service_capability::Type::StageUnstageVolume as i32,
            }],
        }))
    }
}

#[derive(Debug)]
pub enum NodeServeError {
    Io(std::io::Error),
    Transport(tonic::transport::Error),
}

impl fmt::Display for NodeServeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "I/O error: {error}"),
            Self::Transport(error) => write!(f, "transport error: {error}"),
        }
    }
}

impl std::error::Error for NodeServeError {}

pub async fn serve_node_uds<M>(
    socket_path: &Path,
    service: NasCsiNodeService<M>,
) -> Result<(), NodeServeError>
where
    M: NodeMounter + 'static,
{
    match fs::remove_file(socket_path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(NodeServeError::Io(error)),
    }
    let listener = UnixListener::bind(socket_path).map_err(NodeServeError::Io)?;
    Server::builder()
        .add_service(csi::identity_server::IdentityServer::new(service.clone()))
        .add_service(csi::node_server::NodeServer::new(service))
        .serve_with_incoming(UnixListenerStream::new(listener))
        .await
        .map_err(NodeServeError::Transport)
}

#[cfg(test)]
mod tests {
    use super::*;
    use csi::node_server::Node;
    use std::sync::Mutex;

    #[test]
    fn parses_node_runtime_config() {
        let config = parse_node_runtime_config(
            r#"
apiVersion: nas-csi.dev/v1alpha1
kind: NodeRuntimeConfig
nodeName: agent-1
domain: nascsi-agent-1
exports:
  - id: repos-dev
    dataset: tank/repos
    sourcePath: /mnt/tank/repos
    tag: nascsi_repos_dev
    policy: repos-dev
    access: read-write
    guestMountPath: /var/lib/nas-csi/virtiofs/repos-dev
"#,
        )
        .expect("parse runtime config");

        assert_eq!(config.node_name, "agent-1");
        assert_eq!(config.exports[0].tag, "nascsi_repos_dev");
    }

    #[test]
    fn rejects_invalid_node_runtime_config() {
        let error = parse_node_runtime_config(
            r#"
apiVersion: nas-csi.dev/v1alpha1
kind: NodeRuntimeConfig
nodeName: agent-1
domain: nascsi-agent-1
exports:
  - id: repos-dev
    dataset: tank/repos
    sourcePath: /mnt/tank/repos
    tag: nascsi_repos_dev
    policy: repos-dev
    access: read-write
    guestMountPath: /var/lib/nas-csi/virtiofs/repos-dev
  - id: repos-dev
    dataset: tank/repos
    sourcePath: /mnt/tank/repos
    tag: nascsi_repos_dev
    policy: repos-dev
    access: read-write
    guestMountPath: /var/lib/nas-csi/virtiofs/repos-dev
"#,
        )
        .expect_err("duplicate runtime export");

        assert!(matches!(error, NodeRuntimeConfigError::Validate(_)));
    }

    #[test]
    fn parses_mountinfo_line() {
        let entry = parse_mountinfo_line(
            "42 31 0:38 / /var/lib/nas-csi/virtiofs/repos rw,relatime - virtiofs nascsi_repos rw",
        )
        .expect("parse mountinfo");

        assert_eq!(entry.mount_id, 42);
        assert_eq!(entry.parent_id, 31);
        assert_eq!(entry.mount_point, "/var/lib/nas-csi/virtiofs/repos");
        assert_eq!(entry.filesystem_type, "virtiofs");
        assert_eq!(entry.source, "nascsi_repos");
    }

    #[test]
    fn unescapes_mount_paths() {
        let entry = parse_mountinfo_line(
            "42 31 0:38 / /var/lib/nas-csi/with\\040space rw,relatime - virtiofs tag\\040name rw",
        )
        .expect("parse mountinfo");

        assert_eq!(entry.mount_point, "/var/lib/nas-csi/with space");
        assert_eq!(entry.source, "tag name");
    }

    #[test]
    fn plans_bind_mount_when_staged() {
        let mounts =
            parse_mountinfo("42 31 0:38 / /staging/repos rw,relatime - virtiofs nascsi_repos rw\n")
                .expect("parse mountinfo");
        let request = NodePublishRequestPlan {
            volume_id: "repos".to_string(),
            staging_path: "/staging/repos".to_string(),
            target_path: "/pods/pod/vol".to_string(),
            read_only: false,
        };

        assert_eq!(
            plan_node_publish(&request, &mounts),
            NodePublishAction::BindMount {
                source: "/staging/repos".to_string(),
                target: "/pods/pod/vol".to_string(),
                read_only: false,
            }
        );
    }

    #[test]
    fn plans_stage_bind_from_runtime_export() {
        let runtime = parse_node_runtime_config(
            r#"
apiVersion: nas-csi.dev/v1alpha1
kind: NodeRuntimeConfig
nodeName: agent-1
domain: nascsi-agent-1
exports:
  - id: samples-ro
    dataset: tank/samples
    sourcePath: /mnt/tank/samples
    tag: nascsi_samples_ro
    policy: samples-ro
    access: read-only
    guestMountPath: /var/lib/nas-csi/virtiofs/samples-ro
"#,
        )
        .expect("runtime config");
        let mounts = parse_mountinfo(
            "42 31 0:38 / /var/lib/nas-csi/virtiofs/samples-ro ro,relatime - virtiofs nascsi_samples_ro ro\n",
        )
        .expect("mountinfo");
        let request = NodeStageRequestPlan {
            volume_id: "samples-ro".to_string(),
            staging_path: "/var/lib/kubelet/plugins/kubernetes.io/csi/pv/sample/globalmount"
                .to_string(),
            read_only: false,
        };

        assert_eq!(
            plan_node_stage(&request, &runtime, &mounts),
            NodeStageAction::BindMount {
                source: "/var/lib/nas-csi/virtiofs/samples-ro".to_string(),
                target: "/var/lib/kubelet/plugins/kubernetes.io/csi/pv/sample/globalmount"
                    .to_string(),
                read_only: true,
            }
        );
    }

    #[test]
    fn refuses_stage_when_runtime_export_is_absent() {
        let runtime = parse_node_runtime_config(
            r#"
apiVersion: nas-csi.dev/v1alpha1
kind: NodeRuntimeConfig
nodeName: agent-1
domain: nascsi-agent-1
exports: []
"#,
        )
        .expect("runtime config");
        let request = NodeStageRequestPlan {
            volume_id: "repos-dev".to_string(),
            staging_path: "/staging/repos".to_string(),
            read_only: false,
        };

        assert!(matches!(
            plan_node_stage(&request, &runtime, &[]),
            NodeStageAction::Error(_)
        ));
    }

    #[test]
    fn refuses_missing_staging_mount() {
        let request = NodePublishRequestPlan {
            volume_id: "repos".to_string(),
            staging_path: "/staging/repos".to_string(),
            target_path: "/pods/pod/vol".to_string(),
            read_only: false,
        };

        assert!(matches!(
            plan_node_publish(&request, &[]),
            NodePublishAction::Error(_)
        ));
    }

    #[test]
    fn plans_idempotent_unpublish_and_unstage() {
        let mounts =
            parse_mountinfo("42 31 0:38 / /target rw,relatime - virtiofs nascsi_repos rw\n")
                .expect("parse mountinfo");

        assert_eq!(
            plan_node_unpublish(
                &NodeUnpublishRequestPlan {
                    target_path: "/target".to_string(),
                },
                &mounts,
            ),
            NodeUnpublishAction::Unmount {
                target: "/target".to_string()
            }
        );
        assert_eq!(
            plan_node_unstage(
                &NodeUnstageRequestPlan {
                    staging_path: "/missing".to_string(),
                },
                &mounts,
            ),
            NodeUnstageAction::AlreadyUnstaged
        );
    }

    #[tokio::test]
    async fn node_stage_service_executes_bind_mount() {
        let runtime = parse_node_runtime_config(
            r#"
apiVersion: nas-csi.dev/v1alpha1
kind: NodeRuntimeConfig
nodeName: agent-1
domain: nascsi-agent-1
exports:
  - id: repos
    dataset: tank/repos
    sourcePath: /mnt/tank/repos
    tag: nascsi_repos
    policy: repos-dev
    access: read-write
    guestMountPath: /var/lib/nas-csi/virtiofs/repos
"#,
        )
        .expect("runtime config");
        let mounter = FakeMounter::new(
            "42 31 0:38 / /var/lib/nas-csi/virtiofs/repos rw,relatime - virtiofs nascsi_repos rw\n",
        );
        let service = NasCsiNodeService::new(runtime, mounter);

        service
            .node_stage_volume(tonic::Request::new(csi::NodeStageVolumeRequest {
                volume_id: "repos".to_string(),
                staging_target_path:
                    "/var/lib/kubelet/plugins/kubernetes.io/csi/pv/repos/globalmount".to_string(),
                readonly: false,
                volume_capability: None,
            }))
            .await
            .expect("stage volume");

        let mounter = service.mounter;
        assert_eq!(
            mounter.operations.lock().expect("ops").as_slice(),
            &[
                "mkdir:/var/lib/kubelet/plugins/kubernetes.io/csi/pv/repos/globalmount",
                "bind:/var/lib/nas-csi/virtiofs/repos:/var/lib/kubelet/plugins/kubernetes.io/csi/pv/repos/globalmount:false",
            ]
        );
    }

    #[tokio::test]
    async fn node_publish_fails_closed_without_staging_mount() {
        let runtime = parse_node_runtime_config(
            r#"
apiVersion: nas-csi.dev/v1alpha1
kind: NodeRuntimeConfig
nodeName: agent-1
domain: nascsi-agent-1
exports: []
"#,
        )
        .expect("runtime config");
        let service = NasCsiNodeService::new(runtime, FakeMounter::new(""));

        let error = service
            .node_publish_volume(tonic::Request::new(csi::NodePublishVolumeRequest {
                volume_id: "repos".to_string(),
                staging_target_path: "/missing".to_string(),
                target_path: "/target".to_string(),
                readonly: false,
                volume_capability: None,
            }))
            .await
            .expect_err("missing staging mount");

        assert_eq!(error.code(), tonic::Code::FailedPrecondition);
    }

    #[tokio::test]
    async fn node_unpublish_executes_unmount() {
        let runtime = parse_node_runtime_config(
            r#"
apiVersion: nas-csi.dev/v1alpha1
kind: NodeRuntimeConfig
nodeName: agent-1
domain: nascsi-agent-1
exports: []
"#,
        )
        .expect("runtime config");
        let service = NasCsiNodeService::new(
            runtime,
            FakeMounter::new("42 31 0:38 / /target rw,relatime - virtiofs nascsi_repos rw\n"),
        );

        service
            .node_unpublish_volume(tonic::Request::new(csi::NodeUnpublishVolumeRequest {
                volume_id: "repos".to_string(),
                target_path: "/target".to_string(),
            }))
            .await
            .expect("unpublish");

        assert_eq!(
            service.mounter.operations.lock().expect("ops").as_slice(),
            &["umount:/target"]
        );
    }

    struct FakeMounter {
        mountinfo: String,
        operations: Mutex<Vec<String>>,
    }

    impl FakeMounter {
        fn new(mountinfo: &str) -> Self {
            Self {
                mountinfo: mountinfo.to_string(),
                operations: Mutex::new(Vec::new()),
            }
        }
    }

    impl NodeMounter for FakeMounter {
        fn read_mountinfo(&self) -> Result<String, NodeMountError> {
            Ok(self.mountinfo.clone())
        }

        fn ensure_dir(&self, path: &str) -> Result<(), NodeMountError> {
            self.operations
                .lock()
                .expect("ops")
                .push(format!("mkdir:{path}"));
            Ok(())
        }

        fn bind_mount(
            &self,
            source: &str,
            target: &str,
            read_only: bool,
        ) -> Result<(), NodeMountError> {
            self.operations
                .lock()
                .expect("ops")
                .push(format!("bind:{source}:{target}:{read_only}"));
            Ok(())
        }

        fn unmount(&self, target: &str) -> Result<(), NodeMountError> {
            self.operations
                .lock()
                .expect("ops")
                .push(format!("umount:{target}"));
            Ok(())
        }
    }
}
