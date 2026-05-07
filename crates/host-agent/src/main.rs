use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use nas_csi_types::{
    ClusterIntent, DiscoveryInventory, HostConfig, HostConfigDraft, HostSelections,
};
use std::env;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;

#[derive(Debug, Parser)]
#[command(name = "nas-csi-host-agent")]
#[command(about = "TrueNAS-side control agent for nas-csi")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Validate a repo-safe cluster intent file.
    ValidateIntent {
        #[arg(long)]
        intent: PathBuf,
    },
    /// Run read-only local discovery.
    Discover {
        #[arg(long)]
        output: Option<PathBuf>,
    },
    /// Generate a local HostConfigDraft from intent and discovery.
    Init {
        #[arg(long)]
        intent: PathBuf,
        #[arg(long)]
        discovery: PathBuf,
        #[arg(long)]
        output: Option<PathBuf>,
    },
    /// Show the non-mutating plan from a generated HostConfigDraft.
    Plan {
        #[arg(long)]
        config: PathBuf,
    },
    /// Materialize concrete local HostConfig from intent, discovery, and selections.
    Materialize {
        #[arg(long)]
        intent: PathBuf,
        #[arg(long)]
        discovery: PathBuf,
        #[arg(long)]
        selections: PathBuf,
        #[arg(long)]
        output: Option<PathBuf>,
    },
    /// Render local HostConfig artifacts without applying them.
    Render {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        output_dir: PathBuf,
    },
    /// Plan or execute host-side VM, systemd, and seed-image reconciliation.
    Apply {
        #[arg(long)]
        config: PathBuf,
        #[arg(long, default_value = ".nas-csi/rendered")]
        artifact_dir: PathBuf,
        #[arg(long, default_value = "/etc/systemd/system")]
        systemd_unit_dir: PathBuf,
        #[arg(long)]
        execute: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::ValidateIntent { intent } => {
            let intent = load_yaml::<ClusterIntent>(&intent)?;
            report_validation("intent", intent.validate())
        }
        Command::Discover { output } => {
            let inventory = nas_csi_discovery::discover_local();
            report_validation("discovery", inventory.validate())?;
            write_or_print_yaml(&inventory, output.as_deref())
        }
        Command::Init {
            intent,
            discovery,
            output,
        } => {
            let intent = load_yaml::<ClusterIntent>(&intent)?;
            report_validation("intent", intent.validate())?;

            let discovery = load_yaml::<DiscoveryInventory>(&discovery)?;
            report_validation("discovery", discovery.validate())?;

            let draft = HostConfigDraft::from_intent_and_discovery(intent, &discovery);
            report_validation("host config draft", draft.validate())?;
            write_or_print_yaml(&draft, output.as_deref())
        }
        Command::Plan { config } => {
            let kind = load_yaml::<KindOnly>(&config)?.kind;
            match kind.as_str() {
                "HostConfigDraft" => {
                    let draft = load_yaml::<HostConfigDraft>(&config)?;
                    report_validation("host config draft", draft.validate())?;
                    print_draft_plan(&draft);
                }
                "HostConfig" => {
                    let config = load_yaml::<HostConfig>(&config)?;
                    report_validation("host config", config.validate())?;
                    print_host_plan(&config)?;
                }
                _ => anyhow::bail!("unsupported config kind {kind}"),
            }
            Ok(())
        }
        Command::Materialize {
            intent,
            discovery,
            selections,
            output,
        } => {
            let intent = load_yaml::<ClusterIntent>(&intent)?;
            let discovery = load_yaml::<DiscoveryInventory>(&discovery)?;
            let selections = load_yaml::<HostSelections>(&selections)?;
            let config =
                HostConfig::from_intent_discovery_selections(&intent, &discovery, &selections)
                    .map_err(|errors| validation_error("host config", errors))?;
            write_or_print_yaml(&config, output.as_deref())
        }
        Command::Render { config, output_dir } => {
            let config = load_yaml::<HostConfig>(&config)?;
            report_validation("host config", config.validate())?;
            let artifacts = nas_csi_vm_manager::render_host_artifacts(
                &config,
                &render_options_from_config(&config),
            )?;
            write_artifacts(&output_dir, &artifacts.files)?;
            println!(
                "rendered {} file(s) under {}",
                artifacts.files.len(),
                output_dir.display()
            );
            Ok(())
        }
        Command::Apply {
            config,
            artifact_dir,
            systemd_unit_dir,
            execute,
        } => {
            let config = load_yaml::<HostConfig>(&config)?;
            report_validation("host config", config.validate())?;
            let render_options = render_options_from_config(&config);
            let apply_options =
                apply_options_from_config(&config, &artifact_dir, &systemd_unit_dir);
            let plan =
                nas_csi_vm_manager::plan_host_apply(&config, &render_options, &apply_options)?;
            let actual = inspect_actual_state(&config, &plan, &apply_options)?;
            let reconcile_plan = nas_csi_vm_manager::plan_host_reconcile(
                &config,
                &render_options,
                &apply_options,
                &actual,
            )?;
            if execute {
                execute_reconcile_plan(&reconcile_plan)
            } else {
                print_reconcile_plan(&reconcile_plan);
                Ok(())
            }
        }
    }
}

