//! k3s cluster lifecycle planning and config rendering.

use nas_csi_types::{ClusterProfile, HostConfigDraft};
use serde::Serialize;

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
}
