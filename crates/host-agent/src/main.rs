use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use nas_csi_types::{
    ClusterIntent, DiscoveryInventory, HostConfig, HostConfigDraft, HostSelections,
};
use std::collections::BTreeSet;
use std::env;
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::process;
use std::process::Command as ProcessCommand;
use std::time::{SystemTime, UNIX_EPOCH};

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
        allow_running_domain_redefine: bool,
        #[arg(long)]
        execute: bool,
    },
    /// Report actual host-side state without rendering an apply plan.
    Status {
        #[arg(long)]
        config: PathBuf,
        #[arg(long, default_value = ".nas-csi/rendered")]
        artifact_dir: PathBuf,
        #[arg(long, default_value = "/etc/systemd/system")]
        systemd_unit_dir: PathBuf,
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
            allow_running_domain_redefine,
            execute,
        } => {
            let config = load_yaml::<HostConfig>(&config)?;
            report_validation("host config", config.validate())?;
            let runner = RealCommandRunner;
            let render_options = render_options_from_config(&config);
            let apply_options = apply_options_from_config(
                &config,
                &artifact_dir,
                &systemd_unit_dir,
                allow_running_domain_redefine,
            );
            let plan =
                nas_csi_vm_manager::plan_host_apply(&config, &render_options, &apply_options)?;
            let actual = inspect_actual_state(&config, &plan, &apply_options, &runner)?;
            let reconcile_plan = nas_csi_vm_manager::plan_host_reconcile(
                &config,
                &render_options,
                &apply_options,
                &actual,
            )?;
            if execute {
                let _apply_lock = ApplyLock::acquire(&apply_lock_path(&artifact_dir))?;
                let safety = ExecuteSafety::from_config(&config, &apply_options)?;
                execute_reconcile_plan(&reconcile_plan, &runner, &safety)
            } else {
                print_reconcile_plan(&reconcile_plan);
                Ok(())
            }
        }
        Command::Status {
            config,
            artifact_dir,
            systemd_unit_dir,
        } => {
            let config = load_yaml::<HostConfig>(&config)?;
            report_validation("host config", config.validate())?;
            let runner = RealCommandRunner;
            let apply_options =
                apply_options_from_config(&config, &artifact_dir, &systemd_unit_dir, false);
            let actual = inspect_status_state(&config, &apply_options, &runner)?;
            print_host_status(&config, &apply_options, &actual);
            Ok(())
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
        write_text_atomic_if_changed(path.display().to_string().as_str(), &file.contents)?;
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
    allow_running_domain_redefine: bool,
) -> nas_csi_vm_manager::HostApplyPlanOptions {
    nas_csi_vm_manager::HostApplyPlanOptions {
        artifact_dir: artifact_dir.display().to_string(),
        systemd_unit_dir: systemd_unit_dir.display().to_string(),
        qemu_img_path: config.host_tools.qemu_img.clone(),
        virsh_path: config.host_tools.virsh.clone(),
        systemctl_path: config.host_tools.systemctl.clone(),
        allow_running_domain_redefine,
        ..nas_csi_vm_manager::HostApplyPlanOptions::default()
    }
}

fn print_reconcile_plan(plan: &nas_csi_vm_manager::HostReconcilePlan) {
    let summary = summarize_reconcile_plan(plan);
    println!(
        "dry-run reconcile summary: apply={} skip={} refuse={} risky={}",
        summary.apply, summary.skip, summary.refuse, summary.risky
    );
    println!("steps: {}", plan.steps.len());
    println!();
    println!("details:");
    for (index, step) in plan.steps.iter().enumerate() {
        println!("{}. {}", index + 1, step.description);
        match &step.kind {
            nas_csi_vm_manager::ReconcileStepKind::Apply(operation) => {
                println!("   apply");
                print_reconcile_operation(operation);
            }
            nas_csi_vm_manager::ReconcileStepKind::SkipAlreadyCorrect { reason } => {
                println!("   skip: {reason}");
            }
            nas_csi_vm_manager::ReconcileStepKind::Refuse { operation, reason } => {
                println!("   refuse: {reason}");
                if let Some(operation) = operation {
                    print_reconcile_operation(operation);
                }
            }
        }
    }
}

#[derive(Default, Debug, PartialEq, Eq)]
struct ReconcileSummary {
    apply: usize,
    skip: usize,
    refuse: usize,
    risky: usize,
}

fn summarize_reconcile_plan(plan: &nas_csi_vm_manager::HostReconcilePlan) -> ReconcileSummary {
    let mut summary = ReconcileSummary::default();
    for step in &plan.steps {
        match &step.kind {
            nas_csi_vm_manager::ReconcileStepKind::Apply(operation) => {
                summary.apply += 1;
                if is_risky_operation(operation) {
                    summary.risky += 1;
                }
            }
            nas_csi_vm_manager::ReconcileStepKind::SkipAlreadyCorrect { .. } => {
                summary.skip += 1;
            }
            nas_csi_vm_manager::ReconcileStepKind::Refuse { operation, .. } => {
                summary.refuse += 1;
                if operation.as_ref().is_some_and(is_risky_operation) {
                    summary.risky += 1;
                }
            }
        }
    }
    summary
}

fn is_risky_operation(operation: &nas_csi_vm_manager::ReconcileOperation) -> bool {
    matches!(
        operation,
        nas_csi_vm_manager::ReconcileOperation::CreateRootDisk { .. }
            | nas_csi_vm_manager::ReconcileOperation::RewriteSeedImage { .. }
            | nas_csi_vm_manager::ReconcileOperation::RestartVirtiofsdService { .. }
            | nas_csi_vm_manager::ReconcileOperation::DefineDomain { .. }
            | nas_csi_vm_manager::ReconcileOperation::RedefineDomain { .. }
            | nas_csi_vm_manager::ReconcileOperation::RedefineDomainRequiresShutdown { .. }
            | nas_csi_vm_manager::ReconcileOperation::StartDomain { .. }
            | nas_csi_vm_manager::ReconcileOperation::RunCommand { .. }
    )
}