#[derive(serde::Deserialize)]
struct KindOnly {
    kind: String,
}

fn load_yaml<T>(path: &Path) -> Result<T>
where
    T: serde::de::DeserializeOwned,
{
    let content =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    serde_yml::from_str(&content).with_context(|| format!("failed to parse {}", path.display()))
}

fn write_or_print_yaml<T>(value: &T, output: Option<&Path>) -> Result<()>
where
    T: serde::Serialize,
{
    let yaml = serde_yml::to_string(value).context("failed to serialize yaml")?;
    if let Some(path) = output {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        fs::write(path, yaml).with_context(|| format!("failed to write {}", path.display()))?;
    } else {
        print!("{yaml}");
    }
    Ok(())
}

fn write_artifacts(output_dir: &Path, files: &[nas_csi_vm_manager::RenderedFile]) -> Result<()> {
    for file in files {
        let path = output_dir.join(&file.relative_path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        fs::write(&path, &file.contents)
            .with_context(|| format!("failed to write {}", path.display()))?;
    }
    Ok(())
}

fn report_validation(label: &str, errors: Vec<String>) -> Result<()> {
    if errors.is_empty() {
        return Ok(());
    }

    Err(validation_error(label, errors))
}

fn validation_error(label: &str, errors: Vec<String>) -> anyhow::Error {
    for error in &errors {
        eprintln!("{label}: {error}");
    }
    anyhow::anyhow!("{label} validation failed with {} error(s)", errors.len())
}

fn print_draft_plan(draft: &HostConfigDraft) {
    let cluster_plan = nas_csi_cluster_manager::ClusterPlan::from_draft(draft);

    println!("profile: {}", draft.profile);
    println!(
        "nodes: {} server(s), {} agent(s)",
        draft.nodes.servers, draft.nodes.agents
    );

    println!();
    println!("planned actions:");
    for action in &draft.planned_actions {
        println!("- {}", action.description);
    }

    println!();
    println!("maintenance notes:");
    for note in &cluster_plan.notes {
        println!("- {note}");
    }

    println!();
    println!("required selections:");
    for selection in &draft.required_selections {
        println!("- {}: {}", selection.id, selection.description);
        if !selection.candidates.is_empty() {
            println!("  candidates: {}", selection.candidates.join(", "));
        }
    }

    if !draft.discovery_warnings.is_empty() {
        println!();
        println!("discovery warnings:");
        for warning in &draft.discovery_warnings {
            println!("- {warning}");
        }
    }
}

fn print_host_plan(config: &HostConfig) -> Result<()> {
    println!("cluster: {}", config.cluster.name);
    println!("profile: {}", config.cluster.profile);
    println!("distribution: {:?}", config.cluster.distribution);
    println!("api endpoint: {}", config.cluster.api_server.endpoint);

    println!();
    println!("nodes:");
    for node in &config.nodes {
        println!(
            "- {}: role={:?} domain={} vcpus={} memoryMiB={} exports={}",
            node.name,
            node.role,
            node.domain,
            node.vcpus,
            node.memory_mib,
            node.exports.join(",")
        );
    }

    println!();
    println!("exports:");
    for (id, export) in &config.exports {
        println!(
            "- {id}: dataset={} source={} access={}",
            export.dataset, export.source_path, export.access
        );
    }

    let artifacts =
        nas_csi_vm_manager::render_host_artifacts(config, &render_options_from_config(config))?;
    println!();
    println!("renderable artifacts: {}", artifacts.files.len());
    Ok(())
}

fn render_options_from_config(config: &HostConfig) -> nas_csi_vm_manager::ArtifactRenderOptions {
    nas_csi_vm_manager::ArtifactRenderOptions {
        virtiofsd_path: config.host_tools.virtiofsd.clone(),
        ..nas_csi_vm_manager::ArtifactRenderOptions::default()
    }
}

fn apply_options_from_config(
    config: &HostConfig,
    artifact_dir: &Path,
    systemd_unit_dir: &Path,
) -> nas_csi_vm_manager::HostApplyPlanOptions {
    nas_csi_vm_manager::HostApplyPlanOptions {
        artifact_dir: artifact_dir.display().to_string(),
        systemd_unit_dir: systemd_unit_dir.display().to_string(),
        qemu_img_path: config.host_tools.qemu_img.clone(),
        virsh_path: config.host_tools.virsh.clone(),
        systemctl_path: config.host_tools.systemctl.clone(),
        ..nas_csi_vm_manager::HostApplyPlanOptions::default()
    }
}

fn print_reconcile_plan(plan: &nas_csi_vm_manager::HostReconcilePlan) {
    println!("dry-run reconcile: {} step(s)", plan.steps.len());
    for (index, step) in plan.steps.iter().enumerate() {
        println!("{}. {}", index + 1, step.description);
        match &step.kind {
            nas_csi_vm_manager::ReconcileStepKind::Apply(kind) => {
                println!("   apply");
                print_apply_kind(kind);
            }
            nas_csi_vm_manager::ReconcileStepKind::Skip { reason } => {
                println!("   skip: {reason}");
            }
            nas_csi_vm_manager::ReconcileStepKind::Refuse { reason } => {
                println!("   refuse: {reason}");
            }
        }
    }
}

fn print_apply_kind(kind: &nas_csi_vm_manager::ApplyStepKind) {
    match kind {
        nas_csi_vm_manager::ApplyStepKind::EnsureDirectory { path } => {
            println!("   mkdir -p {}", shell_quote(path));
        }
        nas_csi_vm_manager::ApplyStepKind::WriteFile { path, contents } => {
            println!("   write {} ({} bytes)", shell_quote(path), contents.len());
        }
        nas_csi_vm_manager::ApplyStepKind::WriteBinaryFile { path, contents } => {
            println!(
                "   write binary {} ({} bytes)",
                shell_quote(path),
                contents.len()
            );
        }
        nas_csi_vm_manager::ApplyStepKind::RemoveFile { path } => {
            println!("   rm -f {}", shell_quote(path));
        }
        nas_csi_vm_manager::ApplyStepKind::Command { command, creates } => {
            if let Some(path) = creates {
                println!("   creates: {}", shell_quote(path));
            }
            println!("   command: {command}");
        }
    }
}

fn execute_reconcile_plan(plan: &nas_csi_vm_manager::HostReconcilePlan) -> Result<()> {
    if plan.has_refusals() {
        for step in &plan.steps {
            if let nas_csi_vm_manager::ReconcileStepKind::Refuse { reason } = &step.kind {
                eprintln!("refuse {}: {reason}", step.description);
            }
        }
        anyhow::bail!("reconcile plan contains refusal(s); no changes executed");
    }

    for step in &plan.steps {
        match &step.kind {
            nas_csi_vm_manager::ReconcileStepKind::Apply(kind) => {
                println!("{}", step.description);
                execute_apply_kind(kind)?;
            }
            nas_csi_vm_manager::ReconcileStepKind::Skip { reason } => {
                println!("skip {}: {reason}", step.description);
            }
            nas_csi_vm_manager::ReconcileStepKind::Refuse { .. } => unreachable!(),
        }
    }
    Ok(())
}

fn execute_apply_kind(kind: &nas_csi_vm_manager::ApplyStepKind) -> Result<()> {
    match kind {
        nas_csi_vm_manager::ApplyStepKind::EnsureDirectory { path } => {
            fs::create_dir_all(path)
                .with_context(|| format!("failed to create directory {path}"))?;
        }
        nas_csi_vm_manager::ApplyStepKind::WriteFile { path, contents } => {
            write_file_if_changed(path, contents)?;
        }
        nas_csi_vm_manager::ApplyStepKind::WriteBinaryFile { path, contents } => {
            write_binary_file_if_changed(path, contents)?;
        }
        nas_csi_vm_manager::ApplyStepKind::RemoveFile { path } => {
            if Path::new(path).exists() {
                fs::remove_file(path).with_context(|| format!("failed to remove {path}"))?;
            }
        }
        nas_csi_vm_manager::ApplyStepKind::Command { command, creates } => {
            if let Some(path) = creates
                && Path::new(path).exists()
            {
                println!("skip command, {} already exists", path);
                return Ok(());
            }
            run_command(command)?;
        }
    }
    Ok(())
}

fn inspect_actual_state(
    config: &HostConfig,
    desired: &nas_csi_vm_manager::HostApplyPlan,
    apply_options: &nas_csi_vm_manager::HostApplyPlanOptions,
) -> Result<nas_csi_vm_manager::HostActualState> {
    let mut actual = nas_csi_vm_manager::HostActualState::default();
    let mut systemd_unit_paths = std::collections::BTreeMap::new();
    let mut command_programs = std::collections::BTreeSet::new();

    for step in &desired.steps {
        match &step.kind {
            nas_csi_vm_manager::ApplyStepKind::EnsureDirectory { path }
            | nas_csi_vm_manager::ApplyStepKind::WriteFile { path, .. }
            | nas_csi_vm_manager::ApplyStepKind::WriteBinaryFile { path, .. }
            | nas_csi_vm_manager::ApplyStepKind::RemoveFile { path } => {
                actual.paths.insert(path.clone(), inspect_path(path)?);
                if let Some(unit_name) = Path::new(path).file_name().and_then(|name| name.to_str())
                    && unit_name.ends_with(".service")
                {
                    systemd_unit_paths.insert(unit_name.to_string(), path.clone());
                }
            }
            nas_csi_vm_manager::ApplyStepKind::Command { command, creates } => {
                command_programs.insert(command.program.clone());
                if let Some(path) = creates {
                    actual.paths.insert(path.clone(), inspect_path(path)?);
                }
            }
        }
    }

    for program in command_programs {
        actual.tools.insert(program.clone(), inspect_tool(&program));
    }

    for step in &desired.steps {
        if let nas_csi_vm_manager::ApplyStepKind::Command {
            command,
            creates: Some(path),
        } = &step.kind
            && command.args.first().map(String::as_str) == Some("create")
            && actual
                .paths
                .get(path)
                .map(nas_csi_vm_manager::PathActualState::exists)
                .unwrap_or(false)
            && actual
                .tools
                .get(&command.program)
                .map(nas_csi_vm_manager::ToolActualState::is_found)
                .unwrap_or(false)
            && let Some(image) = inspect_qemu_image(&command.program, path)
        {
            actual.qemu_images.insert(path.clone(), image);
        }
    }

    let systemctl_available = actual
        .tools
        .get(&apply_options.systemctl_path)
        .map(nas_csi_vm_manager::ToolActualState::is_found)
        .unwrap_or(false);
    for (unit_name, path) in systemd_unit_paths {
        let installed_hash = actual
            .paths
            .get(&path)
            .and_then(|state| state.content_hash.clone());
        actual.systemd_units.insert(
            unit_name.clone(),
            nas_csi_vm_manager::SystemdUnitActualState {
                installed_hash,
                enabled: systemctl_available.then(|| {
                    command_success(
                        &apply_options.systemctl_path,
                        ["is-enabled", "--quiet", unit_name.as_str()],
                    )
                }),
                active: systemctl_available.then(|| {
                    command_success(
                        &apply_options.systemctl_path,
                        ["is-active", "--quiet", unit_name.as_str()],
                    )
                }),
            },
        );
    }

    let virsh_available = actual
        .tools
        .get(&apply_options.virsh_path)
        .map(nas_csi_vm_manager::ToolActualState::is_found)
        .unwrap_or(false);
    if virsh_available {
        for node in &config.nodes {
            actual.domains.insert(
                node.domain.clone(),
                inspect_domain(&apply_options.virsh_path, &config.libvirt.uri, &node.domain),
            );
        }
    }

    Ok(actual)
}

#[derive(serde::Deserialize)]
struct QemuImgInfo {
    #[serde(default)]
    format: Option<String>,
    #[serde(rename = "backing-filename", default)]
    backing_filename: Option<String>,
    #[serde(rename = "virtual-size", default)]
    virtual_size: Option<u64>,
}

fn inspect_qemu_image(
    program: &str,
    path: &str,
) -> Option<nas_csi_vm_manager::QemuImageActualState> {
    let output = command_output(program, ["info", "--output=json", path])?;
    let info = serde_json::from_str::<QemuImgInfo>(&output).ok()?;
    Some(nas_csi_vm_manager::QemuImageActualState {
        format: info.format,
        backing_file: info.backing_filename,
        virtual_size: info.virtual_size,
    })
}

fn inspect_path(path: &str) -> Result<nas_csi_vm_manager::PathActualState> {
    let path_obj = Path::new(path);
    let Ok(metadata) = fs::metadata(path_obj) else {
        return Ok(nas_csi_vm_manager::PathActualState::missing());
    };
    if metadata.is_dir() {
        return Ok(nas_csi_vm_manager::PathActualState::directory());
    }
    if metadata.is_file() {
        let contents = fs::read(path_obj).with_context(|| format!("failed to read {path}"))?;
        return Ok(nas_csi_vm_manager::PathActualState::file(&contents));
    }
    Ok(nas_csi_vm_manager::PathActualState {
        kind: nas_csi_vm_manager::PathActualKind::Other,
        size: Some(metadata.len()),
        content_hash: None,
    })
}

fn inspect_tool(program: &str) -> nas_csi_vm_manager::ToolActualState {
    let path = Path::new(program);
    if path.components().count() > 1 {
        if path.is_file() {
            return nas_csi_vm_manager::ToolActualState::found(program);
        }
        return nas_csi_vm_manager::ToolActualState::missing();
    }

    let Some(path_var) = env::var_os("PATH") else {
        return nas_csi_vm_manager::ToolActualState::missing();
    };
    for dir in env::split_paths(&path_var) {
        let candidate = dir.join(program);
        if candidate.is_file() {
            return nas_csi_vm_manager::ToolActualState::found(candidate.display().to_string());
        }
    }
    nas_csi_vm_manager::ToolActualState::missing()
}

fn inspect_domain(
    virsh_path: &str,
    uri: &str,
    domain: &str,
) -> nas_csi_vm_manager::DomainActualState {
    let xml = command_output(virsh_path, ["-c", uri, "dumpxml", domain]);
    let exists = xml.is_some();
    if !exists {
        return nas_csi_vm_manager::DomainActualState {
            exists: false,
            active: false,
            autostart: None,
            desired_hash: None,
            xml_hash: None,
        };
    }

    let active = command_output(virsh_path, ["-c", uri, "domstate", domain])
        .map(|state| state.trim() == "running")
        .unwrap_or(false);
    let autostart = command_output(virsh_path, ["-c", uri, "dominfo", domain])
        .and_then(|info| parse_virsh_autostart(&info));

    nas_csi_vm_manager::DomainActualState {
        exists,
        active,
        autostart,
        desired_hash: xml
            .as_deref()
            .and_then(nas_csi_vm_manager::extract_domain_desired_hash),
        xml_hash: xml.map(|xml| nas_csi_vm_manager::content_hash(xml.as_bytes())),
    }
}

fn parse_virsh_autostart(info: &str) -> Option<bool> {
    for line in info.lines() {
        if let Some((key, value)) = line.split_once(':')
            && key.trim() == "Autostart"
        {
            return Some(matches!(value.trim(), "enable" | "enabled" | "yes"));
        }
    }
    None
}

fn command_success<I, S>(program: &str, args: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    ProcessCommand::new(program)
        .args(args)
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn command_output<I, S>(program: &str, args: I) -> Option<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = ProcessCommand::new(program).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let text = stdout.trim();
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

fn write_file_if_changed(path: &str, contents: &str) -> Result<()> {
    let path_obj = Path::new(path);
    if let Some(parent) = path_obj.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    if fs::read_to_string(path_obj).ok().as_deref() == Some(contents) {
        println!("unchanged {path}");
        return Ok(());
    }
    fs::write(path_obj, contents).with_context(|| format!("failed to write {path}"))
}

fn write_binary_file_if_changed(path: &str, contents: &[u8]) -> Result<()> {
    let path_obj = Path::new(path);
    if let Some(parent) = path_obj.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    if fs::read(path_obj).ok().as_deref() == Some(contents) {
        println!("unchanged {path}");
        return Ok(());
    }
    fs::write(path_obj, contents).with_context(|| format!("failed to write {path}"))
}

fn run_command(command: &nas_csi_vm_manager::CommandSpec) -> Result<()> {
    let status = ProcessCommand::new(&command.program)
        .args(&command.args)
        .status()
        .with_context(|| format!("failed to execute {command}"))?;
    if status.success() {
        Ok(())
    } else {
        anyhow::bail!("{command} failed with {status}")
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}
