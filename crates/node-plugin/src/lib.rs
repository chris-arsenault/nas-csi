//! CSI node-side mount planning primitives.

use nas_csi_types::{AccessMode, NodeRuntimeConfig};
use std::fmt;

#[derive(Debug)]
pub enum NodeRuntimeConfigError {
    Parse(serde_yml::Error),
    Validate(Vec<String>),
}

impl fmt::Display for NodeRuntimeConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
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

#[cfg(test)]
mod tests {
    use super::*;

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
            "42 31 0:38 / /var/lib/nas-csi/host-datasets/repos rw,relatime - virtiofs nascsi_repos rw",
        )
        .expect("parse mountinfo");

        assert_eq!(entry.mount_id, 42);
        assert_eq!(entry.parent_id, 31);
        assert_eq!(entry.mount_point, "/var/lib/nas-csi/host-datasets/repos");
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
}
