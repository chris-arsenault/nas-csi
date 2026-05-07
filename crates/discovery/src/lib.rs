use nas_csi_truenas_client::{DatasetRecord, SmbShareRecord};
use nas_csi_types::{
    API_VERSION, BridgeSummary, DatasetSummary, DiscoveryInventory, ExistingProjectState,
    HostFacts, LibvirtFacts, NetworkFacts, PoolSummary, SmbShareSummary, ToolFacts, ToolStatus,
    TrueNasFacts,
};
use std::collections::BTreeSet;
use std::env;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn discover_local() -> DiscoveryInventory {
    let mut warnings = Vec::new();

    let host = HostFacts {
        os_pretty_name: os_pretty_name(),
        architecture: env::consts::ARCH.to_string(),
        cpu_count: std::thread::available_parallelism().ok().map(usize::from),
        memory_total_kib: memory_total_kib(),
    };

    let network = NetworkFacts {
        bridges: discover_bridges(),
        lan_addresses: Vec::new(),
    };
    if network.bridges.is_empty() {
        warnings.push("no Linux bridges discovered under /sys/class/net".to_string());
    }

    let virsh = discover_tool("virsh", &["--version"]);
    let qemu = discover_tool("qemu-system-x86_64", &["--version"]);
    let virtiofsd = discover_tool("virtiofsd", &["--version"]);
    let qemu_img = discover_tool("qemu-img", &["--version"]);
    let systemctl = discover_tool("systemctl", &["--version"]);
    let midclt = discover_tool("midclt", &["--help"]);

    if virsh.path.is_none() {
        warnings.push("virsh was not found in PATH".to_string());
    }
    if qemu.path.is_none() {
        warnings.push("qemu-system-x86_64 was not found in PATH".to_string());
    }
    if virtiofsd.path.is_none() {
        warnings.push("virtiofsd was not found in PATH".to_string());
    }
    if systemctl.path.is_none() {
        warnings.push("systemctl was not found in PATH".to_string());
    }

    let (truenas, truenas_warnings) = discover_truenas(midclt.path.as_deref());
    warnings.extend(truenas_warnings);

    DiscoveryInventory {
        api_version: API_VERSION.to_string(),
        kind: "DiscoveryInventory".to_string(),
        generated_unix_seconds: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or_default(),
        host,
        truenas,
        libvirt: LibvirtFacts {
            uri: "qemu:///system".to_string(),
            virsh,
            qemu,
            default_machine: None,
        },
        network,
        tools: ToolFacts {
            virtiofsd,
            qemu_img,
            systemctl,
            midclt,
        },
        existing_project_state: discover_existing_project_state(),
        warnings,
    }
}

fn os_pretty_name() -> Option<String> {
    let os_release = fs::read_to_string("/etc/os-release").ok()?;
    for line in os_release.lines() {
        if let Some(value) = line.strip_prefix("PRETTY_NAME=") {
            return Some(value.trim_matches('"').to_string());
        }
    }
    None
}

fn memory_total_kib() -> Option<u64> {
    let meminfo = fs::read_to_string("/proc/meminfo").ok()?;
    for line in meminfo.lines() {
        if let Some(rest) = line.strip_prefix("MemTotal:") {
            return rest.split_whitespace().next()?.parse().ok();
        }
    }
    None
}

fn discover_bridges() -> Vec<BridgeSummary> {
    let Ok(entries) = fs::read_dir("/sys/class/net") else {
        return Vec::new();
    };

    let mut bridges = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.join("bridge").is_dir() {
            continue;
        }

        let name = entry.file_name().to_string_lossy().into_owned();
        let interfaces = bridge_interfaces(&path);
        bridges.push(BridgeSummary { name, interfaces });
    }

    bridges.sort_by(|a, b| a.name.cmp(&b.name));
    bridges
}

fn bridge_interfaces(bridge_path: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(bridge_path.join("brif")) else {
        return Vec::new();
    };

    let mut interfaces = entries
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    interfaces.sort();
    interfaces
}

fn discover_tool(command: &str, version_args: &[&str]) -> ToolStatus {
    let path = find_in_path(command).map(|path| path.display().to_string());
    let version = path
        .as_deref()
        .and_then(|tool_path| command_output(tool_path, version_args));

    ToolStatus { path, version }
}