fn print_reconcile_operation(operation: &nas_csi_vm_manager::ReconcileOperation) {
    match operation {
        nas_csi_vm_manager::ReconcileOperation::EnsureDirectory { path } => {
            println!("   mkdir -p {}", shell_quote(path));
        }
        nas_csi_vm_manager::ReconcileOperation::WriteRenderedArtifact { path, contents } => {
            println!("   write {} ({} bytes)", shell_quote(path), contents.len());
        }
        nas_csi_vm_manager::ReconcileOperation::RewriteSeedImage { path, contents } => {
            println!(
                "   rewrite seed image {} ({} bytes)",
                shell_quote(path),
                contents.len()
            );
        }
        nas_csi_vm_manager::ReconcileOperation::InstallOrUpdateSystemdUnit {
            unit_name,
            path,
            contents,
        } => {
            println!(
                "   install systemd unit {} at {} ({} bytes)",
                unit_name,
                shell_quote(path),
                contents.len()
            );
        }
        nas_csi_vm_manager::ReconcileOperation::CreateRootDisk { path, command } => {
            println!("   creates: {}", shell_quote(path));
            println!("   command: {command}");
        }
        nas_csi_vm_manager::ReconcileOperation::ReloadSystemdUnits { command }
        | nas_csi_vm_manager::ReconcileOperation::EnableAndStartVirtiofsdService {
            command, ..
        }
        | nas_csi_vm_manager::ReconcileOperation::RestartVirtiofsdService { command, .. }
        | nas_csi_vm_manager::ReconcileOperation::DefineDomain { command, .. }
        | nas_csi_vm_manager::ReconcileOperation::RedefineDomain { command, .. }
        | nas_csi_vm_manager::ReconcileOperation::EnableDomainAutostart { command, .. }
        | nas_csi_vm_manager::ReconcileOperation::StartDomain { command, .. } => {
            println!("   command: {command}");
        }
        nas_csi_vm_manager::ReconcileOperation::RedefineDomainRequiresShutdown {
            domain,
            xml_path,
        } => {
            println!(
                "   pending domain redefine: domain={} xml={}",
                domain,
                shell_quote(xml_path)
            );
        }
        nas_csi_vm_manager::ReconcileOperation::RunCommand { command, creates } => {
            if let Some(path) = creates {
                println!("   creates: {}", shell_quote(path));
            }
            println!("   command: {command}");
        }
    }
}

trait CommandRunner {
    fn status(&self, command: &nas_csi_vm_manager::CommandSpec) -> Result<bool>;
    fn output(&self, command: &nas_csi_vm_manager::CommandSpec) -> Result<Option<String>>;

    fn run(&self, command: &nas_csi_vm_manager::CommandSpec) -> Result<()> {
        if self.status(command)? {
            Ok(())
        } else {
            anyhow::bail!("{command} failed")
        }
    }
}

struct RealCommandRunner;

impl CommandRunner for RealCommandRunner {
    fn status(&self, command: &nas_csi_vm_manager::CommandSpec) -> Result<bool> {
        let status = ProcessCommand::new(&command.program)
            .args(&command.args)
            .status()
            .with_context(|| format!("failed to execute {command}"))?;
        Ok(status.success())
    }

    fn output(&self, command: &nas_csi_vm_manager::CommandSpec) -> Result<Option<String>> {
        let output = ProcessCommand::new(&command.program)
            .args(&command.args)
            .output()
            .with_context(|| format!("failed to execute {command}"))?;
        if !output.status.success() {
            return Ok(None);
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let text = stdout.trim();
        if text.is_empty() {
            Ok(None)
        } else {
            Ok(Some(text.to_string()))
        }
    }
}

fn execute_reconcile_plan(
    plan: &nas_csi_vm_manager::HostReconcilePlan,
    runner: &impl CommandRunner,
    safety: &ExecuteSafety,
) -> Result<()> {
    if plan.has_refusals() {
        for step in &plan.steps {
            if let nas_csi_vm_manager::ReconcileStepKind::Refuse { reason, .. } = &step.kind {
                eprintln!("refuse {}: {reason}", step.description);
            }
        }
        anyhow::bail!("reconcile plan contains refusal(s); no changes executed");
    }

    let mut rollback = ExecutionRollback::default();
    for step in &plan.steps {
        match &step.kind {
            nas_csi_vm_manager::ReconcileStepKind::Apply(operation) => {
                println!("{}", step.description);
                if let Err(error) =
                    execute_reconcile_operation(operation, runner, &mut rollback, safety)
                {
                    rollback.restore(runner)?;
                    return Err(error);
                }
            }
            nas_csi_vm_manager::ReconcileStepKind::SkipAlreadyCorrect { reason } => {
                println!("skip {}: {reason}", step.description);
            }
            nas_csi_vm_manager::ReconcileStepKind::Refuse { .. } => unreachable!(),
        }
    }
    rollback.commit()?;
    Ok(())
}

fn execute_reconcile_operation(
    operation: &nas_csi_vm_manager::ReconcileOperation,
    runner: &impl CommandRunner,
    rollback: &mut ExecutionRollback,
    safety: &ExecuteSafety,
) -> Result<()> {
    safety.validate(operation)?;

    match operation {
        nas_csi_vm_manager::ReconcileOperation::EnsureDirectory { path } => {
            fs::create_dir_all(path)
                .with_context(|| format!("failed to create directory {path}"))?;
        }
        nas_csi_vm_manager::ReconcileOperation::WriteRenderedArtifact { path, contents } => {
            write_text_atomic_if_changed(path, contents)?;
        }
        nas_csi_vm_manager::ReconcileOperation::InstallOrUpdateSystemdUnit {
            path,
            contents,
            ..
        } => {
            rollback.stage_file_change(path, contents.as_bytes())?;
            write_text_atomic_if_changed(path, contents)?;
        }
        nas_csi_vm_manager::ReconcileOperation::RewriteSeedImage { path, contents } => {
            write_binary_atomic_if_changed(path, contents)?;
        }
        nas_csi_vm_manager::ReconcileOperation::CreateRootDisk { command, .. }
        | nas_csi_vm_manager::ReconcileOperation::ReloadSystemdUnits { command }
        | nas_csi_vm_manager::ReconcileOperation::EnableAndStartVirtiofsdService {
            command, ..
        }
        | nas_csi_vm_manager::ReconcileOperation::RestartVirtiofsdService { command, .. }
        | nas_csi_vm_manager::ReconcileOperation::DefineDomain { command, .. }
        | nas_csi_vm_manager::ReconcileOperation::EnableDomainAutostart { command, .. }
        | nas_csi_vm_manager::ReconcileOperation::StartDomain { command, .. } => {
            runner.run(command)?;
        }
        nas_csi_vm_manager::ReconcileOperation::RedefineDomain {
            domain,
            xml_path,
            previous_xml,
            command,
        } => {
            rollback.stage_domain_redefine(domain, xml_path, command, previous_xml.as_deref())?;
            runner.run(command)?;
        }
        nas_csi_vm_manager::ReconcileOperation::RedefineDomainRequiresShutdown { .. } => {
            anyhow::bail!("domain redefine requires shutdown and cannot be executed directly");
        }
        nas_csi_vm_manager::ReconcileOperation::RunCommand { command, creates } => {
            if let Some(path) = creates
                && Path::new(path).exists()
            {
                println!("skip command, {} already exists", path);
                return Ok(());
            }
            runner.run(command)?;
        }
    }
    Ok(())
}

fn inspect_actual_state(
    config: &HostConfig,
    desired: &nas_csi_vm_manager::HostApplyPlan,
    apply_options: &nas_csi_vm_manager::HostApplyPlanOptions,
    runner: &impl CommandRunner,
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
            && let Some(image) = inspect_qemu_image(runner, &command.program, path)?
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
                enabled: if systemctl_available {
                    Some(command_success(
                        runner,
                        &apply_options.systemctl_path,
                        ["is-enabled", "--quiet", unit_name.as_str()],
                    )?)
                } else {
                    None
                },
                active: if systemctl_available {
                    Some(command_success(
                        runner,
                        &apply_options.systemctl_path,
                        ["is-active", "--quiet", unit_name.as_str()],
                    )?)
                } else {
                    None
                },
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
                inspect_domain(
                    runner,
                    &apply_options.virsh_path,
                    &config.libvirt.uri,
                    &node.domain,
                )?,
            );
        }
    }

    Ok(actual)
}

