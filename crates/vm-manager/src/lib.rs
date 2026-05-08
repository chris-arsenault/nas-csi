//! Libvirt VM lifecycle planning and domain XML rendering.

use nas_csi_cluster_manager::{K3sConfigInput, K3sRole, render_k3s_config};
use nas_csi_types::{
    API_VERSION, AccessMode, DiskFormat, HostConfig, NodeConfig, NodeRole, NodeRuntimeConfig,
    NodeRuntimeExport,
};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DomainSpec {
    pub name: String,
    pub memory_mib: u64,
    pub vcpus: u16,
    pub machine: String,
    pub cpu_mode: String,
    pub root_disk_path: String,
    pub root_disk_format: String,
    pub seed_disk_path: Option<String>,
    pub bridge: String,
    pub mac_address: String,
    pub virtiofs_exports: Vec<VirtiofsExport>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VirtiofsExport {
    pub socket_path: String,
    pub tag: String,
    pub queue_size: u16,
}

pub fn render_domain_xml(spec: &DomainSpec) -> String {
    let desired_hash = domain_desired_hash(spec);
    render_domain_xml_with_metadata(spec, Some(&desired_hash))
}

pub fn domain_desired_hash(spec: &DomainSpec) -> String {
    content_hash(render_domain_xml_with_metadata(spec, None).as_bytes())
}

pub fn extract_domain_desired_hash(xml: &str) -> Option<String> {
    // Libvirt preserves metadata elements but may normalize namespace quoting.
    // The marker is owned by this project, so local-name text extraction is
    // enough until we introduce a broader XML editing surface.
    let marker = "desired-domain-hash";
    let marker_index = xml.find(marker)?;
    let value_start = xml[marker_index..].find('>')? + marker_index + 1;
    let value_end = xml[value_start..].find('<')? + value_start;
    let value = xml[value_start..value_end].trim();
    if value.len() == 16 && value.chars().all(|ch| ch.is_ascii_hexdigit()) {
        Some(value.to_ascii_lowercase())
    } else {
        None
    }
}

pub fn extract_domain_managed(xml: &str) -> bool {
    xml.contains("nas-csi")
        && (xml.contains("desired-domain-hash")
            || xml.contains("<nas-csi:managed>true</nas-csi:managed>"))
}

fn render_domain_xml_with_metadata(spec: &DomainSpec, desired_hash: Option<&str>) -> String {
    let mut devices = String::new();
    devices.push_str(&format!(
        "    <disk type='file' device='disk'>\n      <driver name='qemu' type='{}'/>\n      <source file='{}'/>\n      <target dev='vda' bus='virtio'/>\n    </disk>\n",
        xml_escape(&spec.root_disk_format),
        xml_escape(&spec.root_disk_path)
    ));

    if let Some(seed_disk_path) = &spec.seed_disk_path {
        devices.push_str(&format!(
            "    <disk type='file' device='cdrom'>\n      <driver name='qemu' type='raw'/>\n      <source file='{}'/>\n      <target dev='sda' bus='sata'/>\n      <readonly/>\n    </disk>\n",
            xml_escape(seed_disk_path)
        ));
    }

    devices.push_str(&format!(
        "    <interface type='bridge'>\n      <mac address='{}'/>\n      <source bridge='{}'/>\n      <model type='virtio'/>\n    </interface>\n",
        xml_escape(&spec.mac_address),
        xml_escape(&spec.bridge)
    ));

    devices.push_str(
        "    <channel type='unix'>\n      <target type='virtio' name='org.qemu.guest_agent.0'/>\n    </channel>\n",
    );
    devices.push_str("    <console type='pty'/>\n");

    for export in &spec.virtiofs_exports {
        devices.push_str(&format!(
            "    <filesystem type='mount'>\n      <driver type='virtiofs' queue='{}'/>\n      <source socket='{}'/>\n      <target dir='{}'/>\n    </filesystem>\n",
            export.queue_size,
            xml_escape(&export.socket_path),
            xml_escape(&export.tag)
        ));
    }

    let metadata = desired_hash
        .map(|hash| {
            format!(
                "  <metadata>\n    <nas-csi:managed xmlns:nas-csi='urn:nas-csi.dev:domain'>true</nas-csi:managed>\n    <nas-csi:desired-domain-hash xmlns:nas-csi='urn:nas-csi.dev:domain'>{}</nas-csi:desired-domain-hash>\n  </metadata>\n",
                xml_escape(hash)
            )
        })
        .unwrap_or_default();

    format!(
        "<domain type='kvm'>\n  <name>{}</name>\n{}  <memory unit='MiB'>{}</memory>\n  <vcpu placement='static'>{}</vcpu>\n  <os>\n    <type arch='x86_64' machine='{}'>hvm</type>\n  </os>\n  <features>\n    <acpi/>\n    <apic/>\n  </features>\n  <cpu mode='{}'/>\n  <memoryBacking>\n    <source type='memfd'/>\n    <access mode='shared'/>\n  </memoryBacking>\n  <devices>\n{}  </devices>\n</domain>\n",
        xml_escape(&spec.name),
        metadata,
        spec.memory_mib,
        spec.vcpus,
        xml_escape(&spec.machine),
        xml_escape(&spec.cpu_mode),
        devices
    )
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtifactRenderOptions {
    pub runtime_dir: String,
    pub virtiofsd_path: String,
    pub virtiofsd_cache: String,
    pub virtiofsd_sandbox: String,
    pub virtiofs_queue_size: u16,
    pub k3s_token_path_in_vm: String,
    pub k3s_token: Option<String>,
    pub ssh_authorized_keys: Vec<String>,
}

impl Default for ArtifactRenderOptions {
    fn default() -> Self {
        Self {
            runtime_dir: "/run/nas-csi".to_string(),
            virtiofsd_path: "virtiofsd".to_string(),
            virtiofsd_cache: "auto".to_string(),
            virtiofsd_sandbox: "namespace".to_string(),
            virtiofs_queue_size: 1024,
            k3s_token_path_in_vm: "/etc/rancher/k3s/token".to_string(),
            k3s_token: None,
            ssh_authorized_keys: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderedHostArtifacts {
    pub files: Vec<RenderedFile>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderedFile {
    pub relative_path: String,
    pub contents: String,
}

#[derive(Debug)]
pub enum ArtifactRenderError {
    MissingExport { node: String, export: String },
    MissingArtifact { relative_path: String },
    SeedImage(NoCloudSeedError),
    Yaml(serde_yml::Error),
}

impl fmt::Display for ArtifactRenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingExport { node, export } => {
                write!(f, "node {node} references missing export {export}")
            }
            Self::MissingArtifact { relative_path } => {
                write!(f, "missing rendered artifact {relative_path}")
            }
            Self::SeedImage(error) => write!(f, "failed to render NoCloud seed image: {error}"),
            Self::Yaml(error) => write!(f, "failed to render yaml: {error}"),
        }
    }
}

impl std::error::Error for ArtifactRenderError {}

impl From<serde_yml::Error> for ArtifactRenderError {
    fn from(value: serde_yml::Error) -> Self {
        Self::Yaml(value)
    }
}

impl From<NoCloudSeedError> for ArtifactRenderError {
    fn from(value: NoCloudSeedError) -> Self {
        Self::SeedImage(value)
    }
}

pub fn render_host_artifacts(
    config: &HostConfig,
    options: &ArtifactRenderOptions,
) -> Result<RenderedHostArtifacts, ArtifactRenderError> {
    let mut files = Vec::new();

    for node in &config.nodes {
        let node_dir = format!("nodes/{}", safe_path_segment(&node.name));
        let domain = domain_spec_from_node(config, node, options)?;
        files.push(RenderedFile {
            relative_path: format!("{node_dir}/domain.xml"),
            contents: render_domain_xml(&domain),
        });

        let user_data = cloud_init_for_node(config, node, options)?;
        files.push(RenderedFile {
            relative_path: format!("{node_dir}/cloud-init/user-data"),
            contents: render_cloud_init_user_data(&user_data)?,
        });
        files.push(RenderedFile {
            relative_path: format!("{node_dir}/cloud-init/meta-data"),
            contents: render_cloud_init_meta_data(&format!("iid-{}", node.domain), &node.name),
        });

        let k3s_config = k3s_config_for_node(config, node, options)?;
        files.push(RenderedFile {
            relative_path: format!("{node_dir}/k3s/config.yaml"),
            contents: k3s_config,
        });

        for export_id in &node.exports {
            let export = config.exports.get(export_id).ok_or_else(|| {
                ArtifactRenderError::MissingExport {
                    node: node.name.clone(),
                    export: export_id.clone(),
                }
            })?;
            let unit = VirtiofsdUnitSpec {
                description: format!("nas-csi virtiofsd {} {}", node.domain, export_id),
                virtiofsd_path: options.virtiofsd_path.clone(),
                socket_path: virtiofs_socket_path(options, &node.domain, export_id),
                source_path: export.source_path.clone(),
                cache: options.virtiofsd_cache.clone(),
                sandbox: options.virtiofsd_sandbox.clone(),
                read_only: export.access == AccessMode::ReadOnly,
            };
            files.push(RenderedFile {
                relative_path: format!(
                    "{node_dir}/systemd/{}.service",
                    virtiofsd_service_name(&node.domain, export_id)
                ),
                contents: render_virtiofsd_systemd_unit(&unit),
            });
        }
    }

    Ok(RenderedHostArtifacts { files })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostApplyPlanOptions {
    pub artifact_dir: String,
    pub systemd_unit_dir: String,
    pub qemu_img_path: String,
    pub virsh_path: String,
    pub systemctl_path: String,
    pub start_domains: bool,
    pub allow_running_domain_redefine: bool,
    pub allow_domain_adoption: bool,
}

impl Default for HostApplyPlanOptions {
    fn default() -> Self {
        Self {
            artifact_dir: ".nas-csi/rendered".to_string(),
            systemd_unit_dir: "/etc/systemd/system".to_string(),
            qemu_img_path: "qemu-img".to_string(),
            virsh_path: "virsh".to_string(),
            systemctl_path: "systemctl".to_string(),
            start_domains: false,
            allow_running_domain_redefine: false,
            allow_domain_adoption: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostApplyPlan {
    pub steps: Vec<ApplyStep>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApplyStep {
    pub description: String,
    pub kind: ApplyStepKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApplyStepKind {
    EnsureDirectory {
        path: String,
    },
    WriteFile {
        path: String,
        contents: String,
    },
    WriteBinaryFile {
        path: String,
        contents: Vec<u8>,
    },
    RemoveFile {
        path: String,
    },
    Command {
        command: CommandSpec,
        creates: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandSpec {
    pub program: String,
    pub args: Vec<String>,
}

impl CommandSpec {
    pub fn new(program: impl Into<String>, args: impl IntoIterator<Item = String>) -> Self {
        Self {
            program: program.into(),
            args: args.into_iter().collect(),
        }
    }
}

impl fmt::Display for CommandSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", shell_quote(&self.program))?;
        for arg in &self.args {
            write!(f, " {}", shell_quote(arg))?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HostActualState {
    pub paths: BTreeMap<String, PathActualState>,
    pub tools: BTreeMap<String, ToolActualState>,
    pub systemd_units: BTreeMap<String, SystemdUnitActualState>,
    pub domains: BTreeMap<String, DomainActualState>,
    pub qemu_images: BTreeMap<String, QemuImageActualState>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathActualState {
    pub kind: PathActualKind,
    pub size: Option<u64>,
    pub content_hash: Option<String>,
    pub sha256: Option<String>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PathActualKind {
    File,
    Directory,
    Other,
    Missing,
}

impl PathActualState {
    pub fn missing() -> Self {
        Self {
            kind: PathActualKind::Missing,
            size: None,
            content_hash: None,
            sha256: None,
        }
    }

    pub fn file(contents: &[u8]) -> Self {
        Self {
            kind: PathActualKind::File,
            size: Some(contents.len() as u64),
            content_hash: Some(content_hash(contents)),
            sha256: None,
        }
    }

    pub fn file_with_sha256(contents: &[u8], sha256: impl Into<String>) -> Self {
        Self {
            kind: PathActualKind::File,
            size: Some(contents.len() as u64),
            content_hash: Some(content_hash(contents)),
            sha256: Some(sha256.into()),
        }
    }

    pub fn directory() -> Self {
        Self {
            kind: PathActualKind::Directory,
            size: None,
            content_hash: None,
            sha256: None,
        }
    }

    pub fn exists(&self) -> bool {
        self.kind != PathActualKind::Missing
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolActualState {
    pub path: Option<String>,
}

impl ToolActualState {
    pub fn found(path: impl Into<String>) -> Self {
        Self {
            path: Some(path.into()),
        }
    }

    pub fn missing() -> Self {
        Self { path: None }
    }

    pub fn is_found(&self) -> bool {
        self.path.is_some()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SystemdUnitActualState {
    pub installed_hash: Option<String>,
    pub enabled: Option<bool>,
    pub active: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DomainActualState {
    pub exists: bool,
    pub managed: bool,
    pub active: bool,
    pub autostart: Option<bool>,
    pub desired_hash: Option<String>,
    pub xml: Option<String>,
    pub xml_hash: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QemuImageActualState {
    pub format: Option<String>,
    pub backing_file: Option<String>,
    pub virtual_size: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostReconcilePlan {
    pub steps: Vec<ReconcileStep>,
}

impl HostReconcilePlan {
    pub fn has_refusals(&self) -> bool {
        self.steps.iter().any(|step| {
            matches!(
                step.kind,
                ReconcileStepKind::Refuse {
                    operation: _,
                    reason: _
                }
            )
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReconcileStep {
    pub description: String,
    pub kind: ReconcileStepKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReconcileStepKind {
    Apply(ReconcileOperation),
    SkipAlreadyCorrect {
        reason: String,
    },
    Refuse {
        operation: Option<ReconcileOperation>,
        reason: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReconcileOperation {
    EnsureDirectory {
        path: String,
    },
    WriteRenderedArtifact {
        path: String,
        contents: String,
    },
    CreateRootDisk {
        path: String,
        command: CommandSpec,
    },
    ResizeRootDisk {
        path: String,
        desired_size_bytes: u64,
        command: CommandSpec,
    },
    RewriteSeedImage {
        path: String,
        contents: Vec<u8>,
    },
    InstallOrUpdateSystemdUnit {
        unit_name: String,
        path: String,
        contents: String,
    },
    ReloadSystemdUnits {
        command: CommandSpec,
    },
    EnableAndStartVirtiofsdService {
        unit_name: String,
        socket_path: String,
        command: CommandSpec,
    },
    RestartVirtiofsdService {
        unit_name: String,
        socket_path: String,
        command: CommandSpec,
    },
    DefineDomain {
        domain: String,
        xml_path: String,
        command: CommandSpec,
    },
    RedefineDomain {
        domain: String,
        xml_path: String,
        previous_xml: Option<String>,
        command: CommandSpec,
    },
    RedefineDomainRequiresShutdown {
        domain: String,
        xml_path: String,
    },
    EnableDomainAutostart {
        domain: String,
        command: CommandSpec,
    },
    StartDomain {
        domain: String,
        command: CommandSpec,
    },
    RunCommand {
        command: CommandSpec,
        creates: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NoCloudSeedError {
    Fatfs(String),
    Io(String),
}

impl fmt::Display for NoCloudSeedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Fatfs(error) => write!(f, "FAT filesystem error: {error}"),
            Self::Io(error) => write!(f, "I/O error: {error}"),
        }
    }
}

impl std::error::Error for NoCloudSeedError {}

pub fn render_nocloud_seed_image(
    user_data: &str,
    meta_data: &str,
) -> Result<Vec<u8>, NoCloudSeedError> {
    const SECTOR_SIZE: usize = 512;
    const MIN_IMAGE_SIZE: usize = 2 * 1024 * 1024;
    const EXTRA_SPACE: usize = 512 * 1024;
    const ROUNDING: usize = 1024 * 1024;

    let payload_size = user_data.len() + meta_data.len();
    let image_size = round_up(MIN_IMAGE_SIZE.max(payload_size + EXTRA_SPACE), ROUNDING);
    let total_sectors = (image_size / SECTOR_SIZE) as u32;
    let mut image = vec![0_u8; image_size];

    {
        let cursor = Cursor::new(image.as_mut_slice());
        fatfs::format_volume(
            cursor,
            fatfs::FormatVolumeOptions::new()
                .bytes_per_sector(SECTOR_SIZE as u16)
                .total_sectors(total_sectors)
                .volume_label(*b"CIDATA     "),
        )
        .map_err(|error| NoCloudSeedError::Fatfs(error.to_string()))?;
    }

    {
        let cursor = Cursor::new(image.as_mut_slice());
        let fs = fatfs::FileSystem::new(cursor, fatfs::FsOptions::new())
            .map_err(|error| NoCloudSeedError::Fatfs(error.to_string()))?;
        {
            let root = fs.root_dir();
            {
                let mut file = root
                    .create_file("user-data")
                    .map_err(|error| NoCloudSeedError::Fatfs(error.to_string()))?;
                file.write_all(user_data.as_bytes())
                    .map_err(|error| NoCloudSeedError::Io(error.to_string()))?;
            }
            {
                let mut file = root
                    .create_file("meta-data")
                    .map_err(|error| NoCloudSeedError::Fatfs(error.to_string()))?;
                file.write_all(meta_data.as_bytes())
                    .map_err(|error| NoCloudSeedError::Io(error.to_string()))?;
            }
        }
        fs.unmount()
            .map_err(|error| NoCloudSeedError::Fatfs(error.to_string()))?;
    }

    Ok(image)
}

pub fn plan_host_apply(
    config: &HostConfig,
    render_options: &ArtifactRenderOptions,
    apply_options: &HostApplyPlanOptions,
) -> Result<HostApplyPlan, ArtifactRenderError> {
    let artifacts = render_host_artifacts(config, render_options)?;
    let mut steps = Vec::new();

    for file in &artifacts.files {
        let path = artifact_path(apply_options, &file.relative_path);
        steps.push(ApplyStep {
            description: format!("write rendered artifact {}", path.display()),
            kind: ApplyStepKind::WriteFile {
                path: path.display().to_string(),
                contents: file.contents.clone(),
            },
        });
    }

    for node in &config.nodes {
        let root_disk_parent = parent_dir(&node.root_disk.image);
        steps.push(ApplyStep {
            description: format!("ensure root disk directory for {}", node.name),
            kind: ApplyStepKind::EnsureDirectory {
                path: root_disk_parent,
            },
        });
        steps.push(ApplyStep {
            description: format!("create root disk overlay for {}", node.name),
            kind: ApplyStepKind::Command {
                command: qemu_img_create_command(&apply_options.qemu_img_path, node),
                creates: Some(node.root_disk.image.clone()),
            },
        });

        let seed_path = seed_image_path(&node.root_disk.image, &node.domain);
        steps.push(ApplyStep {
            description: format!("remove stale cloud-init seed for {}", node.name),
            kind: ApplyStepKind::RemoveFile {
                path: seed_path.clone(),
            },
        });
        steps.push(ApplyStep {
            description: format!("write NoCloud seed image for {}", node.name),
            kind: ApplyStepKind::WriteBinaryFile {
                path: seed_path.clone(),
                contents: render_nocloud_seed_image(
                    rendered_artifact_contents(
                        &artifacts,
                        &format!(
                            "nodes/{}/cloud-init/user-data",
                            safe_path_segment(&node.name)
                        ),
                    )?,
                    rendered_artifact_contents(
                        &artifacts,
                        &format!(
                            "nodes/{}/cloud-init/meta-data",
                            safe_path_segment(&node.name)
                        ),
                    )?,
                )?,
            },
        });
    }

    let systemd_units = artifacts
        .files
        .iter()
        .filter_map(|file| {
            if file.relative_path.contains("/systemd/") {
                let name = Path::new(&file.relative_path).file_name()?.to_str()?;
                Some((name.to_string(), file.contents.clone()))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();

    for (unit_name, contents) in &systemd_units {
        let target = Path::new(&apply_options.systemd_unit_dir).join(unit_name);
        steps.push(ApplyStep {
            description: format!("install systemd unit {unit_name}"),
            kind: ApplyStepKind::WriteFile {
                path: target.display().to_string(),
                contents: contents.clone(),
            },
        });
    }
    if !systemd_units.is_empty() {
        steps.push(ApplyStep {
            description: "reload systemd units".to_string(),
            kind: ApplyStepKind::Command {
                command: CommandSpec::new(
                    apply_options.systemctl_path.clone(),
                    ["daemon-reload".to_string()],
                ),
                creates: None,
            },
        });
    }
    for (unit_name, _) in &systemd_units {
        steps.push(ApplyStep {
            description: format!("enable and start {unit_name}"),
            kind: ApplyStepKind::Command {
                command: CommandSpec::new(
                    apply_options.systemctl_path.clone(),
                    ["enable".to_string(), "--now".to_string(), unit_name.clone()],
                ),
                creates: None,
            },
        });
    }

    for node in &config.nodes {
        let domain_xml = artifact_path(
            apply_options,
            &format!("nodes/{}/domain.xml", safe_path_segment(&node.name)),
        );
        steps.push(ApplyStep {
            description: format!("define libvirt domain {}", node.domain),
            kind: ApplyStepKind::Command {
                command: virsh_command(
                    apply_options,
                    config,
                    ["define".to_string(), domain_xml.display().to_string()],
                ),
                creates: None,
            },
        });
        if node.autostart {
            steps.push(ApplyStep {
                description: format!("enable libvirt autostart for {}", node.domain),
                kind: ApplyStepKind::Command {
                    command: virsh_command(
                        apply_options,
                        config,
                        ["autostart".to_string(), node.domain.clone()],
                    ),
                    creates: None,
                },
            });
        }
        if apply_options.start_domains {
            steps.push(ApplyStep {
                description: format!("start libvirt domain {}", node.domain),
                kind: ApplyStepKind::Command {
                    command: virsh_command(
                        apply_options,
                        config,
                        ["start".to_string(), node.domain.clone()],
                    ),
                    creates: None,
                },
            });
        }
    }

    Ok(HostApplyPlan { steps })
}

pub fn plan_host_reconcile(
    config: &HostConfig,
    render_options: &ArtifactRenderOptions,
    apply_options: &HostApplyPlanOptions,
    actual: &HostActualState,
) -> Result<HostReconcilePlan, ArtifactRenderError> {
    let apply_plan = plan_host_apply(config, render_options, apply_options)?;
    let mut steps = Vec::new();
    let mut changed_units = BTreeSet::new();
    let desired_binary_hashes = apply_plan
        .steps
        .iter()
        .filter_map(|step| match &step.kind {
            ApplyStepKind::WriteBinaryFile { path, contents } => {
                Some((path.clone(), content_hash(contents)))
            }
            _ => None,
        })
        .collect::<BTreeMap<_, _>>();
    let desired_domains = desired_domain_states(config, apply_options, render_options)?;
    let desired_root_disks = desired_root_disk_states(config);
    let desired_virtiofs_units = desired_virtiofs_unit_states(config, render_options);
    let systemd_unit_dir = Path::new(&apply_options.systemd_unit_dir);

    for step in &apply_plan.steps {
        match &step.kind {
            ApplyStepKind::EnsureDirectory { path } => {
                let actual_path = actual_path(actual, path);
                if actual_path.kind == PathActualKind::Directory {
                    steps.push(skip(&step.description, "directory already exists"));
                } else {
                    steps.push(apply(
                        step,
                        operation_for_apply_step(step, apply_options, &desired_domains),
                    ));
                }
            }
            ApplyStepKind::WriteFile { path, contents } => {
                let desired_hash = content_hash(contents.as_bytes());
                let actual_path = actual_path(actual, path);
                if actual_path.kind == PathActualKind::File
                    && actual_path.content_hash.as_deref() == Some(desired_hash.as_str())
                {
                    steps.push(skip(
                        &step.description,
                        "file already matches desired content",
                    ));
                    continue;
                }

                if let Some(unit_name) = systemd_unit_name_for_path(systemd_unit_dir, path) {
                    changed_units.insert(unit_name);
                }
                steps.push(apply(
                    step,
                    operation_for_apply_step(step, apply_options, &desired_domains),
                ));
            }
            ApplyStepKind::WriteBinaryFile { path, contents } => {
                let desired_hash = content_hash(contents);
                let actual_path = actual_path(actual, path);
                if actual_path.kind == PathActualKind::File
                    && actual_path.content_hash.as_deref() == Some(desired_hash.as_str())
                {
                    steps.push(skip(
                        &step.description,
                        "binary file already matches desired content",
                    ));
                } else {
                    steps.push(apply(
                        step,
                        operation_for_apply_step(step, apply_options, &desired_domains),
                    ));
                }
            }
            ApplyStepKind::RemoveFile { path } => {
                let actual_path = actual_path(actual, path);
                if let Some(desired_hash) = desired_binary_hashes.get(path)
                    && actual_path.kind == PathActualKind::File
                    && actual_path.content_hash.as_deref() == Some(desired_hash.as_str())
                {
                    steps.push(skip(
                        &step.description,
                        "following binary write is already current",
                    ));
                } else if desired_binary_hashes.contains_key(path) {
                    steps.push(skip(
                        &step.description,
                        "following seed image rewrite will replace content",
                    ));
                } else if actual_path.exists() {
                    steps.push(apply(
                        step,
                        operation_for_apply_step(step, apply_options, &desired_domains),
                    ));
                } else {
                    steps.push(skip(&step.description, "path is already absent"));
                }
            }
            ApplyStepKind::Command { command, creates } => {
                reconcile_command_step(
                    &mut steps,
                    step,
                    command,
                    creates.as_deref(),
                    apply_options,
                    actual,
                    &changed_units,
                    &desired_domains,
                    &desired_root_disks,
                    &desired_virtiofs_units,
                );
            }
        }
    }

    Ok(HostReconcilePlan { steps })
}

fn reconcile_command_step(
    steps: &mut Vec<ReconcileStep>,
    step: &ApplyStep,
    command: &CommandSpec,
    creates: Option<&str>,
    apply_options: &HostApplyPlanOptions,
    actual: &HostActualState,
    changed_units: &BTreeSet<String>,
    desired_domains: &BTreeMap<String, DesiredDomainState>,
    desired_root_disks: &BTreeMap<String, DesiredRootDiskState>,
    desired_virtiofs_units: &BTreeMap<String, DesiredVirtiofsUnitState>,
) {
    if let Some(path) = creates {
        if actual_path(actual, path).exists() {
            if command.program == apply_options.qemu_img_path {
                reconcile_existing_qemu_image(
                    steps,
                    step,
                    command,
                    path,
                    actual,
                    desired_root_disks.get(path),
                );
            } else {
                steps.push(skip(&step.description, "create target already exists"));
            }
            return;
        }
    }

    if !tool_found(actual, &command.program) {
        steps.push(refuse(
            &step.description,
            Some(operation_for_apply_step(
                step,
                apply_options,
                desired_domains,
            )),
            format!("required tool {} was not found", command.program),
        ));
        return;
    }

    if command.program == apply_options.qemu_img_path
        && command.args.first().map(String::as_str) == Some("create")
        && let Some(path) = creates
    {
        reconcile_missing_root_disk_create(
            steps,
            step,
            command,
            path,
            actual,
            desired_root_disks.get(path),
        );
        return;
    }

    if command.program == apply_options.systemctl_path {
        reconcile_systemctl_step(
            steps,
            step,
            command,
            actual,
            changed_units,
            desired_virtiofs_units,
        );
        return;
    }

    if command.program == apply_options.virsh_path {
        reconcile_virsh_step(
            steps,
            step,
            command,
            actual,
            desired_domains,
            apply_options.allow_running_domain_redefine,
            apply_options.allow_domain_adoption,
        );
        return;
    }

    steps.push(apply(
        step,
        operation_for_apply_step(step, apply_options, desired_domains),
    ));
}

fn reconcile_systemctl_step(
    steps: &mut Vec<ReconcileStep>,
    step: &ApplyStep,
    command: &CommandSpec,
    actual: &HostActualState,
    changed_units: &BTreeSet<String>,
    desired_virtiofs_units: &BTreeMap<String, DesiredVirtiofsUnitState>,
) {
    if command.args == ["daemon-reload"] {
        if changed_units.is_empty() {
            steps.push(skip(&step.description, "no systemd unit files changed"));
        } else {
            steps.push(apply(
                step,
                ReconcileOperation::ReloadSystemdUnits {
                    command: command.clone(),
                },
            ));
        }
        return;
    }

    if command.args.len() == 3 && command.args[0] == "enable" && command.args[1] == "--now" {
        let unit_name = &command.args[2];
        let unit = actual.systemd_units.get(unit_name);
        let enabled = unit.and_then(|unit| unit.enabled).unwrap_or(false);
        let active = unit.and_then(|unit| unit.active).unwrap_or(false);
        let unit_changed = changed_units.contains(unit_name);
        let socket_path = desired_virtiofs_units
            .get(unit_name)
            .map(|unit| unit.socket_path.clone())
            .unwrap_or_default();

        if enabled && active && !unit_changed {
            steps.push(skip(
                &step.description,
                "service is already enabled and active",
            ));
        } else if enabled && active && unit_changed {
            steps.push(ReconcileStep {
                description: format!("restart changed systemd unit {unit_name}"),
                kind: ReconcileStepKind::Apply(ReconcileOperation::RestartVirtiofsdService {
                    unit_name: unit_name.clone(),
                    socket_path,
                    command: CommandSpec::new(
                        command.program.clone(),
                        ["restart".to_string(), unit_name.clone()],
                    ),
                }),
            });
        } else {
            steps.push(apply(
                step,
                ReconcileOperation::EnableAndStartVirtiofsdService {
                    unit_name: unit_name.clone(),
                    socket_path,
                    command: command.clone(),
                },
            ));
        }
        return;
    }

    steps.push(apply(
        step,
        ReconcileOperation::RunCommand {
            command: command.clone(),
            creates: None,
        },
    ));
}

fn reconcile_missing_root_disk_create(
    steps: &mut Vec<ReconcileStep>,
    step: &ApplyStep,
    command: &CommandSpec,
    path: &str,
    actual: &HostActualState,
    desired: Option<&DesiredRootDiskState>,
) {
    let operation = ReconcileOperation::CreateRootDisk {
        path: path.to_string(),
        command: command.clone(),
    };
    let Some(desired) = desired else {
        steps.push(refuse(
            &step.description,
            Some(operation),
            "root disk creation target was not present in desired node config",
        ));
        return;
    };

    let Some(source_image) = desired.source_image.as_deref() else {
        steps.push(apply(step, operation));
        return;
    };
    if actual_path(actual, source_image).kind != PathActualKind::File {
        steps.push(refuse(
            &step.description,
            Some(operation),
            format!("base image {source_image} does not exist or is not a regular file"),
        ));
        return;
    }

    let Some(source_format) = desired.source_format.as_deref() else {
        steps.push(refuse(
            &step.description,
            Some(operation),
            format!("base image {source_image} format is not configured"),
        ));
        return;
    };
    let Some(source_info) = actual.qemu_images.get(source_image) else {
        steps.push(refuse(
            &step.description,
            Some(operation),
            format!("base image {source_image} qemu-img info was unavailable"),
        ));
        return;
    };
    if source_info.format.as_deref() != Some(source_format) {
        steps.push(refuse(
            &step.description,
            Some(operation),
            format!(
                "base image {source_image} format {:?} does not match expected {source_format}",
                source_info.format
            ),
        ));
        return;
    }

    let Some(expected_checksum) = desired.source_checksum.as_deref() else {
        steps.push(refuse(
            &step.description,
            Some(operation),
            format!("base image {source_image} checksum is not configured"),
        ));
        return;
    };
    let Some(expected_checksum) = normalize_sha256(expected_checksum) else {
        steps.push(refuse(
            &step.description,
            Some(operation),
            format!("base image {source_image} checksum is not a valid sha256 value"),
        ));
        return;
    };
    let actual_checksum = actual_path(actual, source_image)
        .sha256
        .as_deref()
        .and_then(normalize_sha256);
    if actual_checksum.as_deref() != Some(expected_checksum.as_str()) {
        steps.push(refuse(
            &step.description,
            Some(operation),
            format!("base image {source_image} checksum does not match configured sha256"),
        ));
        return;
    }

    steps.push(apply(step, operation));
}

fn reconcile_existing_qemu_image(
    steps: &mut Vec<ReconcileStep>,
    step: &ApplyStep,
    command: &CommandSpec,
    path: &str,
    actual: &HostActualState,
    desired: Option<&DesiredRootDiskState>,
) {
    if !tool_found(actual, &command.program) {
        steps.push(refuse(
            &step.description,
            Some(ReconcileOperation::CreateRootDisk {
                path: path.to_string(),
                command: command.clone(),
            }),
            format!(
                "required tool {} was not found; cannot verify existing root disk",
                command.program
            ),
        ));
        return;
    }

    let Some(actual_image) = actual.qemu_images.get(path) else {
        steps.push(refuse(
            &step.description,
            Some(ReconcileOperation::CreateRootDisk {
                path: path.to_string(),
                command: command.clone(),
            }),
            "root disk already exists but qemu-img info was unavailable; destructive replacement refused",
        ));
        return;
    };

    let Some(desired) = desired else {
        steps.push(refuse(
            &step.description,
            Some(ReconcileOperation::CreateRootDisk {
                path: path.to_string(),
                command: command.clone(),
            }),
            "existing root disk target was not present in desired node config",
        ));
        return;
    };

    if let Some(expected_format) = desired.format.as_deref()
        && actual_image.format.as_deref() != Some(expected_format)
    {
        steps.push(refuse(
            &step.description,
            Some(ReconcileOperation::CreateRootDisk {
                path: path.to_string(),
                command: command.clone(),
            }),
            format!(
                "root disk {path} exists with format {:?}, expected {expected_format}; destructive replacement refused",
                actual_image.format
            ),
        ));
        return;
    }
    if let Some(expected_backing) = desired.source_image.as_deref()
        && actual_image.backing_file.as_deref() != Some(expected_backing)
    {
        steps.push(refuse(
            &step.description,
            Some(ReconcileOperation::CreateRootDisk {
                path: path.to_string(),
                command: command.clone(),
            }),
            format!(
                "root disk {path} exists with backing {:?}, expected {expected_backing}; destructive replacement refused",
                actual_image.backing_file
            ),
        ));
        return;
    }

    let Some(actual_size) = actual_image.virtual_size else {
        steps.push(refuse(
            &step.description,
            Some(ReconcileOperation::CreateRootDisk {
                path: path.to_string(),
                command: command.clone(),
            }),
            "root disk already exists but qemu-img virtual-size was unavailable; resize safety could not be checked",
        ));
        return;
    };
    if actual_size < desired.size_bytes {
        steps.push(ReconcileStep {
            description: format!("resize root disk {path}"),
            kind: ReconcileStepKind::Apply(ReconcileOperation::ResizeRootDisk {
                path: path.to_string(),
                desired_size_bytes: desired.size_bytes,
                command: qemu_img_resize_command(&command.program, path, &desired.size_arg),
            }),
        });
        return;
    }
    if actual_size > desired.size_bytes {
        steps.push(refuse(
            &step.description,
            Some(ReconcileOperation::ResizeRootDisk {
                path: path.to_string(),
                desired_size_bytes: desired.size_bytes,
                command: qemu_img_resize_command(&command.program, path, &desired.size_arg),
            }),
            format!(
                "root disk {path} is larger than desired size {}; destructive shrink refused",
                desired.size_arg
            ),
        ));
        return;
    }

    steps.push(skip(
        &step.description,
        "root disk already exists and qemu-img info matches desired shape",
    ));
}

fn reconcile_virsh_step(
    steps: &mut Vec<ReconcileStep>,
    step: &ApplyStep,
    command: &CommandSpec,
    actual: &HostActualState,
    desired_domains: &BTreeMap<String, DesiredDomainState>,
    allow_running_domain_redefine: bool,
    allow_domain_adoption: bool,
) {
    let Some(subcommand) = command.args.get(2).map(String::as_str) else {
        steps.push(apply(
            step,
            ReconcileOperation::RunCommand {
                command: command.clone(),
                creates: None,
            },
        ));
        return;
    };

    match subcommand {
        "define" => {
            let Some(xml_path) = command.args.get(3) else {
                steps.push(apply(
                    step,
                    ReconcileOperation::RunCommand {
                        command: command.clone(),
                        creates: None,
                    },
                ));
                return;
            };
            let Some(desired) = desired_domains
                .values()
                .find(|desired| desired.xml_path == *xml_path)
            else {
                steps.push(apply(
                    step,
                    ReconcileOperation::RunCommand {
                        command: command.clone(),
                        creates: None,
                    },
                ));
                return;
            };
            let domain = actual.domains.get(&desired.domain);
            match domain {
                Some(domain) if domain.exists && !domain.managed && !allow_domain_adoption => {
                    steps.push(refuse(
                        &step.description,
                        Some(ReconcileOperation::DefineDomain {
                            domain: desired.domain.clone(),
                            xml_path: xml_path.clone(),
                            command: command.clone(),
                        }),
                        format!(
                            "domain {} exists without nas-csi metadata; adoption requires explicit allow_domain_adoption",
                            desired.domain
                        ),
                    ));
                }
                Some(domain)
                    if domain.exists
                        && domain.desired_hash.as_deref()
                            == Some(desired.desired_hash.as_str()) =>
                {
                    steps.push(skip(
                        &step.description,
                        "domain metadata already matches desired state",
                    ));
                }
                Some(domain)
                    if domain.exists && domain.active && !allow_running_domain_redefine =>
                {
                    steps.push(refuse(
                        &step.description,
                        Some(ReconcileOperation::RedefineDomainRequiresShutdown {
                            domain: desired.domain.clone(),
                            xml_path: xml_path.clone(),
                        }),
                        format!(
                            "domain {} is running and XML differs; stop or drain it before redefining",
                            desired.domain
                        ),
                    ));
                }
                Some(domain) if domain.exists => steps.push(apply(
                    step,
                    ReconcileOperation::RedefineDomain {
                        domain: desired.domain.clone(),
                        xml_path: xml_path.clone(),
                        previous_xml: domain.xml.clone(),
                        command: command.clone(),
                    },
                )),
                _ => steps.push(apply(
                    step,
                    ReconcileOperation::DefineDomain {
                        domain: desired.domain.clone(),
                        xml_path: xml_path.clone(),
                        command: command.clone(),
                    },
                )),
            }
        }
        "autostart" => {
            let Some(domain_name) = command.args.get(3) else {
                steps.push(apply(
                    step,
                    ReconcileOperation::RunCommand {
                        command: command.clone(),
                        creates: None,
                    },
                ));
                return;
            };
            if actual
                .domains
                .get(domain_name)
                .and_then(|domain| domain.autostart)
                .unwrap_or(false)
            {
                steps.push(skip(
                    &step.description,
                    "domain autostart is already enabled",
                ));
            } else {
                steps.push(apply(
                    step,
                    ReconcileOperation::EnableDomainAutostart {
                        domain: domain_name.clone(),
                        command: command.clone(),
                    },
                ));
            }
        }
        "start" => {
            let Some(domain_name) = command.args.get(3) else {
                steps.push(apply(
                    step,
                    ReconcileOperation::RunCommand {
                        command: command.clone(),
                        creates: None,
                    },
                ));
                return;
            };
            if actual
                .domains
                .get(domain_name)
                .map(|domain| domain.active)
                .unwrap_or(false)
            {
                steps.push(skip(&step.description, "domain is already running"));
            } else {
                steps.push(apply(
                    step,
                    ReconcileOperation::StartDomain {
                        domain: domain_name.clone(),
                        command: command.clone(),
                    },
                ));
            }
        }
        _ => {
            steps.push(apply(
                step,
                ReconcileOperation::RunCommand {
                    command: command.clone(),
                    creates: None,
                },
            ));
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DesiredDomainState {
    domain: String,
    xml_path: String,
    desired_hash: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DesiredRootDiskState {
    format: Option<String>,
    source_image: Option<String>,
    source_format: Option<String>,
    source_checksum: Option<String>,
    size_arg: String,
    size_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DesiredVirtiofsUnitState {
    socket_path: String,
}

fn desired_root_disk_states(config: &HostConfig) -> BTreeMap<String, DesiredRootDiskState> {
    config
        .nodes
        .iter()
        .map(|node| {
            (
                node.root_disk.image.clone(),
                DesiredRootDiskState {
                    format: Some(disk_format_name(node.root_disk.format).to_string()),
                    source_image: node.root_disk.source_image.clone(),
                    source_format: node
                        .root_disk
                        .source_format
                        .map(disk_format_name)
                        .map(str::to_string),
                    source_checksum: node.root_disk.source_checksum.clone(),
                    size_arg: format!("{}G", node.root_disk.size_gib),
                    size_bytes: gib_to_bytes(node.root_disk.size_gib),
                },
            )
        })
        .collect()
}

fn desired_virtiofs_unit_states(
    config: &HostConfig,
    render_options: &ArtifactRenderOptions,
) -> BTreeMap<String, DesiredVirtiofsUnitState> {
    let mut units = BTreeMap::new();
    for node in &config.nodes {
        for export_id in &node.exports {
            units.insert(
                format!(
                    "{}.service",
                    virtiofsd_service_name(&node.domain, export_id)
                ),
                DesiredVirtiofsUnitState {
                    socket_path: virtiofs_socket_path(render_options, &node.domain, export_id),
                },
            );
        }
    }
    units
}

fn desired_domain_states(
    config: &HostConfig,
    apply_options: &HostApplyPlanOptions,
    render_options: &ArtifactRenderOptions,
) -> Result<BTreeMap<String, DesiredDomainState>, ArtifactRenderError> {
    let mut desired = BTreeMap::new();
    for node in &config.nodes {
        let relative_path = format!("nodes/{}/domain.xml", safe_path_segment(&node.name));
        let xml_path = artifact_path(apply_options, &relative_path)
            .display()
            .to_string();
        let domain = domain_spec_from_node(config, node, render_options)?;
        desired.insert(
            node.domain.clone(),
            DesiredDomainState {
                domain: node.domain.clone(),
                xml_path,
                desired_hash: domain_desired_hash(&domain),
            },
        );
    }
    Ok(desired)
}

fn normalize_sha256(value: &str) -> Option<String> {
    let value = value
        .trim()
        .strip_prefix("sha256:")
        .unwrap_or_else(|| value.trim())
        .to_ascii_lowercase();
    (value.len() == 64 && value.chars().all(|ch| ch.is_ascii_hexdigit())).then_some(value)
}

fn actual_path<'a>(actual: &'a HostActualState, path: &str) -> &'a PathActualState {
    static MISSING: PathActualState = PathActualState {
        kind: PathActualKind::Missing,
        size: None,
        content_hash: None,
        sha256: None,
    };
    actual.paths.get(path).unwrap_or(&MISSING)
}

fn tool_found(actual: &HostActualState, program: &str) -> bool {
    actual
        .tools
        .get(program)
        .map(ToolActualState::is_found)
        .unwrap_or(false)
}

fn systemd_unit_name_for_path(systemd_unit_dir: &Path, path: &str) -> Option<String> {
    let path = Path::new(path);
    if path.parent()? != systemd_unit_dir {
        return None;
    }
    path.file_name()?.to_str().map(str::to_string)
}

fn operation_for_apply_step(
    step: &ApplyStep,
    apply_options: &HostApplyPlanOptions,
    desired_domains: &BTreeMap<String, DesiredDomainState>,
) -> ReconcileOperation {
    match &step.kind {
        ApplyStepKind::EnsureDirectory { path } => {
            ReconcileOperation::EnsureDirectory { path: path.clone() }
        }
        ApplyStepKind::WriteFile { path, contents } => {
            if let Some(unit_name) =
                systemd_unit_name_for_path(Path::new(&apply_options.systemd_unit_dir), path)
            {
                ReconcileOperation::InstallOrUpdateSystemdUnit {
                    unit_name,
                    path: path.clone(),
                    contents: contents.clone(),
                }
            } else {
                ReconcileOperation::WriteRenderedArtifact {
                    path: path.clone(),
                    contents: contents.clone(),
                }
            }
        }
        ApplyStepKind::WriteBinaryFile { path, contents } => ReconcileOperation::RewriteSeedImage {
            path: path.clone(),
            contents: contents.clone(),
        },
        ApplyStepKind::RemoveFile { path } => ReconcileOperation::RunCommand {
            command: CommandSpec::new("rm".to_string(), ["-f".to_string(), path.clone()]),
            creates: None,
        },
        ApplyStepKind::Command { command, creates } => {
            operation_for_command(command, creates.as_deref(), apply_options, desired_domains)
        }
    }
}

fn operation_for_command(
    command: &CommandSpec,
    creates: Option<&str>,
    apply_options: &HostApplyPlanOptions,
    desired_domains: &BTreeMap<String, DesiredDomainState>,
) -> ReconcileOperation {
    if command.program == apply_options.qemu_img_path
        && command.args.first().map(String::as_str) == Some("create")
        && let Some(path) = creates
    {
        return ReconcileOperation::CreateRootDisk {
            path: path.to_string(),
            command: command.clone(),
        };
    }

    if command.program == apply_options.systemctl_path {
        if command.args == ["daemon-reload"] {
            return ReconcileOperation::ReloadSystemdUnits {
                command: command.clone(),
            };
        }
        if command.args.len() == 3 && command.args[0] == "enable" && command.args[1] == "--now" {
            return ReconcileOperation::EnableAndStartVirtiofsdService {
                unit_name: command.args[2].clone(),
                socket_path: String::new(),
                command: command.clone(),
            };
        }
        if command.args.len() == 2 && command.args[0] == "restart" {
            return ReconcileOperation::RestartVirtiofsdService {
                unit_name: command.args[1].clone(),
                socket_path: String::new(),
                command: command.clone(),
            };
        }
    }

    if command.program == apply_options.virsh_path {
        match command.args.get(2).map(String::as_str) {
            Some("define") => {
                if let Some(xml_path) = command.args.get(3)
                    && let Some(desired) = desired_domains
                        .values()
                        .find(|desired| desired.xml_path == *xml_path)
                {
                    return ReconcileOperation::DefineDomain {
                        domain: desired.domain.clone(),
                        xml_path: xml_path.clone(),
                        command: command.clone(),
                    };
                }
            }
            Some("autostart") => {
                if let Some(domain) = command.args.get(3) {
                    return ReconcileOperation::EnableDomainAutostart {
                        domain: domain.clone(),
                        command: command.clone(),
                    };
                }
            }
            Some("start") => {
                if let Some(domain) = command.args.get(3) {
                    return ReconcileOperation::StartDomain {
                        domain: domain.clone(),
                        command: command.clone(),
                    };
                }
            }
            _ => {}
        }
    }

    ReconcileOperation::RunCommand {
        command: command.clone(),
        creates: creates.map(str::to_string),
    }
}

fn apply(step: &ApplyStep, operation: ReconcileOperation) -> ReconcileStep {
    ReconcileStep {
        description: step.description.clone(),
        kind: ReconcileStepKind::Apply(operation),
    }
}

fn skip(description: &str, reason: impl Into<String>) -> ReconcileStep {
    ReconcileStep {
        description: description.to_string(),
        kind: ReconcileStepKind::SkipAlreadyCorrect {
            reason: reason.into(),
        },
    }
}

fn refuse(
    description: &str,
    operation: Option<ReconcileOperation>,
    reason: impl Into<String>,
) -> ReconcileStep {
    ReconcileStep {
        description: description.to_string(),
        kind: ReconcileStepKind::Refuse {
            operation,
            reason: reason.into(),
        },
    }
}

pub fn qemu_img_create_command(program: &str, node: &NodeConfig) -> CommandSpec {
    let mut args = vec![
        "create".to_string(),
        "-f".to_string(),
        disk_format_name(node.root_disk.format).to_string(),
    ];
    if let Some(source_image) = &node.root_disk.source_image {
        args.push("-F".to_string());
        args.push(
            node.root_disk
                .source_format
                .map(disk_format_name)
                .unwrap_or("qcow2")
                .to_string(),
        );
        args.push("-b".to_string());
        args.push(source_image.clone());
    }
    args.push(node.root_disk.image.clone());
    args.push(format!("{}G", node.root_disk.size_gib));
    CommandSpec::new(program.to_string(), args)
}

pub fn qemu_img_resize_command(program: &str, path: &str, size_arg: &str) -> CommandSpec {
    CommandSpec::new(
        program.to_string(),
        ["resize".to_string(), path.to_string(), size_arg.to_string()],
    )
}

fn virsh_command(
    apply_options: &HostApplyPlanOptions,
    config: &HostConfig,
    args: impl IntoIterator<Item = String>,
) -> CommandSpec {
    let mut full_args = vec!["-c".to_string(), config.libvirt.uri.clone()];
    full_args.extend(args);
    CommandSpec::new(apply_options.virsh_path.clone(), full_args)
}

pub fn domain_spec_from_node(
    config: &HostConfig,
    node: &NodeConfig,
    options: &ArtifactRenderOptions,
) -> Result<DomainSpec, ArtifactRenderError> {
    let mut virtiofs_exports = Vec::new();
    for export_id in &node.exports {
        let export =
            config
                .exports
                .get(export_id)
                .ok_or_else(|| ArtifactRenderError::MissingExport {
                    node: node.name.clone(),
                    export: export_id.clone(),
                })?;
        virtiofs_exports.push(VirtiofsExport {
            socket_path: virtiofs_socket_path(options, &node.domain, export_id),
            tag: export.tag.clone(),
            queue_size: options.virtiofs_queue_size,
        });
    }

    Ok(DomainSpec {
        name: node.domain.clone(),
        memory_mib: node.memory_mib,
        vcpus: node.vcpus,
        machine: node.machine.clone(),
        cpu_mode: node.cpu.clone(),
        root_disk_path: node.root_disk.image.clone(),
        root_disk_format: disk_format_name(node.root_disk.format).to_string(),
        seed_disk_path: Some(seed_image_path(&node.root_disk.image, &node.domain)),
        bridge: node.network.bridge.clone(),
        mac_address: node.network.mac.clone(),
        virtiofs_exports,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VirtiofsdUnitSpec {
    pub description: String,
    pub virtiofsd_path: String,
    pub socket_path: String,
    pub source_path: String,
    pub cache: String,
    pub sandbox: String,
    pub read_only: bool,
}

pub fn render_virtiofsd_systemd_unit(spec: &VirtiofsdUnitSpec) -> String {
    let runtime_directory = systemd_runtime_directory(&spec.socket_path)
        .map(|directory| format!("RuntimeDirectory={directory}\nRuntimeDirectoryMode=0750\n"))
        .unwrap_or_default();
    let mut exec_start = vec![
        systemd_quote(&spec.virtiofsd_path),
        "--socket-path".to_string(),
        systemd_quote(&spec.socket_path),
        "--shared-dir".to_string(),
        systemd_quote(&spec.source_path),
        "--cache".to_string(),
        systemd_quote(&spec.cache),
        "--sandbox".to_string(),
        systemd_quote(&spec.sandbox),
    ];
    if spec.read_only {
        exec_start.push("--readonly".to_string());
    }

    format!(
        "[Unit]\nDescription={}\nAfter=local-fs.target\nRequiresMountsFor={}\n\n[Service]\nType=simple\n{}ExecStartPre=/usr/bin/rm -f {}\nExecStart={}\nRestart=on-failure\nRestartSec=2s\n\n[Install]\nWantedBy=multi-user.target\n",
        spec.description,
        spec.source_path,
        runtime_directory,
        systemd_quote(&spec.socket_path),
        exec_start.join(" ")
    )
}

pub fn virtiofs_socket_path(
    options: &ArtifactRenderOptions,
    domain: &str,
    export_id: &str,
) -> String {
    format!(
        "{}/virtiofs/{}/{}.sock",
        options.runtime_dir.trim_end_matches('/'),
        safe_path_segment(domain),
        safe_path_segment(export_id)
    )
}

pub fn virtiofsd_service_name(domain: &str, export_id: &str) -> String {
    format!(
        "nascsi-virtiofsd-{}-{}",
        safe_path_segment(domain),
        safe_path_segment(export_id)
    )
}

fn cloud_init_for_node(
    config: &HostConfig,
    node: &NodeConfig,
    options: &ArtifactRenderOptions,
) -> Result<CloudInitSpec, ArtifactRenderError> {
    let k3s_config = k3s_config_for_node(config, node, options)?;
    let node_config = node_runtime_config(config, node)?;
    let virtiofs_mounts = guest_virtiofs_mounts(config, node)?;
    let mut run_commands = vec!["systemctl enable --now qemu-guest-agent".to_string()];
    if !virtiofs_mounts.is_empty() {
        run_commands.push(format!(
            "mkdir -p {}",
            virtiofs_mounts
                .iter()
                .map(|mount| shell_quote(&mount.mount_path))
                .collect::<Vec<_>>()
                .join(" ")
        ));
        run_commands.push(
            "grep -q '# nas-csi virtiofs mounts' /etc/fstab || cat /etc/nas-csi/virtiofs.fstab >> /etc/fstab"
                .to_string(),
        );
        run_commands.push("mount -a -t virtiofs".to_string());
    }
    let role = match node.role {
        NodeRole::Server => "server",
        NodeRole::Agent => "agent",
    };

    let token_content = options
        .k3s_token
        .as_deref()
        .unwrap_or("REPLACE_WITH_K3S_TOKEN");

    Ok(CloudInitSpec {
        hostname: node.name.clone(),
        ssh_authorized_keys: options.ssh_authorized_keys.clone(),
        packages: vec!["qemu-guest-agent".to_string()],
        write_files: vec![
            CloudInitWriteFile {
                path: "/etc/rancher/k3s/config.yaml".to_string(),
                permissions: "0600".to_string(),
                content: k3s_config,
            },
            CloudInitWriteFile {
                path: options.k3s_token_path_in_vm.clone(),
                permissions: "0600".to_string(),
                content: format!("{token_content}\n"),
            },
            CloudInitWriteFile {
                path: "/etc/nas-csi/node.yaml".to_string(),
                permissions: "0644".to_string(),
                content: node_config,
            },
            CloudInitWriteFile {
                path: "/etc/nas-csi/virtiofs.fstab".to_string(),
                permissions: "0644".to_string(),
                content: render_virtiofs_fstab(&virtiofs_mounts),
            },
        ],
        run_commands: {
            run_commands.push(format!(
                "curl -sfL https://get.k3s.io | INSTALL_K3S_VERSION={} INSTALL_K3S_EXEC={} sh -",
                shell_quote(&config.cluster.version),
                shell_quote(role)
            ));
            run_commands
        },
    })
}

fn k3s_config_for_node(
    config: &HostConfig,
    node: &NodeConfig,
    options: &ArtifactRenderOptions,
) -> Result<String, serde_yml::Error> {
    let role = match node.role {
        NodeRole::Server => K3sRole::Server,
        NodeRole::Agent => K3sRole::Agent,
    };
    let labels = node
        .k3s
        .labels
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    let taints = node
        .k3s
        .taints
        .iter()
        .map(|taint| format!("{}={}:{}", taint.key, taint.value, taint.effect))
        .collect();
    let is_server = node.role == NodeRole::Server;

    render_k3s_config(&K3sConfigInput {
        role,
        token: None,
        token_file: Some(options.k3s_token_path_in_vm.clone()),
        server_url: node.k3s.server.clone(),
        cluster_init: node.k3s.cluster_init,
        tls_sans: if is_server {
            config.cluster.api_server.tls_sans.clone()
        } else {
            Vec::new()
        },
        node_labels: labels,
        node_taints: taints,
        disable: if is_server {
            config.cluster.disable.clone()
        } else {
            Vec::new()
        },
        cluster_cidr: is_server.then(|| config.cluster.network.cluster_cidr.clone()),
        service_cidr: is_server.then(|| config.cluster.network.service_cidr.clone()),
        flannel_backend: is_server.then(|| config.cluster.network.flannel_backend.clone()),
    })
}

fn node_runtime_config(
    config: &HostConfig,
    node: &NodeConfig,
) -> Result<String, ArtifactRenderError> {
    let mut exports = Vec::new();
    for export_id in &node.exports {
        let export =
            config
                .exports
                .get(export_id)
                .ok_or_else(|| ArtifactRenderError::MissingExport {
                    node: node.name.clone(),
                    export: export_id.clone(),
                })?;
        exports.push(NodeRuntimeExport {
            id: export_id.clone(),
            dataset: export.dataset.clone(),
            source_path: export.source_path.clone(),
            tag: export.tag.clone(),
            policy: export.policy.clone(),
            access: export.access,
            guest_mount_path: guest_virtiofs_mount_path(export_id),
        });
    }

    let runtime = NodeRuntimeConfig {
        api_version: API_VERSION.to_string(),
        kind: "NodeRuntimeConfig".to_string(),
        node_name: node.name.clone(),
        domain: node.domain.clone(),
        exports,
    };
    serde_yml::to_string(&runtime).map_err(ArtifactRenderError::Yaml)
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct GuestVirtiofsMount {
    tag: String,
    mount_path: String,
    read_only: bool,
}

fn guest_virtiofs_mounts(
    config: &HostConfig,
    node: &NodeConfig,
) -> Result<Vec<GuestVirtiofsMount>, ArtifactRenderError> {
    let mut mounts = Vec::new();
    for export_id in &node.exports {
        let export =
            config
                .exports
                .get(export_id)
                .ok_or_else(|| ArtifactRenderError::MissingExport {
                    node: node.name.clone(),
                    export: export_id.clone(),
                })?;
        mounts.push(GuestVirtiofsMount {
            tag: export.tag.clone(),
            mount_path: guest_virtiofs_mount_path(export_id),
            read_only: export.access == AccessMode::ReadOnly,
        });
    }
    Ok(mounts)
}

fn render_virtiofs_fstab(mounts: &[GuestVirtiofsMount]) -> String {
    let mut output = String::from("\n# nas-csi virtiofs mounts\n");
    for mount in mounts {
        let access = if mount.read_only { "ro" } else { "rw" };
        output.push_str(&format!(
            "{} {} virtiofs {},nofail 0 0\n",
            fstab_escape(&mount.tag),
            fstab_escape(&mount.mount_path),
            access
        ));
    }
    output
}

fn guest_virtiofs_mount_path(export_id: &str) -> String {
    format!("/var/lib/nas-csi/virtiofs/{}", safe_path_segment(export_id))
}

fn rendered_artifact_contents<'a>(
    artifacts: &'a RenderedHostArtifacts,
    relative_path: &str,
) -> Result<&'a str, ArtifactRenderError> {
    artifacts
        .files
        .iter()
        .find(|file| file.relative_path == relative_path)
        .map(|file| file.contents.as_str())
        .ok_or_else(|| ArtifactRenderError::MissingArtifact {
            relative_path: relative_path.to_string(),
        })
}

fn artifact_path(options: &HostApplyPlanOptions, relative_path: &str) -> PathBuf {
    Path::new(&options.artifact_dir).join(relative_path)
}

fn parent_dir(path: &str) -> String {
    Path::new(path)
        .parent()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| ".".to_string())
}

fn round_up(value: usize, multiple: usize) -> usize {
    if value == 0 {
        return 0;
    }
    value.div_ceil(multiple) * multiple
}

fn gib_to_bytes(value: u64) -> u64 {
    value.saturating_mul(1024 * 1024 * 1024)
}

pub fn content_hash(contents: &[u8]) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in contents {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

fn disk_format_name(format: DiskFormat) -> &'static str {
    match format {
        DiskFormat::Qcow2 => "qcow2",
        DiskFormat::Raw => "raw",
    }
}

pub fn seed_image_path(root_disk_path: &str, domain: &str) -> String {
    let root_path = Path::new(root_disk_path);
    if let Some(parent) = root_path.parent() {
        return parent
            .join(format!("{domain}-seed.img"))
            .display()
            .to_string();
    }
    format!("{domain}-seed.img")
}

fn safe_path_segment(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn systemd_quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn fstab_escape(value: &str) -> String {
    value
        .chars()
        .flat_map(|ch| match ch {
            ' ' => "\\040".chars().collect::<Vec<_>>(),
            '\t' => "\\011".chars().collect::<Vec<_>>(),
            '\n' => "\\012".chars().collect::<Vec<_>>(),
            '\\' => "\\134".chars().collect::<Vec<_>>(),
            _ => vec![ch],
        })
        .collect()
}

fn systemd_runtime_directory(socket_path: &str) -> Option<String> {
    let path = Path::new(socket_path);
    let parent = path.parent()?.to_str()?;
    parent.strip_prefix("/run/").map(str::to_string)
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('\'', "&apos;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CloudInitSpec {
    pub hostname: String,
    pub ssh_authorized_keys: Vec<String>,
    pub packages: Vec<String>,
    pub write_files: Vec<CloudInitWriteFile>,
    pub run_commands: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CloudInitWriteFile {
    pub path: String,
    pub permissions: String,
    pub content: String,
}

pub fn render_cloud_init_user_data(spec: &CloudInitSpec) -> Result<String, serde_yml::Error> {
    let doc = CloudInitUserData {
        hostname: spec.hostname.as_str(),
        manage_etc_hosts: true,
        ssh_authorized_keys: none_if_empty(&spec.ssh_authorized_keys),
        packages: none_if_empty(&spec.packages),
        write_files: none_if_empty(&spec.write_files),
        run_cmd: none_if_empty(&spec.run_commands),
    };

    let mut output = String::from("#cloud-config\n");
    output.push_str(&serde_yml::to_string(&doc)?);
    Ok(output)
}

pub fn render_cloud_init_meta_data(instance_id: &str, local_hostname: &str) -> String {
    format!("instance-id: {instance_id}\nlocal-hostname: {local_hostname}\n")
}

fn none_if_empty<T>(values: &[T]) -> Option<&[T]> {
    if values.is_empty() {
        None
    } else {
        Some(values)
    }
}

#[derive(Serialize)]
struct CloudInitUserData<'a> {
    hostname: &'a str,
    manage_etc_hosts: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    ssh_authorized_keys: Option<&'a [String]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    packages: Option<&'a [String]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    write_files: Option<&'a [CloudInitWriteFile]>,
    #[serde(rename = "runcmd")]
    #[serde(skip_serializing_if = "Option::is_none")]
    run_cmd: Option<&'a [String]>,
}

impl Serialize for CloudInitWriteFile {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        #[derive(Serialize)]
        struct Helper<'a> {
            path: &'a str,
            permissions: &'a str,
            content: &'a str,
        }

        Helper {
            path: &self.path,
            permissions: &self.permissions,
            content: &self.content,
        }
        .serialize(serializer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_BASE_SHA256: &str =
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    use std::io::Read;

    #[test]
    fn renders_domain_with_shared_memory_and_virtiofs() {
        let xml = render_domain_xml(&DomainSpec {
            name: "nascsi-node-1".to_string(),
            memory_mib: 8192,
            vcpus: 4,
            machine: "q35".to_string(),
            cpu_mode: "host-passthrough".to_string(),
            root_disk_path: "/var/lib/nas-csi/node.qcow2".to_string(),
            root_disk_format: "qcow2".to_string(),
            seed_disk_path: Some("/var/lib/nas-csi/node-seed.img".to_string()),
            bridge: "br-test".to_string(),
            mac_address: "52:54:00:00:00:01".to_string(),
            virtiofs_exports: vec![VirtiofsExport {
                socket_path: "/run/nas-csi/virtiofs/node/export.sock".to_string(),
                tag: "nascsi_export".to_string(),
                queue_size: 1024,
            }],
        });

        assert!(xml.contains("<memoryBacking>"));
        assert!(xml.contains("<source type='memfd'/>"));
        assert!(xml.contains("<access mode='shared'/>"));
        assert!(xml.contains("<driver type='virtiofs' queue='1024'/>"));
        assert!(xml.contains("<target dir='nascsi_export'/>"));
        assert!(xml.contains("<metadata>"));
        assert!(xml.contains("<nas-csi:managed"));
        assert!(xml.contains("desired-domain-hash"));
        assert!(extract_domain_managed(&xml));
        assert_eq!(
            extract_domain_desired_hash(&xml),
            Some(domain_desired_hash(&DomainSpec {
                name: "nascsi-node-1".to_string(),
                memory_mib: 8192,
                vcpus: 4,
                machine: "q35".to_string(),
                cpu_mode: "host-passthrough".to_string(),
                root_disk_path: "/var/lib/nas-csi/node.qcow2".to_string(),
                root_disk_format: "qcow2".to_string(),
                seed_disk_path: Some("/var/lib/nas-csi/node-seed.img".to_string()),
                bridge: "br-test".to_string(),
                mac_address: "52:54:00:00:00:01".to_string(),
                virtiofs_exports: vec![VirtiofsExport {
                    socket_path: "/run/nas-csi/virtiofs/node/export.sock".to_string(),
                    tag: "nascsi_export".to_string(),
                    queue_size: 1024,
                }],
            }))
        );
    }

    #[test]
    fn escapes_xml_values() {
        let xml = render_domain_xml(&DomainSpec {
            name: "node&1".to_string(),
            memory_mib: 1024,
            vcpus: 1,
            machine: "q35".to_string(),
            cpu_mode: "host-passthrough".to_string(),
            root_disk_path: "/tmp/root&disk.qcow2".to_string(),
            root_disk_format: "qcow2".to_string(),
            seed_disk_path: None,
            bridge: "br0".to_string(),
            mac_address: "52:54:00:00:00:01".to_string(),
            virtiofs_exports: Vec::new(),
        });

        assert!(xml.contains("node&amp;1"));
        assert!(xml.contains("/tmp/root&amp;disk.qcow2"));
    }

    #[test]
    fn renders_cloud_init_user_data() {
        let user_data = render_cloud_init_user_data(&CloudInitSpec {
            hostname: "node-1".to_string(),
            ssh_authorized_keys: vec!["ssh-ed25519 test-key".to_string()],
            packages: vec!["qemu-guest-agent".to_string()],
            write_files: vec![CloudInitWriteFile {
                path: "/etc/nas-csi/node.yaml".to_string(),
                permissions: "0644".to_string(),
                content: "node_id: node-1\n".to_string(),
            }],
            run_commands: vec!["systemctl enable --now qemu-guest-agent".to_string()],
        })
        .expect("render cloud-init");

        assert!(user_data.starts_with("#cloud-config\n"));
        assert!(user_data.contains("hostname: node-1"));
        assert!(user_data.contains("ssh-ed25519 test-key"));
        assert!(user_data.contains("qemu-guest-agent"));
        assert!(user_data.contains("/etc/nas-csi/node.yaml"));
    }

    #[test]
    fn renders_cloud_init_meta_data() {
        let meta_data = render_cloud_init_meta_data("iid-node-1", "node-1");
        assert_eq!(
            meta_data,
            "instance-id: iid-node-1\nlocal-hostname: node-1\n"
        );
    }

    #[test]
    fn renders_virtiofsd_systemd_unit() {
        let unit = render_virtiofsd_systemd_unit(&VirtiofsdUnitSpec {
            description: "test virtiofsd".to_string(),
            virtiofsd_path: "/usr/libexec/virtiofsd".to_string(),
            socket_path: "/run/nas-csi/virtiofs/node/repos.sock".to_string(),
            source_path: "/mnt/pool/repos".to_string(),
            cache: "auto".to_string(),
            sandbox: "namespace".to_string(),
            read_only: true,
        });

        assert!(unit.contains("RequiresMountsFor=/mnt/pool/repos"));
        assert!(unit.contains("RuntimeDirectory=nas-csi/virtiofs/node"));
        assert!(unit.contains("RuntimeDirectoryMode=0750"));
        assert!(unit.contains("--socket-path"));
        assert!(unit.contains("--shared-dir"));
        assert!(unit.contains("--readonly"));
    }

    #[test]
    fn renders_host_artifacts_from_config() {
        let config = sample_host_config();
        let artifacts =
            render_host_artifacts(&config, &ArtifactRenderOptions::default()).expect("render");

        let paths = artifacts
            .files
            .iter()
            .map(|file| file.relative_path.as_str())
            .collect::<Vec<_>>();

        assert!(paths.contains(&"nodes/server-1/domain.xml"));
        assert!(paths.contains(&"nodes/server-1/cloud-init/user-data"));
        assert!(paths.contains(&"nodes/server-1/k3s/config.yaml"));
        assert!(
            paths
                .contains(&"nodes/server-1/systemd/nascsi-virtiofsd-nascsi-server-1-repos.service")
        );

        let domain_xml = artifacts
            .files
            .iter()
            .find(|file| file.relative_path == "nodes/server-1/domain.xml")
            .expect("domain xml");
        assert!(domain_xml.contents.contains("<target dir='nascsi_repos'/>"));
        assert!(
            domain_xml
                .contents
                .contains("/run/nas-csi/virtiofs/nascsi-server-1/repos.sock")
        );

        let user_data = artifacts
            .files
            .iter()
            .find(|file| file.relative_path == "nodes/server-1/cloud-init/user-data")
            .expect("user-data");
        assert!(user_data.contents.contains("REPLACE_WITH_K3S_TOKEN"));
        assert!(user_data.contents.contains("/etc/rancher/k3s/config.yaml"));
        assert!(user_data.contents.contains("/etc/nas-csi/virtiofs.fstab"));
        assert!(
            user_data
                .contents
                .contains("/var/lib/nas-csi/virtiofs/repos")
        );
        assert!(user_data.contents.contains("mount -a -t virtiofs"));
    }

    #[test]
    fn builds_qemu_img_overlay_command() {
        let config = sample_host_config();
        let command = qemu_img_create_command("qemu-img", &config.nodes[0]);

        assert_eq!(command.program, "qemu-img");
        assert_eq!(
            command.args,
            vec![
                "create",
                "-f",
                "qcow2",
                "-F",
                "qcow2",
                "-b",
                "/mnt/pool/nas-csi/images/debian.qcow2",
                "/mnt/pool/nas-csi/vms/nascsi-server-1.qcow2",
                "80G",
            ]
        );
    }

    #[test]
    fn renders_readable_nocloud_seed_image() {
        let mut image = render_nocloud_seed_image(
            "#cloud-config\nhostname: test-node\n",
            "instance-id: iid-test\nlocal-hostname: test-node\n",
        )
        .expect("seed image");

        assert!(image.len() >= 2 * 1024 * 1024);

        let fs = fatfs::FileSystem::new(Cursor::new(image.as_mut_slice()), fatfs::FsOptions::new())
            .expect("open generated seed");
        assert_eq!(fs.volume_label_as_bytes(), b"CIDATA");
        let root_label = fs
            .read_volume_label_from_root_dir_as_bytes()
            .expect("read root label")
            .expect("root volume label");
        assert!(root_label.starts_with(b"CIDATA"));

        let root = fs.root_dir();
        let mut user_data = String::new();
        root.open_file("user-data")
            .expect("open user-data")
            .read_to_string(&mut user_data)
            .expect("read user-data");
        assert!(user_data.contains("hostname: test-node"));

        let mut meta_data = String::new();
        root.open_file("meta-data")
            .expect("open meta-data")
            .read_to_string(&mut meta_data)
            .expect("read meta-data");
        assert!(meta_data.contains("instance-id: iid-test"));
    }

    #[test]
    fn plans_host_apply_without_starting_domains_by_default() {
        let config = sample_host_config();
        let plan = plan_host_apply(
            &config,
            &ArtifactRenderOptions::default(),
            &HostApplyPlanOptions {
                artifact_dir: "/tmp/nas-csi/rendered".to_string(),
                systemd_unit_dir: "/tmp/systemd".to_string(),
                ..HostApplyPlanOptions::default()
            },
        )
        .expect("apply plan");

        assert!(plan.steps.iter().any(|step| matches!(
            &step.kind,
            ApplyStepKind::Command {
                command,
                creates: Some(path)
            } if command.program == "qemu-img"
                && path == "/mnt/pool/nas-csi/vms/nascsi-server-1.qcow2"
        )));
        assert!(plan.steps.iter().any(|step| matches!(
            &step.kind,
            ApplyStepKind::WriteBinaryFile { path, contents }
                if path == "/mnt/pool/nas-csi/vms/nascsi-server-1-seed.img"
                    && contents.len() >= 2 * 1024 * 1024
        )));
        assert!(plan.steps.iter().any(|step| matches!(
            &step.kind,
            ApplyStepKind::WriteFile { path, .. }
                if path == "/tmp/systemd/nascsi-virtiofsd-nascsi-server-1-repos.service"
        )));
        assert!(plan.steps.iter().any(|step| matches!(
            &step.kind,
            ApplyStepKind::Command { command, .. }
                if command.program == "virsh" && command.args.contains(&"define".to_string())
        )));
        assert!(!plan.steps.iter().any(|step| matches!(
            &step.kind,
            ApplyStepKind::Command { command, .. }
                if command.program == "virsh" && command.args.contains(&"start".to_string())
        )));
    }

    #[test]
    fn reconcile_skips_current_state() {
        let config = sample_host_config();
        let render_options = ArtifactRenderOptions::default();
        let apply_options = HostApplyPlanOptions {
            artifact_dir: "/tmp/nas-csi/rendered".to_string(),
            systemd_unit_dir: "/tmp/systemd".to_string(),
            ..HostApplyPlanOptions::default()
        };
        let desired_apply =
            plan_host_apply(&config, &render_options, &apply_options).expect("apply plan");
        let actual = current_actual_state(&config, &render_options, &apply_options, &desired_apply);

        let reconcile = plan_host_reconcile(&config, &render_options, &apply_options, &actual)
            .expect("reconcile plan");

        assert!(reconcile.steps.iter().any(|step| matches!(
            &step.kind,
            ReconcileStepKind::SkipAlreadyCorrect { reason }
                if reason.contains("domain metadata already matches")
        )));
        assert!(reconcile.steps.iter().any(|step| matches!(
            &step.kind,
            ReconcileStepKind::SkipAlreadyCorrect { reason }
                if reason.contains("service is already enabled")
        )));
        assert!(
            !reconcile
                .steps
                .iter()
                .any(|step| matches!(&step.kind, ReconcileStepKind::Refuse { .. }))
        );
        assert!(!reconcile.steps.iter().any(|step| matches!(
            &step.kind,
            ReconcileStepKind::Apply(ReconcileOperation::CreateRootDisk { .. })
        )));
    }

    #[test]
    fn reconcile_refuses_running_domain_xml_change() {
        let config = sample_host_config();
        let render_options = ArtifactRenderOptions::default();
        let apply_options = HostApplyPlanOptions {
            artifact_dir: "/tmp/nas-csi/rendered".to_string(),
            systemd_unit_dir: "/tmp/systemd".to_string(),
            ..HostApplyPlanOptions::default()
        };
        let desired_apply =
            plan_host_apply(&config, &render_options, &apply_options).expect("apply plan");
        let mut actual =
            current_actual_state(&config, &render_options, &apply_options, &desired_apply);
        actual.domains.insert(
            "nascsi-server-1".to_string(),
            DomainActualState {
                exists: true,
                managed: true,
                active: true,
                autostart: Some(true),
                desired_hash: Some(content_hash(b"different domain shape")),
                xml: Some("<domain><name>nascsi-server-1</name></domain>".to_string()),
                xml_hash: Some(content_hash(b"different domain xml")),
            },
        );

        let reconcile = plan_host_reconcile(&config, &render_options, &apply_options, &actual)
            .expect("reconcile plan");

        assert!(reconcile.has_refusals());
        assert!(reconcile.steps.iter().any(|step| matches!(
            &step.kind,
            ReconcileStepKind::Refuse { reason, .. }
                if reason.contains("running") && reason.contains("XML differs")
        )));
    }

    #[test]
    fn reconcile_allows_running_domain_redefine_when_explicitly_enabled() {
        let config = sample_host_config();
        let render_options = ArtifactRenderOptions::default();
        let apply_options = HostApplyPlanOptions {
            artifact_dir: "/tmp/nas-csi/rendered".to_string(),
            systemd_unit_dir: "/tmp/systemd".to_string(),
            allow_running_domain_redefine: true,
            ..HostApplyPlanOptions::default()
        };
        let desired_apply =
            plan_host_apply(&config, &render_options, &apply_options).expect("apply plan");
        let mut actual =
            current_actual_state(&config, &render_options, &apply_options, &desired_apply);
        actual.domains.insert(
            "nascsi-server-1".to_string(),
            DomainActualState {
                exists: true,
                managed: true,
                active: true,
                autostart: Some(true),
                desired_hash: Some(content_hash(b"different domain shape")),
                xml: Some("<domain><name>nascsi-server-1</name></domain>".to_string()),
                xml_hash: Some(content_hash(b"different domain xml")),
            },
        );

        let reconcile = plan_host_reconcile(&config, &render_options, &apply_options, &actual)
            .expect("reconcile plan");

        assert!(!reconcile.has_refusals());
        assert!(reconcile.steps.iter().any(|step| matches!(
            &step.kind,
            ReconcileStepKind::Apply(ReconcileOperation::RedefineDomain { domain, .. })
                if domain == "nascsi-server-1"
        )));
    }

    #[test]
    fn reconcile_refuses_unmanaged_existing_domain_without_adoption() {
        let config = sample_host_config();
        let render_options = ArtifactRenderOptions::default();
        let apply_options = HostApplyPlanOptions {
            artifact_dir: "/tmp/nas-csi/rendered".to_string(),
            systemd_unit_dir: "/tmp/systemd".to_string(),
            ..HostApplyPlanOptions::default()
        };
        let desired_apply =
            plan_host_apply(&config, &render_options, &apply_options).expect("apply plan");
        let mut actual =
            current_actual_state(&config, &render_options, &apply_options, &desired_apply);
        actual.domains.insert(
            "nascsi-server-1".to_string(),
            DomainActualState {
                exists: true,
                managed: false,
                active: false,
                autostart: Some(false),
                desired_hash: None,
                xml: Some("<domain><name>nascsi-server-1</name></domain>".to_string()),
                xml_hash: Some(content_hash(b"unmanaged domain xml")),
            },
        );

        let reconcile = plan_host_reconcile(&config, &render_options, &apply_options, &actual)
            .expect("reconcile plan");

        assert!(reconcile.has_refusals());
        assert!(reconcile.steps.iter().any(|step| matches!(
            &step.kind,
            ReconcileStepKind::Refuse { reason, .. }
                if reason.contains("without nas-csi metadata") && reason.contains("adoption")
        )));
    }

    #[test]
    fn reconcile_allows_explicit_adoption_of_stopped_unmanaged_domain() {
        let config = sample_host_config();
        let render_options = ArtifactRenderOptions::default();
        let apply_options = HostApplyPlanOptions {
            artifact_dir: "/tmp/nas-csi/rendered".to_string(),
            systemd_unit_dir: "/tmp/systemd".to_string(),
            allow_domain_adoption: true,
            ..HostApplyPlanOptions::default()
        };
        let desired_apply =
            plan_host_apply(&config, &render_options, &apply_options).expect("apply plan");
        let mut actual =
            current_actual_state(&config, &render_options, &apply_options, &desired_apply);
        actual.domains.insert(
            "nascsi-server-1".to_string(),
            DomainActualState {
                exists: true,
                managed: false,
                active: false,
                autostart: Some(false),
                desired_hash: None,
                xml: Some("<domain><name>nascsi-server-1</name></domain>".to_string()),
                xml_hash: Some(content_hash(b"unmanaged domain xml")),
            },
        );

        let reconcile = plan_host_reconcile(&config, &render_options, &apply_options, &actual)
            .expect("reconcile plan");

        assert!(!reconcile.has_refusals());
        assert!(reconcile.steps.iter().any(|step| matches!(
            &step.kind,
            ReconcileStepKind::Apply(ReconcileOperation::RedefineDomain { domain, .. })
                if domain == "nascsi-server-1"
        )));
    }

    #[test]
    fn reconcile_redefines_stopped_managed_domain_without_starting_by_default() {
        let config = sample_host_config();
        let render_options = ArtifactRenderOptions::default();
        let apply_options = HostApplyPlanOptions {
            artifact_dir: "/tmp/nas-csi/rendered".to_string(),
            systemd_unit_dir: "/tmp/systemd".to_string(),
            ..HostApplyPlanOptions::default()
        };
        let desired_apply =
            plan_host_apply(&config, &render_options, &apply_options).expect("apply plan");
        let mut actual =
            current_actual_state(&config, &render_options, &apply_options, &desired_apply);
        actual
            .domains
            .insert("nascsi-server-1".to_string(), changed_managed_domain(false));

        let reconcile = plan_host_reconcile(&config, &render_options, &apply_options, &actual)
            .expect("reconcile plan");

        assert!(!reconcile.has_refusals());
        assert!(reconcile.steps.iter().any(|step| matches!(
            &step.kind,
            ReconcileStepKind::Apply(ReconcileOperation::RedefineDomain { domain, .. })
                if domain == "nascsi-server-1"
        )));
        assert!(!reconcile.steps.iter().any(|step| matches!(
            &step.kind,
            ReconcileStepKind::Apply(ReconcileOperation::StartDomain { domain, .. })
                if domain == "nascsi-server-1"
        )));
    }

    #[test]
    fn reconcile_can_start_redefined_stopped_domain_when_start_policy_is_enabled() {
        let config = sample_host_config();
        let render_options = ArtifactRenderOptions::default();
        let apply_options = HostApplyPlanOptions {
            artifact_dir: "/tmp/nas-csi/rendered".to_string(),
            systemd_unit_dir: "/tmp/systemd".to_string(),
            start_domains: true,
            ..HostApplyPlanOptions::default()
        };
        let desired_apply =
            plan_host_apply(&config, &render_options, &apply_options).expect("apply plan");
        let mut actual =
            current_actual_state(&config, &render_options, &apply_options, &desired_apply);
        actual
            .domains
            .insert("nascsi-server-1".to_string(), changed_managed_domain(false));

        let reconcile = plan_host_reconcile(&config, &render_options, &apply_options, &actual)
            .expect("reconcile plan");

        assert!(reconcile.steps.iter().any(|step| matches!(
            &step.kind,
            ReconcileStepKind::Apply(ReconcileOperation::StartDomain { domain, .. })
                if domain == "nascsi-server-1"
        )));
    }

    #[test]
    fn reconcile_refuses_existing_root_disk_with_wrong_backing_file() {
        let config = sample_host_config();
        let render_options = ArtifactRenderOptions::default();
        let apply_options = HostApplyPlanOptions {
            artifact_dir: "/tmp/nas-csi/rendered".to_string(),
            systemd_unit_dir: "/tmp/systemd".to_string(),
            ..HostApplyPlanOptions::default()
        };
        let desired_apply =
            plan_host_apply(&config, &render_options, &apply_options).expect("apply plan");
        let mut actual =
            current_actual_state(&config, &render_options, &apply_options, &desired_apply);
        actual.qemu_images.insert(
            "/mnt/pool/nas-csi/vms/nascsi-server-1.qcow2".to_string(),
            QemuImageActualState {
                format: Some("qcow2".to_string()),
                backing_file: Some("/different/base.qcow2".to_string()),
                virtual_size: Some(80 * 1024 * 1024 * 1024),
            },
        );

        let reconcile = plan_host_reconcile(&config, &render_options, &apply_options, &actual)
            .expect("reconcile plan");

        assert!(reconcile.steps.iter().any(|step| matches!(
            &step.kind,
            ReconcileStepKind::Refuse { reason, .. }
                if reason.contains("wrong") || reason.contains("backing")
        )));
    }

    #[test]
    fn reconcile_refuses_existing_root_disk_without_qemu_info() {
        let config = sample_host_config();
        let render_options = ArtifactRenderOptions::default();
        let apply_options = HostApplyPlanOptions {
            artifact_dir: "/tmp/nas-csi/rendered".to_string(),
            systemd_unit_dir: "/tmp/systemd".to_string(),
            ..HostApplyPlanOptions::default()
        };
        let desired_apply =
            plan_host_apply(&config, &render_options, &apply_options).expect("apply plan");
        let mut actual =
            current_actual_state(&config, &render_options, &apply_options, &desired_apply);
        actual
            .qemu_images
            .remove("/mnt/pool/nas-csi/vms/nascsi-server-1.qcow2");

        let reconcile = plan_host_reconcile(&config, &render_options, &apply_options, &actual)
            .expect("reconcile plan");

        assert!(reconcile.steps.iter().any(|step| matches!(
            &step.kind,
            ReconcileStepKind::Refuse {
                operation: Some(ReconcileOperation::CreateRootDisk { path, .. }),
                reason
            } if path == "/mnt/pool/nas-csi/vms/nascsi-server-1.qcow2"
                && reason.contains("qemu-img info was unavailable")
        )));
    }

    #[test]
    fn reconcile_refuses_existing_root_disk_when_qemu_img_missing() {
        let config = sample_host_config();
        let render_options = ArtifactRenderOptions::default();
        let apply_options = HostApplyPlanOptions {
            artifact_dir: "/tmp/nas-csi/rendered".to_string(),
            systemd_unit_dir: "/tmp/systemd".to_string(),
            ..HostApplyPlanOptions::default()
        };
        let desired_apply =
            plan_host_apply(&config, &render_options, &apply_options).expect("apply plan");
        let mut actual =
            current_actual_state(&config, &render_options, &apply_options, &desired_apply);
        actual
            .tools
            .insert("qemu-img".to_string(), ToolActualState::missing());

        let reconcile = plan_host_reconcile(&config, &render_options, &apply_options, &actual)
            .expect("reconcile plan");

        assert!(reconcile.steps.iter().any(|step| matches!(
            &step.kind,
            ReconcileStepKind::Refuse {
                operation: Some(ReconcileOperation::CreateRootDisk { path, .. }),
                reason
            } if path == "/mnt/pool/nas-csi/vms/nascsi-server-1.qcow2"
                && reason.contains("qemu-img")
                && reason.contains("cannot verify")
        )));
    }

    #[test]
    fn reconcile_refuses_root_disk_create_when_base_image_missing() {
        let config = sample_host_config();
        let render_options = ArtifactRenderOptions::default();
        let apply_options = HostApplyPlanOptions {
            artifact_dir: "/tmp/nas-csi/rendered".to_string(),
            systemd_unit_dir: "/tmp/systemd".to_string(),
            ..HostApplyPlanOptions::default()
        };
        let mut actual = HostActualState::default();
        actual.tools.insert(
            "qemu-img".to_string(),
            ToolActualState::found("/usr/bin/qemu-img"),
        );

        let reconcile = plan_host_reconcile(&config, &render_options, &apply_options, &actual)
            .expect("reconcile plan");

        assert!(reconcile.steps.iter().any(|step| matches!(
            &step.kind,
            ReconcileStepKind::Refuse {
                operation: Some(ReconcileOperation::CreateRootDisk { path, .. }),
                reason
            } if path == "/mnt/pool/nas-csi/vms/nascsi-server-1.qcow2"
                && reason.contains("base image")
                && reason.contains("does not exist")
        )));
    }

    #[test]
    fn reconcile_refuses_root_disk_create_when_base_format_differs() {
        let config = sample_host_config();
        let render_options = ArtifactRenderOptions::default();
        let apply_options = HostApplyPlanOptions {
            artifact_dir: "/tmp/nas-csi/rendered".to_string(),
            systemd_unit_dir: "/tmp/systemd".to_string(),
            ..HostApplyPlanOptions::default()
        };
        let mut actual = HostActualState::default();
        actual.tools.insert(
            "qemu-img".to_string(),
            ToolActualState::found("/usr/bin/qemu-img"),
        );
        actual.paths.insert(
            "/mnt/pool/nas-csi/images/debian.qcow2".to_string(),
            PathActualState::file_with_sha256(b"base image", SAMPLE_BASE_SHA256),
        );
        actual.qemu_images.insert(
            "/mnt/pool/nas-csi/images/debian.qcow2".to_string(),
            QemuImageActualState {
                format: Some("raw".to_string()),
                backing_file: None,
                virtual_size: Some(8 * 1024 * 1024 * 1024),
            },
        );

        let reconcile = plan_host_reconcile(&config, &render_options, &apply_options, &actual)
            .expect("reconcile plan");

        assert!(reconcile.steps.iter().any(|step| matches!(
            &step.kind,
            ReconcileStepKind::Refuse {
                operation: Some(ReconcileOperation::CreateRootDisk { path, .. }),
                reason
            } if path == "/mnt/pool/nas-csi/vms/nascsi-server-1.qcow2"
                && reason.contains("format")
                && reason.contains("expected qcow2")
        )));
    }

    #[test]
    fn reconcile_refuses_root_disk_create_when_base_checksum_differs() {
        let config = sample_host_config();
        let render_options = ArtifactRenderOptions::default();
        let apply_options = HostApplyPlanOptions {
            artifact_dir: "/tmp/nas-csi/rendered".to_string(),
            systemd_unit_dir: "/tmp/systemd".to_string(),
            ..HostApplyPlanOptions::default()
        };
        let mut actual = HostActualState::default();
        actual.tools.insert(
            "qemu-img".to_string(),
            ToolActualState::found("/usr/bin/qemu-img"),
        );
        actual.paths.insert(
            "/mnt/pool/nas-csi/images/debian.qcow2".to_string(),
            PathActualState::file_with_sha256(
                b"base image",
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            ),
        );
        actual.qemu_images.insert(
            "/mnt/pool/nas-csi/images/debian.qcow2".to_string(),
            QemuImageActualState {
                format: Some("qcow2".to_string()),
                backing_file: None,
                virtual_size: Some(8 * 1024 * 1024 * 1024),
            },
        );

        let reconcile = plan_host_reconcile(&config, &render_options, &apply_options, &actual)
            .expect("reconcile plan");

        assert!(reconcile.steps.iter().any(|step| matches!(
            &step.kind,
            ReconcileStepKind::Refuse {
                operation: Some(ReconcileOperation::CreateRootDisk { path, .. }),
                reason
            } if path == "/mnt/pool/nas-csi/vms/nascsi-server-1.qcow2"
                && reason.contains("checksum")
        )));
    }

    #[test]
    fn reconcile_resizes_existing_root_disk_when_smaller_than_desired() {
        let config = sample_host_config();
        let render_options = ArtifactRenderOptions::default();
        let apply_options = HostApplyPlanOptions {
            artifact_dir: "/tmp/nas-csi/rendered".to_string(),
            systemd_unit_dir: "/tmp/systemd".to_string(),
            ..HostApplyPlanOptions::default()
        };
        let desired_apply =
            plan_host_apply(&config, &render_options, &apply_options).expect("apply plan");
        let mut actual =
            current_actual_state(&config, &render_options, &apply_options, &desired_apply);
        actual.qemu_images.insert(
            "/mnt/pool/nas-csi/vms/nascsi-server-1.qcow2".to_string(),
            QemuImageActualState {
                format: Some("qcow2".to_string()),
                backing_file: Some("/mnt/pool/nas-csi/images/debian.qcow2".to_string()),
                virtual_size: Some(40 * 1024 * 1024 * 1024),
            },
        );

        let reconcile = plan_host_reconcile(&config, &render_options, &apply_options, &actual)
            .expect("reconcile plan");

        assert!(reconcile.steps.iter().any(|step| matches!(
            &step.kind,
            ReconcileStepKind::Apply(ReconcileOperation::ResizeRootDisk {
                path,
                desired_size_bytes,
                command
            }) if path == "/mnt/pool/nas-csi/vms/nascsi-server-1.qcow2"
                && *desired_size_bytes == 80_u64 * 1024 * 1024 * 1024
                && command.args == ["resize", "/mnt/pool/nas-csi/vms/nascsi-server-1.qcow2", "80G"]
        )));
    }

    #[test]
    fn reconcile_uses_named_operations_for_missing_state() {
        let config = sample_host_config();
        let render_options = ArtifactRenderOptions::default();
        let apply_options = HostApplyPlanOptions {
            artifact_dir: "/tmp/nas-csi/rendered".to_string(),
            systemd_unit_dir: "/tmp/systemd".to_string(),
            ..HostApplyPlanOptions::default()
        };
        let mut actual = HostActualState::default();
        actual.tools.insert(
            "qemu-img".to_string(),
            ToolActualState::found("/usr/bin/qemu-img"),
        );
        actual.tools.insert(
            "systemctl".to_string(),
            ToolActualState::found("/usr/bin/systemctl"),
        );
        actual.tools.insert(
            "virsh".to_string(),
            ToolActualState::found("/usr/bin/virsh"),
        );
        actual.paths.insert(
            "/mnt/pool/nas-csi/images/debian.qcow2".to_string(),
            PathActualState::file_with_sha256(b"base image", SAMPLE_BASE_SHA256),
        );
        actual.qemu_images.insert(
            "/mnt/pool/nas-csi/images/debian.qcow2".to_string(),
            QemuImageActualState {
                format: Some("qcow2".to_string()),
                backing_file: None,
                virtual_size: Some(8 * 1024 * 1024 * 1024),
            },
        );

        let reconcile = plan_host_reconcile(&config, &render_options, &apply_options, &actual)
            .expect("reconcile plan");

        assert!(reconcile.steps.iter().any(|step| matches!(
            &step.kind,
            ReconcileStepKind::Apply(ReconcileOperation::CreateRootDisk { path, .. })
                if path == "/mnt/pool/nas-csi/vms/nascsi-server-1.qcow2"
        )));
        assert!(reconcile.steps.iter().any(|step| matches!(
            &step.kind,
            ReconcileStepKind::Apply(ReconcileOperation::RewriteSeedImage { path, .. })
                if path == "/mnt/pool/nas-csi/vms/nascsi-server-1-seed.img"
        )));
        assert!(reconcile.steps.iter().any(|step| matches!(
            &step.kind,
            ReconcileStepKind::Apply(ReconcileOperation::InstallOrUpdateSystemdUnit {
                unit_name,
                ..
            }) if unit_name == "nascsi-virtiofsd-nascsi-server-1-repos.service"
        )));
        assert!(reconcile.steps.iter().any(|step| matches!(
            &step.kind,
            ReconcileStepKind::Apply(ReconcileOperation::DefineDomain { domain, .. })
                if domain == "nascsi-server-1"
        )));
    }

    fn sample_host_config() -> nas_csi_types::HostConfig {
        use nas_csi_types::{
            API_VERSION, AccessMode, AddonIntent, ApiServerConfig, ClusterConfig,
            ClusterDistribution, ClusterNetworkConfig, ClusterProfile, DatasetRef, DiskFormat,
            ExportConfig, HostLibvirtConfig, HostToolConfig, HostTrueNasConfig, NodeConfig,
            NodeK3sConfig, NodeNetworkConfig, NodeRole, RootDiskConfig,
        };
        use std::collections::BTreeMap;

        let mut exports = BTreeMap::new();
        exports.insert(
            "repos".to_string(),
            ExportConfig {
                dataset: "pool/repos".to_string(),
                source_path: "/mnt/pool/repos".to_string(),
                tag: "nascsi_repos".to_string(),
                policy: "repos-dev".to_string(),
                access: AccessMode::ReadWrite,
            },
        );

        nas_csi_types::HostConfig {
            api_version: API_VERSION.to_string(),
            kind: "HostConfig".to_string(),
            truenas: HostTrueNasConfig {
                url: "wss://127.0.0.1/api/current".to_string(),
                api_key_file: "/etc/nas-csi/secrets/truenas-api-key".to_string(),
            },
            host_tools: HostToolConfig {
                virsh: "/usr/bin/virsh".to_string(),
                qemu_img: "/usr/bin/qemu-img".to_string(),
                virtiofsd: "/usr/libexec/virtiofsd".to_string(),
                systemctl: "/usr/bin/systemctl".to_string(),
            },
            libvirt: HostLibvirtConfig {
                uri: "qemu:///system".to_string(),
                bridge: "br0".to_string(),
            },
            image_cache: DatasetRef {
                dataset: "pool/nas-csi/images".to_string(),
            },
            vm_state: DatasetRef {
                dataset: "pool/nas-csi/vms".to_string(),
            },
            cluster: ClusterConfig {
                name: "nas-csi".to_string(),
                distribution: ClusterDistribution::K3s,
                profile: ClusterProfile::MaintenanceBasic,
                version: "v1.33.0+k3s1".to_string(),
                token_file: "/etc/nas-csi/secrets/k3s-token".to_string(),
                kubeconfig_out: "/etc/nas-csi/kubeconfig".to_string(),
                api_server: ApiServerConfig {
                    endpoint: "https://nas-csi-api.example.test:6443".to_string(),
                    tls_sans: vec!["nas-csi-api.example.test".to_string()],
                },
                network: ClusterNetworkConfig {
                    cluster_cidr: "10.42.0.0/16".to_string(),
                    service_cidr: "10.43.0.0/16".to_string(),
                    cluster_dns: "10.43.0.10".to_string(),
                    flannel_backend: "vxlan".to_string(),
                },
                disable: vec!["traefik".to_string(), "servicelb".to_string()],
                addons: AddonIntent {
                    nas_csi: true,
                    metrics_server: true,
                },
            },
            nodes: vec![NodeConfig {
                name: "server-1".to_string(),
                domain: "nascsi-server-1".to_string(),
                role: NodeRole::Server,
                autostart: true,
                vcpus: 2,
                memory_mib: 4096,
                machine: "q35".to_string(),
                firmware: "efi".to_string(),
                cpu: "host-passthrough".to_string(),
                network: NodeNetworkConfig {
                    bridge: "br0".to_string(),
                    mac: "52:54:00:00:00:01".to_string(),
                },
                root_disk: RootDiskConfig {
                    image: "/mnt/pool/nas-csi/vms/nascsi-server-1.qcow2".to_string(),
                    source_image: Some("/mnt/pool/nas-csi/images/debian.qcow2".to_string()),
                    source_format: Some(DiskFormat::Qcow2),
                    source_checksum: Some(format!("sha256:{SAMPLE_BASE_SHA256}")),
                    size_gib: 80,
                    format: DiskFormat::Qcow2,
                },
                k3s: NodeK3sConfig {
                    cluster_init: true,
                    labels: BTreeMap::from([(
                        "nas-csi.dev/storage-node".to_string(),
                        "true".to_string(),
                    )]),
                    ..NodeK3sConfig::default()
                },
                exports: vec!["repos".to_string()],
            }],
            exports,
        }
    }

    fn changed_managed_domain(active: bool) -> DomainActualState {
        let xml = "<domain><name>nascsi-server-1</name><metadata><nas-csi:managed xmlns:nas-csi='urn:nas-csi.dev:domain'>true</nas-csi:managed></metadata></domain>";
        DomainActualState {
            exists: true,
            managed: true,
            active,
            autostart: Some(true),
            desired_hash: Some(content_hash(b"different domain shape")),
            xml: Some(xml.to_string()),
            xml_hash: Some(content_hash(xml.as_bytes())),
        }
    }

    fn current_actual_state(
        config: &nas_csi_types::HostConfig,
        render_options: &ArtifactRenderOptions,
        apply_options: &HostApplyPlanOptions,
        desired_apply: &HostApplyPlan,
    ) -> HostActualState {
        let mut actual = HostActualState::default();
        actual.tools.insert(
            "qemu-img".to_string(),
            ToolActualState::found("/usr/bin/qemu-img"),
        );
        actual.tools.insert(
            "systemctl".to_string(),
            ToolActualState::found("/usr/bin/systemctl"),
        );
        actual.tools.insert(
            "virsh".to_string(),
            ToolActualState::found("/usr/bin/virsh"),
        );

        for step in &desired_apply.steps {
            match &step.kind {
                ApplyStepKind::EnsureDirectory { path } => {
                    actual
                        .paths
                        .insert(path.clone(), PathActualState::directory());
                }
                ApplyStepKind::WriteFile { path, contents } => {
                    actual
                        .paths
                        .insert(path.clone(), PathActualState::file(contents.as_bytes()));
                    if path.starts_with(&apply_options.systemd_unit_dir)
                        && let Some(unit_name) =
                            Path::new(path).file_name().and_then(|name| name.to_str())
                    {
                        actual.systemd_units.insert(
                            unit_name.to_string(),
                            SystemdUnitActualState {
                                installed_hash: Some(content_hash(contents.as_bytes())),
                                enabled: Some(true),
                                active: Some(true),
                            },
                        );
                    }
                }
                ApplyStepKind::WriteBinaryFile { path, contents } => {
                    actual
                        .paths
                        .insert(path.clone(), PathActualState::file(contents));
                }
                ApplyStepKind::Command {
                    creates: Some(path),
                    ..
                } => {
                    actual
                        .paths
                        .insert(path.clone(), PathActualState::file(b"disk"));
                    actual.qemu_images.insert(
                        path.clone(),
                        QemuImageActualState {
                            format: Some("qcow2".to_string()),
                            backing_file: Some("/mnt/pool/nas-csi/images/debian.qcow2".to_string()),
                            virtual_size: Some(80 * 1024 * 1024 * 1024),
                        },
                    );
                }
                _ => {}
            }
        }

        for node in &config.nodes {
            if let Some(source_image) = &node.root_disk.source_image {
                actual.paths.insert(
                    source_image.clone(),
                    PathActualState::file_with_sha256(b"base image", SAMPLE_BASE_SHA256),
                );
                actual.qemu_images.insert(
                    source_image.clone(),
                    QemuImageActualState {
                        format: Some("qcow2".to_string()),
                        backing_file: None,
                        virtual_size: Some(8 * 1024 * 1024 * 1024),
                    },
                );
            }
        }

        let artifacts = render_host_artifacts(config, render_options).expect("render");
        for node in &config.nodes {
            let relative_path = format!("nodes/{}/domain.xml", safe_path_segment(&node.name));
            let xml = rendered_artifact_contents(&artifacts, &relative_path).expect("xml");
            actual.domains.insert(
                node.domain.clone(),
                DomainActualState {
                    exists: true,
                    managed: true,
                    active: false,
                    autostart: Some(node.autostart),
                    desired_hash: extract_domain_desired_hash(xml),
                    xml: Some(xml.to_string()),
                    xml_hash: Some(content_hash(xml.as_bytes())),
                },
            );
        }

        actual
    }
}
