use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

pub const API_VERSION: &str = "nas-csi.dev/v1alpha1";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ClusterIntent {
    #[serde(rename = "apiVersion")]
    pub api_version: String,
    pub kind: String,
    pub profile: ClusterProfile,
    pub nodes: IntentNodes,
    #[serde(default)]
    pub storage_policies: Vec<StoragePolicyIntent>,
    #[serde(default)]
    pub addons: AddonIntent,
    #[serde(default)]
    pub applications: Vec<serde_json::Value>,
}

impl ClusterIntent {
    pub fn validate(&self) -> Vec<String> {
        let mut errors = Vec::new();

        if self.api_version != API_VERSION {
            errors.push(format!("apiVersion must be {API_VERSION}"));
        }
        if self.kind != "ClusterIntent" {
            errors.push("kind must be ClusterIntent".to_string());
        }
        if self.nodes.servers == 0 {
            errors.push("nodes.servers must be greater than zero".to_string());
        }
        if self.profile == ClusterProfile::MaintenanceControlPlane && self.nodes.servers < 3 {
            errors
                .push("maintenance-control-plane requires at least three server nodes".to_string());
        }
        if self.profile == ClusterProfile::MaintenanceBasic && self.nodes.agents == 0 {
            errors.push("maintenance-basic should include at least one agent node".to_string());
        }
        if !self.applications.is_empty() {
            errors
                .push("applications must be empty; application workloads are out of scope".into());
        }

        let mut policy_names = BTreeSet::new();
        for policy in &self.storage_policies {
            if policy.name.trim().is_empty() {
                errors.push("storage policy names must not be empty".to_string());
            }
            if !policy_names.insert(policy.name.as_str()) {
                errors.push(format!("duplicate storage policy: {}", policy.name));
            }
        }

        errors
    }
}

#[derive(Copy, Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ClusterProfile {
    MaintenanceBasic,
    MaintenanceControlPlane,
}

impl fmt::Display for ClusterProfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MaintenanceBasic => f.write_str("maintenance-basic"),
            Self::MaintenanceControlPlane => f.write_str("maintenance-control-plane"),
        }
    }
}

#[derive(Copy, Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct IntentNodes {
    pub servers: u16,
    pub agents: u16,
}