fn inspect_status_state(
    config: &HostConfig,
    apply_options: &nas_csi_vm_manager::HostApplyPlanOptions,
    runner: &impl CommandRunner,
) -> Result<nas_csi_vm_manager::HostActualState> {
    let mut actual = nas_csi_vm_manager::HostActualState::default();

    for program in [
        config.host_tools.virtiofsd.as_str(),
        apply_options.qemu_img_path.as_str(),
        apply_options.virsh_path.as_str(),
        apply_options.systemctl_path.as_str(),
    ] {
        actual
            .tools
            .insert(program.to_string(), inspect_tool(program));
    }

    for node in &config.nodes {
        let root_parent = parent_dir_for_path(&node.root_disk.image);
        actual
            .paths
            .insert(root_parent.clone(), inspect_path(&root_parent)?);
        actual.paths.insert(
            node.root_disk.image.clone(),
            inspect_path(&node.root_disk.image)?,
        );
        let seed_path = nas_csi_vm_manager::seed_image_path(&node.root_disk.image, &node.domain);
        actual
            .paths
            .insert(seed_path.clone(), inspect_path(&seed_path)?);
    }

    if actual
        .tools
        .get(&apply_options.qemu_img_path)
        .map(nas_csi_vm_manager::ToolActualState::is_found)
        .unwrap_or(false)
    {
        for node in &config.nodes {
            if actual
                .paths
                .get(&node.root_disk.image)
                .map(nas_csi_vm_manager::PathActualState::exists)
                .unwrap_or(false)
                && let Some(image) =
                    inspect_qemu_image(runner, &apply_options.qemu_img_path, &node.root_disk.image)?
            {
                actual
                    .qemu_images
                    .insert(node.root_disk.image.clone(), image);
            }
        }
    }

    let systemctl_available = actual
        .tools
        .get(&apply_options.systemctl_path)
        .map(nas_csi_vm_manager::ToolActualState::is_found)
        .unwrap_or(false);
    for node in &config.nodes {
        for export_id in &node.exports {
            let unit_name = format!(
                "{}.service",
                nas_csi_vm_manager::virtiofsd_service_name(&node.domain, export_id)
            );
            let unit_path = Path::new(&apply_options.systemd_unit_dir).join(&unit_name);
            let unit_path_string = unit_path.display().to_string();
            actual
                .paths
                .insert(unit_path_string.clone(), inspect_path(&unit_path_string)?);
            let installed_hash = actual
                .paths
                .get(&unit_path_string)
                .and_then(|state| state.content_hash.clone());
            actual.systemd_units.insert(
                unit_name.clone(),
                nas_csi_vm_manager::SystemdUnitActualState {
                    installed_hash,
                    enabled: if systemctl_available {
                        Some(command_success(
                            runner,
                            &apply_options.systemctl_path,
                            ["is-enabled", "--quiet", unit_name.as_str()],
                        )?)
                    } else {
                        None
                    },
                    active: if systemctl_available {
                        Some(command_success(
                            runner,
                            &apply_options.systemctl_path,
                            ["is-active", "--quiet", unit_name.as_str()],
                        )?)
                    } else {
                        None
                    },
                },
            );
        }
    }

    if actual
        .tools
        .get(&apply_options.virsh_path)
        .map(nas_csi_vm_manager::ToolActualState::is_found)
        .unwrap_or(false)
    {
        for node in &config.nodes {
            actual.domains.insert(
                node.domain.clone(),
                inspect_domain(
                    runner,
                    &apply_options.virsh_path,
                    &config.libvirt.uri,
                    &node.domain,
                )?,
            );
        }
    }

    Ok(actual)
}

fn print_host_status(
    config: &HostConfig,
    apply_options: &nas_csi_vm_manager::HostApplyPlanOptions,
    actual: &nas_csi_vm_manager::HostActualState,
) {
    println!("host status");
    println!();
    println!("tools:");
    for program in [
        config.host_tools.virtiofsd.as_str(),
        apply_options.qemu_img_path.as_str(),
        apply_options.virsh_path.as_str(),
        apply_options.systemctl_path.as_str(),
    ] {
        let state = actual.tools.get(program);
        match state.and_then(|state| state.path.as_deref()) {
            Some(path) => println!("- {program}: found at {path}"),
            None => println!("- {program}: missing"),
        }
    }

    println!();
    println!("nodes:");
    for node in &config.nodes {
        println!("- {} ({})", node.name, node.domain);
        let root_parent = parent_dir_for_path(&node.root_disk.image);
        println!(
            "  root dir: {} ({})",
            root_parent,
            path_status_label(actual.paths.get(&root_parent))
        );
        println!(
            "  root disk: {} ({})",
            node.root_disk.image,
            path_status_label(actual.paths.get(&node.root_disk.image))
        );
        if let Some(image) = actual.qemu_images.get(&node.root_disk.image) {
            println!(
                "  qemu image: format={} backing={} virtualSize={}",
                image.format.as_deref().unwrap_or("unknown"),
                image.backing_file.as_deref().unwrap_or("none"),
                image
                    .virtual_size
                    .map(|size| size.to_string())
                    .unwrap_or_else(|| "unknown".to_string())
            );
        }
        let seed_path = nas_csi_vm_manager::seed_image_path(&node.root_disk.image, &node.domain);
        println!(
            "  seed image: {} ({})",
            seed_path,
            path_status_label(actual.paths.get(&seed_path))
        );
    }

    println!();
    println!("systemd units:");
    let mut unit_count = 0;
    for node in &config.nodes {
        for export_id in &node.exports {
            unit_count += 1;
            let unit_name = format!(
                "{}.service",
                nas_csi_vm_manager::virtiofsd_service_name(&node.domain, export_id)
            );
            let unit = actual.systemd_units.get(&unit_name);
            println!(
                "- {}: installed={} enabled={} active={} hash={}",
                unit_name,
                unit.and_then(|unit| unit.installed_hash.as_ref()).is_some(),
                optional_bool_label(unit.and_then(|unit| unit.enabled)),
                optional_bool_label(unit.and_then(|unit| unit.active)),
                unit.and_then(|unit| unit.installed_hash.as_deref())
                    .unwrap_or("none")
            );
        }
    }
    if unit_count == 0 {
        println!("- none configured");
    }

    println!();
    println!("libvirt domains:");
    if config.nodes.is_empty() {
        println!("- none configured");
    }
    for node in &config.nodes {
        let domain = actual.domains.get(&node.domain);
        println!(
            "- {}: exists={} active={} autostart={} desiredHash={} xmlHash={}",
            node.domain,
            domain.map(|domain| domain.exists).unwrap_or(false),
            domain.map(|domain| domain.active).unwrap_or(false),
            optional_bool_label(domain.and_then(|domain| domain.autostart)),
            domain
                .and_then(|domain| domain.desired_hash.as_deref())
                .unwrap_or("none"),
            domain
                .and_then(|domain| domain.xml_hash.as_deref())
                .unwrap_or("none")
        );
    }
}

