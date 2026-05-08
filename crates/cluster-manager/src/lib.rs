//! k3s cluster lifecycle planning and config rendering.

use nas_csi_types::{ClusterProfile, HostConfig, HostConfigDraft, NodeConfig, NodeRole, NodeTaint};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClusterPlan {
    pub profile: ClusterProfile,
    pub servers: u16,
    pub agents: u16,
    pub notes: Vec<String>,
}

impl ClusterPlan {
    pub fn from_draft(draft: &HostConfigDraft) -> Self {
        let mut notes = Vec::new();
        match draft.profile {
            ClusterProfile::MaintenanceBasic => {
                notes.push("server VM maintenance is a planned Kubernetes API outage".to_string());
                notes.push("agent VM maintenance can be rolling after drain succeeds".to_string());
            }
            ClusterProfile::MaintenanceControlPlane => {
                notes.push("server VM maintenance requires etcd quorum checks".to_string());
                notes.push("roll one server VM at a time".to_string());
            }
        }

        Self {
            profile: draft.profile,
            servers: draft.nodes.servers,
            agents: draft.nodes.agents,
            notes,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum K3sRole {
    Server,
    Agent,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct K3sConfigInput {
    pub role: K3sRole,
    pub token: Option<String>,
    pub token_file: Option<String>,
    pub server_url: Option<String>,
    pub cluster_init: bool,
    pub tls_sans: Vec<String>,
    pub node_labels: Vec<String>,
    pub node_taints: Vec<String>,
    pub disable: Vec<String>,
    pub cluster_cidr: Option<String>,
    pub service_cidr: Option<String>,
    pub flannel_backend: Option<String>,
}

pub fn render_k3s_config(input: &K3sConfigInput) -> Result<String, serde_yml::Error> {
    let config = K3sConfigFile {
        token: input.token.as_deref(),
        token_file: input.token_file.as_deref(),
        server: input.server_url.as_deref(),
        cluster_init: input.cluster_init.then_some(true),
        tls_san: none_if_empty(&input.tls_sans),
        node_label: none_if_empty(&input.node_labels),
        node_taint: none_if_empty(&input.node_taints),
        disable: none_if_empty(&input.disable),
        cluster_cidr: input.cluster_cidr.as_deref(),
        service_cidr: input.service_cidr.as_deref(),
        flannel_backend: input.flannel_backend.as_deref(),
    };

    serde_yml::to_string(&config)
}

fn none_if_empty(values: &[String]) -> Option<&[String]> {
    if values.is_empty() {
        None
    } else {
        Some(values)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClusterReconcileOptions {
    pub kubectl_path: String,
    pub virsh_path: String,
    pub libvirt_uri: String,
    pub artifact_dir: String,
    pub first_server_kubeconfig_path: String,
    pub guest_k3s_binary: String,
    pub guest_kubeconfig_path: String,
}

impl Default for ClusterReconcileOptions {
    fn default() -> Self {
        Self {
            kubectl_path: "kubectl".to_string(),
            virsh_path: "virsh".to_string(),
            libvirt_uri: "qemu:///system".to_string(),
            artifact_dir: ".nas-csi/rendered".to_string(),
            first_server_kubeconfig_path: "/etc/rancher/k3s/k3s.yaml".to_string(),
            guest_k3s_binary: "/usr/local/bin/k3s".to_string(),
            guest_kubeconfig_path: "/etc/rancher/k3s/k3s.yaml".to_string(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClusterActualState {
    pub token_present: bool,
    pub kubeconfig_present: bool,
    pub api_ready: bool,
    pub nodes: BTreeMap<String, ClusterNodeActualState>,
    pub applied_manifests: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClusterNodeActualState {
    pub domain_running: bool,
    pub k3s_ready: bool,
    pub kubernetes_ready: bool,
    pub labels: BTreeMap<String, String>,
    pub taints: Vec<NodeTaint>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClusterReconcilePlan {
    pub steps: Vec<ClusterReconcileStep>,
}

impl ClusterReconcilePlan {
    pub fn is_current(&self) -> bool {
        self.steps.iter().all(|step| {
            matches!(
                &step.kind,
                ClusterReconcileStepKind::SkipAlreadyCorrect { .. }
            )
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClusterReconcileStep {
    pub description: String,
    pub kind: ClusterReconcileStepKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClusterReconcileStepKind {
    Apply(ClusterOperation),
    SkipAlreadyCorrect { reason: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClusterOperation {
    EnsureToken {
        path: String,
    },
    StartFirstServer {
        node: String,
        domain: String,
        command: ClusterCommandSpec,
    },
    WaitForFirstServer {
        node: String,
        domain: String,
        command: GuestCommandSpec,
    },
    RetrieveKubeconfig {
        node: String,
        domain: String,
        guest_path: String,
        host_path: String,
        server_endpoint: String,
    },
    WaitForClusterApi {
        command: ClusterCommandSpec,
    },
    StartJoinNode {
        node: String,
        domain: String,
        role: NodeRole,
        command: ClusterCommandSpec,
    },
    WaitForNodeReady {
        node: String,
        command: ClusterCommandSpec,
    },
    ReconcileNodeLabels {
        node: String,
        commands: Vec<ClusterCommandSpec>,
    },
    ReconcileNodeTaints {
        node: String,
        commands: Vec<ClusterCommandSpec>,
    },
    ApplyAddon {
        name: String,
        manifest_path: String,
        desired_hash: String,
        command: ClusterCommandSpec,
        marker_path: String,
    },
    ApplyNasCsi {
        manifest_path: String,
        desired_hash: String,
        command: ClusterCommandSpec,
        marker_path: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClusterCommandSpec {
    pub program: String,
    pub args: Vec<String>,
}

impl ClusterCommandSpec {
    pub fn new(program: impl Into<String>, args: impl IntoIterator<Item = String>) -> Self {
        Self {
            program: program.into(),
            args: args.into_iter().collect(),
        }
    }
}

impl fmt::Display for ClusterCommandSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", shell_quote(&self.program))?;
        for arg in &self.args {
            write!(f, " {}", shell_quote(arg))?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GuestCommandSpec {
    pub program: String,
    pub args: Vec<String>,
}

impl GuestCommandSpec {
    pub fn new(program: impl Into<String>, args: impl IntoIterator<Item = String>) -> Self {
        Self {
            program: program.into(),
            args: args.into_iter().collect(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DesiredManifest {
    pub name: String,
    pub path: String,
    pub contents: String,
}

impl DesiredManifest {
    pub fn desired_hash(&self) -> String {
        content_hash(self.contents.as_bytes())
    }
}

pub fn plan_cluster_reconcile(
    config: &HostConfig,
    options: &ClusterReconcileOptions,
    actual: &ClusterActualState,
    manifests: &[DesiredManifest],
) -> ClusterReconcilePlan {
    let mut steps = Vec::new();
    let first_server = config
        .nodes
        .iter()
        .find(|node| node.role == NodeRole::Server && node.k3s.cluster_init)
        .or_else(|| {
            config
                .nodes
                .iter()
                .find(|node| node.role == NodeRole::Server)
        });

    if actual.token_present {
        steps.push(skip(
            "ensure k3s cluster token",
            "token file already exists",
        ));
    } else {
        steps.push(apply(
            "generate k3s cluster token",
            ClusterOperation::EnsureToken {
                path: config.cluster.token_file.clone(),
            },
        ));
    }

    if let Some(node) = first_server {
        let state = actual.nodes.get(&node.name).cloned().unwrap_or_default();
        if state.domain_running {
            steps.push(skip(
                format!("start first k3s server {}", node.name),
                "domain is already running",
            ));
        } else {
            steps.push(apply(
                format!("start first k3s server {}", node.name),
                ClusterOperation::StartFirstServer {
                    node: node.name.clone(),
                    domain: node.domain.clone(),
                    command: start_domain_command(options, &node.domain),
                },
            ));
        }

        if state.k3s_ready {
            steps.push(skip(
                format!("wait for first k3s server {}", node.name),
                "guest k3s API is already ready",
            ));
        } else {
            steps.push(apply(
                format!("wait for first k3s server {}", node.name),
                ClusterOperation::WaitForFirstServer {
                    node: node.name.clone(),
                    domain: node.domain.clone(),
                    command: GuestCommandSpec::new(
                        options.guest_k3s_binary.clone(),
                        [
                            "kubectl".to_string(),
                            "--kubeconfig".to_string(),
                            options.guest_kubeconfig_path.clone(),
                            "get".to_string(),
                            "--raw=/readyz".to_string(),
                        ],
                    ),
                },
            ));
        }

        if actual.kubeconfig_present {
            steps.push(skip(
                "retrieve cluster kubeconfig",
                "host-local kubeconfig already exists",
            ));
        } else {
            steps.push(apply(
                "retrieve cluster kubeconfig",
                ClusterOperation::RetrieveKubeconfig {
                    node: node.name.clone(),
                    domain: node.domain.clone(),
                    guest_path: options.first_server_kubeconfig_path.clone(),
                    host_path: config.cluster.kubeconfig_out.clone(),
                    server_endpoint: config.cluster.api_server.endpoint.clone(),
                },
            ));
        }
    }

    if actual.api_ready {
        steps.push(skip(
            "wait for Kubernetes API readiness",
            "API readiness check already succeeds",
        ));
    } else {
        steps.push(apply(
            "wait for Kubernetes API readiness",
            ClusterOperation::WaitForClusterApi {
                command: kubectl_command(
                    config,
                    options,
                    ["get".to_string(), "--raw=/readyz".to_string()],
                ),
            },
        ));
    }

    for node in &config.nodes {
        if Some(node.name.as_str()) == first_server.map(|server| server.name.as_str()) {
            continue;
        }
        plan_join_node(&mut steps, config, options, actual, node);
    }

    for node in &config.nodes {
        let state = actual.nodes.get(&node.name).cloned().unwrap_or_default();
        if state.kubernetes_ready {
            steps.push(skip(
                format!("wait for Kubernetes node {} Ready", node.name),
                "node is already Ready",
            ));
        } else {
            steps.push(apply(
                format!("wait for Kubernetes node {} Ready", node.name),
                ClusterOperation::WaitForNodeReady {
                    node: node.name.clone(),
                    command: kubectl_command(
                        config,
                        options,
                        [
                            "wait".to_string(),
                            "node".to_string(),
                            node.name.clone(),
                            "--for=condition=Ready".to_string(),
                            "--timeout=10m".to_string(),
                        ],
                    ),
                },
            ));
        }

        let label_commands = label_reconcile_commands(config, options, node, &state.labels);
        if label_commands.is_empty() {
            steps.push(skip(
                format!("reconcile labels for node {}", node.name),
                "labels already match desired values",
            ));
        } else {
            steps.push(apply(
                format!("reconcile labels for node {}", node.name),
                ClusterOperation::ReconcileNodeLabels {
                    node: node.name.clone(),
                    commands: label_commands,
                },
            ));
        }

        let taint_commands = taint_reconcile_commands(config, options, node, &state.taints);
        if taint_commands.is_empty() {
            steps.push(skip(
                format!("reconcile taints for node {}", node.name),
                "taints already match desired values",
            ));
        } else {
            steps.push(apply(
                format!("reconcile taints for node {}", node.name),
                ClusterOperation::ReconcileNodeTaints {
                    node: node.name.clone(),
                    commands: taint_commands,
                },
            ));
        }
    }

    for manifest in manifests {
        let desired_hash = manifest.desired_hash();
        let marker_path = manifest_marker_path(options, &manifest.name);
        if actual.applied_manifests.get(&manifest.name) == Some(&desired_hash) {
            steps.push(skip(
                format!("apply substrate manifest {}", manifest.name),
                "manifest hash already recorded",
            ));
            continue;
        }

        let operation = if manifest.name == "nas-csi" {
            ClusterOperation::ApplyNasCsi {
                manifest_path: manifest.path.clone(),
                desired_hash,
                command: kubectl_apply_command(config, options, &manifest.path),
                marker_path,
            }
        } else {
            ClusterOperation::ApplyAddon {
                name: manifest.name.clone(),
                manifest_path: manifest.path.clone(),
                desired_hash,
                command: kubectl_apply_command(config, options, &manifest.path),
                marker_path,
            }
        };
        steps.push(apply(
            format!("apply substrate manifest {}", manifest.name),
            operation,
        ));
    }

    ClusterReconcilePlan { steps }
}

fn plan_join_node(
    steps: &mut Vec<ClusterReconcileStep>,
    config: &HostConfig,
    options: &ClusterReconcileOptions,
    actual: &ClusterActualState,
    node: &NodeConfig,
) {
    let state = actual.nodes.get(&node.name).cloned().unwrap_or_default();
    if state.domain_running {
        steps.push(skip(
            format!("start k3s join node {}", node.name),
            "domain is already running",
        ));
    } else {
        steps.push(apply(
            format!("start k3s join node {}", node.name),
            ClusterOperation::StartJoinNode {
                node: node.name.clone(),
                domain: node.domain.clone(),
                role: node.role,
                command: start_domain_command(options, &node.domain),
            },
        ));
    }

    if state.k3s_ready {
        steps.push(skip(
            format!("wait for k3s join node {}", node.name),
            "guest k3s service is already ready",
        ));
    } else {
        let service_name = match node.role {
            NodeRole::Server => "k3s",
            NodeRole::Agent => "k3s-agent",
        };
        steps.push(apply(
            format!("wait for k3s join node {}", node.name),
            ClusterOperation::WaitForFirstServer {
                node: node.name.clone(),
                domain: node.domain.clone(),
                command: GuestCommandSpec::new(
                    "/bin/systemctl".to_string(),
                    [
                        "is-active".to_string(),
                        "--quiet".to_string(),
                        service_name.to_string(),
                    ],
                ),
            },
        ));
    }

    if node.k3s.server.is_none() && node.role != NodeRole::Server {
        steps.push(apply(
            format!("verify k3s server URL for {}", node.name),
            ClusterOperation::WaitForClusterApi {
                command: kubectl_command(
                    config,
                    options,
                    ["get".to_string(), "--raw=/readyz".to_string()],
                ),
            },
        ));
    }
}

fn label_reconcile_commands(
    config: &HostConfig,
    options: &ClusterReconcileOptions,
    node: &NodeConfig,
    actual_labels: &BTreeMap<String, String>,
) -> Vec<ClusterCommandSpec> {
    node.k3s
        .labels
        .iter()
        .filter(|(key, value)| actual_labels.get(*key) != Some(*value))
        .map(|(key, value)| {
            kubectl_command(
                config,
                options,
                [
                    "label".to_string(),
                    "node".to_string(),
                    node.name.clone(),
                    format!("{key}={value}"),
                    "--overwrite".to_string(),
                ],
            )
        })
        .collect()
}

fn taint_reconcile_commands(
    config: &HostConfig,
    options: &ClusterReconcileOptions,
    node: &NodeConfig,
    actual_taints: &[NodeTaint],
) -> Vec<ClusterCommandSpec> {
    let actual = actual_taints.iter().map(taint_key).collect::<BTreeSet<_>>();
    node.k3s
        .taints
        .iter()
        .filter(|taint| !actual.contains(&taint_key(taint)))
        .map(|taint| {
            kubectl_command(
                config,
                options,
                [
                    "taint".to_string(),
                    "node".to_string(),
                    node.name.clone(),
                    format!("{}={}:{}", taint.key, taint.value, taint.effect),
                    "--overwrite".to_string(),
                ],
            )
        })
        .collect()
}

fn taint_key(taint: &NodeTaint) -> String {
    format!("{}={}:{}", taint.key, taint.value, taint.effect)
}

fn start_domain_command(options: &ClusterReconcileOptions, domain: &str) -> ClusterCommandSpec {
    ClusterCommandSpec::new(
        options.virsh_path.clone(),
        [
            "-c".to_string(),
            options.libvirt_uri.clone(),
            "start".to_string(),
            domain.to_string(),
        ],
    )
}

fn kubectl_apply_command(
    config: &HostConfig,
    options: &ClusterReconcileOptions,
    manifest_path: &str,
) -> ClusterCommandSpec {
    kubectl_command(
        config,
        options,
        [
            "apply".to_string(),
            "--server-side".to_string(),
            "-f".to_string(),
            manifest_path.to_string(),
        ],
    )
}

fn kubectl_command(
    config: &HostConfig,
    options: &ClusterReconcileOptions,
    args: impl IntoIterator<Item = String>,
) -> ClusterCommandSpec {
    let mut full_args = vec![
        "--kubeconfig".to_string(),
        config.cluster.kubeconfig_out.clone(),
    ];
    full_args.extend(args);
    ClusterCommandSpec::new(options.kubectl_path.clone(), full_args)
}

pub fn manifest_marker_path(options: &ClusterReconcileOptions, name: &str) -> String {
    Path::new(&options.artifact_dir)
        .join("cluster")
        .join("applied")
        .join(format!("{}.sha256", safe_path_segment(name)))
        .display()
        .to_string()
}

pub fn rewrite_kubeconfig_server(input: &str, server_endpoint: &str) -> String {
    let mut output = String::new();
    let mut replaced = false;

    for line in input.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("server: ") && !replaced {
            let indent_len = line.len() - trimmed.len();
            output.push_str(&line[..indent_len]);
            output.push_str("server: ");
            output.push_str(server_endpoint);
            output.push('\n');
            replaced = true;
        } else {
            output.push_str(line);
            output.push('\n');
        }
    }

    if input.is_empty() {
        output.clear();
    }
    output
}

pub fn token_looks_valid(token: &str) -> bool {
    let trimmed = token.trim();
    trimmed.len() >= 32
        && trimmed
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
}

fn apply(description: impl Into<String>, operation: ClusterOperation) -> ClusterReconcileStep {
    ClusterReconcileStep {
        description: description.into(),
        kind: ClusterReconcileStepKind::Apply(operation),
    }
}

fn skip(description: impl Into<String>, reason: impl Into<String>) -> ClusterReconcileStep {
    ClusterReconcileStep {
        description: description.into(),
        kind: ClusterReconcileStepKind::SkipAlreadyCorrect {
            reason: reason.into(),
        },
    }
}

fn content_hash(contents: &[u8]) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in contents {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
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

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct K3sConfigFile<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    token: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    token_file: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    server: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cluster_init: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tls_san: Option<&'a [String]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    node_label: Option<&'a [String]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    node_taint: Option<&'a [String]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    disable: Option<&'a [String]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cluster_cidr: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    service_cidr: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    flannel_backend: Option<&'a str>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_initial_server_config() {
        let yaml = render_k3s_config(&K3sConfigInput {
            role: K3sRole::Server,
            token: Some("token-value".to_string()),
            token_file: None,
            server_url: None,
            cluster_init: true,
            tls_sans: vec!["cluster.example.test".to_string()],
            node_labels: vec!["nas-csi.dev/storage-node=true".to_string()],
            node_taints: Vec::new(),
            disable: vec!["traefik".to_string(), "servicelb".to_string()],
            cluster_cidr: Some("10.42.0.0/16".to_string()),
            service_cidr: Some("10.43.0.0/16".to_string()),
            flannel_backend: Some("vxlan".to_string()),
        })
        .expect("render config");

        assert!(yaml.contains("cluster-init: true"));
        assert!(yaml.contains("token: token-value"));
        assert!(yaml.contains("cluster.example.test"));
        assert!(yaml.contains("traefik"));
    }

    #[test]
    fn renders_agent_join_config() {
        let yaml = render_k3s_config(&K3sConfigInput {
            role: K3sRole::Agent,
            token: None,
            token_file: Some("/etc/rancher/k3s/token".to_string()),
            server_url: Some("https://cluster.example.test:6443".to_string()),
            cluster_init: false,
            tls_sans: Vec::new(),
            node_labels: Vec::new(),
            node_taints: Vec::new(),
            disable: Vec::new(),
            cluster_cidr: None,
            service_cidr: None,
            flannel_backend: None,
        })
        .expect("render config");

        assert!(yaml.contains("server: https://cluster.example.test:6443"));
        assert!(yaml.contains("token-file: /etc/rancher/k3s/token"));
        assert!(!yaml.contains("cluster-init"));
    }

    #[test]
    fn plans_ordered_cluster_bootstrap_and_join() {
        let config = sample_host_config();
        let options = ClusterReconcileOptions {
            artifact_dir: "/var/lib/nas-csi/rendered".to_string(),
            virsh_path: "/usr/bin/virsh".to_string(),
            libvirt_uri: "qemu:///system".to_string(),
            ..ClusterReconcileOptions::default()
        };
        let manifests = vec![
            DesiredManifest {
                name: "metrics-server".to_string(),
                path: "/var/lib/nas-csi/rendered/addons/metrics-server.yaml".to_string(),
                contents: "kind: Deployment\n".to_string(),
            },
            DesiredManifest {
                name: "nas-csi".to_string(),
                path: "/var/lib/nas-csi/rendered/addons/nas-csi.yaml".to_string(),
                contents: "kind: CSIDriver\n".to_string(),
            },
        ];

        let plan = plan_cluster_reconcile(
            &config,
            &options,
            &ClusterActualState::default(),
            &manifests,
        );

        assert!(matches!(
            &plan.steps[0].kind,
            ClusterReconcileStepKind::Apply(ClusterOperation::EnsureToken { path })
                if path == "/etc/nas-csi/secrets/k3s-token"
        ));
        assert!(plan.steps.iter().any(|step| matches!(
            &step.kind,
            ClusterReconcileStepKind::Apply(ClusterOperation::StartFirstServer { node, .. })
                if node == "server-1"
        )));
        assert!(plan.steps.iter().any(|step| matches!(
            &step.kind,
            ClusterReconcileStepKind::Apply(ClusterOperation::StartJoinNode {
                node,
                role: NodeRole::Agent,
                ..
            }) if node == "agent-1"
        )));
        assert!(plan.steps.iter().any(|step| matches!(
            &step.kind,
            ClusterReconcileStepKind::Apply(ClusterOperation::ReconcileNodeLabels {
                node,
                commands
            }) if node == "server-1"
                && commands[0].args.contains(&"nas-csi.dev/storage-node=true".to_string())
        )));
        assert!(plan.steps.iter().any(|step| matches!(
            &step.kind,
            ClusterReconcileStepKind::Apply(ClusterOperation::ApplyNasCsi { marker_path, .. })
                if marker_path == "/var/lib/nas-csi/rendered/cluster/applied/nas-csi.sha256"
        )));
    }

    #[test]
    fn skips_current_cluster_state() {
        let config = sample_host_config();
        let options = ClusterReconcileOptions::default();
        let manifests = vec![DesiredManifest {
            name: "nas-csi".to_string(),
            path: ".nas-csi/rendered/addons/nas-csi.yaml".to_string(),
            contents: "kind: CSIDriver\n".to_string(),
        }];
        let mut actual = ClusterActualState {
            token_present: true,
            kubeconfig_present: true,
            api_ready: true,
            ..ClusterActualState::default()
        };
        for node in &config.nodes {
            actual.nodes.insert(
                node.name.clone(),
                ClusterNodeActualState {
                    domain_running: true,
                    k3s_ready: true,
                    kubernetes_ready: true,
                    labels: node.k3s.labels.clone(),
                    taints: node.k3s.taints.clone(),
                },
            );
        }
        actual
            .applied_manifests
            .insert("nas-csi".to_string(), manifests[0].desired_hash());

        let plan = plan_cluster_reconcile(&config, &options, &actual, &manifests);

        assert!(plan.is_current(), "{plan:?}");
    }

    #[test]
    fn rewrites_only_first_kubeconfig_server() {
        let input =
            "clusters:\n- cluster:\n    server: https://127.0.0.1:6443\nusers:\n- name: test\n";

        let output = rewrite_kubeconfig_server(input, "https://nas-csi-api.example.test:6443");

        assert!(output.contains("server: https://nas-csi-api.example.test:6443"));
        assert!(!output.contains("https://127.0.0.1:6443"));
    }

    #[test]
    fn validates_token_shape() {
        assert!(token_looks_valid("abcdefghijklmnopqrstuvwxyz0123456789"));
        assert!(!token_looks_valid("short"));
        assert!(!token_looks_valid("abcdefghijklmnopqrstuvwxyz0123456789$"));
    }

    fn sample_host_config() -> HostConfig {
        use nas_csi_types::{
            API_VERSION, AddonIntent, ApiServerConfig, ClusterConfig, ClusterDistribution,
            ClusterNetworkConfig, ClusterProfile, DatasetRef, DiskFormat, HostLibvirtConfig,
            HostToolConfig, HostTrueNasConfig, NodeK3sConfig, NodeNetworkConfig, RootDiskConfig,
        };

        HostConfig {
            api_version: API_VERSION.to_string(),
            kind: "HostConfig".to_string(),
            truenas: HostTrueNasConfig {
                url: "wss://127.0.0.1/api/current".to_string(),
                api_key_file: "/etc/nas-csi/secrets/truenas-api-key".to_string(),
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
            nodes: vec![
                NodeConfig {
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
                        image: "/pool/vms/server.qcow2".to_string(),
                        source_image: Some("/pool/images/base.qcow2".to_string()),
                        source_format: Some(DiskFormat::Qcow2),
                        source_checksum: Some(
                            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                                .to_string(),
                        ),
                        size_gib: 80,
                        format: DiskFormat::Qcow2,
                    },
                    k3s: NodeK3sConfig {
                        cluster_init: true,
                        labels: BTreeMap::from([(
                            "nas-csi.dev/storage-node".to_string(),
                            "true".to_string(),
                        )]),
                        taints: Vec::new(),
                        server: None,
                    },
                    exports: Vec::new(),
                },
                NodeConfig {
                    name: "agent-1".to_string(),
                    domain: "nascsi-agent-1".to_string(),
                    role: NodeRole::Agent,
                    autostart: true,
                    vcpus: 2,
                    memory_mib: 4096,
                    machine: "q35".to_string(),
                    firmware: "efi".to_string(),
                    cpu: "host-passthrough".to_string(),
                    network: NodeNetworkConfig {
                        bridge: "br0".to_string(),
                        mac: "52:54:00:00:00:02".to_string(),
                    },
                    root_disk: RootDiskConfig {
                        image: "/pool/vms/agent.qcow2".to_string(),
                        source_image: Some("/pool/images/base.qcow2".to_string()),
                        source_format: Some(DiskFormat::Qcow2),
                        source_checksum: Some(
                            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                                .to_string(),
                        ),
                        size_gib: 80,
                        format: DiskFormat::Qcow2,
                    },
                    k3s: NodeK3sConfig {
                        cluster_init: false,
                        labels: BTreeMap::from([(
                            "nas-csi.dev/storage-node".to_string(),
                            "true".to_string(),
                        )]),
                        taints: Vec::new(),
                        server: Some("https://nas-csi-api.example.test:6443".to_string()),
                    },
                    exports: Vec::new(),
                },
            ],
            exports: BTreeMap::new(),
        }
    }
}