impl IntentNodes {
    pub fn total(self) -> u16 {
        self.servers + self.agents
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AddonIntent {
    #[serde(default)]
    pub nas_csi: bool,
    #[serde(default)]
    pub metrics_server: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StoragePolicyIntent {
    pub name: String,
    pub access: AccessMode,
    pub workload: String,
}

#[derive(Copy, Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum AccessMode {
    ReadWrite,
    ReadOnly,
}

impl fmt::Display for AccessMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadWrite => f.write_str("read-write"),
            Self::ReadOnly => f.write_str("read-only"),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveryInventory {
    #[serde(rename = "apiVersion")]
    pub api_version: String,
    pub kind: String,
    pub generated_unix_seconds: u64,
    pub host: HostFacts,
    pub truenas: TrueNasFacts,
    pub libvirt: LibvirtFacts,
    pub network: NetworkFacts,
    pub tools: ToolFacts,
    #[serde(default)]
    pub existing_project_state: ExistingProjectState,
    #[serde(default)]
    pub warnings: Vec<String>,
}

impl DiscoveryInventory {
    pub fn validate(&self) -> Vec<String> {
        let mut errors = Vec::new();
        if self.api_version != API_VERSION {
            errors.push(format!("apiVersion must be {API_VERSION}"));
        }
        if self.kind != "DiscoveryInventory" {
            errors.push("kind must be DiscoveryInventory".to_string());
        }
        errors
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HostFacts {
    pub os_pretty_name: Option<String>,
    pub architecture: String,
    pub cpu_count: Option<usize>,
    pub memory_total_kib: Option<u64>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TrueNasFacts {
    pub local_api_url: Option<String>,
    pub version: Option<String>,
    #[serde(default)]
    pub pools: Vec<PoolSummary>,
    #[serde(default)]
    pub filesystem_datasets: Vec<DatasetSummary>,
    #[serde(default)]
    pub smb_shares: Vec<SmbShareSummary>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PoolSummary {
    pub name: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DatasetSummary {
    pub name: String,
    pub mountpoint: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SmbShareSummary {
    pub name: String,
    pub path: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LibvirtFacts {
    pub uri: String,
    pub virsh: ToolStatus,
    pub qemu: ToolStatus,
    pub default_machine: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NetworkFacts {
    #[serde(default)]
    pub bridges: Vec<BridgeSummary>,
    #[serde(default)]
    pub lan_addresses: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BridgeSummary {
    pub name: String,
    #[serde(default)]
    pub interfaces: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ToolFacts {
    pub virtiofsd: ToolStatus,
    pub qemu_img: ToolStatus,
    #[serde(default)]
    pub systemctl: ToolStatus,
    #[serde(default)]
    pub midclt: ToolStatus,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ToolStatus {
    pub path: Option<String>,
    pub version: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExistingProjectState {
    #[serde(default)]
    pub config_paths: Vec<String>,
    #[serde(default)]
    pub libvirt_domains: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HostConfigDraft {
    #[serde(rename = "apiVersion")]
    pub api_version: String,
    pub kind: String,
    pub profile: ClusterProfile,
    pub nodes: IntentNodes,
    #[serde(default)]
    pub storage_policies: Vec<StoragePolicyIntent>,
    #[serde(default)]
    pub required_selections: Vec<RequiredSelection>,
    #[serde(default)]
    pub planned_actions: Vec<PlannedAction>,
    #[serde(default)]
    pub discovery_warnings: Vec<String>,
}

impl HostConfigDraft {
    pub fn from_intent_and_discovery(
        intent: ClusterIntent,
        discovery: &DiscoveryInventory,
    ) -> Self {
        let mut required_selections = vec![
            RequiredSelection::new(
                "truenas.apiKeyFile",
                "Local path to a TrueNAS API key readable by the host agent",
                Vec::new(),
            ),
            RequiredSelection::new(
                "cluster.version",
                "Pinned k3s version to install",
                Vec::new(),
            ),
            RequiredSelection::new(
                "image.source",
                "Cloud image source or existing local image for node root disks",
                Vec::new(),
            ),
        ];

        required_selections.push(RequiredSelection::new(
            "libvirt.bridge",
            "Bridge to attach node VMs to",
            discovery
                .network
                .bridges
                .iter()
                .map(|bridge| bridge.name.clone())
                .collect(),
        ));

        let dataset_candidates = discovery
            .truenas
            .filesystem_datasets
            .iter()
            .map(|dataset| dataset.name.clone())
            .collect::<Vec<_>>();

        required_selections.push(RequiredSelection::new(
            "imageCache.dataset",
            "Dataset for cached base images",
            dataset_candidates.clone(),
        ));
        required_selections.push(RequiredSelection::new(
            "vmState.dataset",
            "Dataset for disposable node VM state",
            dataset_candidates.clone(),
        ));

        for policy in &intent.storage_policies {
            required_selections.push(RequiredSelection::new(
                format!("exports.{}.dataset", policy.name),
                format!(
                    "TrueNAS filesystem dataset for {} ({})",
                    policy.name, policy.access
                ),
                dataset_candidates.clone(),
            ));
        }

        let planned_actions = vec![
            PlannedAction::new(format!(
                "Prepare {} k3s server VM(s) and {} agent VM(s)",
                intent.nodes.servers, intent.nodes.agents
            )),
            PlannedAction::new(format!("Bootstrap {} cluster", intent.profile)),
            PlannedAction::new(format!(
                "Configure {} storage policy export(s)",
                intent.storage_policies.len()
            )),
            PlannedAction::new("Install nas-csi substrate components"),
        ];

        Self {
            api_version: API_VERSION.to_string(),
            kind: "HostConfigDraft".to_string(),
            profile: intent.profile,
            nodes: intent.nodes,
            storage_policies: intent.storage_policies,
            required_selections,
            planned_actions,
            discovery_warnings: discovery.warnings.clone(),
        }
    }

    pub fn validate(&self) -> Vec<String> {
        let mut errors = Vec::new();
        if self.api_version != API_VERSION {
            errors.push(format!("apiVersion must be {API_VERSION}"));
        }
        if self.kind != "HostConfigDraft" {
            errors.push("kind must be HostConfigDraft".to_string());
        }
        errors
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RequiredSelection {
    pub id: String,
    pub description: String,
    #[serde(default)]
    pub candidates: Vec<String>,
}

impl RequiredSelection {
    pub fn new(
        id: impl Into<String>,
        description: impl Into<String>,
        candidates: Vec<String>,
    ) -> Self {
        Self {
            id: id.into(),
            description: description.into(),
            candidates,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PlannedAction {
    pub description: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HostConfig {
    #[serde(rename = "apiVersion")]
    pub api_version: String,
    pub kind: String,
    pub truenas: HostTrueNasConfig,
    #[serde(default)]
    pub host_tools: HostToolConfig,
    pub libvirt: HostLibvirtConfig,
    pub image_cache: DatasetRef,
    pub vm_state: DatasetRef,
    pub cluster: ClusterConfig,
    #[serde(default)]
    pub nodes: Vec<NodeConfig>,
    #[serde(default)]
    pub exports: BTreeMap<String, ExportConfig>,
}

impl HostConfig {
    pub fn from_intent_discovery_selections(
        intent: &ClusterIntent,
        discovery: &DiscoveryInventory,
        selections: &HostSelections,
    ) -> Result<Self, Vec<String>> {
        let mut errors = Vec::new();
        errors.extend(intent.validate());
        errors.extend(discovery.validate());
        errors.extend(selections.validate());

        if !discovery.network.bridges.is_empty()
            && !discovery
                .network
                .bridges
                .iter()
                .any(|bridge| bridge.name == selections.libvirt.bridge)
        {
            errors.push(format!(
                "selected bridge {} was not discovered",
                selections.libvirt.bridge
            ));
        }

        for policy in &intent.storage_policies {
            if !selections.exports.contains_key(&policy.name) {
                errors.push(format!(
                    "missing export selection for storage policy {}",
                    policy.name
                ));
            }
        }

        if !errors.is_empty() {
            return Err(errors);
        }

        let vm_state_path = dataset_path(discovery, &selections.datasets.vm_state);
        let mut exports = BTreeMap::new();
        for policy in &intent.storage_policies {
            let selection = selections
                .exports
                .get(&policy.name)
                .expect("validated export selection");
            exports.insert(
                policy.name.clone(),
                ExportConfig {
                    dataset: selection.dataset.clone(),
                    source_path: selection
                        .source_path
                        .clone()
                        .unwrap_or_else(|| dataset_path(discovery, &selection.dataset)),
                    tag: selection
                        .tag
                        .clone()
                        .unwrap_or_else(|| virtiofs_tag(&policy.name)),
                    policy: policy.name.clone(),
                    access: policy.access,
                },
            );
        }

        let mut nodes = Vec::new();
        for index in 1..=intent.nodes.servers {
            nodes.push(node_config_from_selection(
                "server",
                index,
                NodeRole::Server,
                intent,
                selections,
                &vm_state_path,
                index == 1,
            ));
        }
        for index in 1..=intent.nodes.agents {
            nodes.push(node_config_from_selection(
                "agent",
                index,
                NodeRole::Agent,
                intent,
                selections,
                &vm_state_path,
                false,
            ));
        }

        let config = Self {
            api_version: API_VERSION.to_string(),
            kind: "HostConfig".to_string(),
            truenas: HostTrueNasConfig {
                url: selections
                    .truenas
                    .url
                    .clone()
                    .or_else(|| discovery.truenas.local_api_url.clone())
                    .unwrap_or_else(|| "wss://127.0.0.1/api/current".to_string()),
                api_key_file: selections.truenas.api_key_file.clone(),
            },
            host_tools: HostToolConfig {
                virsh: tool_or_default(&discovery.libvirt.virsh, "virsh"),
                qemu_img: tool_or_default(&discovery.tools.qemu_img, "qemu-img"),
                virtiofsd: tool_or_default(&discovery.tools.virtiofsd, "virtiofsd"),
                systemctl: tool_or_default(&discovery.tools.systemctl, "systemctl"),
            },
            libvirt: HostLibvirtConfig {
                uri: selections
                    .libvirt
                    .uri
                    .clone()
                    .unwrap_or_else(|| discovery.libvirt.uri.clone()),
                bridge: selections.libvirt.bridge.clone(),
            },
            image_cache: DatasetRef {
                dataset: selections.datasets.image_cache.clone(),
            },
            vm_state: DatasetRef {
                dataset: selections.datasets.vm_state.clone(),
            },
            cluster: ClusterConfig {
                name: selections.cluster.name.clone(),
                distribution: ClusterDistribution::K3s,
                profile: intent.profile,
                version: selections.cluster.version.clone(),
                token_file: selections.cluster.token_file.clone(),
                kubeconfig_out: selections.cluster.kubeconfig_out.clone(),
                api_server: ApiServerConfig {
                    endpoint: selections.cluster.api_endpoint.clone(),
                    tls_sans: selections.cluster.tls_sans.clone(),
                },
                network: selections.cluster.network.clone().into(),
                disable: selections.cluster.disable.clone(),
                addons: intent.addons.clone(),
            },
            nodes,
            exports,
        };

        let validation_errors = config.validate();
        if validation_errors.is_empty() {
            Ok(config)
        } else {
            Err(validation_errors)
        }
    }

    pub fn validate(&self) -> Vec<String> {
        let mut errors = Vec::new();

        if self.api_version != API_VERSION {
            errors.push(format!("apiVersion must be {API_VERSION}"));
        }
        if self.kind != "HostConfig" {
            errors.push("kind must be HostConfig".to_string());
        }
        if self.nodes.is_empty() {
            errors.push("at least one node is required".to_string());
        }
        errors.extend(self.host_tools.validate());

        let mut node_names = BTreeSet::new();
        let mut domains = BTreeSet::new();
        let mut macs = BTreeSet::new();
        let mut server_count = 0u16;
        let mut agent_count = 0u16;

        for node in &self.nodes {
            if !node_names.insert(node.name.as_str()) {
                errors.push(format!("duplicate node name: {}", node.name));
            }
            if !domains.insert(node.domain.as_str()) {
                errors.push(format!("duplicate libvirt domain: {}", node.domain));
            }
            if !macs.insert(node.network.mac.as_str()) {
                errors.push(format!("duplicate MAC address: {}", node.network.mac));
            }
            if node.vcpus == 0 {
                errors.push(format!("node {} must have at least one vCPU", node.name));
            }
            if node.memory_mib == 0 {
                errors.push(format!("node {} must have memoryMiB > 0", node.name));
            }
            match node.role {
                NodeRole::Server => server_count += 1,
                NodeRole::Agent => agent_count += 1,
            }
            for export in &node.exports {
                if !self.exports.contains_key(export) {
                    errors.push(format!(
                        "node {} references missing export {}",
                        node.name, export
                    ));
                }
            }
        }

        if server_count == 0 {
            errors.push("at least one server node is required".to_string());
        }
        if self.cluster.profile == ClusterProfile::MaintenanceBasic && agent_count == 0 {
            errors.push("maintenance-basic should include at least one agent node".to_string());
        }
        if self.cluster.profile == ClusterProfile::MaintenanceControlPlane && server_count < 3 {
            errors
                .push("maintenance-control-plane requires at least three server nodes".to_string());
        }

        for (id, export) in &self.exports {
            if export.dataset.trim().is_empty() {
                errors.push(format!("export {id} dataset must not be empty"));
            }
            if export.source_path.trim().is_empty() {
                errors.push(format!("export {id} sourcePath must not be empty"));
            }
            if export.tag.trim().is_empty() {
                errors.push(format!("export {id} tag must not be empty"));
            }
            if export.access == AccessMode::ReadOnly && export.policy.contains("dev") {
                errors.push(format!(
                    "export {id} is read-only but uses dev-looking policy {}",
                    export.policy
                ));
            }
        }

        errors
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HostSelections {
    #[serde(rename = "apiVersion")]
    pub api_version: String,
    pub kind: String,
    pub truenas: HostTrueNasSelection,
    pub libvirt: HostLibvirtSelection,
    pub image: ImageSelection,
    pub datasets: HostDatasetSelections,
    pub cluster: ClusterSelection,
    pub node_defaults: NodeDefaultSelections,
    #[serde(default)]
    pub exports: BTreeMap<String, ExportSelection>,
}

impl HostSelections {
    pub fn validate(&self) -> Vec<String> {
        let mut errors = Vec::new();

        if self.api_version != API_VERSION {
            errors.push(format!("apiVersion must be {API_VERSION}"));
        }
        if self.kind != "HostSelections" {
            errors.push("kind must be HostSelections".to_string());
        }
        push_non_empty(
            &mut errors,
            "truenas.apiKeyFile",
            &self.truenas.api_key_file,
        );
        push_non_empty(&mut errors, "libvirt.bridge", &self.libvirt.bridge);
        push_non_empty(&mut errors, "image.source", &self.image.source);
        push_non_empty(
            &mut errors,
            "datasets.imageCache",
            &self.datasets.image_cache,
        );
        push_non_empty(&mut errors, "datasets.vmState", &self.datasets.vm_state);
        push_non_empty(&mut errors, "cluster.name", &self.cluster.name);
        push_non_empty(&mut errors, "cluster.version", &self.cluster.version);
        push_non_empty(
            &mut errors,
            "cluster.apiEndpoint",
            &self.cluster.api_endpoint,
        );
        push_non_empty(&mut errors, "cluster.tokenFile", &self.cluster.token_file);
        push_non_empty(
            &mut errors,
            "cluster.kubeconfigOut",
            &self.cluster.kubeconfig_out,
        );
        validate_node_size(
            &mut errors,
            "nodeDefaults.server",
            self.node_defaults.server,
        );
        validate_node_size(&mut errors, "nodeDefaults.agent", self.node_defaults.agent);

        for (name, export) in &self.exports {
            push_non_empty(
                &mut errors,
                &format!("exports.{name}.dataset"),
                &export.dataset,
            );
        }

        errors
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HostTrueNasSelection {
    pub api_key_file: String,
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HostLibvirtSelection {
    pub bridge: String,
    #[serde(default)]
    pub uri: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ImageSelection {
    pub source: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HostDatasetSelections {
    pub image_cache: String,
    pub vm_state: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ClusterSelection {
    pub name: String,
    pub version: String,
    pub api_endpoint: String,
    pub token_file: String,
    pub kubeconfig_out: String,
    #[serde(default)]
    pub tls_sans: Vec<String>,
    #[serde(default = "default_disabled_k3s_components")]
    pub disable: Vec<String>,
    pub network: ClusterNetworkSelection,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ClusterNetworkSelection {
    pub cluster_cidr: String,
    pub service_cidr: String,
    pub cluster_dns: String,
    pub flannel_backend: String,
}

impl From<ClusterNetworkSelection> for ClusterNetworkConfig {
    fn from(value: ClusterNetworkSelection) -> Self {
        Self {
            cluster_cidr: value.cluster_cidr,
            service_cidr: value.service_cidr,
            cluster_dns: value.cluster_dns,
            flannel_backend: value.flannel_backend,
        }
    }
}

#[derive(Copy, Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NodeDefaultSelections {
    pub server: NodeSizeSelection,
    pub agent: NodeSizeSelection,
}

#[derive(Copy, Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NodeSizeSelection {
    pub vcpus: u16,
    pub memory_mib: u64,
    pub root_disk_size_gib: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExportSelection {
    pub dataset: String,
    #[serde(default)]
    pub source_path: Option<String>,
    #[serde(default)]
    pub tag: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NodeRuntimeConfig {
    #[serde(rename = "apiVersion")]
    pub api_version: String,
    pub kind: String,
    pub node_name: String,
    pub domain: String,
    #[serde(default)]
    pub exports: Vec<NodeRuntimeExport>,
}

impl NodeRuntimeConfig {
    pub fn validate(&self) -> Vec<String> {
        let mut errors = Vec::new();
        if self.api_version != API_VERSION {
            errors.push(format!("apiVersion must be {API_VERSION}"));
        }
        if self.kind != "NodeRuntimeConfig" {
            errors.push("kind must be NodeRuntimeConfig".to_string());
        }
        push_non_empty(&mut errors, "nodeName", &self.node_name);
        push_non_empty(&mut errors, "domain", &self.domain);

        let mut ids = BTreeSet::new();
        let mut tags = BTreeSet::new();
        let mut mount_paths = BTreeSet::new();
        for export in &self.exports {
            push_non_empty(&mut errors, "exports.id", &export.id);
            push_non_empty(&mut errors, "exports.dataset", &export.dataset);
            push_non_empty(&mut errors, "exports.sourcePath", &export.source_path);
            push_non_empty(&mut errors, "exports.tag", &export.tag);
            push_non_empty(&mut errors, "exports.policy", &export.policy);
            push_non_empty(
                &mut errors,
                "exports.guestMountPath",
                &export.guest_mount_path,
            );
            if !ids.insert(export.id.as_str()) {
                errors.push(format!("duplicate runtime export id: {}", export.id));
            }
            if !tags.insert(export.tag.as_str()) {
                errors.push(format!("duplicate runtime export tag: {}", export.tag));
            }
            if !mount_paths.insert(export.guest_mount_path.as_str()) {
                errors.push(format!(
                    "duplicate runtime export mount path: {}",
                    export.guest_mount_path
                ));
            }
        }

        errors
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NodeRuntimeExport {
    pub id: String,
    pub dataset: String,
    pub source_path: String,
    pub tag: String,
    pub policy: String,
    pub access: AccessMode,
    pub guest_mount_path: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HostTrueNasConfig {
    pub url: String,
    pub api_key_file: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HostToolConfig {
    pub virsh: String,
    pub qemu_img: String,
    pub virtiofsd: String,
    pub systemctl: String,
}

impl Default for HostToolConfig {
    fn default() -> Self {
        Self {
            virsh: "virsh".to_string(),
            qemu_img: "qemu-img".to_string(),
            virtiofsd: "virtiofsd".to_string(),
            systemctl: "systemctl".to_string(),
        }
    }
}

impl HostToolConfig {
    pub fn validate(&self) -> Vec<String> {
        let mut errors = Vec::new();
        push_non_empty(&mut errors, "hostTools.virsh", &self.virsh);
        push_non_empty(&mut errors, "hostTools.qemuImg", &self.qemu_img);
        push_non_empty(&mut errors, "hostTools.virtiofsd", &self.virtiofsd);
        push_non_empty(&mut errors, "hostTools.systemctl", &self.systemctl);
        errors
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HostLibvirtConfig {
    pub uri: String,
    pub bridge: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DatasetRef {
    pub dataset: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ClusterConfig {
    pub name: String,
    pub distribution: ClusterDistribution,
    pub profile: ClusterProfile,
    pub version: String,
    pub token_file: String,
    pub kubeconfig_out: String,
    pub api_server: ApiServerConfig,
    pub network: ClusterNetworkConfig,
    #[serde(default)]
    pub disable: Vec<String>,
    #[serde(default)]
    pub addons: AddonIntent,
}

#[derive(Copy, Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ClusterDistribution {
    K3s,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ApiServerConfig {
    pub endpoint: String,
    #[serde(default)]
    pub tls_sans: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ClusterNetworkConfig {
    pub cluster_cidr: String,
    pub service_cidr: String,
    pub cluster_dns: String,
    pub flannel_backend: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NodeConfig {
    pub name: String,
    pub domain: String,
    pub role: NodeRole,
    pub autostart: bool,
    pub vcpus: u16,
    pub memory_mib: u64,
    pub machine: String,
    pub firmware: String,
    pub cpu: String,
    pub network: NodeNetworkConfig,
    pub root_disk: RootDiskConfig,
    pub k3s: NodeK3sConfig,
    #[serde(default)]
    pub exports: Vec<String>,
}

#[derive(Copy, Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum NodeRole {
    Server,
    Agent,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NodeNetworkConfig {
    pub bridge: String,
    pub mac: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RootDiskConfig {
    pub image: String,
    #[serde(default)]
    pub source_image: Option<String>,
    #[serde(default)]
    pub source_format: Option<DiskFormat>,
    pub size_gib: u64,
    pub format: DiskFormat,
}

#[derive(Copy, Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum DiskFormat {
    Qcow2,
    Raw,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NodeK3sConfig {
    #[serde(default)]
    pub cluster_init: bool,
    pub server: Option<String>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    #[serde(default)]
    pub taints: Vec<NodeTaint>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NodeTaint {
    pub key: String,
    pub value: String,
    pub effect: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExportConfig {
    pub dataset: String,
    pub source_path: String,
    pub tag: String,
    pub policy: String,
    pub access: AccessMode,
}

fn node_config_from_selection(
    prefix: &str,
    index: u16,
    role: NodeRole,
    intent: &ClusterIntent,
    selections: &HostSelections,
    vm_state_path: &str,
    cluster_init: bool,
) -> NodeConfig {
    let name = format!("{prefix}-{index}");
    let domain = format!("nascsi-{name}");
    let size = match role {
        NodeRole::Server => selections.node_defaults.server,
        NodeRole::Agent => selections.node_defaults.agent,
    };
    let sequence = match role {
        NodeRole::Server => index,
        NodeRole::Agent => intent.nodes.servers + index,
    };
    let server = if cluster_init {
        None
    } else {
        Some(selections.cluster.api_endpoint.clone())
    };

    NodeConfig {
        name,
        domain: domain.clone(),
        role,
        autostart: true,
        vcpus: size.vcpus,
        memory_mib: size.memory_mib,
        machine: "q35".to_string(),
        firmware: "efi".to_string(),
        cpu: "host-passthrough".to_string(),
        network: NodeNetworkConfig {
            bridge: selections.libvirt.bridge.clone(),
            mac: generated_mac(sequence),
        },
        root_disk: RootDiskConfig {
            image: format!(
                "{}/nodes/{domain}/root.qcow2",
                vm_state_path.trim_end_matches('/')
            ),
            source_image: Some(selections.image.source.clone()),
            source_format: Some(DiskFormat::Qcow2),
            size_gib: size.root_disk_size_gib,
            format: DiskFormat::Qcow2,
        },
        k3s: NodeK3sConfig {
            cluster_init,
            server,
            labels: BTreeMap::from([("nas-csi.dev/storage-node".to_string(), "true".to_string())]),
            taints: Vec::new(),
        },
        exports: intent
            .storage_policies
            .iter()
            .map(|policy| policy.name.clone())
            .collect(),
    }
}

fn generated_mac(sequence: u16) -> String {
    let value = u32::from(sequence);
    format!(
        "52:54:4e:{:02x}:{:02x}:{:02x}",
        (value >> 16) & 0xff,
        (value >> 8) & 0xff,
        value & 0xff
    )
}

fn dataset_path(discovery: &DiscoveryInventory, dataset_name: &str) -> String {
    discovery
        .truenas
        .filesystem_datasets
        .iter()
        .find(|dataset| dataset.name == dataset_name)
        .and_then(|dataset| dataset.mountpoint.as_deref())
        .filter(|mountpoint| {
            let trimmed = mountpoint.trim();
            !trimmed.is_empty() && trimmed != "none" && trimmed != "legacy"
        })
        .map(str::to_string)
        .unwrap_or_else(|| format!("/mnt/{dataset_name}"))
}

fn tool_or_default(tool: &ToolStatus, default_command: &str) -> String {
    tool.path
        .clone()
        .unwrap_or_else(|| default_command.to_string())
}

fn virtiofs_tag(policy_name: &str) -> String {
    format!("nascsi_{}", safe_identifier(policy_name))
}

fn safe_identifier(value: &str) -> String {
    value
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect()
}

fn default_disabled_k3s_components() -> Vec<String> {
    vec!["traefik".to_string(), "servicelb".to_string()]
}

fn push_non_empty(errors: &mut Vec<String>, field: &str, value: &str) {
    if value.trim().is_empty() {
        errors.push(format!("{field} must not be empty"));
    }
}

fn validate_node_size(errors: &mut Vec<String>, field: &str, size: NodeSizeSelection) {
    if size.vcpus == 0 {
        errors.push(format!("{field}.vcpus must be greater than zero"));
    }
    if size.memory_mib == 0 {
        errors.push(format!("{field}.memoryMib must be greater than zero"));
    }
    if size.root_disk_size_gib == 0 {
        errors.push(format!("{field}.rootDiskSizeGib must be greater than zero"));
    }
}

impl PlannedAction {
    pub fn new(description: impl Into<String>) -> Self {
        Self {
            description: description.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_basic_intent() {
        let intent = ClusterIntent {
            api_version: API_VERSION.to_string(),
            kind: "ClusterIntent".to_string(),
            profile: ClusterProfile::MaintenanceBasic,
            nodes: IntentNodes {
                servers: 1,
                agents: 2,
            },
            storage_policies: vec![],
            addons: AddonIntent::default(),
            applications: vec![],
        };

        assert!(intent.validate().is_empty());
    }

    #[test]
    fn control_plane_profile_requires_three_servers() {
        let intent = ClusterIntent {
            api_version: API_VERSION.to_string(),
            kind: "ClusterIntent".to_string(),
            profile: ClusterProfile::MaintenanceControlPlane,
            nodes: IntentNodes {
                servers: 1,
                agents: 0,
            },
            storage_policies: vec![],
            addons: AddonIntent::default(),
            applications: vec![],
        };

        assert!(
            intent
                .validate()
                .iter()
                .any(|error| error.contains("at least three"))
        );
    }

    #[test]
    fn repo_intent_examples_validate() {
        let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let repo_root = manifest_dir
            .parent()
            .and_then(std::path::Path::parent)
            .expect("repo root");
        let examples_dir = repo_root.join("examples").join("intents");

        for file_name in ["maintenance-basic.yaml", "maintenance-control-plane.yaml"] {
            let path = examples_dir.join(file_name);
            let content = std::fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
            let intent: ClusterIntent = serde_yml::from_str(&content)
                .unwrap_or_else(|error| panic!("parse {}: {error}", path.display()));
            let errors = intent.validate();
            assert!(errors.is_empty(), "{}: {errors:?}", path.display());
        }
    }

    #[test]
    fn repo_host_config_examples_validate() {
        let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let repo_root = manifest_dir
            .parent()
            .and_then(std::path::Path::parent)
            .expect("repo root");
        let examples_dir = repo_root.join("examples").join("configs");

        for file_name in ["host.sample.yaml"] {
            let path = examples_dir.join(file_name);
            let content = std::fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
            let config: HostConfig = serde_yml::from_str(&content)
                .unwrap_or_else(|error| panic!("parse {}: {error}", path.display()));
            let errors = config.validate();
            assert!(errors.is_empty(), "{}: {errors:?}", path.display());
        }
    }

    #[test]
    fn repo_materialization_examples_validate() {
        let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let repo_root = manifest_dir
            .parent()
            .and_then(std::path::Path::parent)
            .expect("repo root");

        let intent_path = repo_root
            .join("examples")
            .join("intents")
            .join("maintenance-basic.yaml");
        let discovery_path = repo_root
            .join("examples")
            .join("configs")
            .join("discovery.sample.yaml");
        let selections_path = repo_root
            .join("examples")
            .join("configs")
            .join("selections.sample.yaml");

        let intent: ClusterIntent =
            serde_yml::from_str(&std::fs::read_to_string(&intent_path).expect("read intent"))
                .expect("parse intent");
        let discovery: DiscoveryInventory =
            serde_yml::from_str(&std::fs::read_to_string(&discovery_path).expect("read discovery"))
                .expect("parse discovery");
        let selections: HostSelections = serde_yml::from_str(
            &std::fs::read_to_string(&selections_path).expect("read selections"),
        )
        .expect("parse selections");

        let config = HostConfig::from_intent_discovery_selections(&intent, &discovery, &selections)
            .expect("materialize example config");

        assert!(config.validate().is_empty());
        assert_eq!(config.nodes.len(), 3);
        assert_eq!(config.exports.len(), 2);
    }

    #[test]
    fn materializes_host_config_from_discovery_and_selections() {
        let intent = ClusterIntent {
            api_version: API_VERSION.to_string(),
            kind: "ClusterIntent".to_string(),
            profile: ClusterProfile::MaintenanceBasic,
            nodes: IntentNodes {
                servers: 1,
                agents: 1,
            },
            storage_policies: vec![StoragePolicyIntent {
                name: "repos-dev".to_string(),
                access: AccessMode::ReadWrite,
                workload: "git-repositories".to_string(),
            }],
            addons: AddonIntent {
                nas_csi: true,
                metrics_server: true,
            },
            applications: vec![],
        };
        let discovery = sample_discovery();
        let selections = sample_host_selections();

        let config = HostConfig::from_intent_discovery_selections(&intent, &discovery, &selections)
            .expect("materialize config");

        assert!(config.validate().is_empty());
        assert_eq!(config.nodes.len(), 2);
        assert_eq!(config.nodes[0].name, "server-1");
        assert!(config.nodes[0].k3s.cluster_init);
        assert_eq!(
            config.nodes[1].k3s.server.as_deref(),
            Some("https://nas-csi-api.example.test:6443")
        );
        assert_eq!(config.exports["repos-dev"].source_path, "/mnt/tank/repos");
        assert_eq!(
            config.nodes[0].root_disk.image,
            "/mnt/tank/nas-csi/vms/nodes/nascsi-server-1/root.qcow2"
        );
    }

    #[test]
    fn materialize_rejects_missing_export_selection() {
        let intent = ClusterIntent {
            api_version: API_VERSION.to_string(),
            kind: "ClusterIntent".to_string(),
            profile: ClusterProfile::MaintenanceBasic,
            nodes: IntentNodes {
                servers: 1,
                agents: 1,
            },
            storage_policies: vec![StoragePolicyIntent {
                name: "repos-dev".to_string(),
                access: AccessMode::ReadWrite,
                workload: "git-repositories".to_string(),
            }],
            addons: AddonIntent::default(),
            applications: vec![],
        };
        let discovery = sample_discovery();
        let mut selections = sample_host_selections();
        selections.exports.clear();

        let errors = HostConfig::from_intent_discovery_selections(&intent, &discovery, &selections)
            .expect_err("missing export selection");

        assert!(
            errors
                .iter()
                .any(|error| error.contains("missing export selection"))
        );
    }

    #[test]
    fn validates_node_runtime_config() {
        let runtime = NodeRuntimeConfig {
            api_version: API_VERSION.to_string(),
            kind: "NodeRuntimeConfig".to_string(),
            node_name: "agent-1".to_string(),
            domain: "nascsi-agent-1".to_string(),
            exports: vec![NodeRuntimeExport {
                id: "repos-dev".to_string(),
                dataset: "tank/repos".to_string(),
                source_path: "/mnt/tank/repos".to_string(),
                tag: "nascsi_repos_dev".to_string(),
                policy: "repos-dev".to_string(),
                access: AccessMode::ReadWrite,
                guest_mount_path: "/var/lib/nas-csi/virtiofs/repos-dev".to_string(),
            }],
        };

        assert!(runtime.validate().is_empty());
    }

    #[test]
    fn validates_host_config_references() {
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

        let config = HostConfig {
            api_version: API_VERSION.to_string(),
            kind: "HostConfig".to_string(),
            truenas: HostTrueNasConfig {
                url: "wss://127.0.0.1/api/current".to_string(),
                api_key_file: "/local/key".to_string(),
            },
            host_tools: HostToolConfig::default(),
            libvirt: HostLibvirtConfig {
                uri: "qemu:///system".to_string(),
                bridge: "br0".to_string(),
            },
            image_cache: DatasetRef {
                dataset: "pool/images".to_string(),
            },
            vm_state: DatasetRef {
                dataset: "pool/vms".to_string(),
            },
            cluster: ClusterConfig {
                name: "test".to_string(),
                distribution: ClusterDistribution::K3s,
                profile: ClusterProfile::MaintenanceBasic,
                version: "v1.test".to_string(),
                token_file: "/local/token".to_string(),
                kubeconfig_out: "/local/kubeconfig".to_string(),
                api_server: ApiServerConfig {
                    endpoint: "https://cluster.example.test:6443".to_string(),
                    tls_sans: Vec::new(),
                },
                network: ClusterNetworkConfig {
                    cluster_cidr: "10.42.0.0/16".to_string(),
                    service_cidr: "10.43.0.0/16".to_string(),
                    cluster_dns: "10.43.0.10".to_string(),
                    flannel_backend: "vxlan".to_string(),
                },
                disable: Vec::new(),
                addons: AddonIntent::default(),
            },
            nodes: vec![
                NodeConfig {
                    name: "server-1".to_string(),
                    domain: "nascsi-server-1".to_string(),
                    role: NodeRole::Server,
                    autostart: true,
                    vcpus: 2,
                    memory_mib: 2048,
                    machine: "q35".to_string(),
                    firmware: "efi".to_string(),
                    cpu: "host-passthrough".to_string(),
                    network: NodeNetworkConfig {
                        bridge: "br0".to_string(),
                        mac: "52:54:00:00:00:01".to_string(),
                    },
                    root_disk: RootDiskConfig {
                        image: "image.qcow2".to_string(),
                        source_image: None,
                        source_format: None,
                        size_gib: 80,
                        format: DiskFormat::Qcow2,
                    },
                    k3s: NodeK3sConfig {
                        cluster_init: true,
                        ..NodeK3sConfig::default()
                    },
                    exports: vec!["repos".to_string()],
                },
                NodeConfig {
                    name: "agent-1".to_string(),
                    domain: "nascsi-agent-1".to_string(),
                    role: NodeRole::Agent,
                    autostart: true,
                    vcpus: 2,
                    memory_mib: 2048,
                    machine: "q35".to_string(),
                    firmware: "efi".to_string(),
                    cpu: "host-passthrough".to_string(),
                    network: NodeNetworkConfig {
                        bridge: "br0".to_string(),
                        mac: "52:54:00:00:00:02".to_string(),
                    },
                    root_disk: RootDiskConfig {
                        image: "image.qcow2".to_string(),
                        source_image: None,
                        source_format: None,
                        size_gib: 80,
                        format: DiskFormat::Qcow2,
                    },
                    k3s: NodeK3sConfig {
                        server: Some("https://cluster.example.test:6443".to_string()),
                        ..NodeK3sConfig::default()
                    },
                    exports: vec!["repos".to_string()],
                },
            ],
            exports,
        };

        assert!(config.validate().is_empty());
    }

    #[test]
    fn rejects_node_missing_export_reference() {
        let mut config = HostConfig {
            api_version: API_VERSION.to_string(),
            kind: "HostConfig".to_string(),
            truenas: HostTrueNasConfig {
                url: "wss://127.0.0.1/api/current".to_string(),
                api_key_file: "/local/key".to_string(),
            },
            host_tools: HostToolConfig::default(),
            libvirt: HostLibvirtConfig {
                uri: "qemu:///system".to_string(),
                bridge: "br0".to_string(),
            },
            image_cache: DatasetRef {
                dataset: "pool/images".to_string(),
            },
            vm_state: DatasetRef {
                dataset: "pool/vms".to_string(),
            },
            cluster: ClusterConfig {
                name: "test".to_string(),
                distribution: ClusterDistribution::K3s,
                profile: ClusterProfile::MaintenanceBasic,
                version: "v1.test".to_string(),
                token_file: "/local/token".to_string(),
                kubeconfig_out: "/local/kubeconfig".to_string(),
                api_server: ApiServerConfig {
                    endpoint: "https://cluster.example.test:6443".to_string(),
                    tls_sans: Vec::new(),
                },
                network: ClusterNetworkConfig {
                    cluster_cidr: "10.42.0.0/16".to_string(),
                    service_cidr: "10.43.0.0/16".to_string(),
                    cluster_dns: "10.43.0.10".to_string(),
                    flannel_backend: "vxlan".to_string(),
                },
                disable: Vec::new(),
                addons: AddonIntent::default(),
            },
            nodes: Vec::new(),
            exports: BTreeMap::new(),
        };
        config.nodes.push(NodeConfig {
            name: "server-1".to_string(),
            domain: "nascsi-server-1".to_string(),
            role: NodeRole::Server,
            autostart: true,
            vcpus: 2,
            memory_mib: 2048,
            machine: "q35".to_string(),
            firmware: "efi".to_string(),
            cpu: "host-passthrough".to_string(),
            network: NodeNetworkConfig {
                bridge: "br0".to_string(),
                mac: "52:54:00:00:00:01".to_string(),
            },
            root_disk: RootDiskConfig {
                image: "image.qcow2".to_string(),
                source_image: None,
                source_format: None,
                size_gib: 80,
                format: DiskFormat::Qcow2,
            },
            k3s: NodeK3sConfig {
                cluster_init: true,
                ..NodeK3sConfig::default()
            },
            exports: vec!["missing".to_string()],
        });

        assert!(
            config
                .validate()
                .iter()
                .any(|error| error.contains("missing export"))
        );
    }

    fn sample_discovery() -> DiscoveryInventory {
        DiscoveryInventory {
            api_version: API_VERSION.to_string(),
            kind: "DiscoveryInventory".to_string(),
            generated_unix_seconds: 1,
            host: HostFacts {
                os_pretty_name: Some("TrueNAS SCALE test".to_string()),
                architecture: "x86_64".to_string(),
                cpu_count: Some(16),
                memory_total_kib: Some(64 * 1024 * 1024),
            },
            truenas: TrueNasFacts {
                local_api_url: Some("wss://127.0.0.1/api/current".to_string()),
                version: Some("test-version".to_string()),
                pools: vec![PoolSummary {
                    name: "tank".to_string(),
                }],
                filesystem_datasets: vec![
                    DatasetSummary {
                        name: "tank/repos".to_string(),
                        mountpoint: Some("/mnt/tank/repos".to_string()),
                    },
                    DatasetSummary {
                        name: "tank/nas-csi/images".to_string(),
                        mountpoint: Some("/mnt/tank/nas-csi/images".to_string()),
                    },
                    DatasetSummary {
                        name: "tank/nas-csi/vms".to_string(),
                        mountpoint: Some("/mnt/tank/nas-csi/vms".to_string()),
                    },
                ],
                smb_shares: vec![SmbShareSummary {
                    name: "repos".to_string(),
                    path: "/mnt/tank/repos".to_string(),
                }],
            },
            libvirt: LibvirtFacts {
                uri: "qemu:///system".to_string(),
                virsh: ToolStatus {
                    path: Some("/usr/bin/virsh".to_string()),
                    version: Some("9.0".to_string()),
                },
                qemu: ToolStatus {
                    path: Some("/usr/bin/qemu-system-x86_64".to_string()),
                    version: Some("QEMU emulator".to_string()),
                },
                default_machine: Some("q35".to_string()),
            },
            network: NetworkFacts {
                bridges: vec![BridgeSummary {
                    name: "br0".to_string(),
                    interfaces: vec!["eno1".to_string()],
                }],
                lan_addresses: Vec::new(),
            },
            tools: ToolFacts {
                virtiofsd: ToolStatus {
                    path: Some("/usr/libexec/virtiofsd".to_string()),
                    version: Some("virtiofsd 1.0".to_string()),
                },
                qemu_img: ToolStatus {
                    path: Some("/usr/bin/qemu-img".to_string()),
                    version: Some("qemu-img".to_string()),
                },
                systemctl: ToolStatus {
                    path: Some("/usr/bin/systemctl".to_string()),
                    version: Some("systemd 252".to_string()),
                },
                midclt: ToolStatus {
                    path: Some("/usr/bin/midclt".to_string()),
                    version: Some("midclt sample".to_string()),
                },
            },
            existing_project_state: ExistingProjectState::default(),
            warnings: Vec::new(),
        }
    }

    fn sample_host_selections() -> HostSelections {
        HostSelections {
            api_version: API_VERSION.to_string(),
            kind: "HostSelections".to_string(),
            truenas: HostTrueNasSelection {
                api_key_file: "/etc/nas-csi/secrets/truenas-api-key".to_string(),
                url: None,
            },
            libvirt: HostLibvirtSelection {
                bridge: "br0".to_string(),
                uri: None,
            },
            image: ImageSelection {
                source: "/mnt/tank/nas-csi/images/debian.qcow2".to_string(),
            },
            datasets: HostDatasetSelections {
                image_cache: "tank/nas-csi/images".to_string(),
                vm_state: "tank/nas-csi/vms".to_string(),
            },
            cluster: ClusterSelection {
                name: "nas-csi".to_string(),
                version: "v1.33.0+k3s1".to_string(),
                api_endpoint: "https://nas-csi-api.example.test:6443".to_string(),
                token_file: "/etc/nas-csi/secrets/k3s-token".to_string(),
                kubeconfig_out: "/etc/nas-csi/kubeconfig".to_string(),
                tls_sans: vec!["nas-csi-api.example.test".to_string()],
                disable: default_disabled_k3s_components(),
                network: ClusterNetworkSelection {
                    cluster_cidr: "10.42.0.0/16".to_string(),
                    service_cidr: "10.43.0.0/16".to_string(),
                    cluster_dns: "10.43.0.10".to_string(),
                    flannel_backend: "vxlan".to_string(),
                },
            },
            node_defaults: NodeDefaultSelections {
                server: NodeSizeSelection {
                    vcpus: 2,
                    memory_mib: 4096,
                    root_disk_size_gib: 80,
                },
                agent: NodeSizeSelection {
                    vcpus: 6,
                    memory_mib: 16384,
                    root_disk_size_gib: 120,
                },
            },
            exports: BTreeMap::from([(
                "repos-dev".to_string(),
                ExportSelection {
                    dataset: "tank/repos".to_string(),
                    source_path: None,
                    tag: None,
                },
            )]),
        }
    }
}