fn path_status_label(state: Option<&nas_csi_vm_manager::PathActualState>) -> String {
    let Some(state) = state else {
        return "unknown".to_string();
    };
    match state.kind {
        nas_csi_vm_manager::PathActualKind::File => format!(
            "file size={} hash={}",
            state
                .size
                .map(|size| size.to_string())
                .unwrap_or_else(|| "unknown".to_string()),
            state.content_hash.as_deref().unwrap_or("none")
        ),
        nas_csi_vm_manager::PathActualKind::Directory => "directory".to_string(),
        nas_csi_vm_manager::PathActualKind::Other => format!(
            "other size={}",
            state
                .size
                .map(|size| size.to_string())
                .unwrap_or_else(|| "unknown".to_string())
        ),
        nas_csi_vm_manager::PathActualKind::Missing => "missing".to_string(),
    }
}

fn optional_bool_label(value: Option<bool>) -> &'static str {
    match value {
        Some(true) => "true",
        Some(false) => "false",
        None => "unknown",
    }
}

fn parent_dir_for_path(path: &str) -> String {
    Path::new(path)
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(|parent| parent.display().to_string())
        .unwrap_or_else(|| ".".to_string())
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
    runner: &impl CommandRunner,
    program: &str,
    path: &str,
) -> Result<Option<nas_csi_vm_manager::QemuImageActualState>> {
    let command = nas_csi_vm_manager::CommandSpec::new(
        program.to_string(),
        [
            "info".to_string(),
            "--output=json".to_string(),
            path.to_string(),
        ],
    );
    let Some(output) = runner.output(&command)? else {
        return Ok(None);
    };
    let Ok(info) = serde_json::from_str::<QemuImgInfo>(&output) else {
        return Ok(None);
    };
    Ok(Some(nas_csi_vm_manager::QemuImageActualState {
        format: info.format,
        backing_file: info.backing_filename,
        virtual_size: info.virtual_size,
    }))
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
    runner: &impl CommandRunner,
    virsh_path: &str,
    uri: &str,
    domain: &str,
) -> Result<nas_csi_vm_manager::DomainActualState> {
    let xml = command_output(runner, virsh_path, ["-c", uri, "dumpxml", domain])?;
    let exists = xml.is_some();
    if !exists {
        return Ok(nas_csi_vm_manager::DomainActualState {
            exists: false,
            active: false,
            autostart: None,
            desired_hash: None,
            xml: None,
            xml_hash: None,
        });
    }

    let active = command_output(runner, virsh_path, ["-c", uri, "domstate", domain])?
        .map(|state| state.trim() == "running")
        .unwrap_or(false);
    let autostart = command_output(runner, virsh_path, ["-c", uri, "dominfo", domain])?
        .and_then(|info| parse_virsh_autostart(&info));
    let desired_hash = xml
        .as_deref()
        .and_then(nas_csi_vm_manager::extract_domain_desired_hash);
    let xml_hash = xml
        .as_ref()
        .map(|xml| nas_csi_vm_manager::content_hash(xml.as_bytes()));

    Ok(nas_csi_vm_manager::DomainActualState {
        exists,
        active,
        autostart,
        desired_hash,
        xml,
        xml_hash,
    })
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

fn command_success<I, S>(runner: &impl CommandRunner, program: &str, args: I) -> Result<bool>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    runner.status(&command_spec(program, args))
}

fn command_output<I, S>(
    runner: &impl CommandRunner,
    program: &str,
    args: I,
) -> Result<Option<String>>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    runner.output(&command_spec(program, args))
}

fn command_spec<I, S>(program: &str, args: I) -> nas_csi_vm_manager::CommandSpec
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    nas_csi_vm_manager::CommandSpec::new(
        program.to_string(),
        args.into_iter()
            .map(|arg| arg.as_ref().to_string_lossy().to_string()),
    )
}

struct ApplyLock {
    path: PathBuf,
}

impl ApplyLock {
    fn acquire(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create lock directory {}", parent.display()))?;
        }

        let mut lock_file = match OpenOptions::new().write(true).create_new(true).open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                anyhow::bail!(
                    "another host-agent apply appears to be running; lock file exists at {}",
                    path.display()
                )
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("failed to create apply lock {}", path.display()));
            }
        };
        writeln!(lock_file, "pid={}", process::id())
            .with_context(|| format!("failed to write apply lock {}", path.display()))?;
        lock_file
            .sync_all()
            .with_context(|| format!("failed to fsync apply lock {}", path.display()))?;
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            sync_directory(parent)?;
        }

        Ok(Self {
            path: path.to_path_buf(),
        })
    }
}

impl Drop for ApplyLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn apply_lock_path(artifact_dir: &Path) -> PathBuf {
    let base = artifact_dir
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    base.join("apply.lock")
}

struct ExecuteSafety {
    artifact_dir: PathBuf,
    systemd_unit_dir: PathBuf,
    allowed_systemd_units: BTreeSet<String>,
    allowed_root_dirs: BTreeSet<PathBuf>,
    allowed_root_disks: BTreeSet<PathBuf>,
    allowed_seed_images: BTreeSet<PathBuf>,
    allowed_domains: BTreeSet<String>,
}

impl ExecuteSafety {
    fn from_config(
        config: &HostConfig,
        apply_options: &nas_csi_vm_manager::HostApplyPlanOptions,
    ) -> Result<Self> {
        let artifact_dir = checked_path(&apply_options.artifact_dir, "artifact directory")?;
        let systemd_unit_dir =
            checked_path(&apply_options.systemd_unit_dir, "systemd unit directory")?;
        let mut allowed_systemd_units = BTreeSet::new();
        let mut allowed_root_dirs = BTreeSet::new();
        let mut allowed_root_disks = BTreeSet::new();
        let mut allowed_seed_images = BTreeSet::new();
        let mut allowed_domains = BTreeSet::new();

        for node in &config.nodes {
            allowed_domains.insert(node.domain.clone());
            let root_disk = checked_path(&node.root_disk.image, "root disk image")?;
            allowed_root_disks.insert(root_disk);
            allowed_root_dirs.insert(checked_path(
                &parent_dir_for_path(&node.root_disk.image),
                "root disk directory",
            )?);
            allowed_seed_images.insert(checked_path(
                &nas_csi_vm_manager::seed_image_path(&node.root_disk.image, &node.domain),
                "seed image",
            )?);

            for export_id in &node.exports {
                allowed_systemd_units.insert(format!(
                    "{}.service",
                    nas_csi_vm_manager::virtiofsd_service_name(&node.domain, export_id)
                ));
            }
        }

        Ok(Self {
            artifact_dir,
            systemd_unit_dir,
            allowed_systemd_units,
            allowed_root_dirs,
            allowed_root_disks,
            allowed_seed_images,
            allowed_domains,
        })
    }