fn find_in_path(command: &str) -> Option<PathBuf> {
    let command_path = Path::new(command);
    if command_path.components().count() > 1 && is_executable(command_path) {
        return Some(command_path.to_path_buf());
    }

    let path_var = env::var_os("PATH")?;
    for dir in env::split_paths(&path_var) {
        let candidate = dir.join(command);
        if is_executable(&candidate) {
            return Some(candidate);
        }
    }
    None
}

fn is_executable(path: &Path) -> bool {
    path.is_file()
}

fn command_output(command: impl AsRef<OsStr>, args: &[&str]) -> Option<String> {
    let output = Command::new(command).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let text = if stdout.trim().is_empty() {
        stderr.trim()
    } else {
        stdout.trim()
    };

    if text.is_empty() {
        None
    } else {
        Some(text.lines().next().unwrap_or(text).to_string())
    }
}

fn discover_truenas(midclt_path: Option<&str>) -> (TrueNasFacts, Vec<String>) {
    let mut facts = TrueNasFacts {
        local_api_url: Some("wss://127.0.0.1/api/current".to_string()),
        version: read_first_existing(&["/etc/version", "/etc/truenas-release"]),
        ..TrueNasFacts::default()
    };
    let mut warnings = Vec::new();

    if let Some(path) = midclt_path {
        match discover_truenas_with_midclt(path, facts.version.clone()) {
            Ok(midclt_facts) => {
                return (midclt_facts, warnings);
            }
            Err(error) => warnings.push(format!(
                "midclt TrueNAS discovery failed; falling back to /mnt scan: {error}"
            )),
        }
    }

    add_mnt_fallback_datasets(&mut facts);
    (facts, warnings)
}

fn discover_truenas_with_midclt(
    midclt_path: &str,
    fallback_version: Option<String>,
) -> Result<TrueNasFacts, String> {
    let version = midclt_call_json::<String>(midclt_path, "system.version")
        .ok()
        .or(fallback_version);
    let datasets = midclt_call_json::<Vec<DatasetRecord>>(midclt_path, "pool.dataset.query")?;
    let smb_shares = midclt_call_json::<Vec<SmbShareRecord>>(midclt_path, "sharing.smb.query")
        .unwrap_or_default();

    Ok(truenas_facts_from_records(version, &datasets, &smb_shares))
}

fn midclt_call_json<T>(midclt_path: &str, method: &str) -> Result<T, String>
where
    T: serde::de::DeserializeOwned,
{
    let output = Command::new(midclt_path)
        .args(["call", method])
        .output()
        .map_err(|error| format!("failed to execute midclt: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "midclt call {method} exited with {}: {}",
            output.status,
            stderr.trim()
        ));
    }

    serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("failed to parse midclt call {method} JSON: {error}"))
}

pub fn truenas_facts_from_midclt_json(
    version_json: Option<&str>,
    datasets_json: &str,
    smb_shares_json: &str,
) -> Result<TrueNasFacts, String> {
    let version = version_json
        .map(|text| {
            serde_json::from_str::<String>(text)
                .map_err(|error| format!("failed to parse system.version JSON: {error}"))
        })
        .transpose()?;
    let datasets = serde_json::from_str::<Vec<DatasetRecord>>(datasets_json)
        .map_err(|error| format!("failed to parse pool.dataset.query JSON: {error}"))?;
    let smb_shares = serde_json::from_str::<Vec<SmbShareRecord>>(smb_shares_json)
        .map_err(|error| format!("failed to parse sharing.smb.query JSON: {error}"))?;

    Ok(truenas_facts_from_records(version, &datasets, &smb_shares))
}

pub fn truenas_facts_from_records(
    version: Option<String>,
    datasets: &[DatasetRecord],
    smb_shares: &[SmbShareRecord],
) -> TrueNasFacts {
    let mut pool_names = BTreeSet::new();
    let mut filesystem_datasets = Vec::new();

    for dataset in datasets {
        if dataset
            .kind_value()
            .is_some_and(|kind| kind != "FILESYSTEM")
        {
            continue;
        }

        if let Some(pool_name) = dataset.name.split('/').next()
            && !pool_name.is_empty()
        {
            pool_names.insert(pool_name.to_string());
        }

        filesystem_datasets.push(DatasetSummary {
            name: dataset.name.clone(),
            mountpoint: dataset.mountpoint_value().map(str::to_string),
        });
    }

    let smb_shares = smb_shares
        .iter()
        .filter(|share| share.enabled.unwrap_or(true))
        .map(|share| SmbShareSummary {
            name: share.name.clone(),
            path: share.path.clone(),
        })
        .collect();

    TrueNasFacts {
        local_api_url: Some("wss://127.0.0.1/api/current".to_string()),
        version,
        pools: pool_names
            .into_iter()
            .map(|name| PoolSummary { name })
            .collect(),
        filesystem_datasets,
        smb_shares,
    }
}