    fn validate(&self, operation: &nas_csi_vm_manager::ReconcileOperation) -> Result<()> {
        match operation {
            nas_csi_vm_manager::ReconcileOperation::EnsureDirectory { path } => {
                let path = checked_path(path, "directory")?;
                if self.allowed_root_dirs.contains(&path)
                    || path.starts_with(&self.artifact_dir)
                    || path.starts_with(&self.systemd_unit_dir)
                {
                    Ok(())
                } else {
                    anyhow::bail!(
                        "execute guard refused directory creation outside expected paths: {}",
                        path.display()
                    )
                }
            }
            nas_csi_vm_manager::ReconcileOperation::WriteRenderedArtifact { path, .. } => {
                let path = checked_path(path, "rendered artifact")?;
                self.require_under(&path, &self.artifact_dir, "rendered artifact")
            }
            nas_csi_vm_manager::ReconcileOperation::CreateRootDisk { path, command } => {
                let path = checked_path(path, "root disk")?;
                self.require_exact(&path, &self.allowed_root_disks, "root disk")?;
                require_command_mentions_path(command, path.to_string_lossy().as_ref())
            }
            nas_csi_vm_manager::ReconcileOperation::RewriteSeedImage { path, .. } => {
                let path = checked_path(path, "seed image")?;
                self.require_exact(&path, &self.allowed_seed_images, "seed image")
            }
            nas_csi_vm_manager::ReconcileOperation::InstallOrUpdateSystemdUnit {
                unit_name,
                path,
                ..
            } => self.validate_systemd_unit(unit_name, path),
            nas_csi_vm_manager::ReconcileOperation::ReloadSystemdUnits { .. } => Ok(()),
            nas_csi_vm_manager::ReconcileOperation::EnableAndStartVirtiofsdService {
                unit_name,
                ..
            }
            | nas_csi_vm_manager::ReconcileOperation::RestartVirtiofsdService {
                unit_name, ..
            } => self.require_systemd_unit(unit_name),
            nas_csi_vm_manager::ReconcileOperation::DefineDomain {
                domain,
                xml_path,
                command,
            }
            | nas_csi_vm_manager::ReconcileOperation::RedefineDomain {
                domain,
                xml_path,
                command,
                ..
            } => {
                self.require_domain(domain)?;
                let path = checked_path(xml_path, "domain XML")?;
                self.require_under(&path, &self.artifact_dir, "domain XML")?;
                require_command_mentions_path(command, xml_path)
            }
            nas_csi_vm_manager::ReconcileOperation::RedefineDomainRequiresShutdown { .. } => {
                anyhow::bail!("domain redefine requires shutdown and cannot be executed directly")
            }
            nas_csi_vm_manager::ReconcileOperation::EnableDomainAutostart { domain, .. }
            | nas_csi_vm_manager::ReconcileOperation::StartDomain { domain, .. } => {
                self.require_domain(domain)
            }
            nas_csi_vm_manager::ReconcileOperation::RunCommand { .. } => {
                anyhow::bail!("execute guard refused unclassified command operation")
            }
        }
    }

    fn validate_systemd_unit(&self, unit_name: &str, path: &str) -> Result<()> {
        self.require_systemd_unit(unit_name)?;
        let path = checked_path(path, "systemd unit")?;
        let expected_path = self.systemd_unit_dir.join(unit_name);
        if path == expected_path {
            Ok(())
        } else {
            anyhow::bail!(
                "execute guard refused systemd unit path {}; expected {}",
                path.display(),
                expected_path.display()
            )
        }
    }

    fn require_systemd_unit(&self, unit_name: &str) -> Result<()> {
        if self.allowed_systemd_units.contains(unit_name)
            && unit_name.starts_with("nascsi-virtiofsd-")
            && unit_name.ends_with(".service")
        {
            Ok(())
        } else {
            anyhow::bail!("execute guard refused unmanaged systemd unit {unit_name}")
        }
    }

    fn require_domain(&self, domain: &str) -> Result<()> {
        if self.allowed_domains.contains(domain) {
            Ok(())
        } else {
            anyhow::bail!("execute guard refused unmanaged libvirt domain {domain}")
        }
    }

    fn require_under(&self, path: &Path, root: &Path, label: &str) -> Result<()> {
        if path.starts_with(root) {
            Ok(())
        } else {
            anyhow::bail!(
                "execute guard refused {label} path {} outside {}",
                path.display(),
                root.display()
            )
        }
    }

    fn require_exact(&self, path: &Path, allowed: &BTreeSet<PathBuf>, label: &str) -> Result<()> {
        if allowed.contains(path) {
            Ok(())
        } else {
            anyhow::bail!(
                "execute guard refused unexpected {label} path {}",
                path.display()
            )
        }
    }

    #[cfg(test)]
    fn for_test(artifact_dir: &Path, systemd_unit_dir: &Path) -> Self {
        Self {
            artifact_dir: artifact_dir.to_path_buf(),
            systemd_unit_dir: systemd_unit_dir.to_path_buf(),
            allowed_systemd_units: BTreeSet::new(),
            allowed_root_dirs: BTreeSet::new(),
            allowed_root_disks: BTreeSet::new(),
            allowed_seed_images: BTreeSet::new(),
            allowed_domains: BTreeSet::new(),
        }
    }

    #[cfg(test)]
    fn allow_systemd_unit(mut self, unit_name: &str) -> Self {
        self.allowed_systemd_units.insert(unit_name.to_string());
        self
    }

    #[cfg(test)]
    fn allow_domain(mut self, domain: &str) -> Self {
        self.allowed_domains.insert(domain.to_string());
        self
    }
}

fn checked_path(path: &str, label: &str) -> Result<PathBuf> {
    let path = Path::new(path);
    if path.as_os_str().is_empty() {
        anyhow::bail!("execute guard refused empty {label} path");
    }
    if path
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        anyhow::bail!(
            "execute guard refused {label} path containing parent traversal: {}",
            path.display()
        );
    }
    Ok(path.to_path_buf())
}

fn require_command_mentions_path(
    command: &nas_csi_vm_manager::CommandSpec,
    path: &str,
) -> Result<()> {
    if command.args.iter().any(|arg| arg == path) {
        Ok(())
    } else {
        anyhow::bail!("execute guard refused command without expected path argument: {command}")
    }
}