fn add_mnt_fallback_datasets(facts: &mut TrueNasFacts) {
    let Ok(entries) = fs::read_dir("/mnt") else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.is_empty() {
            continue;
        }

        if !facts.pools.iter().any(|pool| pool.name == name) {
            facts.pools.push(PoolSummary { name: name.clone() });
        }

        let dataset_name = name.clone();
        if !facts
            .filesystem_datasets
            .iter()
            .any(|dataset| dataset.name == dataset_name)
        {
            facts.filesystem_datasets.push(DatasetSummary {
                name: dataset_name,
                mountpoint: Some(path.display().to_string()),
            });
        }
    }
}

fn read_first_existing(paths: &[&str]) -> Option<String> {
    for path in paths {
        if let Ok(value) = fs::read_to_string(path) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

fn discover_existing_project_state() -> ExistingProjectState {
    let candidate_paths = ["/etc/nas-csi/host.yaml", "/etc/nas-csi/discovery.yaml"];
    let config_paths = candidate_paths
        .iter()
        .filter(|path| Path::new(path).exists())
        .map(|path| (*path).to_string())
        .collect();

    ExistingProjectState {
        config_paths,
        libvirt_domains: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_inventory_has_expected_kind() {
        let inventory = discover_local();
        assert_eq!(inventory.api_version, API_VERSION);
        assert_eq!(inventory.kind, "DiscoveryInventory");
        assert!(!inventory.host.architecture.is_empty());
    }

    #[test]
    fn builds_truenas_facts_from_api_records() {
        let datasets = vec![
            DatasetRecord {
                id: "pool/repos".to_string(),
                name: "pool/repos".to_string(),
                mountpoint: Some(nas_csi_truenas_client::ValueField {
                    value: "/mnt/pool/repos".to_string(),
                }),
                kind: Some(nas_csi_truenas_client::ValueField {
                    value: "FILESYSTEM".to_string(),
                }),
            },
            DatasetRecord {
                id: "pool/block".to_string(),
                name: "pool/block".to_string(),
                mountpoint: None,
                kind: Some(nas_csi_truenas_client::ValueField {
                    value: "VOLUME".to_string(),
                }),
            },
        ];
        let shares = vec![SmbShareRecord {
            id: 1,
            name: "repos".to_string(),
            path: "/mnt/pool/repos".to_string(),
            enabled: Some(true),
        }];

        let facts =
            truenas_facts_from_records(Some("test-version".to_string()), &datasets, &shares);

        assert_eq!(facts.pools[0].name, "pool");
        assert_eq!(facts.filesystem_datasets.len(), 1);
        assert_eq!(facts.filesystem_datasets[0].name, "pool/repos");
        assert_eq!(facts.smb_shares[0].path, "/mnt/pool/repos");
    }

    #[test]
    fn builds_truenas_facts_from_midclt_json() {
        let facts = truenas_facts_from_midclt_json(
            Some(r#""TrueNAS-SCALE-test""#),
            r#"[{"id":"tank/repos","name":"tank/repos","type":{"value":"FILESYSTEM"},"mountpoint":{"value":"/mnt/tank/repos"}},{"id":"tank/zvol","name":"tank/zvol","type":{"value":"VOLUME"}}]"#,
            r#"[{"id":1,"name":"repos","path":"/mnt/tank/repos","enabled":true},{"id":2,"name":"old","path":"/mnt/tank/old","enabled":false}]"#,
        )
        .expect("parse midclt json");

        assert_eq!(facts.version.as_deref(), Some("TrueNAS-SCALE-test"));
        assert_eq!(facts.pools[0].name, "tank");
        assert_eq!(facts.filesystem_datasets.len(), 1);
        assert_eq!(facts.smb_shares.len(), 1);
        assert_eq!(facts.smb_shares[0].name, "repos");
    }
}