#[derive(Default)]
struct ExecutionRollback {
    actions: Vec<RollbackAction>,
}

enum RollbackAction {
    File(FileRollback),
    Domain(DomainRollback),
}

enum FileRollback {
    Restore { path: PathBuf, backup_path: PathBuf },
    Remove { path: PathBuf },
}

struct DomainRollback {
    domain: String,
    backup_path: PathBuf,
    restore_command: nas_csi_vm_manager::CommandSpec,
}

impl RollbackAction {
    fn path(&self) -> &Path {
        match self {
            Self::File(FileRollback::Restore { path, .. })
            | Self::File(FileRollback::Remove { path }) => path,
            Self::Domain(DomainRollback { backup_path, .. }) => backup_path,
        }
    }
}

impl ExecutionRollback {
    fn stage_file_change(&mut self, path: &str, desired_contents: &[u8]) -> Result<()> {
        let path_obj = Path::new(path);
        if self.actions.iter().any(|backup| backup.path() == path_obj) {
            return Ok(());
        }

        match fs::read(path_obj) {
            Ok(existing_contents) if existing_contents == desired_contents => Ok(()),
            Ok(existing_contents) => {
                let backup_path = backup_path_for(path_obj, "bak");
                write_binary_atomic_if_changed(
                    &backup_path.display().to_string(),
                    &existing_contents,
                )?;
                self.actions
                    .push(RollbackAction::File(FileRollback::Restore {
                        path: path_obj.to_path_buf(),
                        backup_path,
                    }));
                Ok(())
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {
                self.actions
                    .push(RollbackAction::File(FileRollback::Remove {
                        path: path_obj.to_path_buf(),
                    }));
                Ok(())
            }
            Err(error) => {
                Err(error).with_context(|| format!("failed to read existing file {path}"))
            }
        }
    }

    fn stage_domain_redefine(
        &mut self,
        domain: &str,
        xml_path: &str,
        command: &nas_csi_vm_manager::CommandSpec,
        previous_xml: Option<&str>,
    ) -> Result<()> {
        let previous_xml = previous_xml.ok_or_else(|| {
            anyhow::anyhow!("cannot redefine domain {domain}; previous XML was not captured")
        })?;
        let desired_xml_path = Path::new(xml_path);
        let backup_path = backup_path_for(desired_xml_path, &format!("{domain}.previous.xml"));
        if self
            .actions
            .iter()
            .any(|action| action.path() == backup_path)
        {
            return Ok(());
        }
        write_text_atomic_if_changed(&backup_path.display().to_string(), previous_xml)?;

        let mut restore_command = command.clone();
        if let Some(last_arg) = restore_command.args.last_mut() {
            *last_arg = backup_path.display().to_string();
        } else {
            anyhow::bail!(
                "cannot build rollback command for domain {domain}; virsh define argv is empty"
            );
        }
        self.actions.push(RollbackAction::Domain(DomainRollback {
            domain: domain.to_string(),
            backup_path,
            restore_command,
        }));
        Ok(())
    }

    fn restore(&mut self, runner: &impl CommandRunner) -> Result<()> {
        for backup in self.actions.iter().rev() {
            match backup {
                RollbackAction::File(FileRollback::Restore { path, backup_path }) => {
                    fs::rename(backup_path, path).with_context(|| {
                        format!(
                            "failed to restore {} from {}",
                            path.display(),
                            backup_path.display()
                        )
                    })?;
                    if let Some(parent) = path.parent()
                        && !parent.as_os_str().is_empty()
                    {
                        sync_directory(parent)?;
                    }
                }
                RollbackAction::File(FileRollback::Remove { path }) => {
                    match fs::remove_file(path) {
                        Ok(()) => {}
                        Err(error) if error.kind() == ErrorKind::NotFound => {}
                        Err(error) => {
                            return Err(error).with_context(|| {
                                format!("failed to remove created file {}", path.display())
                            });
                        }
                    }
                    if let Some(parent) = path.parent()
                        && !parent.as_os_str().is_empty()
                    {
                        sync_directory(parent)?;
                    }
                }
                RollbackAction::Domain(DomainRollback {
                    domain,
                    restore_command,
                    ..
                }) => {
                    runner
                        .run(restore_command)
                        .with_context(|| format!("failed to restore libvirt domain {domain}"))?;
                }
            }
        }
        Ok(())
    }

    fn commit(self) -> Result<()> {
        for backup in self.actions {
            let backup_path = match backup {
                RollbackAction::File(FileRollback::Restore { backup_path, .. })
                | RollbackAction::Domain(DomainRollback { backup_path, .. }) => Some(backup_path),
                RollbackAction::File(FileRollback::Remove { .. }) => None,
            };
            if let Some(backup_path) = backup_path {
                match fs::remove_file(&backup_path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == ErrorKind::NotFound => {}
                    Err(error) => {
                        return Err(error).with_context(|| {
                            format!("failed to remove backup file {}", backup_path.display())
                        });
                    }
                }
            }
        }
        Ok(())
    }
}

fn backup_path_for(path: &Path, suffix: &str) -> PathBuf {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("artifact");
    parent.join(format!("{file_name}.nas-csi.{suffix}"))
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum WriteOutcome {
    Unchanged,
    Written,
}

fn write_text_atomic_if_changed(path: &str, contents: &str) -> Result<WriteOutcome> {
    write_atomic_if_changed(path, contents.as_bytes())
}

fn write_binary_atomic_if_changed(path: &str, contents: &[u8]) -> Result<WriteOutcome> {
    write_atomic_if_changed(path, contents)
}

fn write_atomic_if_changed(path: &str, contents: &[u8]) -> Result<WriteOutcome> {
    let path_obj = Path::new(path);
    if let Some(parent) = path_obj.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    if fs::read(path_obj).ok().as_deref() == Some(contents) {
        println!("unchanged {path}");
        return Ok(WriteOutcome::Unchanged);
    }

    let parent = path_obj
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    let temp_path = create_temp_file_path(path_obj);
    let mut temp_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp_path)
        .with_context(|| format!("failed to create temporary file {}", temp_path.display()))?;
    let write_result = (|| -> Result<()> {
        temp_file
            .write_all(contents)
            .with_context(|| format!("failed to write temporary file {}", temp_path.display()))?;
        temp_file
            .sync_all()
            .with_context(|| format!("failed to fsync temporary file {}", temp_path.display()))?;
        drop(temp_file);
        fs::rename(&temp_path, path_obj).with_context(|| {
            format!(
                "failed to rename {} to {}",
                temp_path.display(),
                path_obj.display()
            )
        })?;
        if let Some(parent) = parent {
            sync_directory(parent)?;
        }
        Ok(())
    })();

    if write_result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    write_result?;
    println!("wrote {path}");
    Ok(WriteOutcome::Written)
}

fn create_temp_file_path(path: &Path) -> PathBuf {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("artifact");
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    parent.join(format!(
        ".{file_name}.nas-csi.tmp.{}.{}",
        process::id(),
        now
    ))
}

fn sync_directory(path: &Path) -> Result<()> {
    match File::open(path) {
        Ok(directory) => directory
            .sync_all()
            .with_context(|| format!("failed to fsync directory {}", path.display())),
        Err(error) if error.kind() == ErrorKind::PermissionDenied => Ok(()),
        Err(error) => {
            Err(error).with_context(|| format!("failed to open directory {}", path.display()))
        }
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    #[derive(Default)]
    struct FakeCommandRunner {
        statuses: BTreeMap<String, bool>,
        outputs: BTreeMap<String, Option<String>>,
        status_calls: RefCell<Vec<nas_csi_vm_manager::CommandSpec>>,
        output_calls: RefCell<Vec<nas_csi_vm_manager::CommandSpec>>,
    }

    impl FakeCommandRunner {
        fn with_status(mut self, command: &nas_csi_vm_manager::CommandSpec, status: bool) -> Self {
            self.statuses.insert(command_key(command), status);
            self
        }

        fn with_output(
            mut self,
            command: &nas_csi_vm_manager::CommandSpec,
            output: Option<&str>,
        ) -> Self {
            self.outputs
                .insert(command_key(command), output.map(str::to_string));
            self
        }
    }

    impl CommandRunner for FakeCommandRunner {
        fn status(&self, command: &nas_csi_vm_manager::CommandSpec) -> Result<bool> {
            self.status_calls.borrow_mut().push(command.clone());
            Ok(*self.statuses.get(&command_key(command)).unwrap_or(&true))
        }

        fn output(&self, command: &nas_csi_vm_manager::CommandSpec) -> Result<Option<String>> {
            self.output_calls.borrow_mut().push(command.clone());
            Ok(self
                .outputs
                .get(&command_key(command))
                .cloned()
                .unwrap_or(None))
        }
    }

    #[test]
    fn execute_reconcile_plan_uses_runner_argv() {
        let command = nas_csi_vm_manager::CommandSpec::new(
            "/usr/bin/systemctl",
            ["daemon-reload".to_string()],
        );
        let runner = FakeCommandRunner::default().with_status(&command, true);
        let plan = nas_csi_vm_manager::HostReconcilePlan {
            steps: vec![nas_csi_vm_manager::ReconcileStep {
                description: "reload systemd units".to_string(),
                kind: nas_csi_vm_manager::ReconcileStepKind::Apply(
                    nas_csi_vm_manager::ReconcileOperation::ReloadSystemdUnits {
                        command: command.clone(),
                    },
                ),
            }],
        };

        let safety =
            ExecuteSafety::for_test(Path::new("/tmp/artifacts"), Path::new("/tmp/systemd"));
        execute_reconcile_plan(&plan, &runner, &safety).expect("execute");

        let calls = runner.status_calls.borrow();
        assert_eq!(calls.as_slice(), &[command]);
    }

    #[test]
    fn inspect_domain_reads_current_xml_with_runner() {
        let dumpxml = nas_csi_vm_manager::CommandSpec::new(
            "/usr/bin/virsh",
            [
                "-c".to_string(),
                "qemu:///system".to_string(),
                "dumpxml".to_string(),
                "nascsi-node-1".to_string(),
            ],
        );
        let domstate = nas_csi_vm_manager::CommandSpec::new(
            "/usr/bin/virsh",
            [
                "-c".to_string(),
                "qemu:///system".to_string(),
                "domstate".to_string(),
                "nascsi-node-1".to_string(),
            ],
        );
        let dominfo = nas_csi_vm_manager::CommandSpec::new(
            "/usr/bin/virsh",
            [
                "-c".to_string(),
                "qemu:///system".to_string(),
                "dominfo".to_string(),
                "nascsi-node-1".to_string(),
            ],
        );
        let xml = "<domain><metadata><nas-csi:desired-domain-hash xmlns:nas-csi='urn:nas-csi.dev:domain'>0123456789abcdef</nas-csi:desired-domain-hash></metadata></domain>";
        let runner = FakeCommandRunner::default()
            .with_output(&dumpxml, Some(xml))
            .with_output(&domstate, Some("running"))
            .with_output(&dominfo, Some("Autostart: enable"));

        let domain = inspect_domain(&runner, "/usr/bin/virsh", "qemu:///system", "nascsi-node-1")
            .expect("inspect domain");

        assert!(domain.exists);
        assert!(domain.active);
        assert_eq!(domain.autostart, Some(true));
        assert_eq!(domain.desired_hash.as_deref(), Some("0123456789abcdef"));
        assert_eq!(domain.xml.as_deref(), Some(xml));
        assert_eq!(
            runner.output_calls.borrow().as_slice(),
            &[dumpxml, domstate, dominfo]
        );
    }

    #[test]
    fn apply_lock_refuses_second_holder() {
        let root = unique_test_dir("apply-lock");
        let lock_path = root.join("apply.lock");
        let lock = ApplyLock::acquire(&lock_path).expect("first lock");

        assert!(ApplyLock::acquire(&lock_path).is_err());

        drop(lock);
        ApplyLock::acquire(&lock_path).expect("lock after drop");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn reconcile_summary_counts_actions_and_risk() {
        let risky_command =
            nas_csi_vm_manager::CommandSpec::new("/usr/bin/virsh", ["start".to_string()]);
        let plan = nas_csi_vm_manager::HostReconcilePlan {
            steps: vec![
                nas_csi_vm_manager::ReconcileStep {
                    description: "write artifact".to_string(),
                    kind: nas_csi_vm_manager::ReconcileStepKind::Apply(
                        nas_csi_vm_manager::ReconcileOperation::WriteRenderedArtifact {
                            path: "/tmp/domain.xml".to_string(),
                            contents: "xml".to_string(),
                        },
                    ),
                },
                nas_csi_vm_manager::ReconcileStep {
                    description: "start domain".to_string(),
                    kind: nas_csi_vm_manager::ReconcileStepKind::Apply(
                        nas_csi_vm_manager::ReconcileOperation::StartDomain {
                            domain: "nascsi-node-1".to_string(),
                            command: risky_command,
                        },
                    ),
                },
                nas_csi_vm_manager::ReconcileStep {
                    description: "skip".to_string(),
                    kind: nas_csi_vm_manager::ReconcileStepKind::SkipAlreadyCorrect {
                        reason: "current".to_string(),
                    },
                },
                nas_csi_vm_manager::ReconcileStep {
                    description: "refuse".to_string(),
                    kind: nas_csi_vm_manager::ReconcileStepKind::Refuse {
                        operation: None,
                        reason: "blocked".to_string(),
                    },
                },
            ],
        };

        assert_eq!(
            summarize_reconcile_plan(&plan),
            ReconcileSummary {
                apply: 2,
                skip: 1,
                refuse: 1,
                risky: 1,
            }
        );
    }

    #[test]
    fn execute_reconcile_plan_rolls_back_changed_systemd_unit_on_failure() {
        let root = unique_test_dir("systemd-rollback");
        let unit_path = root.join("nascsi-virtiofsd-test.service");
        fs::write(&unit_path, "old").expect("write old unit");
        let failing_command = nas_csi_vm_manager::CommandSpec::new(
            "/usr/bin/systemctl",
            ["daemon-reload".to_string()],
        );
        let runner = FakeCommandRunner::default().with_status(&failing_command, false);
        let plan = nas_csi_vm_manager::HostReconcilePlan {
            steps: vec![
                nas_csi_vm_manager::ReconcileStep {
                    description: "install systemd unit".to_string(),
                    kind: nas_csi_vm_manager::ReconcileStepKind::Apply(
                        nas_csi_vm_manager::ReconcileOperation::InstallOrUpdateSystemdUnit {
                            unit_name: "nascsi-virtiofsd-test.service".to_string(),
                            path: unit_path.display().to_string(),
                            contents: "new".to_string(),
                        },
                    ),
                },
                nas_csi_vm_manager::ReconcileStep {
                    description: "reload systemd".to_string(),
                    kind: nas_csi_vm_manager::ReconcileStepKind::Apply(
                        nas_csi_vm_manager::ReconcileOperation::ReloadSystemdUnits {
                            command: failing_command,
                        },
                    ),
                },
            ],
        };

        let safety = ExecuteSafety::for_test(&root.join("artifacts"), &root)
            .allow_systemd_unit("nascsi-virtiofsd-test.service");
        assert!(execute_reconcile_plan(&plan, &runner, &safety).is_err());
        assert_eq!(
            fs::read_to_string(&unit_path).expect("unit restored"),
            "old"
        );
        assert!(!backup_path_for(&unit_path, "bak").exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn execute_reconcile_plan_restores_domain_xml_on_later_failure() {
        let root = unique_test_dir("domain-rollback");
        let desired_xml_path = root.join("domain.xml");
        fs::write(&desired_xml_path, "<domain>new</domain>").expect("write desired xml");
        let redefine_command = nas_csi_vm_manager::CommandSpec::new(
            "/usr/bin/virsh",
            [
                "-c".to_string(),
                "qemu:///system".to_string(),
                "define".to_string(),
                desired_xml_path.display().to_string(),
            ],
        );
        let failing_command = nas_csi_vm_manager::CommandSpec::new(
            "/usr/bin/virsh",
            [
                "-c".to_string(),
                "qemu:///system".to_string(),
                "autostart".to_string(),
                "nascsi-node-1".to_string(),
            ],
        );
        let backup_path = backup_path_for(&desired_xml_path, "nascsi-node-1.previous.xml");
        let restore_command = nas_csi_vm_manager::CommandSpec::new(
            "/usr/bin/virsh",
            [
                "-c".to_string(),
                "qemu:///system".to_string(),
                "define".to_string(),
                backup_path.display().to_string(),
            ],
        );
        let runner = FakeCommandRunner::default().with_status(&failing_command, false);
        let plan = nas_csi_vm_manager::HostReconcilePlan {
            steps: vec![
                nas_csi_vm_manager::ReconcileStep {
                    description: "redefine domain".to_string(),
                    kind: nas_csi_vm_manager::ReconcileStepKind::Apply(
                        nas_csi_vm_manager::ReconcileOperation::RedefineDomain {
                            domain: "nascsi-node-1".to_string(),
                            xml_path: desired_xml_path.display().to_string(),
                            previous_xml: Some("<domain>old</domain>".to_string()),
                            command: redefine_command.clone(),
                        },
                    ),
                },
                nas_csi_vm_manager::ReconcileStep {
                    description: "autostart domain".to_string(),
                    kind: nas_csi_vm_manager::ReconcileStepKind::Apply(
                        nas_csi_vm_manager::ReconcileOperation::EnableDomainAutostart {
                            domain: "nascsi-node-1".to_string(),
                            command: failing_command.clone(),
                        },
                    ),
                },
            ],
        };

        let safety =
            ExecuteSafety::for_test(&root, &root.join("systemd")).allow_domain("nascsi-node-1");
        assert!(execute_reconcile_plan(&plan, &runner, &safety).is_err());
        assert_eq!(
            fs::read_to_string(&backup_path).expect("domain rollback backup"),
            "<domain>old</domain>"
        );
        assert_eq!(
            runner.status_calls.borrow().as_slice(),
            &[redefine_command, failing_command, restore_command]
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn execute_guard_refuses_rendered_artifact_outside_artifact_dir() {
        let root = unique_test_dir("execute-guard-artifact");
        let forbidden_path = root.join("outside.txt");
        let plan = nas_csi_vm_manager::HostReconcilePlan {
            steps: vec![nas_csi_vm_manager::ReconcileStep {
                description: "write outside artifact dir".to_string(),
                kind: nas_csi_vm_manager::ReconcileStepKind::Apply(
                    nas_csi_vm_manager::ReconcileOperation::WriteRenderedArtifact {
                        path: forbidden_path.display().to_string(),
                        contents: "nope".to_string(),
                    },
                ),
            }],
        };
        let runner = FakeCommandRunner::default();
        let safety = ExecuteSafety::for_test(&root.join("artifacts"), &root.join("systemd"));

        let error = execute_reconcile_plan(&plan, &runner, &safety).expect_err("guard refuses");

        assert!(error.to_string().contains("outside"));
        assert!(!forbidden_path.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn execute_guard_refuses_unclassified_commands() {
        let command =
            nas_csi_vm_manager::CommandSpec::new("rm", ["-rf".to_string(), "/".to_string()]);
        let plan = nas_csi_vm_manager::HostReconcilePlan {
            steps: vec![nas_csi_vm_manager::ReconcileStep {
                description: "unclassified command".to_string(),
                kind: nas_csi_vm_manager::ReconcileStepKind::Apply(
                    nas_csi_vm_manager::ReconcileOperation::RunCommand {
                        command,
                        creates: None,
                    },
                ),
            }],
        };
        let runner = FakeCommandRunner::default();
        let safety =
            ExecuteSafety::for_test(Path::new("/tmp/artifacts"), Path::new("/tmp/systemd"));

        let error = execute_reconcile_plan(&plan, &runner, &safety).expect_err("guard refuses");

        assert!(error.to_string().contains("unclassified command"));
        assert!(runner.status_calls.borrow().is_empty());
    }

    fn unique_test_dir(name: &str) -> PathBuf {
        let path = env::temp_dir().join(format!("nas-csi-host-agent-{name}-{}", process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create temp test dir");
        path
    }

    fn command_key(command: &nas_csi_vm_manager::CommandSpec) -> String {
        format!("{}\0{}", command.program, command.args.join("\0"))
    }
}
