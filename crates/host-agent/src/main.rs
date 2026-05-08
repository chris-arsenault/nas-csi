use anyhow::{Context, Result};
use base64::Engine;
use clap::{Parser, Subcommand};
use nas_csi_cluster_manager::{
    ClusterActualState, ClusterCommandSpec, ClusterNodeActualState, ClusterOperation,
    ClusterReconcileOptions, ClusterReconcilePlan, ClusterReconcileStepKind, DesiredManifest,
    GuestCommandSpec,
};
use nas_csi_types::{
    AccessMode, ClusterIntent, DiscoveryInventory, HostConfig, HostConfigDraft, HostSelections,
    NodeConfig, NodeRole, NodeTaint,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process;
use std::process::Command as ProcessCommand;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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
        allow_domain_adoption: bool,
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
    /// Report health for tools, services, domains, sockets, and datasets.
    Health {
        #[arg(long)]
        config: PathBuf,
        #[arg(long, default_value = ".nas-csi/rendered")]
        artifact_dir: PathBuf,
        #[arg(long, default_value = "/etc/systemd/system")]
        systemd_unit_dir: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Run first-host installation workflow: discover, materialize, apply, and verify.
    HostInstall {
        #[arg(long)]
        intent: Option<PathBuf>,
        #[arg(long)]
        selections: Option<PathBuf>,
        #[arg(long, default_value = "/etc/nas-csi/discovery.yaml")]
        discovery: PathBuf,
        #[arg(long, default_value = "/etc/nas-csi/host.yaml")]
        config: PathBuf,
        #[arg(long, default_value = "/var/lib/nas-csi/rendered")]
        artifact_dir: PathBuf,
        #[arg(long, default_value = "/etc/systemd/system")]
        systemd_unit_dir: PathBuf,
        #[arg(long)]
        allow_running_domain_redefine: bool,
        #[arg(long)]
        allow_domain_adoption: bool,
        #[arg(long)]
        no_start_domains: bool,
        #[arg(long, default_value_t = 600)]
        guest_agent_timeout_seconds: u64,
        #[arg(long)]
        post_reboot_check: bool,
        #[arg(long)]
        execute: bool,
    },
    /// Plan, apply, or inspect host-agent-owned k3s cluster substrate.
    Cluster {
        #[command(subcommand)]
        command: ClusterCommand,
    },
    /// Install and verify static existing-dataset CSI volumes.
    Csi {
        #[command(subcommand)]
        command: CsiCommand,
    },
    /// Validate real repo and read-only content workloads against static CSI volumes.
    Workload {
        #[command(subcommand)]
        command: WorkloadCommand,
    },
}

#[derive(Debug, Subcommand)]
enum ClusterCommand {
    /// Plan k3s bootstrap/join and Kubernetes substrate reconciliation.
    Plan {
        #[arg(long)]
        config: PathBuf,
        #[arg(long, default_value = ".nas-csi/rendered")]
        artifact_dir: PathBuf,
        #[arg(long, default_value = "deploy")]
        manifest_root: PathBuf,
        #[arg(long, default_value = "kubectl")]
        kubectl: String,
    },
    /// Apply k3s bootstrap/join and Kubernetes substrate reconciliation.
    Apply {
        #[arg(long)]
        config: PathBuf,
        #[arg(long, default_value = ".nas-csi/rendered")]
        artifact_dir: PathBuf,
        #[arg(long, default_value = "deploy")]
        manifest_root: PathBuf,
        #[arg(long, default_value = "kubectl")]
        kubectl: String,
        #[arg(long)]
        execute: bool,
    },
    /// Report cluster-side state observed through libvirt, guest agent, and kubectl.
    Status {
        #[arg(long)]
        config: PathBuf,
        #[arg(long, default_value = ".nas-csi/rendered")]
        artifact_dir: PathBuf,
        #[arg(long, default_value = "deploy")]
        manifest_root: PathBuf,
        #[arg(long, default_value = "kubectl")]
        kubectl: String,
    },
    /// Run first-cluster installation workflow: plan, apply, and verify substrate.
    Install {
        #[arg(long, default_value = "/etc/nas-csi/host.yaml")]
        config: PathBuf,
        #[arg(long, default_value = "/var/lib/nas-csi/rendered")]
        artifact_dir: PathBuf,
        #[arg(long, default_value = "/usr/local/share/nas-csi/deploy")]
        manifest_root: PathBuf,
        #[arg(long, default_value = "kubectl")]
        kubectl: String,
        #[arg(long)]
        reboot_node: Option<String>,
        #[arg(long)]
        post_reboot_check: bool,
        #[arg(long, default_value_t = 600)]
        wait_timeout_seconds: u64,
        #[arg(long)]
        execute: bool,
    },
}

#[derive(Debug, Subcommand)]
enum CsiCommand {
    /// Install node runtime config, static PV/PVCs, and smoke-test existing datasets.
    Install {
        #[arg(long, default_value = "/etc/nas-csi/host.yaml")]
        config: PathBuf,
        #[arg(long, default_value = "/var/lib/nas-csi/rendered")]
        artifact_dir: PathBuf,
        #[arg(long, default_value = "/usr/local/share/nas-csi/deploy")]
        manifest_root: PathBuf,
        #[arg(long, default_value = "kubectl")]
        kubectl: String,
        #[arg(long, default_value = "default")]
        namespace: String,
        #[arg(long, default_value = "busybox:1.36")]
        smoke_image: String,
        #[arg(long, default_value_t = 600)]
        wait_timeout_seconds: u64,
        #[arg(long)]
        execute: bool,
    },
}

#[derive(Debug, Subcommand)]
enum WorkloadCommand {
    /// Run repo/content workload validation and record virtiofs coherency observations.
    Validate {
        #[arg(long, default_value = "/etc/nas-csi/host.yaml")]
        config: PathBuf,
        #[arg(long, default_value = "/var/lib/nas-csi/rendered")]
        artifact_dir: PathBuf,
        #[arg(long, default_value = "kubectl")]
        kubectl: String,
        #[arg(long, default_value = "default")]
        namespace: String,
        /// Export id for repository workload validation. Defaults to the first read-write export.
        #[arg(long)]
        repo_export: Option<String>,
        /// Export id for read-only streaming validation. Defaults to the first read-only export.
        #[arg(long)]
        content_export: Option<String>,
        #[arg(long, default_value = "node:22-bookworm")]
        repo_image: String,
        #[arg(long, default_value = "busybox:1.36")]
        content_image: String,
        #[arg(long, default_value = "httpd -f -p 8080 -h /content")]
        content_command: String,
        #[arg(long, default_value_t = 600)]
        wait_timeout_seconds: u64,
        #[arg(long, default_value_t = 200)]
        small_file_count: u32,
        /// Leave validation pods running for manual inspection instead of deleting them on success.
        #[arg(long)]
        keep_pods: bool,
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
            allow_running_domain_redefine,
            allow_domain_adoption,
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
                allow_domain_adoption,
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
            log_reconcile_decisions(&reconcile_plan);
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
                apply_options_from_config(&config, &artifact_dir, &systemd_unit_dir, false, false);
            let actual = inspect_status_state(&config, &apply_options, &runner)?;
            print_host_status(&config, &apply_options, &actual);
            Ok(())
        }
        Command::Health {
            config,
            artifact_dir,
            systemd_unit_dir,
            json,
        } => {
            let config = load_yaml::<HostConfig>(&config)?;
            report_validation("host config", config.validate())?;
            let runner = RealCommandRunner;
            let render_options = render_options_from_config(&config);
            let apply_options =
                apply_options_from_config(&config, &artifact_dir, &systemd_unit_dir, false, false);
            let actual = inspect_status_state(&config, &apply_options, &runner)?;
            let report = build_health_report(&config, &render_options, &actual)?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&report)
                        .context("failed to serialize health report")?
                );
            } else {
                print_health_report(&report);
            }
            Ok(())
        }
        Command::HostInstall {
            intent,
            selections,
            discovery,
            config,
            artifact_dir,
            systemd_unit_dir,
            allow_running_domain_redefine,
            allow_domain_adoption,
            no_start_domains,
            guest_agent_timeout_seconds,
            post_reboot_check,
            execute,
        } => {
            let runner = RealCommandRunner;
            run_host_install(
                HostInstallOptions {
                    intent,
                    selections,
                    discovery,
                    config,
                    artifact_dir,
                    systemd_unit_dir,
                    allow_running_domain_redefine,
                    allow_domain_adoption,
                    start_domains: !no_start_domains,
                    guest_agent_timeout: Duration::from_secs(guest_agent_timeout_seconds),
                    post_reboot_check,
                    execute,
                },
                &runner,
            )
        }
        Command::Cluster { command } => match command {
            ClusterCommand::Plan {
                config,
                artifact_dir,
                manifest_root,
                kubectl,
            } => {
                let config = load_yaml::<HostConfig>(&config)?;
                report_validation("host config", config.validate())?;
                let runner = RealCommandRunner;
                let options = cluster_options_from_config(&config, &artifact_dir, &kubectl);
                let manifests = load_cluster_manifests(&config, &manifest_root)?;
                let actual = inspect_cluster_actual_state(&config, &options, &manifests, &runner)?;
                let plan = nas_csi_cluster_manager::plan_cluster_reconcile(
                    &config, &options, &actual, &manifests,
                );
                log_cluster_reconcile_decisions(&plan);
                print_cluster_reconcile_plan(&plan);
                Ok(())
            }
            ClusterCommand::Apply {
                config,
                artifact_dir,
                manifest_root,
                kubectl,
                execute,
            } => {
                let config = load_yaml::<HostConfig>(&config)?;
                report_validation("host config", config.validate())?;
                let runner = RealCommandRunner;
                let options = cluster_options_from_config(&config, &artifact_dir, &kubectl);
                let manifests = load_cluster_manifests(&config, &manifest_root)?;
                let actual = inspect_cluster_actual_state(&config, &options, &manifests, &runner)?;
                let plan = nas_csi_cluster_manager::plan_cluster_reconcile(
                    &config, &options, &actual, &manifests,
                );
                log_cluster_reconcile_decisions(&plan);
                if execute {
                    execute_cluster_reconcile_plan(&plan, &options, &runner)
                } else {
                    print_cluster_reconcile_plan(&plan);
                    Ok(())
                }
            }
            ClusterCommand::Status {
                config,
                artifact_dir,
                manifest_root,
                kubectl,
            } => {
                let config = load_yaml::<HostConfig>(&config)?;
                report_validation("host config", config.validate())?;
                let runner = RealCommandRunner;
                let options = cluster_options_from_config(&config, &artifact_dir, &kubectl);
                let manifests = load_cluster_manifests(&config, &manifest_root)?;
                let actual = inspect_cluster_actual_state(&config, &options, &manifests, &runner)?;
                print_cluster_status(&config, &actual, &manifests);
                Ok(())
            }
            ClusterCommand::Install {
                config,
                artifact_dir,
                manifest_root,
                kubectl,
                reboot_node,
                post_reboot_check,
                wait_timeout_seconds,
                execute,
            } => {
                let config = load_yaml::<HostConfig>(&config)?;
                report_validation("host config", config.validate())?;
                let runner = RealCommandRunner;
                run_cluster_install(
                    &config,
                    ClusterInstallOptions {
                        artifact_dir,
                        manifest_root,
                        kubectl,
                        reboot_node,
                        post_reboot_check,
                        wait_timeout: Duration::from_secs(wait_timeout_seconds),
                        execute,
                    },
                    &runner,
                )
            }
        },
        Command::Csi { command } => match command {
            CsiCommand::Install {
                config,
                artifact_dir,
                manifest_root,
                kubectl,
                namespace,
                smoke_image,
                wait_timeout_seconds,
                execute,
            } => {
                let config = load_yaml::<HostConfig>(&config)?;
                report_validation("host config", config.validate())?;
                let runner = RealCommandRunner;
                run_csi_install(
                    &config,
                    CsiInstallOptions {
                        artifact_dir,
                        manifest_root,
                        kubectl,
                        namespace,
                        smoke_image,
                        wait_timeout: Duration::from_secs(wait_timeout_seconds),
                        execute,
                    },
                    &runner,
                )
            }
        },
        Command::Workload { command } => match command {
            WorkloadCommand::Validate {
                config,
                artifact_dir,
                kubectl,
                namespace,
                repo_export,
                content_export,
                repo_image,
                content_image,
                content_command,
                wait_timeout_seconds,
                small_file_count,
                keep_pods,
                execute,
            } => {
                let config = load_yaml::<HostConfig>(&config)?;
                report_validation("host config", config.validate())?;
                let runner = RealCommandRunner;
                run_workload_validation(
                    &config,
                    WorkloadValidationOptions {
                        artifact_dir,
                        kubectl,
                        namespace,
                        repo_export,
                        content_export,
                        repo_image,
                        content_image,
                        content_command,
                        wait_timeout: Duration::from_secs(wait_timeout_seconds),
                        small_file_count,
                        keep_pods,
                        execute,
                    },
                    &runner,
                )
            }
        },
    }
}

#[derive(Debug)]
struct HostInstallOptions {
    intent: Option<PathBuf>,
    selections: Option<PathBuf>,
    discovery: PathBuf,
    config: PathBuf,
    artifact_dir: PathBuf,
    systemd_unit_dir: PathBuf,
    allow_running_domain_redefine: bool,
    allow_domain_adoption: bool,
    start_domains: bool,
    guest_agent_timeout: Duration,
    post_reboot_check: bool,
    execute: bool,
}

fn run_host_install(options: HostInstallOptions, runner: &impl CommandRunner) -> Result<()> {
    if options.post_reboot_check {
        let config = load_yaml::<HostConfig>(&options.config)?;
        report_validation("host config", config.validate())?;
        return run_host_install_post_reboot_check(&config, &options, runner);
    }

    let intent_path = options.intent.as_ref().ok_or_else(|| {
        anyhow::anyhow!("host-install requires --intent unless --post-reboot-check is used")
    })?;
    let selections_path = options.selections.as_ref().ok_or_else(|| {
        anyhow::anyhow!("host-install requires --selections unless --post-reboot-check is used")
    })?;

    println!("host-install: loading intent {}", intent_path.display());
    let intent = load_yaml::<ClusterIntent>(intent_path)?;
    report_validation("intent", intent.validate())?;

    println!(
        "host-install: loading selections {}",
        selections_path.display()
    );
    let selections = load_yaml::<HostSelections>(selections_path)?;
    report_validation("host selections", selections.validate())?;

    println!("host-install: running read-only discovery");
    let discovery = nas_csi_discovery::discover_local();
    report_validation("discovery", discovery.validate())?;
    write_yaml_atomic_if_changed(&options.discovery, &discovery)?;
    println!(
        "host-install: wrote discovery {}",
        options.discovery.display()
    );

    println!("host-install: materializing host config");
    let config = HostConfig::from_intent_discovery_selections(&intent, &discovery, &selections)
        .map_err(|errors| validation_error("host config", errors))?;
    write_yaml_atomic_if_changed(&options.config, &config)?;
    println!("host-install: wrote config {}", options.config.display());

    run_host_install_apply(&config, &options, runner)
}

fn run_host_install_apply(
    config: &HostConfig,
    options: &HostInstallOptions,
    runner: &impl CommandRunner,
) -> Result<()> {
    let render_options = render_options_from_config(config);
    let mut apply_options = apply_options_from_config(
        config,
        &options.artifact_dir,
        &options.systemd_unit_dir,
        options.allow_running_domain_redefine,
        options.allow_domain_adoption,
    );
    apply_options.start_domains = options.start_domains;

    let desired_apply =
        nas_csi_vm_manager::plan_host_apply(config, &render_options, &apply_options)?;
    let before_datasets = observe_datasets(config)?;
    let actual = inspect_actual_state(config, &desired_apply, &apply_options, runner)?;
    let reconcile_plan =
        nas_csi_vm_manager::plan_host_reconcile(config, &render_options, &apply_options, &actual)?;
    log_reconcile_decisions(&reconcile_plan);
    print_reconcile_plan(&reconcile_plan);
    verify_no_dataset_mutating_operations(config, &reconcile_plan)?;

    if !options.execute {
        println!("host-install: dry run only; pass --execute to apply changes");
        return Ok(());
    }

    let _apply_lock = ApplyLock::acquire(&apply_lock_path(&options.artifact_dir))?;
    let safety = ExecuteSafety::from_config(config, &apply_options)?;
    execute_reconcile_plan(&reconcile_plan, runner, &safety)?;

    if options.start_domains {
        wait_for_all_guest_agents(
            config,
            &cluster_options_from_config(config, &options.artifact_dir, "kubectl"),
            runner,
            options.guest_agent_timeout,
        )?;
    }

    let post_actual = inspect_status_state(config, &apply_options, runner)?;
    print_host_status(config, &apply_options, &post_actual);
    let health = build_health_report(config, &render_options, &post_actual)?;
    print_health_report(&health);
    verify_host_install_state(
        config,
        &desired_apply,
        &apply_options,
        &render_options,
        &post_actual,
        &health,
        options.start_domains,
    )?;
    let after_datasets = observe_datasets(config)?;
    verify_dataset_observations_stable(&before_datasets, &after_datasets)?;

    let post_reconcile = nas_csi_vm_manager::plan_host_reconcile(
        config,
        &render_options,
        &apply_options,
        &post_actual,
    )?;
    verify_post_install_idempotence(&post_reconcile)?;

    println!("host-install: completed host bring-up verification");
    Ok(())
}

fn run_host_install_post_reboot_check(
    config: &HostConfig,
    options: &HostInstallOptions,
    runner: &impl CommandRunner,
) -> Result<()> {
    println!(
        "host-install: running post-reboot verification from {}",
        options.config.display()
    );

    let render_options = render_options_from_config(config);
    let mut apply_options = apply_options_from_config(
        config,
        &options.artifact_dir,
        &options.systemd_unit_dir,
        options.allow_running_domain_redefine,
        options.allow_domain_adoption,
    );
    apply_options.start_domains = options.start_domains;
    let desired_apply =
        nas_csi_vm_manager::plan_host_apply(config, &render_options, &apply_options)?;
    let actual = inspect_status_state(config, &apply_options, runner)?;
    print_host_status(config, &apply_options, &actual);
    let health = build_health_report(config, &render_options, &actual)?;
    print_health_report(&health);

    if options.start_domains {
        wait_for_all_guest_agents(
            config,
            &cluster_options_from_config(config, &options.artifact_dir, "kubectl"),
            runner,
            options.guest_agent_timeout,
        )?;
    }

    verify_host_install_state(
        config,
        &desired_apply,
        &apply_options,
        &render_options,
        &actual,
        &health,
        options.start_domains,
    )?;
    let post_reconcile =
        nas_csi_vm_manager::plan_host_reconcile(config, &render_options, &apply_options, &actual)?;
    verify_post_install_idempotence(&post_reconcile)?;

    println!("host-install: post-reboot verification passed");
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DatasetObservation {
    source_path: String,
    exists: bool,
    mounted: bool,
}

fn observe_datasets(config: &HostConfig) -> Result<BTreeMap<String, DatasetObservation>> {
    let mount_points = read_mount_points()?;
    Ok(config
        .exports
        .iter()
        .map(|(export_id, export)| {
            (
                export_id.clone(),
                DatasetObservation {
                    source_path: export.source_path.clone(),
                    exists: Path::new(&export.source_path).exists(),
                    mounted: mount_points.contains(&export.source_path),
                },
            )
        })
        .collect())
}

fn verify_dataset_observations_stable(
    before: &BTreeMap<String, DatasetObservation>,
    after: &BTreeMap<String, DatasetObservation>,
) -> Result<()> {
    if before == after {
        return Ok(());
    }

    anyhow::bail!(
        "dataset observations changed during host install; before={before:?} after={after:?}"
    )
}

fn verify_no_dataset_mutating_operations(
    config: &HostConfig,
    plan: &nas_csi_vm_manager::HostReconcilePlan,
) -> Result<()> {
    let dataset_roots = config
        .exports
        .values()
        .map(|export| PathBuf::from(&export.source_path))
        .collect::<Vec<_>>();

    for step in &plan.steps {
        let nas_csi_vm_manager::ReconcileStepKind::Apply(operation) = &step.kind else {
            continue;
        };
        for target in reconcile_operation_target_paths(operation) {
            let target_path = Path::new(&target);
            for dataset_root in &dataset_roots {
                if target_path.starts_with(dataset_root) {
                    anyhow::bail!(
                        "refusing host install plan that writes under exported dataset {} via step {}",
                        dataset_root.display(),
                        step.description
                    );
                }
            }
        }
    }

    Ok(())
}

fn reconcile_operation_target_paths(
    operation: &nas_csi_vm_manager::ReconcileOperation,
) -> Vec<String> {
    match operation {
        nas_csi_vm_manager::ReconcileOperation::EnsureDirectory { path }
        | nas_csi_vm_manager::ReconcileOperation::WriteRenderedArtifact { path, .. }
        | nas_csi_vm_manager::ReconcileOperation::RewriteSeedImage { path, .. }
        | nas_csi_vm_manager::ReconcileOperation::InstallOrUpdateSystemdUnit { path, .. }
        | nas_csi_vm_manager::ReconcileOperation::CreateRootDisk { path, .. }
        | nas_csi_vm_manager::ReconcileOperation::ResizeRootDisk { path, .. } => {
            vec![path.clone()]
        }
        nas_csi_vm_manager::ReconcileOperation::RedefineDomain { xml_path, .. }
        | nas_csi_vm_manager::ReconcileOperation::RedefineDomainRequiresShutdown {
            xml_path, ..
        } => vec![xml_path.clone()],
        nas_csi_vm_manager::ReconcileOperation::RunCommand { creates, .. } => {
            creates.iter().cloned().collect()
        }
        nas_csi_vm_manager::ReconcileOperation::ReloadSystemdUnits { .. }
        | nas_csi_vm_manager::ReconcileOperation::EnableAndStartVirtiofsdService { .. }
        | nas_csi_vm_manager::ReconcileOperation::RestartVirtiofsdService { .. }
        | nas_csi_vm_manager::ReconcileOperation::DefineDomain { .. }
        | nas_csi_vm_manager::ReconcileOperation::EnableDomainAutostart { .. }
        | nas_csi_vm_manager::ReconcileOperation::StartDomain { .. } => Vec::new(),
    }
}

fn wait_for_all_guest_agents(
    config: &HostConfig,
    options: &ClusterReconcileOptions,
    runner: &impl CommandRunner,
    timeout: Duration,
) -> Result<()> {
    for node in &config.nodes {
        wait_for_guest_agent(config, options, runner, &node.domain, timeout)?;
    }
    Ok(())
}

fn wait_for_guest_agent(
    config: &HostConfig,
    options: &ClusterReconcileOptions,
    runner: &impl CommandRunner,
    domain: &str,
    timeout: Duration,
) -> Result<()> {
    let command = GuestCommandSpec::new(
        "/bin/sh".to_string(),
        ["-c".to_string(), "true".to_string()],
    );
    let deadline = Instant::now() + timeout;
    loop {
        match guest_command_success(runner, options, domain, &command) {
            Ok(true) => {
                println!("host-install: qemu guest agent ready for {domain}");
                return Ok(());
            }
            Ok(false) | Err(_) => {}
        }
        if Instant::now() >= deadline {
            anyhow::bail!(
                "timed out waiting for VM {domain} qemu guest agent for cluster {}",
                config.cluster.name
            );
        }
        thread::sleep(Duration::from_secs(5));
    }
}

fn verify_host_install_state(
    config: &HostConfig,
    desired_apply: &nas_csi_vm_manager::HostApplyPlan,
    _apply_options: &nas_csi_vm_manager::HostApplyPlanOptions,
    render_options: &nas_csi_vm_manager::ArtifactRenderOptions,
    actual: &nas_csi_vm_manager::HostActualState,
    health: &HostHealthReport,
    domains_started: bool,
) -> Result<()> {
    if health.status != "ok" {
        anyhow::bail!("host health is degraded after install");
    }

    let expected_seed_hashes = expected_seed_hashes(desired_apply);
    for node in &config.nodes {
        let root_disk = actual
            .paths
            .get(&node.root_disk.image)
            .ok_or_else(|| anyhow::anyhow!("missing root disk status for {}", node.name))?;
        if !root_disk.exists() {
            anyhow::bail!("root disk does not exist for {}", node.name);
        }
        if !actual.qemu_images.contains_key(&node.root_disk.image) {
            anyhow::bail!("root disk qemu-img info is unavailable for {}", node.name);
        }

        let seed_path = nas_csi_vm_manager::seed_image_path(&node.root_disk.image, &node.domain);
        let seed = actual
            .paths
            .get(&seed_path)
            .ok_or_else(|| anyhow::anyhow!("missing seed image status for {}", node.name))?;
        if !seed.exists() {
            anyhow::bail!("cloud-init seed image does not exist for {}", node.name);
        }
        if let Some(expected_hash) = expected_seed_hashes.get(&seed_path)
            && seed.content_hash.as_deref() != Some(expected_hash.as_str())
        {
            anyhow::bail!(
                "cloud-init seed image hash does not match desired content for {}",
                node.name
            );
        }

        let domain = actual
            .domains
            .get(&node.domain)
            .ok_or_else(|| anyhow::anyhow!("missing libvirt domain status for {}", node.name))?;
        if !domain.exists || !domain.managed {
            anyhow::bail!("libvirt domain {} is not present and managed", node.domain);
        }
        if node.autostart && domain.autostart != Some(true) {
            anyhow::bail!("libvirt domain {} autostart is not enabled", node.domain);
        }
        if domains_started && !domain.active {
            anyhow::bail!("libvirt domain {} is not running", node.domain);
        }

        for export_id in &node.exports {
            let unit_name = format!(
                "{}.service",
                nas_csi_vm_manager::virtiofsd_service_name(&node.domain, export_id)
            );
            let unit = actual
                .systemd_units
                .get(&unit_name)
                .ok_or_else(|| anyhow::anyhow!("missing systemd unit status for {unit_name}"))?;
            if unit.installed_hash.is_none()
                || unit.enabled != Some(true)
                || unit.active != Some(true)
            {
                anyhow::bail!("virtiofsd unit {unit_name} is not installed, enabled, and active");
            }

            let socket_path =
                nas_csi_vm_manager::virtiofs_socket_path(render_options, &node.domain, export_id);
            let (_, socket) = inspect_socket_path(&socket_path);
            if !socket {
                anyhow::bail!("virtiofs socket is not ready: {socket_path}");
            }
        }
    }

    Ok(())
}

fn expected_seed_hashes(
    desired_apply: &nas_csi_vm_manager::HostApplyPlan,
) -> BTreeMap<String, String> {
    desired_apply
        .steps
        .iter()
        .filter_map(|step| match &step.kind {
            nas_csi_vm_manager::ApplyStepKind::WriteBinaryFile { path, contents } => {
                Some((path.clone(), nas_csi_vm_manager::content_hash(contents)))
            }
            _ => None,
        })
        .collect()
}

fn verify_post_install_idempotence(plan: &nas_csi_vm_manager::HostReconcilePlan) -> Result<()> {
    let summary = summarize_reconcile_plan(plan);
    if summary.apply == 0 && summary.refuse == 0 {
        return Ok(());
    }

    anyhow::bail!(
        "host install was not idempotent after execute: apply={} refuse={}",
        summary.apply,
        summary.refuse
    )
}

#[derive(Debug)]
struct ClusterInstallOptions {
    artifact_dir: PathBuf,
    manifest_root: PathBuf,
    kubectl: String,
    reboot_node: Option<String>,
    post_reboot_check: bool,
    wait_timeout: Duration,
    execute: bool,
}

fn run_cluster_install(
    config: &HostConfig,
    install_options: ClusterInstallOptions,
    runner: &impl CommandRunner,
) -> Result<()> {
    if install_options.post_reboot_check && install_options.reboot_node.is_some() {
        anyhow::bail!("--post-reboot-check cannot be combined with --reboot-node");
    }

    let options = cluster_options_from_config(
        config,
        &install_options.artifact_dir,
        &install_options.kubectl,
    );
    let manifests = load_cluster_manifests(config, &install_options.manifest_root)?;
    verify_substrate_manifest_scope(&manifests)?;
    let actual = inspect_cluster_actual_state(config, &options, &manifests, runner)?;
    let plan =
        nas_csi_cluster_manager::plan_cluster_reconcile(config, &options, &actual, &manifests);
    verify_cluster_plan_order(config, &plan)?;
    log_cluster_reconcile_decisions(&plan);
    print_cluster_reconcile_plan(&plan);

    if install_options.post_reboot_check {
        print_cluster_status(config, &actual, &manifests);
        verify_cluster_install_state(config, &actual, &manifests, &options)?;
        verify_cluster_install_idempotence(&plan)?;
        println!("cluster install: post-reboot verification passed");
        return Ok(());
    }

    if !install_options.execute {
        println!("cluster install: dry run only; pass --execute to apply changes");
        return Ok(());
    }

    execute_cluster_reconcile_plan(&plan, &options, runner)?;
    let actual = inspect_cluster_actual_state(config, &options, &manifests, runner)?;
    print_cluster_status(config, &actual, &manifests);
    verify_cluster_install_state(config, &actual, &manifests, &options)?;
    let post_plan =
        nas_csi_cluster_manager::plan_cluster_reconcile(config, &options, &actual, &manifests);
    verify_cluster_install_idempotence(&post_plan)?;

    if let Some(node_name) = install_options.reboot_node.as_deref() {
        reboot_cluster_node(
            config,
            &options,
            runner,
            node_name,
            install_options.wait_timeout,
        )?;
        let actual = inspect_cluster_actual_state(config, &options, &manifests, runner)?;
        print_cluster_status(config, &actual, &manifests);
        verify_cluster_install_state(config, &actual, &manifests, &options)?;
    }

    println!("cluster install: completed cluster substrate verification");
    Ok(())
}

const LAB_CONTROLLER_IMAGE: &str = "ghcr.io/chris-arsenault/nas-csi-controller:0.1.0-lab1";
const LAB_NODE_IMAGE: &str = "ghcr.io/chris-arsenault/nas-csi-node:0.1.0-lab1";
const CSI_DRIVER_NAME: &str = "nas-csi.dev";
const EXISTING_DATASET_STORAGE_CLASS: &str = "nas-csi-existing";
const NODE_RUNTIME_CONFIG_PATH: &str = "/etc/nas-csi/node.yaml";
const MISSING_EXPORT_ID: &str = "nas-csi-missing-export";

#[derive(Debug)]
struct CsiInstallOptions {
    artifact_dir: PathBuf,
    manifest_root: PathBuf,
    kubectl: String,
    namespace: String,
    smoke_image: String,
    wait_timeout: Duration,
    execute: bool,
}

#[derive(Debug)]
struct WorkloadValidationOptions {
    artifact_dir: PathBuf,
    kubectl: String,
    namespace: String,
    repo_export: Option<String>,
    content_export: Option<String>,
    repo_image: String,
    content_image: String,
    content_command: String,
    wait_timeout: Duration,
    small_file_count: u32,
    keep_pods: bool,
    execute: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct WorkloadValidationSelection {
    repo_export: String,
    content_export: String,
}

fn run_csi_install(
    config: &HostConfig,
    install_options: CsiInstallOptions,
    runner: &impl CommandRunner,
) -> Result<()> {
    verify_csi_install_inputs(config, &install_options)?;

    let cluster_options = cluster_options_from_config(
        config,
        &install_options.artifact_dir,
        &install_options.kubectl,
    );
    let node_configs = render_node_runtime_configs(config)?;
    let nas_csi_manifest_path = install_options
        .manifest_root
        .join("kubernetes")
        .join("nas-csi")
        .join("nas-csi.yaml");
    let nas_csi_manifest = fs::read_to_string(&nas_csi_manifest_path).with_context(|| {
        format!(
            "failed to read nas-csi manifest {}",
            nas_csi_manifest_path.display()
        )
    })?;
    verify_nas_csi_manifest_uses_lab_images(&nas_csi_manifest)?;

    let csi_artifact_dir = install_options.artifact_dir.join("csi");
    let static_manifest =
        render_static_existing_dataset_manifest(config, &install_options.namespace);
    let static_manifest_path = csi_artifact_dir.join("static-existing-datasets.yaml");
    let smoke_manifest = render_csi_smoke_pod_manifest(config, &install_options);
    let smoke_manifest_path = csi_artifact_dir.join("smoke-pods.yaml");
    let missing_manifest = render_missing_export_manifest(config, &install_options);
    let missing_manifest_path = csi_artifact_dir.join("missing-export-probe.yaml");

    print_csi_install_plan(
        config,
        &install_options,
        &nas_csi_manifest_path,
        &static_manifest_path,
        &smoke_manifest_path,
    );

    if !install_options.execute {
        println!("csi install: dry run only; pass --execute to apply changes");
        return Ok(());
    }

    let before_datasets = observe_datasets(config)?;

    install_node_runtime_configs(config, &cluster_options, runner, &node_configs)?;
    verify_node_runtime_configs(config, &cluster_options, runner, &node_configs)?;

    apply_nas_csi_manifest(config, &install_options, runner, &nas_csi_manifest_path)?;
    wait_for_nas_csi_rollout(config, &install_options, runner)?;

    apply_generated_manifest(
        config,
        &install_options,
        runner,
        &static_manifest_path,
        &static_manifest,
        "static existing-dataset PV/PVC manifest",
    )?;
    verify_guest_virtiofs_mounts(config, &cluster_options, runner)?;

    apply_generated_manifest(
        config,
        &install_options,
        runner,
        &smoke_manifest_path,
        &smoke_manifest,
        "CSI smoke pod manifest",
    )?;
    wait_for_smoke_pods(config, &install_options, runner)?;
    verify_pod_mounts(config, &install_options, runner)?;
    verify_smoke_pod_restart(
        config,
        &install_options,
        runner,
        &smoke_manifest_path,
        &smoke_manifest,
    )?;
    verify_node_plugin_restart(config, &install_options, runner)?;
    verify_missing_export_fails_closed(
        config,
        &install_options,
        runner,
        &missing_manifest_path,
        &missing_manifest,
    )?;
    verify_read_only_exports_are_mounted_read_only(config, &install_options, runner)?;
    verify_pods_match_host_dataset_entries(config, &install_options, runner)?;

    let after_datasets = observe_datasets(config)?;
    verify_dataset_observations_stable(&before_datasets, &after_datasets)?;

    println!("csi install: completed static existing-dataset CSI verification");
    Ok(())
}

fn run_workload_validation(
    config: &HostConfig,
    options: WorkloadValidationOptions,
    runner: &impl CommandRunner,
) -> Result<()> {
    verify_workload_validation_inputs(config, &options)?;
    let selection = select_workload_validation_exports(config, &options)?;
    let cluster_options =
        cluster_options_from_config(config, &options.artifact_dir, &options.kubectl);
    let validation_dir = options.artifact_dir.join("workload-validation");
    let manifest_path = validation_dir.join("workload-pods.yaml");
    let report_path = validation_dir.join("report.txt");
    let manifest = render_workload_validation_manifest(config, &options, &selection)?;

    print_workload_validation_plan(config, &options, &selection, &manifest_path, &report_path);
    write_text_atomic_if_changed(&manifest_path.display().to_string(), &manifest)?;

    if !options.execute {
        write_text_atomic_if_changed(
            &report_path.display().to_string(),
            &render_workload_validation_dry_run_report(config, &options, &selection),
        )?;
        println!("workload validate: dry run only; pass --execute to run target-host validation");
        return Ok(());
    }

    let before_datasets = observe_datasets(config)?;
    let mut report = render_workload_validation_report_header(config, &options, &selection);
    let run_result = (|| -> Result<()> {
        apply_workload_validation_manifest(config, &options, runner, &manifest_path)?;
        wait_for_workload_pods(config, &options, runner, &selection)?;

        report.push_str("\n## virtiofsd before workloads\n");
        report.push_str(&capture_virtiofsd_observations(
            config,
            &options,
            &cluster_options,
            runner,
            &selection,
            "before",
        )?);

        report.push_str("\n## repository workload\n");
        report.push_str(&run_repository_workload(
            config, &options, runner, &selection,
        )?);
        report.push_str(&verify_export_visibility_from_host_write(
            config,
            &options,
            &cluster_options,
            runner,
            &selection.repo_export,
            &workload_repo_pod_name(&selection.repo_export),
            REPO_WORKLOAD_MOUNT_PATH,
            "repo-smb-visible",
        )?);

        report.push_str("\n## read-only content workload\n");
        report.push_str(&run_content_streaming_workload(
            config, &options, runner, &selection,
        )?);
        report.push_str(&verify_export_visibility_from_host_write(
            config,
            &options,
            &cluster_options,
            runner,
            &selection.content_export,
            &workload_content_pod_name(&selection.content_export),
            CONTENT_WORKLOAD_MOUNT_PATH,
            "content-smb-visible",
        )?);

        report.push_str("\n## virtiofsd restart behavior\n");
        report.push_str(&restart_workload_virtiofsd_service(
            config, &options, runner, &selection,
        )?);

        report.push_str("\n## virtiofsd after workloads\n");
        report.push_str(&capture_virtiofsd_observations(
            config,
            &options,
            &cluster_options,
            runner,
            &selection,
            "after",
        )?);

        verify_dataset_observations_stable(&before_datasets, &observe_datasets(config)?)?;
        Ok(())
    })();

    match run_result {
        Ok(()) => {
            report.push_str("\n## virtiofsd fork decision\n");
            report.push_str("decision: no fork is justified by this validation run\n");
            report.push_str(
                "reason: repo, SMB-visible dataset coherency, read-only streaming, and virtiofsd restart checks completed without a specific failing case\n",
            );
            write_text_atomic_if_changed(&report_path.display().to_string(), &report)?;
            if !options.keep_pods {
                delete_workload_validation_pods(config, &options, runner, &manifest_path)?;
            }
            println!(
                "workload validate: completed real workload validation; report={}",
                report_path.display()
            );
            Ok(())
        }
        Err(error) => {
            report.push_str("\n## virtiofsd fork decision\n");
            report.push_str("decision: do not fork from guesswork\n");
            report.push_str(&format!(
                "failingCase: {}\n",
                error.to_string().replace('\n', " ")
            ));
            report.push_str(
                "reason: a fork decision must be tied to the captured failing validation step above\n",
            );
            let _ = write_text_atomic_if_changed(&report_path.display().to_string(), &report);
            Err(error)
        }
    }
}

fn verify_workload_validation_inputs(
    config: &HostConfig,
    options: &WorkloadValidationOptions,
) -> Result<()> {
    if config.nodes.is_empty() {
        anyhow::bail!("workload validation requires at least one configured VM node");
    }
    if config.exports.is_empty() {
        anyhow::bail!("workload validation requires at least one configured export");
    }
    if options.namespace.trim().is_empty() {
        anyhow::bail!("workload validation namespace must not be empty");
    }
    if options.repo_image.trim().is_empty() {
        anyhow::bail!("workload validation repo image must not be empty");
    }
    if options.content_image.trim().is_empty() {
        anyhow::bail!("workload validation content image must not be empty");
    }
    if options.content_command.trim().is_empty() {
        anyhow::bail!("workload validation content command must not be empty");
    }
    if options.small_file_count == 0 {
        anyhow::bail!("workload validation small-file count must be greater than zero");
    }
    for export_id in config.exports.keys() {
        if node_for_export(config, export_id).is_none() {
            anyhow::bail!("export {export_id} is not assigned to any node");
        }
    }
    Ok(())
}

fn select_workload_validation_exports(
    config: &HostConfig,
    options: &WorkloadValidationOptions,
) -> Result<WorkloadValidationSelection> {
    let repo_export = select_workload_export(
        config,
        options.repo_export.as_deref(),
        AccessMode::ReadWrite,
    )
    .context("failed to select repository workload export")?;
    let content_export = select_workload_export(
        config,
        options.content_export.as_deref(),
        AccessMode::ReadOnly,
    )
    .context("failed to select read-only content workload export")?;
    Ok(WorkloadValidationSelection {
        repo_export,
        content_export,
    })
}

fn select_workload_export(
    config: &HostConfig,
    requested: Option<&str>,
    required_access: AccessMode,
) -> Result<String> {
    if let Some(export_id) = requested {
        let export = config
            .exports
            .get(export_id)
            .ok_or_else(|| anyhow::anyhow!("export {export_id} is not configured"))?;
        if export.access != required_access {
            anyhow::bail!(
                "export {export_id} has access {}, expected {}",
                export.access,
                required_access
            );
        }
        return Ok(export_id.to_string());
    }

    config
        .exports
        .iter()
        .find_map(|(export_id, export)| {
            (export.access == required_access).then(|| export_id.clone())
        })
        .ok_or_else(|| anyhow::anyhow!("no {required_access} export is configured"))
}

const REPO_WORKLOAD_MOUNT_PATH: &str = "/work/repo";
const CONTENT_WORKLOAD_MOUNT_PATH: &str = "/content";

fn render_workload_validation_manifest(
    config: &HostConfig,
    options: &WorkloadValidationOptions,
    selection: &WorkloadValidationSelection,
) -> Result<String> {
    let _repo_export = config
        .exports
        .get(&selection.repo_export)
        .ok_or_else(|| anyhow::anyhow!("missing export {}", selection.repo_export))?;
    let _content_export = config
        .exports
        .get(&selection.content_export)
        .ok_or_else(|| anyhow::anyhow!("missing export {}", selection.content_export))?;
    let repo_node = node_for_export(config, &selection.repo_export).ok_or_else(|| {
        anyhow::anyhow!("export {} is not assigned to a node", selection.repo_export)
    })?;
    let content_node = node_for_export(config, &selection.content_export).ok_or_else(|| {
        anyhow::anyhow!(
            "export {} is not assigned to a node",
            selection.content_export
        )
    })?;
    Ok(format!(
        "apiVersion: v1\nkind: Pod\nmetadata:\n  name: {}\n  namespace: {}\n  labels:\n    app.kubernetes.io/name: nas-csi\n    app.kubernetes.io/component: workload-validation\n    nas-csi.dev/workload: repo\n    nas-csi.dev/export-id: {}\nspec:\n  restartPolicy: Always\n  nodeName: {}\n  containers:\n    - name: repo\n      image: {}\n      command:\n        - /bin/sh\n        - -c\n        - {}\n      volumeMounts:\n        - name: dataset\n          mountPath: {REPO_WORKLOAD_MOUNT_PATH}\n          readOnly: false\n  volumes:\n    - name: dataset\n      persistentVolumeClaim:\n        claimName: {}\n        readOnly: false\n---\napiVersion: v1\nkind: Pod\nmetadata:\n  name: {}\n  namespace: {}\n  labels:\n    app.kubernetes.io/name: nas-csi\n    app.kubernetes.io/component: workload-validation\n    nas-csi.dev/workload: content\n    nas-csi.dev/export-id: {}\nspec:\n  restartPolicy: Always\n  nodeName: {}\n  containers:\n    - name: content\n      image: {}\n      command:\n        - /bin/sh\n        - -c\n        - {}\n      volumeMounts:\n        - name: dataset\n          mountPath: {CONTENT_WORKLOAD_MOUNT_PATH}\n          readOnly: true\n  volumes:\n    - name: dataset\n      persistentVolumeClaim:\n        claimName: {}\n        readOnly: true\n",
        workload_repo_pod_name(&selection.repo_export),
        yaml_quote(&options.namespace),
        yaml_quote(&selection.repo_export),
        yaml_quote(&repo_node.name),
        yaml_quote(&options.repo_image),
        yaml_quote("trap : TERM INT; sleep 2147483647 & wait"),
        existing_dataset_resource_name(&selection.repo_export),
        workload_content_pod_name(&selection.content_export),
        yaml_quote(&options.namespace),
        yaml_quote(&selection.content_export),
        yaml_quote(&content_node.name),
        yaml_quote(&options.content_image),
        yaml_quote(&options.content_command),
        existing_dataset_resource_name(&selection.content_export),
    ))
}

fn print_workload_validation_plan(
    config: &HostConfig,
    options: &WorkloadValidationOptions,
    selection: &WorkloadValidationSelection,
    manifest_path: &Path,
    report_path: &Path,
) {
    let repo = &config.exports[&selection.repo_export];
    let content = &config.exports[&selection.content_export];
    println!("workload validation plan");
    println!();
    println!("namespace: {}", options.namespace);
    println!("manifest: {}", manifest_path.display());
    println!("report: {}", report_path.display());
    println!(
        "repo export: {} dataset={} source={} image={} smallFiles={}",
        selection.repo_export,
        repo.dataset,
        repo.source_path,
        options.repo_image,
        options.small_file_count
    );
    println!(
        "content export: {} dataset={} source={} image={}",
        selection.content_export, content.dataset, content.source_path, options.content_image
    );
    println!("content command: {}", options.content_command);
}

fn render_workload_validation_dry_run_report(
    config: &HostConfig,
    options: &WorkloadValidationOptions,
    selection: &WorkloadValidationSelection,
) -> String {
    let mut report = render_workload_validation_report_header(config, options, selection);
    report.push_str("\n## dry run\n");
    report.push_str("execute: false\n");
    report.push_str("status: artifacts rendered; no target-host workload validation was run\n");
    report
}

fn render_workload_validation_report_header(
    config: &HostConfig,
    options: &WorkloadValidationOptions,
    selection: &WorkloadValidationSelection,
) -> String {
    let repo = &config.exports[&selection.repo_export];
    let content = &config.exports[&selection.content_export];
    format!(
        "# nas-csi workload validation report\n\ncluster: {}\nnamespace: {}\nrepoExport: {}\nrepoDataset: {}\nrepoSource: {}\ncontentExport: {}\ncontentDataset: {}\ncontentSource: {}\nsmallFileCount: {}\n",
        config.cluster.name,
        options.namespace,
        selection.repo_export,
        repo.dataset,
        repo.source_path,
        selection.content_export,
        content.dataset,
        content.source_path,
        options.small_file_count
    )
}

fn apply_workload_validation_manifest(
    config: &HostConfig,
    options: &WorkloadValidationOptions,
    runner: &impl CommandRunner,
    manifest_path: &Path,
) -> Result<()> {
    let command = kubectl_command(
        config,
        &options.kubectl,
        [
            "apply".to_string(),
            "-f".to_string(),
            manifest_path.display().to_string(),
        ],
    );
    run_cluster_command(runner, &command).context("failed to apply workload validation pods")?;
    println!(
        "workload validate: applied validation pods {}",
        manifest_path.display()
    );
    Ok(())
}

fn wait_for_workload_pods(
    config: &HostConfig,
    options: &WorkloadValidationOptions,
    runner: &impl CommandRunner,
    selection: &WorkloadValidationSelection,
) -> Result<()> {
    for pod in [
        workload_repo_pod_name(&selection.repo_export),
        workload_content_pod_name(&selection.content_export),
    ] {
        let command = kubectl_command(
            config,
            &options.kubectl,
            [
                "-n".to_string(),
                options.namespace.clone(),
                "wait".to_string(),
                format!("pod/{pod}"),
                "--for=condition=Ready".to_string(),
                format!("--timeout={}s", options.wait_timeout.as_secs()),
            ],
        );
        wait_for_cluster_command_with_timeout(runner, &command, options.wait_timeout)
            .with_context(|| format!("timed out waiting for workload pod {pod}"))?;
        println!("workload validate: pod {pod} is Ready");
    }
    Ok(())
}

fn run_repository_workload(
    config: &HostConfig,
    options: &WorkloadValidationOptions,
    runner: &impl CommandRunner,
    selection: &WorkloadValidationSelection,
) -> Result<String> {
    let pod = workload_repo_pod_name(&selection.repo_export);
    let output = run_workload_pod_script(
        config,
        options,
        runner,
        &pod,
        repository_workload_script(),
        [
            REPO_WORKLOAD_MOUNT_PATH.to_string(),
            options.small_file_count.to_string(),
        ],
    )
    .context("repository workload validation failed")?;
    println!("workload validate: repository workload completed in pod {pod}");
    Ok(format!("{output}\n"))
}

fn run_content_streaming_workload(
    config: &HostConfig,
    options: &WorkloadValidationOptions,
    runner: &impl CommandRunner,
    selection: &WorkloadValidationSelection,
) -> Result<String> {
    let pod = workload_content_pod_name(&selection.content_export);
    let output = run_workload_pod_script(
        config,
        options,
        runner,
        &pod,
        content_streaming_workload_script(),
        [CONTENT_WORKLOAD_MOUNT_PATH.to_string()],
    )
    .context("read-only content streaming workload validation failed")?;
    println!("workload validate: content streaming workload completed in pod {pod}");
    Ok(format!("{output}\n"))
}

fn run_workload_pod_script<I>(
    config: &HostConfig,
    options: &WorkloadValidationOptions,
    runner: &impl CommandRunner,
    pod: &str,
    script: &str,
    args: I,
) -> Result<String>
where
    I: IntoIterator<Item = String>,
{
    let mut command_args = vec![
        "/bin/sh".to_string(),
        "-c".to_string(),
        script.to_string(),
        "nas-csi-workload".to_string(),
    ];
    command_args.extend(args);
    let command = kubectl_exec_command_in_namespace(
        config,
        &options.kubectl,
        &options.namespace,
        pod,
        command_args,
    );
    let vm_command = cluster_command_to_vm_command(&command);
    let output = runner.output(&vm_command)?.ok_or_else(|| {
        anyhow::anyhow!("workload command failed or returned no output: {command}")
    })?;
    Ok(output)
}

fn verify_export_visibility_from_host_write(
    config: &HostConfig,
    options: &WorkloadValidationOptions,
    cluster_options: &ClusterReconcileOptions,
    runner: &impl CommandRunner,
    export_id: &str,
    pod: &str,
    pod_mount_path: &str,
    label: &str,
) -> Result<String> {
    let export = config
        .exports
        .get(export_id)
        .ok_or_else(|| anyhow::anyhow!("missing export {export_id}"))?;
    let relative_path = format!(
        ".nas-csi-validation/{}-{}.txt",
        safe_mount_segment(label),
        validation_run_id()
    );
    let host_path = Path::new(&export.source_path).join(&relative_path);
    let contents = format!(
        "nas-csi workload validation\nexport={export_id}\nlabel={label}\nrun={}\n",
        validation_run_id()
    );
    write_text_atomic_if_changed(&host_path.display().to_string(), &contents).with_context(
        || {
            format!(
                "failed to write validation sentinel {}",
                host_path.display()
            )
        },
    )?;

    let result = (|| -> Result<String> {
        let mut verified_nodes = 0_usize;
        for node in config.nodes.iter().filter(|node| {
            node.exports
                .iter()
                .any(|node_export| node_export == export_id)
        }) {
            let guest_path = format!(
                "{}/{}",
                nas_csi_vm_manager::guest_virtiofs_mount_path(export_id),
                relative_path
            );
            let actual = guest_exec_output(
                runner,
                cluster_options,
                &node.domain,
                &GuestCommandSpec::new("/bin/cat".to_string(), [guest_path.clone()]),
            )
            .with_context(|| {
                format!(
                    "guest {} did not observe validation sentinel {}",
                    node.name, guest_path
                )
            })?;
            if actual != contents {
                anyhow::bail!(
                    "guest {} saw different sentinel contents for export {export_id}",
                    node.name
                );
            }
            verified_nodes += 1;
        }

        let pod_path = format!("{pod_mount_path}/{relative_path}");
        let command = kubectl_exec_command_in_namespace(
            config,
            &options.kubectl,
            &options.namespace,
            pod,
            ["/bin/cat".to_string(), pod_path.clone()],
        );
        let pod_contents = runner
            .output(&cluster_command_to_vm_command(&command))?
            .ok_or_else(|| {
                anyhow::anyhow!("pod {pod} did not observe validation sentinel {pod_path}")
            })?;
        if pod_contents != contents {
            anyhow::bail!("pod {pod} saw different sentinel contents for export {export_id}");
        }

        println!(
            "workload validate: verified {label} host/guest/pod visibility for export {export_id}"
        );
        Ok(format!(
            "{label}: relativePath={relative_path} verifiedGuestNodes={verified_nodes} verifiedPod={pod}\n"
        ))
    })();

    let _ = fs::remove_file(&host_path);
    if let Some(parent) = host_path.parent() {
        let _ = fs::remove_dir(parent);
    }

    result
}

fn capture_virtiofsd_observations(
    config: &HostConfig,
    options: &WorkloadValidationOptions,
    cluster_options: &ClusterReconcileOptions,
    runner: &impl CommandRunner,
    selection: &WorkloadValidationSelection,
    phase: &str,
) -> Result<String> {
    let render_options = render_options_from_config(config);
    let mut export_ids = BTreeSet::new();
    export_ids.insert(selection.repo_export.clone());
    export_ids.insert(selection.content_export.clone());
    let mut report = String::new();
    for export_id in export_ids {
        let export = config
            .exports
            .get(&export_id)
            .ok_or_else(|| anyhow::anyhow!("missing export {export_id}"))?;
        for node in config.nodes.iter().filter(|node| {
            node.exports
                .iter()
                .any(|node_export| node_export == &export_id)
        }) {
            let unit = format!(
                "{}.service",
                nas_csi_vm_manager::virtiofsd_service_name(&node.domain, &export_id)
            );
            let socket_path =
                nas_csi_vm_manager::virtiofs_socket_path(&render_options, &node.domain, &export_id);
            let systemd = command_output(
                runner,
                &config.host_tools.systemctl,
                [
                    "show",
                    &unit,
                    "-p",
                    "ActiveState",
                    "-p",
                    "SubState",
                    "-p",
                    "Result",
                    "-p",
                    "MainPID",
                    "-p",
                    "NRestarts",
                    "-p",
                    "MemoryCurrent",
                    "-p",
                    "CPUUsageNSec",
                    "-p",
                    "ExecMainStatus",
                    "--no-pager",
                ],
            )?
            .unwrap_or_else(|| "systemctl show unavailable".to_string());
            let socket_status = match fs::metadata(&socket_path) {
                Ok(metadata) if metadata.file_type().is_socket() => "socket",
                Ok(_) => "non-socket",
                Err(error) if error.kind() == ErrorKind::NotFound => "missing",
                Err(_) => "error",
            };
            let domain = inspect_domain(
                runner,
                &cluster_options.virsh_path,
                &cluster_options.libvirt_uri,
                &node.domain,
            )?;
            let cache_policy = domain
                .xml
                .as_deref()
                .map(|xml| extract_virtiofs_cache_policy(xml, &export.tag))
                .unwrap_or_else(|| "domain-xml-unavailable".to_string());
            let mount_path = nas_csi_vm_manager::guest_virtiofs_mount_path(&export_id);
            let mountinfo = guest_exec_output(
                runner,
                cluster_options,
                &node.domain,
                &GuestCommandSpec::new(
                    "/bin/sh".to_string(),
                    [
                        "-c".to_string(),
                        "awk -v mp=\"$1\" '$5 == mp { print }' /proc/self/mountinfo".to_string(),
                        "nas-csi-mountinfo".to_string(),
                        mount_path.clone(),
                    ],
                ),
            )
            .unwrap_or_else(|error| format!("mountinfo unavailable: {error}"));
            report.push_str(&format!(
                "phase={phase} export={export_id} node={} unit={unit} socket={} socketStatus={socket_status} cachePolicy={cache_policy}\n",
                node.name, socket_path
            ));
            for line in systemd.lines() {
                report.push_str(&format!("systemd.{unit}.{line}\n"));
            }
            let mountinfo = mountinfo.replace('\n', " | ");
            report.push_str(&format!(
                "guestMount export={export_id} node={} mountPath={mount_path} mountinfo={mountinfo}\n",
                node.name
            ));
        }
    }
    let _ = options;
    Ok(report)
}

fn restart_workload_virtiofsd_service(
    config: &HostConfig,
    options: &WorkloadValidationOptions,
    runner: &impl CommandRunner,
    selection: &WorkloadValidationSelection,
) -> Result<String> {
    let node = node_for_export(config, &selection.repo_export)
        .ok_or_else(|| anyhow::anyhow!("repo export is not assigned to a node"))?;
    let unit = format!(
        "{}.service",
        nas_csi_vm_manager::virtiofsd_service_name(&node.domain, &selection.repo_export)
    );
    let command = nas_csi_vm_manager::CommandSpec::new(
        config.host_tools.systemctl.clone(),
        ["restart".to_string(), unit.clone()],
    );
    runner
        .run(&command)
        .with_context(|| format!("failed to restart virtiofsd unit {unit}"))?;

    let socket_path = nas_csi_vm_manager::virtiofs_socket_path(
        &render_options_from_config(config),
        &node.domain,
        &selection.repo_export,
    );
    wait_for_virtiofs_socket_with_timeout(&socket_path, options.wait_timeout)
        .with_context(|| format!("virtiofsd socket did not recover for {unit}"))?;
    verify_workload_mount_ready(
        config,
        options,
        runner,
        &workload_repo_pod_name(&selection.repo_export),
        REPO_WORKLOAD_MOUNT_PATH,
    )?;
    verify_workload_mount_ready(
        config,
        options,
        runner,
        &workload_content_pod_name(&selection.content_export),
        CONTENT_WORKLOAD_MOUNT_PATH,
    )?;
    println!("workload validate: restarted {unit} and verified workload pods still read mounts");
    Ok(format!(
        "restartedUnit={unit} socket={socket_path} result=workload-mounts-readable\n"
    ))
}

fn verify_workload_mount_ready(
    config: &HostConfig,
    options: &WorkloadValidationOptions,
    runner: &impl CommandRunner,
    pod: &str,
    mount_path: &str,
) -> Result<()> {
    let command = kubectl_exec_command_in_namespace(
        config,
        &options.kubectl,
        &options.namespace,
        pod,
        [
            "/bin/sh".to_string(),
            "-c".to_string(),
            "test -d \"$1\" && ls -A \"$1\" >/dev/null 2>&1".to_string(),
            "nas-csi-mount-ready".to_string(),
            mount_path.to_string(),
        ],
    );
    if !runner.status(&cluster_command_to_vm_command(&command))? {
        anyhow::bail!("workload pod {pod} cannot read mount {mount_path}");
    }
    Ok(())
}

fn delete_workload_validation_pods(
    config: &HostConfig,
    options: &WorkloadValidationOptions,
    runner: &impl CommandRunner,
    manifest_path: &Path,
) -> Result<()> {
    let command = kubectl_command(
        config,
        &options.kubectl,
        [
            "delete".to_string(),
            "-f".to_string(),
            manifest_path.display().to_string(),
            "--ignore-not-found=true".to_string(),
            "--wait=true".to_string(),
        ],
    );
    run_cluster_command(runner, &command).context("failed to delete workload validation pods")?;
    println!("workload validate: deleted validation pods");
    Ok(())
}

fn repository_workload_script() -> &'static str {
    r#"set -eu
mount_path="$1"
small_file_count="$2"
test -d "$mount_path"
echo "repoMount=$mount_path"
if command -v git >/dev/null 2>&1; then
    git_dir=""
    if [ -d "$mount_path/.git" ]; then
        git_dir="$mount_path/.git"
    else
        git_dir="$(find "$mount_path" -mindepth 2 -maxdepth 4 -type d -name .git -print -quit 2>/dev/null || true)"
    fi
    if [ -n "$git_dir" ]; then
        repo="${git_dir%/.git}"
        git -C "$repo" status --short >/tmp/nas-csi-git-status.txt
        echo "gitStatusRepo=$repo"
    else
        echo "gitStatusRepo=none-found"
    fi
else
    echo "gitStatusRepo=git-missing"
fi
if command -v npm >/dev/null 2>&1; then
    package_file="$(find "$mount_path" -maxdepth 4 -type f -name package.json -print -quit 2>/dev/null || true)"
    if [ -n "$package_file" ]; then
        project="$(dirname "$package_file")"
        rm -rf /tmp/nas-csi-npm
        mkdir -p /tmp/nas-csi-npm
        cp -a "$project"/. /tmp/nas-csi-npm/
        cd /tmp/nas-csi-npm
        npm install --ignore-scripts --no-audit --no-fund
        npm run build --if-present
        echo "npmProject=$project"
    else
        echo "npmProject=none-found"
    fi
else
    echo "npmProject=npm-missing"
fi
scratch="$mount_path/.nas-csi-validation/repo-small-files-$$"
rm -rf "$scratch"
mkdir -p "$scratch"
i=0
while [ "$i" -lt "$small_file_count" ]; do
    printf 'nas-csi small-file validation %s\n' "$i" > "$scratch/file-$i.txt"
    i=$((i + 1))
done
count="$(find "$scratch" -type f | wc -l | tr -d ' ')"
test "$count" = "$small_file_count"
rm -rf "$scratch"
test ! -e "$scratch"
echo "smallFilesWritten=$count"
"#
}

fn content_streaming_workload_script() -> &'static str {
    r#"set -eu
mount_path="$1"
test -d "$mount_path"
echo "contentMount=$mount_path"
first_file="$(find "$mount_path" -type f -print -quit 2>/dev/null || true)"
if [ -n "$first_file" ]; then
    bytes="$(wc -c < "$first_file" | tr -d ' ')"
    dd if="$first_file" of=/dev/null bs=1048576 count=16 2>/tmp/nas-csi-dd.log || true
    echo "sampleFile=$first_file"
    echo "sampleBytes=$bytes"
else
    echo "sampleFile=none-found"
fi
if command -v wget >/dev/null 2>&1; then
    wget -qO- http://127.0.0.1:8080/ >/tmp/nas-csi-content-index
    echo "httpProbe=ok"
else
    echo "httpProbe=wget-missing"
fi
find "$mount_path" -maxdepth 1 -print >/tmp/nas-csi-content-list
echo "contentStreaming=ok"
"#
}

fn workload_repo_pod_name(export_id: &str) -> String {
    safe_k8s_name("nas-csi-workload-repo", export_id)
}

fn workload_content_pod_name(export_id: &str) -> String {
    safe_k8s_name("nas-csi-workload-content", export_id)
}

fn validation_run_id() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default();
    format!("{}-{now}", process::id())
}

fn extract_virtiofs_cache_policy(xml: &str, tag: &str) -> String {
    let single_quoted_target = format!("dir='{tag}'");
    let double_quoted_target = format!("dir=\"{tag}\"");
    for block in xml.split("<filesystem").skip(1) {
        let filesystem = block.split("</filesystem>").next().unwrap_or(block);
        if !filesystem.contains(&single_quoted_target)
            && !filesystem.contains(&double_quoted_target)
        {
            continue;
        }
        return xml_attr_value(filesystem, "cache").unwrap_or_else(|| "not-set".to_string());
    }
    "not-found".to_string()
}

fn xml_attr_value(xml: &str, name: &str) -> Option<String> {
    let single = format!("{name}='");
    if let Some(start) = xml.find(&single) {
        let value_start = start + single.len();
        return xml[value_start..]
            .find('\'')
            .map(|end| xml[value_start..value_start + end].to_string());
    }
    let double = format!("{name}=\"");
    if let Some(start) = xml.find(&double) {
        let value_start = start + double.len();
        return xml[value_start..]
            .find('"')
            .map(|end| xml[value_start..value_start + end].to_string());
    }
    None
}

fn verify_csi_install_inputs(config: &HostConfig, options: &CsiInstallOptions) -> Result<()> {
    if config.nodes.is_empty() {
        anyhow::bail!("csi install requires at least one configured VM node");
    }
    if config.exports.is_empty() {
        anyhow::bail!("csi install requires at least one configured export");
    }
    if options.namespace.trim().is_empty() {
        anyhow::bail!("csi install namespace must not be empty");
    }
    if options.smoke_image.trim().is_empty() {
        anyhow::bail!("csi install smoke image must not be empty");
    }

    for export_id in config.exports.keys() {
        if node_for_export(config, export_id).is_none() {
            anyhow::bail!("export {export_id} is not assigned to any node");
        }
    }

    Ok(())
}

fn render_node_runtime_configs(config: &HostConfig) -> Result<BTreeMap<String, String>> {
    let mut configs = BTreeMap::new();
    for node in &config.nodes {
        configs.insert(
            node.name.clone(),
            nas_csi_vm_manager::render_node_runtime_config(config, node)?,
        );
    }
    Ok(configs)
}

fn install_node_runtime_configs(
    config: &HostConfig,
    options: &ClusterReconcileOptions,
    runner: &impl CommandRunner,
    node_configs: &BTreeMap<String, String>,
) -> Result<()> {
    for node in &config.nodes {
        let contents = node_configs
            .get(&node.name)
            .ok_or_else(|| anyhow::anyhow!("missing rendered node config for {}", node.name))?;
        guest_write_text_file(
            runner,
            options,
            &node.domain,
            NODE_RUNTIME_CONFIG_PATH,
            contents,
            0o644,
        )
        .with_context(|| format!("failed to install node runtime config on {}", node.name))?;
        println!(
            "csi install: installed {} on {}",
            NODE_RUNTIME_CONFIG_PATH, node.name
        );
    }
    Ok(())
}

fn guest_write_text_file(
    runner: &impl CommandRunner,
    options: &ClusterReconcileOptions,
    domain: &str,
    path: &str,
    contents: &str,
    mode: u32,
) -> Result<()> {
    let parent = parent_dir_for_path(path);
    let encoded = base64::engine::general_purpose::STANDARD.encode(contents.as_bytes());
    let script = format!(
        "set -eu\numask 022\ninstall -d -m 0755 {}\nbase64 -d > {} <<'NAS_CSI_CONTENT'\n{}\nNAS_CSI_CONTENT\nchmod {:04o} {}\nsync {} 2>/dev/null || sync\n",
        shell_quote(&parent),
        shell_quote(path),
        encoded,
        mode,
        shell_quote(path),
        shell_quote(path)
    );
    let command = GuestCommandSpec::new("/bin/sh".to_string(), ["-c".to_string(), script]);
    guest_exec_output(runner, options, domain, &command)?;
    Ok(())
}

fn verify_node_runtime_configs(
    config: &HostConfig,
    options: &ClusterReconcileOptions,
    runner: &impl CommandRunner,
    node_configs: &BTreeMap<String, String>,
) -> Result<()> {
    for node in &config.nodes {
        let expected = node_configs
            .get(&node.name)
            .ok_or_else(|| anyhow::anyhow!("missing rendered node config for {}", node.name))?;
        let actual = guest_exec_output(
            runner,
            options,
            &node.domain,
            &GuestCommandSpec::new(
                "/bin/cat".to_string(),
                [NODE_RUNTIME_CONFIG_PATH.to_string()],
            ),
        )
        .with_context(|| format!("failed to read node runtime config on {}", node.name))?;
        if actual != *expected {
            anyhow::bail!(
                "{} on {} does not match host-local config",
                NODE_RUNTIME_CONFIG_PATH,
                node.name
            );
        }
        println!(
            "csi install: verified {} on {}",
            NODE_RUNTIME_CONFIG_PATH, node.name
        );
    }
    Ok(())
}

fn apply_nas_csi_manifest(
    config: &HostConfig,
    options: &CsiInstallOptions,
    runner: &impl CommandRunner,
    manifest_path: &Path,
) -> Result<()> {
    let path = manifest_path.display().to_string();
    let command = csi_kubectl_command(
        config,
        options,
        [
            "apply".to_string(),
            "--server-side".to_string(),
            "-f".to_string(),
            path.clone(),
        ],
    );
    run_cluster_command(runner, &command)
        .with_context(|| format!("failed to apply nas-csi manifest {path}"))?;
    println!("csi install: applied nas-csi manifest {path}");
    Ok(())
}

fn apply_generated_manifest(
    config: &HostConfig,
    options: &CsiInstallOptions,
    runner: &impl CommandRunner,
    path: &Path,
    contents: &str,
    label: &str,
) -> Result<()> {
    let path_string = path.display().to_string();
    write_text_atomic_if_changed(&path_string, contents)?;
    let command = csi_kubectl_command(
        config,
        options,
        ["apply".to_string(), "-f".to_string(), path_string.clone()],
    );
    run_cluster_command(runner, &command).with_context(|| format!("failed to apply {label}"))?;
    println!("csi install: applied {label} {path_string}");
    Ok(())
}

fn wait_for_nas_csi_rollout(
    config: &HostConfig,
    options: &CsiInstallOptions,
    runner: &impl CommandRunner,
) -> Result<()> {
    for resource in ["deployment/nas-csi-controller", "daemonset/nas-csi-node"] {
        let command = csi_kubectl_command(
            config,
            options,
            [
                "-n".to_string(),
                "kube-system".to_string(),
                "rollout".to_string(),
                "status".to_string(),
                resource.to_string(),
                format!("--timeout={}s", options.wait_timeout.as_secs()),
            ],
        );
        wait_for_cluster_command_with_timeout(runner, &command, options.wait_timeout)
            .with_context(|| format!("timed out waiting for {resource} rollout"))?;
        println!("csi install: {resource} rollout is complete");
    }
    Ok(())
}

fn verify_guest_virtiofs_mounts(
    config: &HostConfig,
    options: &ClusterReconcileOptions,
    runner: &impl CommandRunner,
) -> Result<()> {
    for node in &config.nodes {
        for export_id in &node.exports {
            let export = config
                .exports
                .get(export_id)
                .ok_or_else(|| anyhow::anyhow!("missing export {export_id}"))?;
            let mount_path = nas_csi_vm_manager::guest_virtiofs_mount_path(export_id);
            let command = GuestCommandSpec::new(
                "/bin/sh".to_string(),
                [
                    "-c".to_string(),
                    virtiofs_mount_verify_script().to_string(),
                    "nas-csi-verify".to_string(),
                    mount_path.clone(),
                    export.tag.clone(),
                ],
            );
            if !guest_command_success(runner, options, &node.domain, &command)? {
                anyhow::bail!(
                    "guest virtiofs mount for export {export_id} on {} is not mounted from tag {} at {}",
                    node.name,
                    export.tag,
                    mount_path
                );
            }
            println!(
                "csi install: verified guest virtiofs mount {}/{} at {}",
                node.name, export_id, mount_path
            );
        }
    }
    Ok(())
}

fn wait_for_smoke_pods(
    config: &HostConfig,
    options: &CsiInstallOptions,
    runner: &impl CommandRunner,
) -> Result<()> {
    for export_id in config.exports.keys() {
        let pod = smoke_pod_name(export_id);
        let command = csi_kubectl_command(
            config,
            options,
            [
                "-n".to_string(),
                options.namespace.clone(),
                "wait".to_string(),
                format!("pod/{pod}"),
                "--for=condition=Ready".to_string(),
                format!("--timeout={}s", options.wait_timeout.as_secs()),
            ],
        );
        wait_for_cluster_command_with_timeout(runner, &command, options.wait_timeout)
            .with_context(|| format!("timed out waiting for smoke pod {pod}"))?;
        println!("csi install: smoke pod {pod} is Ready");
    }
    Ok(())
}

fn verify_pod_mounts(
    config: &HostConfig,
    options: &CsiInstallOptions,
    runner: &impl CommandRunner,
) -> Result<()> {
    for (export_id, export) in &config.exports {
        let pod = smoke_pod_name(export_id);
        let mount_path = smoke_mount_path(export_id);
        let command = kubectl_exec_command(
            config,
            options,
            &pod,
            [
                "/bin/sh".to_string(),
                "-c".to_string(),
                virtiofs_mount_verify_script().to_string(),
                "nas-csi-verify".to_string(),
                mount_path.clone(),
                export.tag.clone(),
            ],
        );
        if !runner.status(&cluster_command_to_vm_command(&command))? {
            anyhow::bail!(
                "smoke pod {pod} does not have export {export_id} bind-mounted at {mount_path}"
            );
        }
        println!("csi install: verified pod mount {pod}:{mount_path}");
    }
    Ok(())
}

fn verify_smoke_pod_restart(
    config: &HostConfig,
    options: &CsiInstallOptions,
    runner: &impl CommandRunner,
    smoke_manifest_path: &Path,
    smoke_manifest: &str,
) -> Result<()> {
    let path_string = smoke_manifest_path.display().to_string();
    let delete_command = csi_kubectl_command(
        config,
        options,
        [
            "delete".to_string(),
            "-f".to_string(),
            path_string.clone(),
            "--ignore-not-found=true".to_string(),
            "--wait=true".to_string(),
        ],
    );
    run_cluster_command(runner, &delete_command).context("failed to delete CSI smoke pods")?;
    apply_generated_manifest(
        config,
        options,
        runner,
        smoke_manifest_path,
        smoke_manifest,
        "CSI smoke pod manifest",
    )?;
    wait_for_smoke_pods(config, options, runner)?;
    verify_pod_mounts(config, options, runner)?;
    println!("csi install: verified smoke pod restart behavior");
    Ok(())
}

fn verify_node_plugin_restart(
    config: &HostConfig,
    options: &CsiInstallOptions,
    runner: &impl CommandRunner,
) -> Result<()> {
    let restart = csi_kubectl_command(
        config,
        options,
        [
            "-n".to_string(),
            "kube-system".to_string(),
            "rollout".to_string(),
            "restart".to_string(),
            "daemonset/nas-csi-node".to_string(),
        ],
    );
    run_cluster_command(runner, &restart).context("failed to restart nas-csi node DaemonSet")?;
    let status = csi_kubectl_command(
        config,
        options,
        [
            "-n".to_string(),
            "kube-system".to_string(),
            "rollout".to_string(),
            "status".to_string(),
            "daemonset/nas-csi-node".to_string(),
            format!("--timeout={}s", options.wait_timeout.as_secs()),
        ],
    );
    wait_for_cluster_command_with_timeout(runner, &status, options.wait_timeout)
        .context("timed out waiting for nas-csi node DaemonSet after restart")?;
    verify_pod_mounts(config, options, runner)?;
    println!("csi install: verified node plugin restart behavior");
    Ok(())
}

fn verify_missing_export_fails_closed(
    config: &HostConfig,
    options: &CsiInstallOptions,
    runner: &impl CommandRunner,
    missing_manifest_path: &Path,
    missing_manifest: &str,
) -> Result<()> {
    apply_generated_manifest(
        config,
        options,
        runner,
        missing_manifest_path,
        missing_manifest,
        "missing export fail-closed probe manifest",
    )?;
    let pod = missing_export_pod_name();
    let wait = csi_kubectl_command(
        config,
        options,
        [
            "-n".to_string(),
            options.namespace.clone(),
            "wait".to_string(),
            format!("pod/{pod}"),
            "--for=condition=Ready".to_string(),
            "--timeout=30s".to_string(),
        ],
    );
    let became_ready = runner.status(&cluster_command_to_vm_command(&wait))?;
    let delete = csi_kubectl_command(
        config,
        options,
        [
            "delete".to_string(),
            "-f".to_string(),
            missing_manifest_path.display().to_string(),
            "--ignore-not-found=true".to_string(),
            "--wait=true".to_string(),
        ],
    );
    let _ = runner.status(&cluster_command_to_vm_command(&delete));
    if became_ready {
        anyhow::bail!("missing export probe pod unexpectedly became Ready");
    }
    println!("csi install: verified missing virtiofs export fails closed");
    Ok(())
}

fn verify_read_only_exports_are_mounted_read_only(
    config: &HostConfig,
    options: &CsiInstallOptions,
    runner: &impl CommandRunner,
) -> Result<()> {
    for (export_id, export) in &config.exports {
        if export.access != AccessMode::ReadOnly {
            continue;
        }
        let pod = smoke_pod_name(export_id);
        let mount_path = smoke_mount_path(export_id);
        let command = kubectl_exec_command(
            config,
            options,
            &pod,
            [
                "/bin/sh".to_string(),
                "-c".to_string(),
                readonly_mount_verify_script().to_string(),
                "nas-csi-verify".to_string(),
                mount_path.clone(),
            ],
        );
        if !runner.status(&cluster_command_to_vm_command(&command))? {
            anyhow::bail!("read-only export {export_id} is not mounted read-only in pod {pod}");
        }
        println!("csi install: verified read-only policy for {export_id} at {pod}:{mount_path}");
    }
    Ok(())
}

fn verify_pods_match_host_dataset_entries(
    config: &HostConfig,
    options: &CsiInstallOptions,
    runner: &impl CommandRunner,
) -> Result<()> {
    for (export_id, export) in &config.exports {
        let host_entries = host_top_level_entries(&export.source_path)
            .with_context(|| format!("failed to list host dataset {}", export.source_path))?;
        let pod_entries = pod_top_level_entries(config, options, runner, export_id)?;
        if host_entries != pod_entries {
            let diff = first_entry_diff(&host_entries, &pod_entries)
                .unwrap_or_else(|| "entry lists differ".to_string());
            anyhow::bail!(
                "pod view for export {export_id} does not match host dataset {}; hostEntries={} podEntries={} firstDiff={diff}",
                export.source_path,
                host_entries.len(),
                pod_entries.len()
            );
        }
        println!("csi install: verified pod and host dataset entries match for {export_id}");
    }
    Ok(())
}

fn pod_top_level_entries(
    config: &HostConfig,
    options: &CsiInstallOptions,
    runner: &impl CommandRunner,
    export_id: &str,
) -> Result<Vec<String>> {
    let pod = smoke_pod_name(export_id);
    let mount_path = smoke_mount_path(export_id);
    let command = kubectl_exec_command(
        config,
        options,
        &pod,
        [
            "/bin/sh".to_string(),
            "-c".to_string(),
            "cd \"$1\" && ls -A | sort".to_string(),
            "nas-csi-list".to_string(),
            mount_path,
        ],
    );
    let vm_command = cluster_command_to_vm_command(&command);
    if !runner.status(&vm_command)? {
        anyhow::bail!("failed to list pod mount for export {export_id}");
    }
    let output = runner.output(&vm_command)?.unwrap_or_default();
    Ok(parse_listing_output(&output))
}

fn host_top_level_entries(path: &str) -> Result<Vec<String>> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(path).with_context(|| format!("failed to read directory {path}"))? {
        let entry =
            entry.with_context(|| format!("failed to read directory entry under {path}"))?;
        entries.push(entry.file_name().to_string_lossy().to_string());
    }
    entries.sort();
    Ok(entries)
}

fn parse_listing_output(output: &str) -> Vec<String> {
    output
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

fn first_entry_diff(host: &[String], pod: &[String]) -> Option<String> {
    let max = host.len().max(pod.len());
    for index in 0..max {
        match (host.get(index), pod.get(index)) {
            (Some(host), Some(pod)) if host != pod => {
                return Some(format!("index={index} host={host:?} pod={pod:?}"));
            }
            (Some(host), None) => return Some(format!("index={index} host={host:?} pod=<none>")),
            (None, Some(pod)) => return Some(format!("index={index} host=<none> pod={pod:?}")),
            _ => {}
        }
    }
    None
}

fn render_static_existing_dataset_manifest(config: &HostConfig, namespace: &str) -> String {
    let mut output = String::new();
    for (index, (export_id, export)) in config.exports.iter().enumerate() {
        if index > 0 {
            output.push_str("---\n");
        }
        let name = existing_dataset_resource_name(export_id);
        let access_mode = k8s_access_mode(export.access);
        let node_affinity = render_pv_node_affinity(&node_names_for_export(config, export_id));
        output.push_str(&format!(
            "apiVersion: v1\nkind: PersistentVolume\nmetadata:\n  name: {}\n  labels:\n    app.kubernetes.io/name: nas-csi\n    nas-csi.dev/export-id: {}\nspec:\n  capacity:\n    storage: 1Ti\n  accessModes:\n    - {}\n  persistentVolumeReclaimPolicy: Retain\n  storageClassName: {}\n  volumeMode: Filesystem\n{}  claimRef:\n    namespace: {}\n    name: {}\n  csi:\n    driver: {}\n    volumeHandle: {}\n    volumeAttributes:\n      nas-csi.dev/exportId: {}\n      nas-csi.dev/dataset: {}\n      nas-csi.dev/sourcePath: {}\n      nas-csi.dev/tag: {}\n      nas-csi.dev/policy: {}\n      nas-csi.dev/access: {}\n      nas-csi.dev/readOnly: {}\n---\napiVersion: v1\nkind: PersistentVolumeClaim\nmetadata:\n  name: {}\n  namespace: {}\n  labels:\n    app.kubernetes.io/name: nas-csi\n    nas-csi.dev/export-id: {}\nspec:\n  storageClassName: {}\n  volumeName: {}\n  accessModes:\n    - {}\n  resources:\n    requests:\n      storage: 1Ti\n",
            name,
            yaml_quote(export_id),
            access_mode,
            EXISTING_DATASET_STORAGE_CLASS,
            node_affinity,
            yaml_quote(namespace),
            name,
            CSI_DRIVER_NAME,
            yaml_quote(export_id),
            yaml_quote(export_id),
            yaml_quote(&export.dataset),
            yaml_quote(&export.source_path),
            yaml_quote(&export.tag),
            yaml_quote(&export.policy),
            yaml_quote(&export.access.to_string()),
            yaml_quote(if export.access == AccessMode::ReadOnly { "true" } else { "false" }),
            name,
            yaml_quote(namespace),
            yaml_quote(export_id),
            EXISTING_DATASET_STORAGE_CLASS,
            name,
            access_mode
        ));
    }
    output
}

fn render_csi_smoke_pod_manifest(config: &HostConfig, options: &CsiInstallOptions) -> String {
    let mut output = String::new();
    for (index, (export_id, export)) in config.exports.iter().enumerate() {
        if index > 0 {
            output.push_str("---\n");
        }
        let pod = smoke_pod_name(export_id);
        let pvc = existing_dataset_resource_name(export_id);
        let node = node_for_export(config, export_id).expect("validated export node");
        output.push_str(&format!(
            "apiVersion: v1\nkind: Pod\nmetadata:\n  name: {}\n  namespace: {}\n  labels:\n    app.kubernetes.io/name: nas-csi\n    app.kubernetes.io/component: smoke\n    nas-csi.dev/export-id: {}\nspec:\n  restartPolicy: Always\n  nodeName: {}\n  containers:\n    - name: smoke\n      image: {}\n      command:\n        - /bin/sh\n        - -c\n        - {}\n      volumeMounts:\n        - name: dataset\n          mountPath: {}\n          readOnly: {}\n  volumes:\n    - name: dataset\n      persistentVolumeClaim:\n        claimName: {}\n        readOnly: {}\n",
            pod,
            yaml_quote(&options.namespace),
            yaml_quote(export_id),
            yaml_quote(&node.name),
            yaml_quote(&options.smoke_image),
            yaml_quote("trap : TERM INT; sleep 2147483647 & wait"),
            smoke_mount_path(export_id),
            export.access == AccessMode::ReadOnly,
            pvc,
            export.access == AccessMode::ReadOnly
        ));
    }
    output
}

fn render_missing_export_manifest(config: &HostConfig, options: &CsiInstallOptions) -> String {
    let pod = missing_export_pod_name();
    let node = &config.nodes[0];
    format!(
        "apiVersion: v1\nkind: PersistentVolume\nmetadata:\n  name: {pod}\n  labels:\n    app.kubernetes.io/name: nas-csi\n    nas-csi.dev/probe: missing-export\nspec:\n  capacity:\n    storage: 1Gi\n  accessModes:\n    - ReadWriteMany\n  persistentVolumeReclaimPolicy: Retain\n  storageClassName: {EXISTING_DATASET_STORAGE_CLASS}\n  volumeMode: Filesystem\n  claimRef:\n    namespace: {}\n    name: {pod}\n  csi:\n    driver: {CSI_DRIVER_NAME}\n    volumeHandle: {MISSING_EXPORT_ID}\n    volumeAttributes:\n      nas-csi.dev/exportId: {MISSING_EXPORT_ID}\n      nas-csi.dev/dataset: tank/nas-csi-missing-export\n      nas-csi.dev/sourcePath: /var/empty/nas-csi-missing-export\n      nas-csi.dev/tag: nas_csi_missing_export\n      nas-csi.dev/policy: missing-export-probe\n      nas-csi.dev/access: read-write\n      nas-csi.dev/readOnly: \"false\"\n---\napiVersion: v1\nkind: PersistentVolumeClaim\nmetadata:\n  name: {pod}\n  namespace: {}\n  labels:\n    app.kubernetes.io/name: nas-csi\n    nas-csi.dev/probe: missing-export\nspec:\n  storageClassName: {EXISTING_DATASET_STORAGE_CLASS}\n  volumeName: {pod}\n  accessModes:\n    - ReadWriteMany\n  resources:\n    requests:\n      storage: 1Gi\n---\napiVersion: v1\nkind: Pod\nmetadata:\n  name: {pod}\n  namespace: {}\n  labels:\n    app.kubernetes.io/name: nas-csi\n    app.kubernetes.io/component: missing-export-probe\nspec:\n  restartPolicy: Never\n  nodeName: {}\n  containers:\n    - name: smoke\n      image: {}\n      command:\n        - /bin/sh\n        - -c\n        - {}\n      volumeMounts:\n        - name: dataset\n          mountPath: /mnt/nas-csi/missing-export\n  volumes:\n    - name: dataset\n      persistentVolumeClaim:\n        claimName: {pod}\n",
        yaml_quote(&options.namespace),
        yaml_quote(&options.namespace),
        yaml_quote(&options.namespace),
        yaml_quote(&node.name),
        yaml_quote(&options.smoke_image),
        yaml_quote("sleep 30"),
    )
}

fn print_csi_install_plan(
    config: &HostConfig,
    options: &CsiInstallOptions,
    nas_csi_manifest_path: &Path,
    static_manifest_path: &Path,
    smoke_manifest_path: &Path,
) {
    println!("csi install plan");
    println!();
    println!("nodes: {}", config.nodes.len());
    println!("exports: {}", config.exports.len());
    println!("namespace: {}", options.namespace);
    println!("nas-csi manifest: {}", nas_csi_manifest_path.display());
    println!("static PV/PVC manifest: {}", static_manifest_path.display());
    println!("smoke pod manifest: {}", smoke_manifest_path.display());
    println!();
    for (export_id, export) in &config.exports {
        let node = node_for_export(config, export_id)
            .map(|node| node.name.as_str())
            .unwrap_or("none");
        println!(
            "- {export_id}: dataset={} source={} access={} smokeNode={node}",
            export.dataset, export.source_path, export.access
        );
    }
}

fn verify_nas_csi_manifest_uses_lab_images(contents: &str) -> Result<()> {
    for image in [LAB_CONTROLLER_IMAGE, LAB_NODE_IMAGE] {
        if !contents.contains(image) {
            anyhow::bail!("nas-csi manifest does not use required lab image {image}");
        }
    }
    Ok(())
}

fn csi_kubectl_command<I>(
    config: &HostConfig,
    options: &CsiInstallOptions,
    args: I,
) -> ClusterCommandSpec
where
    I: IntoIterator<Item = String>,
{
    kubectl_command(config, &options.kubectl, args)
}

fn kubectl_command<I>(config: &HostConfig, kubectl: &str, args: I) -> ClusterCommandSpec
where
    I: IntoIterator<Item = String>,
{
    let mut command_args = vec![
        "--kubeconfig".to_string(),
        config.cluster.kubeconfig_out.clone(),
    ];
    command_args.extend(args);
    ClusterCommandSpec::new(kubectl.to_string(), command_args)
}

fn kubectl_exec_command<I>(
    config: &HostConfig,
    options: &CsiInstallOptions,
    pod: &str,
    args: I,
) -> ClusterCommandSpec
where
    I: IntoIterator<Item = String>,
{
    kubectl_exec_command_in_namespace(config, &options.kubectl, &options.namespace, pod, args)
}

fn kubectl_exec_command_in_namespace<I>(
    config: &HostConfig,
    kubectl: &str,
    namespace: &str,
    pod: &str,
    args: I,
) -> ClusterCommandSpec
where
    I: IntoIterator<Item = String>,
{
    let mut command_args = vec![
        "-n".to_string(),
        namespace.to_string(),
        "exec".to_string(),
        pod.to_string(),
        "--".to_string(),
    ];
    command_args.extend(args);
    kubectl_command(config, kubectl, command_args)
}

fn virtiofs_mount_verify_script() -> &'static str {
    r#"awk -v mp="$1" -v src="$2" '
$5 == mp {
    sep = 0
    for (i = 1; i <= NF; i++) {
        if ($i == "-") {
            sep = i
            break
        }
    }
    if (sep && $(sep + 1) == "virtiofs" && $(sep + 2) == src) {
        found = 1
    }
}
END { exit found ? 0 : 1 }
' /proc/self/mountinfo"#
}

fn readonly_mount_verify_script() -> &'static str {
    r#"awk -v mp="$1" '
$5 == mp {
    split($6, options, ",")
    for (i in options) {
        if (options[i] == "ro") {
            found = 1
        }
    }
}
END { exit found ? 0 : 1 }
' /proc/self/mountinfo"#
}

fn node_for_export<'a>(config: &'a HostConfig, export_id: &str) -> Option<&'a NodeConfig> {
    config.nodes.iter().find(|node| {
        node.exports
            .iter()
            .any(|node_export| node_export == export_id)
    })
}

fn node_names_for_export(config: &HostConfig, export_id: &str) -> Vec<String> {
    config
        .nodes
        .iter()
        .filter(|node| {
            node.exports
                .iter()
                .any(|node_export| node_export == export_id)
        })
        .map(|node| node.name.clone())
        .collect()
}

fn render_pv_node_affinity(node_names: &[String]) -> String {
    if node_names.is_empty() {
        return String::new();
    }

    let mut output = String::from(
        "  nodeAffinity:\n    required:\n      nodeSelectorTerms:\n        - matchExpressions:\n            - key: kubernetes.io/hostname\n              operator: In\n              values:\n",
    );
    for node_name in node_names {
        output.push_str(&format!("                - {}\n", yaml_quote(node_name)));
    }
    output
}

fn existing_dataset_resource_name(export_id: &str) -> String {
    safe_k8s_name("nas-csi", export_id)
}

fn smoke_pod_name(export_id: &str) -> String {
    safe_k8s_name("nas-csi-smoke", export_id)
}

fn missing_export_pod_name() -> String {
    safe_k8s_name("nas-csi", MISSING_EXPORT_ID)
}

fn smoke_mount_path(export_id: &str) -> String {
    format!("/mnt/nas-csi/{}", safe_mount_segment(export_id))
}

fn k8s_access_mode(access: AccessMode) -> &'static str {
    match access {
        AccessMode::ReadWrite => "ReadWriteMany",
        AccessMode::ReadOnly => "ReadOnlyMany",
    }
}

fn safe_k8s_name(prefix: &str, value: &str) -> String {
    let body = safe_k8s_body(value);
    let candidate = format!("{prefix}-{body}");
    if candidate.len() <= 63 {
        return candidate;
    }

    let hash = &sha256_hex(value.as_bytes())[..8];
    let max_body_len = 63usize.saturating_sub(prefix.len() + 10).max(1);
    let truncated = trim_k8s_hyphens(&body.chars().take(max_body_len).collect::<String>());
    format!("{prefix}-{truncated}-{hash}")
}

fn safe_k8s_body(value: &str) -> String {
    let mut output = String::new();
    let mut last_was_dash = false;
    for ch in value.chars() {
        let next = if ch.is_ascii_alphanumeric() {
            last_was_dash = false;
            ch.to_ascii_lowercase()
        } else if last_was_dash {
            continue;
        } else {
            last_was_dash = true;
            '-'
        };
        output.push(next);
    }
    let output = trim_k8s_hyphens(&output);
    if output.is_empty() {
        "x".to_string()
    } else {
        output
    }
}

fn trim_k8s_hyphens(value: &str) -> String {
    value.trim_matches('-').to_string()
}

fn safe_mount_segment(value: &str) -> String {
    let mut output = String::new();
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
            output.push(ch);
        } else {
            output.push('_');
        }
    }
    if output.is_empty() {
        "export".to_string()
    } else {
        output
    }
}

fn yaml_quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn verify_substrate_manifest_scope(manifests: &[DesiredManifest]) -> Result<()> {
    for manifest in manifests {
        if manifest.name != "metrics-server" && manifest.name != "nas-csi" {
            log_cluster_reconcile_refusal(
                "verify_substrate_manifest_scope",
                &format!("non-substrate manifest {}", manifest.name),
            );
            anyhow::bail!(
                "cluster install refuses non-substrate manifest {}",
                manifest.name
            );
        }
    }
    Ok(())
}

fn verify_cluster_plan_order(config: &HostConfig, plan: &ClusterReconcilePlan) -> Result<()> {
    let first_server = config
        .nodes
        .iter()
        .find(|node| node.role == NodeRole::Server && node.k3s.cluster_init)
        .or_else(|| {
            config
                .nodes
                .iter()
                .find(|node| node.role == NodeRole::Server)
        })
        .ok_or_else(|| anyhow::anyhow!("cluster install requires at least one server node"))?;

    let first_start_index = plan.steps.iter().position(|step| {
        matches!(
            &step.kind,
            ClusterReconcileStepKind::Apply(ClusterOperation::StartFirstServer { node, .. })
                if node == &first_server.name
        ) || matches!(
            &step.kind,
            ClusterReconcileStepKind::SkipAlreadyCorrect { .. }
        ) && step
            .description
            .contains(&format!("first k3s server {}", first_server.name))
    });

    for node in &config.nodes {
        if node.name == first_server.name {
            continue;
        }
        let Some(join_index) = plan.steps.iter().position(|step| {
            matches!(
                &step.kind,
                ClusterReconcileStepKind::Apply(ClusterOperation::StartJoinNode { node: planned, .. })
                    if planned == &node.name
            ) || matches!(
                &step.kind,
                ClusterReconcileStepKind::SkipAlreadyCorrect { .. }
            ) && step
                .description
                .contains(&format!("k3s join node {}", node.name))
        }) else {
            continue;
        };
        if let Some(first_start_index) = first_start_index
            && join_index <= first_start_index
        {
            anyhow::bail!(
                "cluster install plan starts join node {} before first server {}",
                node.name,
                first_server.name
            );
        }
    }

    Ok(())
}

fn verify_cluster_install_state(
    config: &HostConfig,
    actual: &ClusterActualState,
    manifests: &[DesiredManifest],
    options: &ClusterReconcileOptions,
) -> Result<()> {
    if !actual.token_present {
        anyhow::bail!("k3s token is missing or invalid");
    }
    verify_private_file(&config.cluster.token_file, "k3s token")?;

    if !actual.kubeconfig_present {
        anyhow::bail!("host kubeconfig is missing");
    }
    verify_private_file(&config.cluster.kubeconfig_out, "kubeconfig")?;
    let kubeconfig = fs::read_to_string(&config.cluster.kubeconfig_out).with_context(|| {
        format!(
            "failed to read kubeconfig {}",
            config.cluster.kubeconfig_out
        )
    })?;
    if !kubeconfig.contains(&format!("server: {}", config.cluster.api_server.endpoint)) {
        anyhow::bail!(
            "kubeconfig does not point at configured endpoint {}",
            config.cluster.api_server.endpoint
        );
    }

    if !actual.api_ready {
        anyhow::bail!("Kubernetes API readiness check is not passing");
    }

    for node in &config.nodes {
        let state = actual
            .nodes
            .get(&node.name)
            .ok_or_else(|| anyhow::anyhow!("missing cluster state for node {}", node.name))?;
        if !state.domain_running {
            anyhow::bail!("node domain is not running: {}", node.name);
        }
        if !state.k3s_ready {
            anyhow::bail!("k3s service is not ready on node {}", node.name);
        }
        if !state.kubernetes_ready {
            anyhow::bail!("Kubernetes node is not Ready: {}", node.name);
        }
        for (key, value) in &node.k3s.labels {
            if state.labels.get(key) != Some(value) {
                anyhow::bail!(
                    "node {} label {} does not match desired value {}",
                    node.name,
                    key,
                    value
                );
            }
        }
        let actual_taints = state
            .taints
            .iter()
            .map(node_taint_key)
            .collect::<BTreeSet<_>>();
        for taint in &node.k3s.taints {
            if !actual_taints.contains(&node_taint_key(taint)) {
                anyhow::bail!("node {} missing taint {}", node.name, node_taint_key(taint));
            }
        }
    }

    for manifest in manifests {
        let desired_hash = manifest.desired_hash();
        let applied = actual.applied_manifests.get(&manifest.name);
        if applied != Some(&desired_hash) {
            anyhow::bail!(
                "substrate manifest {} is not applied at desired hash via marker {}",
                manifest.name,
                nas_csi_cluster_manager::manifest_marker_path(options, &manifest.name)
            );
        }
    }

    Ok(())
}

fn verify_private_file(path: &str, label: &str) -> Result<()> {
    let metadata = fs::metadata(path).with_context(|| format!("{label} file missing: {path}"))?;
    if !metadata.is_file() {
        anyhow::bail!("{label} path is not a file: {path}");
    }
    let mode = metadata.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        anyhow::bail!("{label} file {path} must not be group/world accessible; mode={mode:o}");
    }
    Ok(())
}

fn node_taint_key(taint: &NodeTaint) -> String {
    format!("{}={}:{}", taint.key, taint.value, taint.effect)
}

fn verify_cluster_install_idempotence(plan: &ClusterReconcilePlan) -> Result<()> {
    if plan.is_current() {
        return Ok(());
    }
    let mut apply = 0usize;
    for step in &plan.steps {
        if matches!(step.kind, ClusterReconcileStepKind::Apply(_)) {
            apply += 1;
        }
    }
    log_cluster_reconcile_refusal(
        "verify_cluster_install_idempotence",
        &format!("post-execute plan still has {apply} apply step(s)"),
    );
    anyhow::bail!("cluster install was not idempotent after execute: apply={apply}");
}

fn reboot_cluster_node(
    config: &HostConfig,
    options: &ClusterReconcileOptions,
    runner: &impl CommandRunner,
    node_name: &str,
    timeout: Duration,
) -> Result<()> {
    let node = config
        .nodes
        .iter()
        .find(|node| node.name == node_name)
        .ok_or_else(|| anyhow::anyhow!("unknown node requested for reboot: {node_name}"))?;
    let reboot = ClusterCommandSpec::new(
        options.virsh_path.clone(),
        [
            "-c".to_string(),
            options.libvirt_uri.clone(),
            "reboot".to_string(),
            node.domain.clone(),
        ],
    );
    run_cluster_command(runner, &reboot)
        .with_context(|| format!("failed to request reboot for node {}", node.name))?;

    let service_name = match node.role {
        NodeRole::Server => "k3s",
        NodeRole::Agent => "k3s-agent",
    };
    wait_for_guest_command_tolerant(
        runner,
        options,
        &node.domain,
        &GuestCommandSpec::new(
            "/bin/systemctl".to_string(),
            [
                "is-active".to_string(),
                "--quiet".to_string(),
                service_name.to_string(),
            ],
        ),
        timeout,
    )?;
    wait_for_cluster_command_with_timeout(
        runner,
        &cluster_ready_command(config, options),
        timeout,
    )?;
    wait_for_cluster_command_with_timeout(
        runner,
        &node_ready_command(config, options, &node.name, timeout),
        timeout,
    )?;
    println!("cluster install: node {} returned after reboot", node.name);
    Ok(())
}

fn cluster_ready_command(
    config: &HostConfig,
    options: &ClusterReconcileOptions,
) -> ClusterCommandSpec {
    ClusterCommandSpec::new(
        options.kubectl_path.clone(),
        [
            "--kubeconfig".to_string(),
            config.cluster.kubeconfig_out.clone(),
            "get".to_string(),
            "--raw=/readyz".to_string(),
        ],
    )
}

fn node_ready_command(
    config: &HostConfig,
    options: &ClusterReconcileOptions,
    node_name: &str,
    timeout: Duration,
) -> ClusterCommandSpec {
    ClusterCommandSpec::new(
        options.kubectl_path.clone(),
        [
            "--kubeconfig".to_string(),
            config.cluster.kubeconfig_out.clone(),
            "wait".to_string(),
            "node".to_string(),
            node_name.to_string(),
            "--for=condition=Ready".to_string(),
            format!("--timeout={}s", timeout.as_secs()),
        ],
    )
}

fn cluster_options_from_config(
    config: &HostConfig,
    artifact_dir: &Path,
    kubectl_path: &str,
) -> ClusterReconcileOptions {
    ClusterReconcileOptions {
        kubectl_path: kubectl_path.to_string(),
        virsh_path: config.host_tools.virsh.clone(),
        libvirt_uri: config.libvirt.uri.clone(),
        artifact_dir: artifact_dir.display().to_string(),
        ..ClusterReconcileOptions::default()
    }
}

fn load_cluster_manifests(
    config: &HostConfig,
    manifest_root: &Path,
) -> Result<Vec<DesiredManifest>> {
    let mut manifests = Vec::new();
    if config.cluster.addons.metrics_server {
        manifests.push(load_desired_manifest(
            "metrics-server",
            &manifest_root
                .join("addons")
                .join("metrics-server")
                .join("metrics-server.yaml"),
        )?);
    }
    if config.cluster.addons.nas_csi {
        manifests.push(load_desired_manifest(
            "nas-csi",
            &manifest_root
                .join("kubernetes")
                .join("nas-csi")
                .join("nas-csi.yaml"),
        )?);
    }
    Ok(manifests)
}

fn load_desired_manifest(name: &str, path: &Path) -> Result<DesiredManifest> {
    let contents = fs::read_to_string(path)
        .with_context(|| format!("failed to read manifest {}", path.display()))?;
    Ok(DesiredManifest {
        name: name.to_string(),
        path: path.display().to_string(),
        contents,
    })
}

fn inspect_cluster_actual_state(
    config: &HostConfig,
    options: &ClusterReconcileOptions,
    manifests: &[DesiredManifest],
    runner: &impl CommandRunner,
) -> Result<ClusterActualState> {
    let token_present = fs::read_to_string(&config.cluster.token_file)
        .map(|token| nas_csi_cluster_manager::token_looks_valid(&token))
        .unwrap_or(false);
    let kubeconfig_present = Path::new(&config.cluster.kubeconfig_out).is_file();
    let api_ready = kubeconfig_present
        && command_success(
            runner,
            &options.kubectl_path,
            [
                "--kubeconfig",
                config.cluster.kubeconfig_out.as_str(),
                "get",
                "--raw=/readyz",
            ],
        )
        .unwrap_or(false);

    let mut nodes = BTreeMap::new();
    let kubernetes_nodes = if api_ready {
        inspect_kubernetes_nodes(config, options, runner)?
    } else {
        BTreeMap::new()
    };

    for node in &config.nodes {
        let domain_state = inspect_domain(
            runner,
            &options.virsh_path,
            &options.libvirt_uri,
            &node.domain,
        )
        .unwrap_or(nas_csi_vm_manager::DomainActualState {
            exists: false,
            managed: false,
            active: false,
            autostart: None,
            desired_hash: None,
            xml: None,
            xml_hash: None,
        });
        let domain_running = domain_state.active;
        let k3s_ready = if domain_running {
            let service_name = match node.role {
                NodeRole::Server => "k3s",
                NodeRole::Agent => "k3s-agent",
            };
            guest_command_success(
                runner,
                options,
                &node.domain,
                &GuestCommandSpec::new(
                    "/bin/systemctl".to_string(),
                    [
                        "is-active".to_string(),
                        "--quiet".to_string(),
                        service_name.to_string(),
                    ],
                ),
            )
            .unwrap_or(false)
        } else {
            false
        };
        let kubernetes = kubernetes_nodes
            .get(&node.name)
            .cloned()
            .unwrap_or_default();
        nodes.insert(
            node.name.clone(),
            ClusterNodeActualState {
                domain_running,
                k3s_ready,
                kubernetes_ready: kubernetes.ready,
                labels: kubernetes.labels,
                taints: kubernetes.taints,
            },
        );
    }

    let mut applied_manifests = BTreeMap::new();
    for manifest in manifests {
        let marker_path = nas_csi_cluster_manager::manifest_marker_path(options, &manifest.name);
        if let Ok(hash) = fs::read_to_string(&marker_path) {
            applied_manifests.insert(manifest.name.clone(), hash.trim().to_string());
        }
    }

    Ok(ClusterActualState {
        token_present,
        kubeconfig_present,
        api_ready,
        nodes,
        applied_manifests,
    })
}

#[derive(Clone, Debug, Default)]
struct KubernetesNodeActual {
    ready: bool,
    labels: BTreeMap<String, String>,
    taints: Vec<NodeTaint>,
}

#[derive(serde::Deserialize)]
struct KubernetesNodeList {
    #[serde(default)]
    items: Vec<KubernetesNode>,
}

#[derive(serde::Deserialize)]
struct KubernetesNode {
    metadata: KubernetesMetadata,
    #[serde(default)]
    spec: KubernetesNodeSpec,
    #[serde(default)]
    status: KubernetesNodeStatus,
}

#[derive(serde::Deserialize)]
struct KubernetesMetadata {
    name: String,
    #[serde(default)]
    labels: BTreeMap<String, String>,
}

#[derive(Default, serde::Deserialize)]
struct KubernetesNodeSpec {
    #[serde(default)]
    taints: Vec<KubernetesTaint>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct KubernetesTaint {
    key: String,
    #[serde(default)]
    value: String,
    effect: String,
}

#[derive(Default, serde::Deserialize)]
struct KubernetesNodeStatus {
    #[serde(default)]
    conditions: Vec<KubernetesCondition>,
}

#[derive(serde::Deserialize)]
struct KubernetesCondition {
    #[serde(rename = "type")]
    condition_type: String,
    status: String,
}

fn inspect_kubernetes_nodes(
    config: &HostConfig,
    options: &ClusterReconcileOptions,
    runner: &impl CommandRunner,
) -> Result<BTreeMap<String, KubernetesNodeActual>> {
    let command = command_spec(
        &options.kubectl_path,
        [
            "--kubeconfig",
            config.cluster.kubeconfig_out.as_str(),
            "get",
            "nodes",
            "-o",
            "json",
        ],
    );
    let Some(output) = runner.output(&command)? else {
        return Ok(BTreeMap::new());
    };
    let parsed: KubernetesNodeList =
        serde_json::from_str(&output).context("failed to parse kubectl node JSON")?;
    Ok(parsed
        .items
        .into_iter()
        .map(|node| {
            let ready =
                node.status.conditions.iter().any(|condition| {
                    condition.condition_type == "Ready" && condition.status == "True"
                });
            (
                node.metadata.name,
                KubernetesNodeActual {
                    ready,
                    labels: node.metadata.labels,
                    taints: node
                        .spec
                        .taints
                        .into_iter()
                        .map(|taint| NodeTaint {
                            key: taint.key,
                            value: taint.value,
                            effect: taint.effect,
                        })
                        .collect(),
                },
            )
        })
        .collect())
}

fn print_cluster_reconcile_plan(plan: &ClusterReconcilePlan) {
    let mut apply_count = 0usize;
    let mut skip_count = 0usize;
    for step in &plan.steps {
        match step.kind {
            ClusterReconcileStepKind::Apply(_) => apply_count += 1,
            ClusterReconcileStepKind::SkipAlreadyCorrect { .. } => skip_count += 1,
        }
    }
    println!(
        "cluster reconcile summary: apply={} skip={}",
        apply_count, skip_count
    );
    println!("steps: {}", plan.steps.len());
    println!();
    for (index, step) in plan.steps.iter().enumerate() {
        println!("{}. {}", index + 1, step.description);
        match &step.kind {
            ClusterReconcileStepKind::Apply(operation) => print_cluster_operation(operation),
            ClusterReconcileStepKind::SkipAlreadyCorrect { reason } => {
                println!("   skip: {reason}");
            }
        }
    }
}

fn print_cluster_operation(operation: &ClusterOperation) {
    match operation {
        ClusterOperation::EnsureToken { path } => {
            println!("   generate token: {}", shell_quote(path));
        }
        ClusterOperation::StartFirstServer {
            domain, command, ..
        }
        | ClusterOperation::StartJoinNode {
            domain, command, ..
        } => {
            println!("   domain: {domain}");
            println!("   command: {}", cluster_command_display(command));
        }
        ClusterOperation::WaitForFirstServer {
            domain, command, ..
        } => {
            println!("   guest domain: {domain}");
            println!("   guest command: {}", guest_command_display(command));
        }
        ClusterOperation::RetrieveKubeconfig {
            domain,
            guest_path,
            host_path,
            server_endpoint,
            ..
        } => {
            println!("   guest domain: {domain}");
            println!("   read: {}", shell_quote(guest_path));
            println!("   write: {}", shell_quote(host_path));
            println!("   server: {server_endpoint}");
        }
        ClusterOperation::WaitForClusterApi { command }
        | ClusterOperation::WaitForNodeReady { command, .. } => {
            println!("   command: {}", cluster_command_display(command));
        }
        ClusterOperation::ReconcileNodeLabels { commands, .. }
        | ClusterOperation::ReconcileNodeTaints { commands, .. } => {
            for command in commands {
                println!("   command: {}", cluster_command_display(command));
            }
        }
        ClusterOperation::ApplyAddon {
            name,
            manifest_path,
            command,
            marker_path,
            ..
        } => {
            println!("   addon: {name}");
            println!("   manifest: {}", shell_quote(manifest_path));
            println!("   command: {}", cluster_command_display(command));
            println!("   marker: {}", shell_quote(marker_path));
        }
        ClusterOperation::ApplyNasCsi {
            manifest_path,
            command,
            marker_path,
            ..
        } => {
            println!("   nas-csi manifest: {}", shell_quote(manifest_path));
            println!("   command: {}", cluster_command_display(command));
            println!("   marker: {}", shell_quote(marker_path));
        }
    }
}

fn print_cluster_status(
    config: &HostConfig,
    actual: &ClusterActualState,
    manifests: &[DesiredManifest],
) {
    println!("cluster status");
    println!();
    println!("token: {}", bool_status(actual.token_present));
    println!("kubeconfig: {}", bool_status(actual.kubeconfig_present));
    println!("api ready: {}", bool_status(actual.api_ready));
    println!();
    println!("nodes:");
    for node in &config.nodes {
        let state = actual.nodes.get(&node.name).cloned().unwrap_or_default();
        println!(
            "- {}: domainRunning={} k3sReady={} kubernetesReady={} labels={} taints={}",
            node.name,
            state.domain_running,
            state.k3s_ready,
            state.kubernetes_ready,
            state.labels.len(),
            state.taints.len()
        );
    }
    println!();
    println!("substrate manifests:");
    if manifests.is_empty() {
        println!("- none configured");
    }
    for manifest in manifests {
        println!(
            "- {}: desired={} applied={}",
            manifest.name,
            manifest.desired_hash(),
            actual
                .applied_manifests
                .get(&manifest.name)
                .map(String::as_str)
                .unwrap_or("none")
        );
    }
}

fn execute_cluster_reconcile_plan(
    plan: &ClusterReconcilePlan,
    options: &ClusterReconcileOptions,
    runner: &impl CommandRunner,
) -> Result<()> {
    for (index, step) in plan.steps.iter().enumerate() {
        match &step.kind {
            ClusterReconcileStepKind::SkipAlreadyCorrect { reason } => {
                log_cluster_reconcile_skip(index + 1, step, reason);
                println!("skip {}: {reason}", step.description);
            }
            ClusterReconcileStepKind::Apply(operation) => {
                println!("{}", step.description);
                log_cluster_operation_start(index + 1, step, operation);
                match execute_cluster_operation(operation, options, runner) {
                    Ok(()) => log_cluster_operation_finish(index + 1, step, operation),
                    Err(error) => {
                        log_cluster_operation_failure(index + 1, step, operation, &error);
                        return Err(error)
                            .with_context(|| format!("failed cluster step: {}", step.description));
                    }
                }
            }
        }
    }
    Ok(())
}

fn execute_cluster_operation(
    operation: &ClusterOperation,
    options: &ClusterReconcileOptions,
    runner: &impl CommandRunner,
) -> Result<()> {
    match operation {
        ClusterOperation::EnsureToken { path } => ensure_cluster_token(path),
        ClusterOperation::StartFirstServer { command, .. }
        | ClusterOperation::StartJoinNode { command, .. } => run_cluster_command(runner, command),
        ClusterOperation::WaitForFirstServer {
            domain, command, ..
        } => wait_for_guest_command(runner, options, domain, command),
        ClusterOperation::RetrieveKubeconfig {
            domain,
            guest_path,
            host_path,
            server_endpoint,
            ..
        } => {
            let kubeconfig = guest_exec_output(
                runner,
                options,
                domain,
                &GuestCommandSpec::new("/bin/cat".to_string(), [guest_path.to_string()]),
            )?;
            let rewritten =
                nas_csi_cluster_manager::rewrite_kubeconfig_server(&kubeconfig, server_endpoint);
            write_secret_text_atomic_if_changed(host_path, &rewritten, 0o600)?;
            Ok(())
        }
        ClusterOperation::WaitForClusterApi { command }
        | ClusterOperation::WaitForNodeReady { command, .. } => {
            wait_for_cluster_command(runner, command)
        }
        ClusterOperation::ReconcileNodeLabels { commands, .. }
        | ClusterOperation::ReconcileNodeTaints { commands, .. } => {
            for command in commands {
                run_cluster_command(runner, command)?;
            }
            Ok(())
        }
        ClusterOperation::ApplyAddon {
            command,
            marker_path,
            desired_hash,
            ..
        }
        | ClusterOperation::ApplyNasCsi {
            command,
            marker_path,
            desired_hash,
            ..
        } => {
            run_cluster_command(runner, command)?;
            write_text_atomic_if_changed(marker_path, &format!("{desired_hash}\n"))?;
            Ok(())
        }
    }
}

fn ensure_cluster_token(path: &str) -> Result<()> {
    if let Ok(existing) = fs::read_to_string(path)
        && nas_csi_cluster_manager::token_looks_valid(&existing)
    {
        return Ok(());
    }
    let token = generate_cluster_token()?;
    write_secret_text_atomic_if_changed(path, &format!("{token}\n"), 0o600)?;
    Ok(())
}

fn generate_cluster_token() -> Result<String> {
    let mut file = File::open("/dev/urandom").context("failed to open /dev/urandom")?;
    let mut bytes = [0_u8; 32];
    std::io::Read::read_exact(&mut file, &mut bytes)
        .context("failed to read random k3s token bytes")?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn run_cluster_command(runner: &impl CommandRunner, command: &ClusterCommandSpec) -> Result<()> {
    runner.run(&cluster_command_to_vm_command(command))
}

fn wait_for_cluster_command(
    runner: &impl CommandRunner,
    command: &ClusterCommandSpec,
) -> Result<()> {
    wait_for_cluster_command_with_timeout(runner, command, Duration::from_secs(600))
}

fn wait_for_cluster_command_with_timeout(
    runner: &impl CommandRunner,
    command: &ClusterCommandSpec,
    timeout: Duration,
) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        if runner.status(&cluster_command_to_vm_command(command))? {
            return Ok(());
        }
        if Instant::now() >= deadline {
            anyhow::bail!(
                "timed out waiting for command {}",
                cluster_command_display(command)
            );
        }
        thread::sleep(Duration::from_secs(5));
    }
}

fn wait_for_guest_command(
    runner: &impl CommandRunner,
    options: &ClusterReconcileOptions,
    domain: &str,
    command: &GuestCommandSpec,
) -> Result<()> {
    wait_for_guest_command_with_timeout(runner, options, domain, command, Duration::from_secs(600))
}

fn wait_for_guest_command_with_timeout(
    runner: &impl CommandRunner,
    options: &ClusterReconcileOptions,
    domain: &str,
    command: &GuestCommandSpec,
    timeout: Duration,
) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        if guest_command_success(runner, options, domain, command)? {
            return Ok(());
        }
        if Instant::now() >= deadline {
            anyhow::bail!("timed out waiting for guest command on {domain}");
        }
        thread::sleep(Duration::from_secs(5));
    }
}

fn wait_for_guest_command_tolerant(
    runner: &impl CommandRunner,
    options: &ClusterReconcileOptions,
    domain: &str,
    command: &GuestCommandSpec,
    timeout: Duration,
) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        match guest_command_success(runner, options, domain, command) {
            Ok(true) => return Ok(()),
            Ok(false) | Err(_) => {}
        }
        if Instant::now() >= deadline {
            anyhow::bail!("timed out waiting for guest command on {domain}");
        }
        thread::sleep(Duration::from_secs(5));
    }
}

fn guest_command_success(
    runner: &impl CommandRunner,
    options: &ClusterReconcileOptions,
    domain: &str,
    command: &GuestCommandSpec,
) -> Result<bool> {
    Ok(guest_exec(runner, options, domain, command)?.exit_code == 0)
}

fn guest_exec_output(
    runner: &impl CommandRunner,
    options: &ClusterReconcileOptions,
    domain: &str,
    command: &GuestCommandSpec,
) -> Result<String> {
    let result = guest_exec(runner, options, domain, command)?;
    if result.exit_code != 0 {
        anyhow::bail!(
            "guest command failed on {domain} with exit code {}: {}",
            result.exit_code,
            result.stderr
        );
    }
    Ok(result.stdout)
}

struct GuestExecResult {
    exit_code: i64,
    stdout: String,
    stderr: String,
}

fn guest_exec(
    runner: &impl CommandRunner,
    options: &ClusterReconcileOptions,
    domain: &str,
    command: &GuestCommandSpec,
) -> Result<GuestExecResult> {
    let request = serde_json::json!({
        "execute": "guest-exec",
        "arguments": {
            "path": command.program,
            "arg": command.args,
            "capture-output": true
        }
    });
    let output = runner
        .output(&virsh_qemu_agent_command(options, domain, request))?
        .ok_or_else(|| anyhow::anyhow!("guest-exec returned no response for {domain}"))?;
    let response: serde_json::Value =
        serde_json::from_str(&output).context("failed to parse guest-exec response")?;
    let pid = response
        .pointer("/return/pid")
        .and_then(serde_json::Value::as_i64)
        .ok_or_else(|| anyhow::anyhow!("guest-exec response did not contain a pid"))?;

    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let status_request = serde_json::json!({
            "execute": "guest-exec-status",
            "arguments": { "pid": pid }
        });
        let status_output = runner
            .output(&virsh_qemu_agent_command(options, domain, status_request))?
            .ok_or_else(|| anyhow::anyhow!("guest-exec-status returned no response"))?;
        let status_response: serde_json::Value =
            serde_json::from_str(&status_output).context("failed to parse guest-exec-status")?;
        if status_response
            .pointer("/return/exited")
            .and_then(serde_json::Value::as_bool)
            == Some(true)
        {
            let exit_code = status_response
                .pointer("/return/exitcode")
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(1);
            let stdout = decode_guest_exec_data(
                status_response
                    .pointer("/return/out-data")
                    .and_then(serde_json::Value::as_str),
            )?;
            let stderr = decode_guest_exec_data(
                status_response
                    .pointer("/return/err-data")
                    .and_then(serde_json::Value::as_str),
            )?;
            return Ok(GuestExecResult {
                exit_code,
                stdout,
                stderr,
            });
        }
        if Instant::now() >= deadline {
            anyhow::bail!("timed out waiting for guest-exec-status pid {pid} on {domain}");
        }
        thread::sleep(Duration::from_millis(250));
    }
}

fn virsh_qemu_agent_command(
    options: &ClusterReconcileOptions,
    domain: &str,
    request: serde_json::Value,
) -> nas_csi_vm_manager::CommandSpec {
    nas_csi_vm_manager::CommandSpec::new(
        options.virsh_path.clone(),
        [
            "-c".to_string(),
            options.libvirt_uri.clone(),
            "qemu-agent-command".to_string(),
            domain.to_string(),
            request.to_string(),
        ],
    )
}

fn decode_guest_exec_data(value: Option<&str>) -> Result<String> {
    let Some(value) = value else {
        return Ok(String::new());
    };
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(value)
        .context("failed to decode guest-exec base64 output")?;
    Ok(String::from_utf8_lossy(&bytes).to_string())
}

fn cluster_command_to_vm_command(command: &ClusterCommandSpec) -> nas_csi_vm_manager::CommandSpec {
    nas_csi_vm_manager::CommandSpec::new(command.program.clone(), command.args.clone())
}

fn cluster_command_display(command: &ClusterCommandSpec) -> String {
    cluster_command_to_vm_command(command).to_string()
}

fn guest_command_display(command: &GuestCommandSpec) -> String {
    let command =
        nas_csi_vm_manager::CommandSpec::new(command.program.clone(), command.args.clone());
    command.to_string()
}

fn bool_status(value: bool) -> &'static str {
    if value { "ok" } else { "missing" }
}

fn write_secret_text_atomic_if_changed(path: &str, contents: &str, mode: u32) -> Result<()> {
    write_text_atomic_if_changed(path, contents)?;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .with_context(|| format!("failed to set permissions on {path}"))?;
    Ok(())
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

fn write_yaml_atomic_if_changed<T>(path: &Path, value: &T) -> Result<WriteOutcome>
where
    T: serde::Serialize,
{
    let yaml = serde_yml::to_string(value).context("failed to serialize yaml")?;
    write_text_atomic_if_changed(&path.display().to_string(), &yaml)
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
    allow_domain_adoption: bool,
) -> nas_csi_vm_manager::HostApplyPlanOptions {
    nas_csi_vm_manager::HostApplyPlanOptions {
        artifact_dir: artifact_dir.display().to_string(),
        systemd_unit_dir: systemd_unit_dir.display().to_string(),
        qemu_img_path: config.host_tools.qemu_img.clone(),
        virsh_path: config.host_tools.virsh.clone(),
        systemctl_path: config.host_tools.systemctl.clone(),
        allow_running_domain_redefine,
        allow_domain_adoption,
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
            | nas_csi_vm_manager::ReconcileOperation::ResizeRootDisk { .. }
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
        nas_csi_vm_manager::ReconcileOperation::ResizeRootDisk {
            path,
            desired_size_bytes,
            command,
        } => {
            println!(
                "   resize {} to {} bytes",
                shell_quote(path),
                desired_size_bytes
            );
            println!("   command: {command}");
        }
        nas_csi_vm_manager::ReconcileOperation::ReloadSystemdUnits { command }
        | nas_csi_vm_manager::ReconcileOperation::DefineDomain { command, .. }
        | nas_csi_vm_manager::ReconcileOperation::RedefineDomain { command, .. }
        | nas_csi_vm_manager::ReconcileOperation::EnableDomainAutostart { command, .. }
        | nas_csi_vm_manager::ReconcileOperation::StartDomain { command, .. } => {
            println!("   command: {command}");
        }
        nas_csi_vm_manager::ReconcileOperation::EnableAndStartVirtiofsdService {
            socket_path,
            command,
            ..
        }
        | nas_csi_vm_manager::ReconcileOperation::RestartVirtiofsdService {
            socket_path,
            command,
            ..
        } => {
            println!("   command: {command}");
            println!("   wait for socket: {}", shell_quote(socket_path));
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

fn log_reconcile_decisions(plan: &nas_csi_vm_manager::HostReconcilePlan) {
    for (index, step) in plan.steps.iter().enumerate() {
        match &step.kind {
            nas_csi_vm_manager::ReconcileStepKind::Apply(operation) => {
                structured_log(serde_json::json!({
                    "event": "reconcile_decision",
                    "stepIndex": index + 1,
                    "decision": "apply",
                    "description": step.description,
                    "operation": reconcile_operation_name(operation),
                }));
            }
            nas_csi_vm_manager::ReconcileStepKind::SkipAlreadyCorrect { reason } => {
                structured_log(serde_json::json!({
                    "event": "reconcile_decision",
                    "stepIndex": index + 1,
                    "decision": "skip",
                    "description": step.description,
                    "reason": reason,
                }));
            }
            nas_csi_vm_manager::ReconcileStepKind::Refuse { operation, reason } => {
                structured_log(serde_json::json!({
                    "event": "reconcile_decision",
                    "stepIndex": index + 1,
                    "decision": "refuse",
                    "description": step.description,
                    "operation": operation.as_ref().map(reconcile_operation_name),
                    "reason": reason,
                }));
            }
        }
    }
}

fn log_cluster_reconcile_decisions(plan: &ClusterReconcilePlan) {
    for (index, step) in plan.steps.iter().enumerate() {
        match &step.kind {
            ClusterReconcileStepKind::Apply(operation) => {
                structured_log(serde_json::json!({
                    "event": "cluster_reconcile_decision",
                    "stepIndex": index + 1,
                    "decision": "apply",
                    "description": step.description,
                    "operation": cluster_operation_name(operation),
                }));
            }
            ClusterReconcileStepKind::SkipAlreadyCorrect { reason } => {
                structured_log(serde_json::json!({
                    "event": "cluster_reconcile_decision",
                    "stepIndex": index + 1,
                    "decision": "skip",
                    "description": step.description,
                    "reason": reason,
                }));
            }
        }
    }
}

fn log_cluster_reconcile_skip(
    index: usize,
    step: &nas_csi_cluster_manager::ClusterReconcileStep,
    reason: &str,
) {
    structured_log(serde_json::json!({
        "event": "cluster_reconcile_skip",
        "stepIndex": index,
        "description": step.description,
        "reason": reason,
        "result": "success",
    }));
}

fn log_cluster_operation_start(
    index: usize,
    step: &nas_csi_cluster_manager::ClusterReconcileStep,
    operation: &ClusterOperation,
) {
    structured_log(serde_json::json!({
        "event": "cluster_reconcile_operation_start",
        "stepIndex": index,
        "description": step.description,
        "operation": cluster_operation_name(operation),
    }));
}

fn log_cluster_operation_finish(
    index: usize,
    step: &nas_csi_cluster_manager::ClusterReconcileStep,
    operation: &ClusterOperation,
) {
    structured_log(serde_json::json!({
        "event": "cluster_reconcile_operation_finish",
        "stepIndex": index,
        "description": step.description,
        "operation": cluster_operation_name(operation),
        "result": "success",
    }));
}

fn log_cluster_operation_failure(
    index: usize,
    step: &nas_csi_cluster_manager::ClusterReconcileStep,
    operation: &ClusterOperation,
    error: &anyhow::Error,
) {
    structured_log(serde_json::json!({
        "event": "cluster_reconcile_operation_finish",
        "stepIndex": index,
        "description": step.description,
        "operation": cluster_operation_name(operation),
        "result": "failure",
        "error": error.to_string(),
    }));
}

fn log_cluster_reconcile_refusal(reason: &str, detail: &str) {
    structured_log(serde_json::json!({
        "event": "cluster_reconcile_refusal",
        "decision": "refuse",
        "reason": reason,
        "detail": detail,
    }));
}

fn reconcile_operation_name(operation: &nas_csi_vm_manager::ReconcileOperation) -> &'static str {
    match operation {
        nas_csi_vm_manager::ReconcileOperation::EnsureDirectory { .. } => "EnsureDirectory",
        nas_csi_vm_manager::ReconcileOperation::WriteRenderedArtifact { .. } => {
            "WriteRenderedArtifact"
        }
        nas_csi_vm_manager::ReconcileOperation::CreateRootDisk { .. } => "CreateRootDisk",
        nas_csi_vm_manager::ReconcileOperation::ResizeRootDisk { .. } => "ResizeRootDisk",
        nas_csi_vm_manager::ReconcileOperation::RewriteSeedImage { .. } => "RewriteSeedImage",
        nas_csi_vm_manager::ReconcileOperation::InstallOrUpdateSystemdUnit { .. } => {
            "InstallOrUpdateSystemdUnit"
        }
        nas_csi_vm_manager::ReconcileOperation::ReloadSystemdUnits { .. } => "ReloadSystemdUnits",
        nas_csi_vm_manager::ReconcileOperation::EnableAndStartVirtiofsdService { .. } => {
            "EnableAndStartVirtiofsdService"
        }
        nas_csi_vm_manager::ReconcileOperation::RestartVirtiofsdService { .. } => {
            "RestartVirtiofsdService"
        }
        nas_csi_vm_manager::ReconcileOperation::DefineDomain { .. } => "DefineDomain",
        nas_csi_vm_manager::ReconcileOperation::RedefineDomain { .. } => "RedefineDomain",
        nas_csi_vm_manager::ReconcileOperation::RedefineDomainRequiresShutdown { .. } => {
            "RedefineDomainRequiresShutdown"
        }
        nas_csi_vm_manager::ReconcileOperation::EnableDomainAutostart { .. } => {
            "EnableDomainAutostart"
        }
        nas_csi_vm_manager::ReconcileOperation::StartDomain { .. } => "StartDomain",
        nas_csi_vm_manager::ReconcileOperation::RunCommand { .. } => "RunCommand",
    }
}

fn cluster_operation_name(operation: &ClusterOperation) -> &'static str {
    match operation {
        ClusterOperation::EnsureToken { .. } => "EnsureToken",
        ClusterOperation::StartFirstServer { .. } => "StartFirstServer",
        ClusterOperation::WaitForFirstServer { .. } => "WaitForFirstServer",
        ClusterOperation::RetrieveKubeconfig { .. } => "RetrieveKubeconfig",
        ClusterOperation::WaitForClusterApi { .. } => "WaitForClusterApi",
        ClusterOperation::StartJoinNode { .. } => "StartJoinNode",
        ClusterOperation::WaitForNodeReady { .. } => "WaitForNodeReady",
        ClusterOperation::ReconcileNodeLabels { .. } => "ReconcileNodeLabels",
        ClusterOperation::ReconcileNodeTaints { .. } => "ReconcileNodeTaints",
        ClusterOperation::ApplyAddon { .. } => "ApplyAddon",
        ClusterOperation::ApplyNasCsi { .. } => "ApplyNasCsi",
    }
}

fn log_command_start(kind: &str, command: &nas_csi_vm_manager::CommandSpec) {
    structured_log(serde_json::json!({
        "event": "command_start",
        "kind": kind,
        "program": command.program,
        "args": sanitized_command_args(command),
    }));
}

fn log_command_finish(
    kind: &str,
    command: &nas_csi_vm_manager::CommandSpec,
    success: bool,
    exit_code: Option<i32>,
) {
    structured_log(serde_json::json!({
        "event": "command_finish",
        "kind": kind,
        "program": command.program,
        "args": sanitized_command_args(command),
        "success": success,
        "exitCode": exit_code,
    }));
}

fn log_command_error(kind: &str, command: &nas_csi_vm_manager::CommandSpec, error: &str) {
    structured_log(serde_json::json!({
        "event": "command_error",
        "kind": kind,
        "program": command.program,
        "args": sanitized_command_args(command),
        "error": error,
    }));
}

fn structured_log(value: serde_json::Value) {
    eprintln!("{value}");
}

fn sanitized_command_args(command: &nas_csi_vm_manager::CommandSpec) -> Vec<String> {
    let qemu_agent_index = command
        .args
        .iter()
        .position(|arg| arg == "qemu-agent-command");
    let mut redact_next = false;
    let mut sanitized = Vec::with_capacity(command.args.len());
    for (index, arg) in command.args.iter().enumerate() {
        if qemu_agent_index.is_some_and(|qemu_index| index > qemu_index + 1) {
            sanitized.push("[REDACTED_QEMU_AGENT_PAYLOAD]".to_string());
            continue;
        }
        if redact_next {
            sanitized.push("[REDACTED]".to_string());
            redact_next = false;
            continue;
        }
        if is_sensitive_command_flag(arg) {
            sanitized.push(arg.clone());
            redact_next = true;
            continue;
        }
        if let Some((key, _value)) = arg.split_once('=')
            && is_sensitive_command_flag(key)
        {
            sanitized.push(format!("{key}=[REDACTED]"));
            continue;
        }
        sanitized.push(sanitized_command_arg(arg));
    }
    sanitized
}

fn is_sensitive_command_flag(arg: &str) -> bool {
    matches!(
        arg,
        "--api-key"
            | "--apiKey"
            | "--api-key-file"
            | "--apiKeyFile"
            | "--kubeconfig"
            | "--password"
            | "--token"
    )
}

fn sanitized_command_arg(arg: &str) -> String {
    if arg.contains("NAS_CSI_CONTENT") {
        return "[REDACTED_FILE_CONTENT]".to_string();
    }
    if arg.len() > 512 {
        return format!("[REDACTED_LONG_ARG:{} bytes]", arg.len());
    }
    arg.to_string()
}

fn command_log_display(command: &nas_csi_vm_manager::CommandSpec) -> String {
    let sanitized = nas_csi_vm_manager::CommandSpec::new(
        command.program.clone(),
        sanitized_command_args(command),
    );
    sanitized.to_string()
}

trait CommandRunner {
    fn status(&self, command: &nas_csi_vm_manager::CommandSpec) -> Result<bool>;
    fn output(&self, command: &nas_csi_vm_manager::CommandSpec) -> Result<Option<String>>;

    fn run(&self, command: &nas_csi_vm_manager::CommandSpec) -> Result<()> {
        if self.status(command)? {
            Ok(())
        } else {
            anyhow::bail!("{} failed", command_log_display(command))
        }
    }
}

struct RealCommandRunner;

impl CommandRunner for RealCommandRunner {
    fn status(&self, command: &nas_csi_vm_manager::CommandSpec) -> Result<bool> {
        log_command_start("status", command);
        let status = match ProcessCommand::new(&command.program)
            .args(&command.args)
            .status()
        {
            Ok(status) => status,
            Err(error) => {
                log_command_error("status", command, &error.to_string());
                return Err(error).with_context(|| format!("failed to execute {command}"));
            }
        };
        log_command_finish("status", command, status.success(), status.code());
        Ok(status.success())
    }

    fn output(&self, command: &nas_csi_vm_manager::CommandSpec) -> Result<Option<String>> {
        log_command_start("output", command);
        let output = match ProcessCommand::new(&command.program)
            .args(&command.args)
            .output()
        {
            Ok(output) => output,
            Err(error) => {
                log_command_error("output", command, &error.to_string());
                return Err(error).with_context(|| format!("failed to execute {command}"));
            }
        };
        log_command_finish(
            "output",
            command,
            output.status.success(),
            output.status.code(),
        );
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
        | nas_csi_vm_manager::ReconcileOperation::ResizeRootDisk { command, .. }
        | nas_csi_vm_manager::ReconcileOperation::ReloadSystemdUnits { command }
        | nas_csi_vm_manager::ReconcileOperation::DefineDomain { command, .. }
        | nas_csi_vm_manager::ReconcileOperation::EnableDomainAutostart { command, .. }
        | nas_csi_vm_manager::ReconcileOperation::StartDomain { command, .. } => {
            runner.run(command)?;
        }
        nas_csi_vm_manager::ReconcileOperation::EnableAndStartVirtiofsdService {
            socket_path,
            command,
            ..
        }
        | nas_csi_vm_manager::ReconcileOperation::RestartVirtiofsdService {
            socket_path,
            command,
            ..
        } => {
            runner.run(command)?;
            wait_for_virtiofs_socket(socket_path)?;
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
    for node in &config.nodes {
        if let Some(source_image) = &node.root_disk.source_image {
            actual
                .paths
                .insert(source_image.clone(), inspect_path(source_image)?);
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
    if actual
        .tools
        .get(&apply_options.qemu_img_path)
        .map(nas_csi_vm_manager::ToolActualState::is_found)
        .unwrap_or(false)
    {
        for node in &config.nodes {
            if let Some(source_image) = &node.root_disk.source_image
                && actual
                    .paths
                    .get(source_image)
                    .map(nas_csi_vm_manager::PathActualState::exists)
                    .unwrap_or(false)
                && let Some(image) =
                    inspect_qemu_image(runner, &apply_options.qemu_img_path, source_image)?
            {
                actual.qemu_images.insert(source_image.clone(), image);
            }
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
        if let Some(source_image) = &node.root_disk.source_image {
            actual
                .paths
                .insert(source_image.clone(), inspect_path(source_image)?);
        }
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
            if let Some(source_image) = &node.root_disk.source_image
                && actual
                    .paths
                    .get(source_image)
                    .map(nas_csi_vm_manager::PathActualState::exists)
                    .unwrap_or(false)
                && let Some(image) =
                    inspect_qemu_image(runner, &apply_options.qemu_img_path, source_image)?
            {
                actual.qemu_images.insert(source_image.clone(), image);
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
            "- {}: exists={} managed={} active={} autostart={} desiredHash={} xmlHash={}",
            node.domain,
            domain.map(|domain| domain.exists).unwrap_or(false),
            domain.map(|domain| domain.managed).unwrap_or(false),
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

#[derive(Debug, serde::Serialize)]
struct HostHealthReport {
    status: String,
    tools: Vec<ToolHealth>,
    systemd_units: Vec<SystemdHealth>,
    libvirt_domains: Vec<DomainHealth>,
    virtiofs_sockets: Vec<SocketHealth>,
    datasets: Vec<DatasetHealth>,
}

#[derive(Debug, serde::Serialize)]
struct ToolHealth {
    name: String,
    program: String,
    status: String,
    found_path: Option<String>,
}

#[derive(Debug, serde::Serialize)]
struct SystemdHealth {
    unit_name: String,
    status: String,
    installed: bool,
    enabled: Option<bool>,
    active: Option<bool>,
}

#[derive(Debug, serde::Serialize)]
struct DomainHealth {
    domain: String,
    status: String,
    exists: bool,
    managed: bool,
    active: bool,
    autostart: Option<bool>,
}

#[derive(Debug, serde::Serialize)]
struct SocketHealth {
    node: String,
    export_id: String,
    path: String,
    status: String,
    exists: bool,
    socket: bool,
}

#[derive(Debug, serde::Serialize)]
struct DatasetHealth {
    export_id: String,
    dataset: String,
    source_path: String,
    status: String,
    exists: bool,
    mounted: bool,
}

fn build_health_report(
    config: &HostConfig,
    render_options: &nas_csi_vm_manager::ArtifactRenderOptions,
    actual: &nas_csi_vm_manager::HostActualState,
) -> Result<HostHealthReport> {
    let tools = [
        ("virtiofsd", config.host_tools.virtiofsd.as_str()),
        ("qemu-img", config.host_tools.qemu_img.as_str()),
        ("virsh", config.host_tools.virsh.as_str()),
        ("systemctl", config.host_tools.systemctl.as_str()),
    ]
    .into_iter()
    .map(|(name, program)| {
        let found_path = actual
            .tools
            .get(program)
            .and_then(|state| state.path.clone());
        ToolHealth {
            name: name.to_string(),
            program: program.to_string(),
            status: health_status(found_path.is_some()),
            found_path,
        }
    })
    .collect::<Vec<_>>();

    let mut systemd_units = Vec::new();
    let mut libvirt_domains = Vec::new();
    let mut virtiofs_sockets = Vec::new();
    for node in &config.nodes {
        libvirt_domains.push(domain_health(
            &node.domain,
            actual.domains.get(&node.domain),
        ));

        for export_id in &node.exports {
            let unit_name = format!(
                "{}.service",
                nas_csi_vm_manager::virtiofsd_service_name(&node.domain, export_id)
            );
            systemd_units.push(systemd_health(
                &unit_name,
                actual.systemd_units.get(&unit_name),
            ));

            let socket_path =
                nas_csi_vm_manager::virtiofs_socket_path(render_options, &node.domain, export_id);
            let (exists, socket) = inspect_socket_path(&socket_path);
            virtiofs_sockets.push(SocketHealth {
                node: node.name.clone(),
                export_id: export_id.clone(),
                path: socket_path,
                status: health_status(socket),
                exists,
                socket,
            });
        }
    }

    let mount_points = read_mount_points()?;
    let datasets = config
        .exports
        .iter()
        .map(|(export_id, export)| {
            let exists = Path::new(&export.source_path).exists();
            let mounted = mount_points.contains(&export.source_path);
            DatasetHealth {
                export_id: export_id.clone(),
                dataset: export.dataset.clone(),
                source_path: export.source_path.clone(),
                status: health_status(exists && mounted),
                exists,
                mounted,
            }
        })
        .collect::<Vec<_>>();

    let degraded = tools.iter().any(is_degraded)
        || systemd_units.iter().any(is_degraded)
        || libvirt_domains.iter().any(is_degraded)
        || virtiofs_sockets.iter().any(is_degraded)
        || datasets.iter().any(is_degraded);

    Ok(HostHealthReport {
        status: health_status(!degraded),
        tools,
        systemd_units,
        libvirt_domains,
        virtiofs_sockets,
        datasets,
    })
}

fn print_health_report(report: &HostHealthReport) {
    println!("health: {}", report.status);

    println!();
    println!("tools:");
    for tool in &report.tools {
        println!(
            "- {}: {} ({})",
            tool.name,
            tool.status,
            tool.found_path.as_deref().unwrap_or(tool.program.as_str())
        );
    }

    println!();
    println!("systemd units:");
    if report.systemd_units.is_empty() {
        println!("- none configured");
    }
    for unit in &report.systemd_units {
        println!(
            "- {}: {} installed={} enabled={} active={}",
            unit.unit_name,
            unit.status,
            unit.installed,
            optional_bool_label(unit.enabled),
            optional_bool_label(unit.active)
        );
    }

    println!();
    println!("libvirt domains:");
    if report.libvirt_domains.is_empty() {
        println!("- none configured");
    }
    for domain in &report.libvirt_domains {
        println!(
            "- {}: {} exists={} managed={} active={} autostart={}",
            domain.domain,
            domain.status,
            domain.exists,
            domain.managed,
            domain.active,
            optional_bool_label(domain.autostart)
        );
    }

    println!();
    println!("virtiofs sockets:");
    if report.virtiofs_sockets.is_empty() {
        println!("- none configured");
    }
    for socket in &report.virtiofs_sockets {
        println!(
            "- {}/{}: {} path={} exists={} socket={}",
            socket.node, socket.export_id, socket.status, socket.path, socket.exists, socket.socket
        );
    }

    println!();
    println!("mounted datasets:");
    if report.datasets.is_empty() {
        println!("- none configured");
    }
    for dataset in &report.datasets {
        println!(
            "- {}: {} dataset={} path={} exists={} mounted={}",
            dataset.export_id,
            dataset.status,
            dataset.dataset,
            dataset.source_path,
            dataset.exists,
            dataset.mounted
        );
    }
}

trait HealthItem {
    fn status(&self) -> &str;
}

impl HealthItem for ToolHealth {
    fn status(&self) -> &str {
        &self.status
    }
}

impl HealthItem for SystemdHealth {
    fn status(&self) -> &str {
        &self.status
    }
}

impl HealthItem for DomainHealth {
    fn status(&self) -> &str {
        &self.status
    }
}

impl HealthItem for SocketHealth {
    fn status(&self) -> &str {
        &self.status
    }
}

impl HealthItem for DatasetHealth {
    fn status(&self) -> &str {
        &self.status
    }
}

fn is_degraded(item: &impl HealthItem) -> bool {
    item.status() == "degraded"
}

fn systemd_health(
    unit_name: &str,
    unit: Option<&nas_csi_vm_manager::SystemdUnitActualState>,
) -> SystemdHealth {
    let installed = unit.and_then(|unit| unit.installed_hash.as_ref()).is_some();
    let enabled = unit.and_then(|unit| unit.enabled);
    let active = unit.and_then(|unit| unit.active);
    SystemdHealth {
        unit_name: unit_name.to_string(),
        status: health_status(installed && enabled == Some(true) && active == Some(true)),
        installed,
        enabled,
        active,
    }
}

fn domain_health(
    domain_name: &str,
    domain: Option<&nas_csi_vm_manager::DomainActualState>,
) -> DomainHealth {
    let exists = domain.map(|domain| domain.exists).unwrap_or(false);
    let managed = domain.map(|domain| domain.managed).unwrap_or(false);
    DomainHealth {
        domain: domain_name.to_string(),
        status: health_status(exists && managed),
        exists,
        managed,
        active: domain.map(|domain| domain.active).unwrap_or(false),
        autostart: domain.and_then(|domain| domain.autostart),
    }
}

fn health_status(ok: bool) -> String {
    if ok {
        "ok".to_string()
    } else {
        "degraded".to_string()
    }
}

fn inspect_socket_path(path: &str) -> (bool, bool) {
    match fs::symlink_metadata(path) {
        Ok(metadata) => (true, metadata.file_type().is_socket()),
        Err(error) if error.kind() == ErrorKind::NotFound => (false, false),
        Err(_) => (false, false),
    }
}

fn read_mount_points() -> Result<BTreeSet<String>> {
    let content = fs::read_to_string("/proc/self/mountinfo")
        .context("failed to read /proc/self/mountinfo")?;
    Ok(content.lines().filter_map(mountinfo_mount_point).collect())
}

fn mountinfo_mount_point(line: &str) -> Option<String> {
    line.split(' ').nth(4).map(mountinfo_unescape)
}

fn mountinfo_unescape(value: &str) -> String {
    let mut output = String::new();
    let mut chars = value.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            output.push(ch);
            continue;
        }

        let digits = [chars.next(), chars.next(), chars.next()];
        if let [Some(a), Some(b), Some(c)] = digits {
            let octal = [a, b, c].iter().collect::<String>();
            if let Ok(value) = u8::from_str_radix(&octal, 8) {
                output.push(value as char);
                continue;
            }
            output.push('\\');
            output.push_str(&octal);
        } else {
            output.push('\\');
            for digit in digits.into_iter().flatten() {
                output.push(digit);
            }
        }
    }
    output
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

fn sha256_hex(contents: &[u8]) -> String {
    let digest = Sha256::digest(contents);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
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
        return Ok(nas_csi_vm_manager::PathActualState {
            kind: nas_csi_vm_manager::PathActualKind::File,
            size: Some(contents.len() as u64),
            content_hash: Some(nas_csi_vm_manager::content_hash(&contents)),
            sha256: Some(sha256_hex(&contents)),
        });
    }
    Ok(nas_csi_vm_manager::PathActualState {
        kind: nas_csi_vm_manager::PathActualKind::Other,
        size: Some(metadata.len()),
        content_hash: None,
        sha256: None,
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
            managed: false,
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
    let managed = xml
        .as_deref()
        .map(nas_csi_vm_manager::extract_domain_managed)
        .unwrap_or(false);
    let xml_hash = xml
        .as_ref()
        .map(|xml| nas_csi_vm_manager::content_hash(xml.as_bytes()));

    Ok(nas_csi_vm_manager::DomainActualState {
        exists,
        managed,
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
    allowed_virtiofs_sockets: BTreeSet<PathBuf>,
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
        let mut allowed_virtiofs_sockets = BTreeSet::new();
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
                allowed_virtiofs_sockets.insert(checked_path(
                    &nas_csi_vm_manager::virtiofs_socket_path(
                        &render_options_from_config(config),
                        &node.domain,
                        export_id,
                    ),
                    "virtiofs socket",
                )?);
            }
        }

        Ok(Self {
            artifact_dir,
            systemd_unit_dir,
            allowed_systemd_units,
            allowed_root_dirs,
            allowed_root_disks,
            allowed_seed_images,
            allowed_virtiofs_sockets,
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
            nas_csi_vm_manager::ReconcileOperation::ResizeRootDisk { path, command, .. } => {
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
                socket_path,
                ..
            }
            | nas_csi_vm_manager::ReconcileOperation::RestartVirtiofsdService {
                unit_name,
                socket_path,
                ..
            } => {
                self.require_systemd_unit(unit_name)?;
                let socket_path = checked_path(socket_path, "virtiofs socket")?;
                self.require_exact(
                    &socket_path,
                    &self.allowed_virtiofs_sockets,
                    "virtiofs socket",
                )
            }
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
            allowed_virtiofs_sockets: BTreeSet::new(),
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

    #[cfg(test)]
    fn allow_virtiofs_socket(mut self, socket_path: &Path) -> Self {
        self.allowed_virtiofs_sockets
            .insert(socket_path.to_path_buf());
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

fn wait_for_virtiofs_socket(path: &str) -> Result<()> {
    wait_for_virtiofs_socket_with_timeout(path, Duration::from_secs(5))
}

fn wait_for_virtiofs_socket_with_timeout(path: &str, timeout: Duration) -> Result<()> {
    if path.is_empty() {
        anyhow::bail!("virtiofs socket path is empty");
    }

    let deadline = Instant::now() + timeout;
    loop {
        match fs::metadata(path) {
            Ok(metadata) if metadata.file_type().is_socket() => return Ok(()),
            Ok(_) => anyhow::bail!("{path} exists but is not a Unix socket"),
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("failed to inspect virtiofs socket {path}"));
            }
        }

        if Instant::now() >= deadline {
            anyhow::bail!("timed out waiting for virtiofs socket {path}");
        }
        thread::sleep(Duration::from_millis(100));
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
    use std::os::unix::net::UnixListener;

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

    #[test]
    fn execute_virtiofs_service_waits_for_socket() {
        let root = unique_test_dir("virtiofs-socket");
        let socket_path = root.join("repos.sock");
        let _listener = UnixListener::bind(&socket_path).expect("bind unix socket");
        let command = nas_csi_vm_manager::CommandSpec::new(
            "/usr/bin/systemctl",
            [
                "enable".to_string(),
                "--now".to_string(),
                "nascsi-virtiofsd-test.service".to_string(),
            ],
        );
        let runner = FakeCommandRunner::default().with_status(&command, true);
        let plan = nas_csi_vm_manager::HostReconcilePlan {
            steps: vec![nas_csi_vm_manager::ReconcileStep {
                description: "start virtiofsd".to_string(),
                kind: nas_csi_vm_manager::ReconcileStepKind::Apply(
                    nas_csi_vm_manager::ReconcileOperation::EnableAndStartVirtiofsdService {
                        unit_name: "nascsi-virtiofsd-test.service".to_string(),
                        socket_path: socket_path.display().to_string(),
                        command: command.clone(),
                    },
                ),
            }],
        };
        let safety = ExecuteSafety::for_test(&root.join("artifacts"), &root.join("systemd"))
            .allow_systemd_unit("nascsi-virtiofsd-test.service")
            .allow_virtiofs_socket(&socket_path);

        execute_reconcile_plan(&plan, &runner, &safety).expect("execute");

        assert_eq!(runner.status_calls.borrow().as_slice(), &[command]);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn parses_mountinfo_mount_point_escapes() {
        let line = "42 31 0:38 / /mnt/pool/repos\\040dev rw,relatime - zfs pool/repos rw";

        assert_eq!(
            mountinfo_mount_point(line).as_deref(),
            Some("/mnt/pool/repos dev")
        );
    }

    #[test]
    fn host_install_refuses_writes_under_exported_datasets() {
        let config = sample_host_config();
        let export = config.exports.get("repos").expect("repos export");
        let forbidden_path = Path::new(&export.source_path).join("vm-state");
        let plan = nas_csi_vm_manager::HostReconcilePlan {
            steps: vec![nas_csi_vm_manager::ReconcileStep {
                description: "write under exported dataset".to_string(),
                kind: nas_csi_vm_manager::ReconcileStepKind::Apply(
                    nas_csi_vm_manager::ReconcileOperation::EnsureDirectory {
                        path: forbidden_path.display().to_string(),
                    },
                ),
            }],
        };

        let error =
            verify_no_dataset_mutating_operations(&config, &plan).expect_err("dataset guard");

        assert!(error.to_string().contains("exported dataset"));
    }

    #[test]
    fn host_install_idempotence_rejects_remaining_apply_steps() {
        let plan = nas_csi_vm_manager::HostReconcilePlan {
            steps: vec![nas_csi_vm_manager::ReconcileStep {
                description: "still needs root disk".to_string(),
                kind: nas_csi_vm_manager::ReconcileStepKind::Apply(
                    nas_csi_vm_manager::ReconcileOperation::CreateRootDisk {
                        path: "/var/lib/nas-csi/node.qcow2".to_string(),
                        command: nas_csi_vm_manager::CommandSpec::new(
                            "qemu-img",
                            ["create".to_string()],
                        ),
                    },
                ),
            }],
        };

        let error = verify_post_install_idempotence(&plan).expect_err("not idempotent");

        assert!(error.to_string().contains("not idempotent"));
    }

    #[test]
    fn host_install_verifies_complete_post_apply_state() {
        let config = sample_host_config();
        let root = unique_test_dir("host-install-state");
        let mut render_options = render_options_from_config(&config);
        render_options.runtime_dir = root.join("run").display().to_string();
        let mut apply_options = apply_options_from_config(
            &config,
            &root.join("artifacts"),
            &root.join("systemd"),
            false,
            false,
        );
        apply_options.start_domains = true;
        let desired_apply =
            nas_csi_vm_manager::plan_host_apply(&config, &render_options, &apply_options)
                .expect("desired apply");
        let seed_hashes = expected_seed_hashes(&desired_apply);

        let mut actual = nas_csi_vm_manager::HostActualState::default();
        let mut socket_listeners = Vec::new();
        for node in &config.nodes {
            actual.paths.insert(
                node.root_disk.image.clone(),
                nas_csi_vm_manager::PathActualState::file(b"root-disk"),
            );
            actual.qemu_images.insert(
                node.root_disk.image.clone(),
                nas_csi_vm_manager::QemuImageActualState {
                    format: Some("qcow2".to_string()),
                    backing_file: node.root_disk.source_image.clone(),
                    virtual_size: Some(node.root_disk.size_gib * 1024 * 1024 * 1024),
                },
            );

            let seed_path =
                nas_csi_vm_manager::seed_image_path(&node.root_disk.image, &node.domain);
            actual.paths.insert(
                seed_path.clone(),
                nas_csi_vm_manager::PathActualState {
                    kind: nas_csi_vm_manager::PathActualKind::File,
                    size: Some(1),
                    content_hash: seed_hashes.get(&seed_path).cloned(),
                    sha256: None,
                },
            );

            actual.domains.insert(
                node.domain.clone(),
                nas_csi_vm_manager::DomainActualState {
                    exists: true,
                    managed: true,
                    active: true,
                    autostart: Some(node.autostart),
                    desired_hash: Some("desired".to_string()),
                    xml: Some("<domain/>".to_string()),
                    xml_hash: Some("xml".to_string()),
                },
            );

            for export_id in &node.exports {
                let unit_name = format!(
                    "{}.service",
                    nas_csi_vm_manager::virtiofsd_service_name(&node.domain, export_id)
                );
                actual.systemd_units.insert(
                    unit_name,
                    nas_csi_vm_manager::SystemdUnitActualState {
                        installed_hash: Some("unit".to_string()),
                        enabled: Some(true),
                        active: Some(true),
                    },
                );

                let socket_path = nas_csi_vm_manager::virtiofs_socket_path(
                    &render_options,
                    &node.domain,
                    export_id,
                );
                let socket_path = PathBuf::from(socket_path);
                fs::create_dir_all(socket_path.parent().expect("socket parent"))
                    .expect("create socket parent");
                socket_listeners.push(UnixListener::bind(&socket_path).expect("bind socket"));
            }
        }

        let health = HostHealthReport {
            status: "ok".to_string(),
            tools: Vec::new(),
            systemd_units: Vec::new(),
            libvirt_domains: Vec::new(),
            virtiofs_sockets: Vec::new(),
            datasets: Vec::new(),
        };

        verify_host_install_state(
            &config,
            &desired_apply,
            &apply_options,
            &render_options,
            &actual,
            &health,
            true,
        )
        .expect("complete state verifies");

        drop(socket_listeners);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn cluster_install_refuses_non_substrate_manifest() {
        let manifests = vec![DesiredManifest {
            name: "some-app".to_string(),
            path: "/tmp/app.yaml".to_string(),
            contents: "kind: Deployment\n".to_string(),
        }];

        let error = verify_substrate_manifest_scope(&manifests).expect_err("scope guard");

        assert!(error.to_string().contains("non-substrate"));
    }

    #[test]
    fn cluster_install_verifies_complete_cluster_state() {
        let mut config = sample_host_config();
        let root = unique_test_dir("cluster-install-state");
        config.cluster.token_file = root.join("k3s-token").display().to_string();
        config.cluster.kubeconfig_out = root.join("kubeconfig").display().to_string();

        fs::write(&config.cluster.token_file, "a".repeat(64)).expect("write token");
        fs::set_permissions(
            &config.cluster.token_file,
            fs::Permissions::from_mode(0o600),
        )
        .expect("token perms");
        fs::write(
            &config.cluster.kubeconfig_out,
            format!(
                "apiVersion: v1\nclusters:\n- cluster:\n    server: {}\n",
                config.cluster.api_server.endpoint
            ),
        )
        .expect("write kubeconfig");
        fs::set_permissions(
            &config.cluster.kubeconfig_out,
            fs::Permissions::from_mode(0o600),
        )
        .expect("kubeconfig perms");

        let manifests = vec![
            DesiredManifest {
                name: "metrics-server".to_string(),
                path: root.join("metrics-server.yaml").display().to_string(),
                contents: "kind: Deployment\n".to_string(),
            },
            DesiredManifest {
                name: "nas-csi".to_string(),
                path: root.join("nas-csi.yaml").display().to_string(),
                contents: "kind: CSIDriver\n".to_string(),
            },
        ];
        let options = cluster_options_from_config(&config, &root.join("rendered"), "kubectl");
        let mut nodes = BTreeMap::new();
        for node in &config.nodes {
            nodes.insert(
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
        let actual = ClusterActualState {
            token_present: true,
            kubeconfig_present: true,
            api_ready: true,
            nodes,
            applied_manifests: manifests
                .iter()
                .map(|manifest| (manifest.name.clone(), manifest.desired_hash()))
                .collect(),
        };

        verify_cluster_install_state(&config, &actual, &manifests, &options)
            .expect("cluster state verifies");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn cluster_install_rejects_remaining_apply_steps() {
        let plan = ClusterReconcilePlan {
            steps: vec![nas_csi_cluster_manager::ClusterReconcileStep {
                description: "wait for api".to_string(),
                kind: ClusterReconcileStepKind::Apply(ClusterOperation::WaitForClusterApi {
                    command: ClusterCommandSpec::new("kubectl", ["get".to_string()]),
                }),
            }],
        };

        let error = verify_cluster_install_idempotence(&plan).expect_err("not idempotent");

        assert!(error.to_string().contains("not idempotent"));
    }

    #[test]
    fn csi_manifest_guard_requires_lab_images() {
        let manifest = format!("image: {LAB_CONTROLLER_IMAGE}\n---\nimage: {LAB_NODE_IMAGE}\n");
        verify_nas_csi_manifest_uses_lab_images(&manifest).expect("lab images");

        let error = verify_nas_csi_manifest_uses_lab_images("image: nas-csi-controller:latest\n")
            .expect_err("missing lab image");

        assert!(error.to_string().contains(LAB_CONTROLLER_IMAGE));
    }

    #[test]
    fn renders_static_existing_dataset_pvs_for_rw_and_ro_exports() {
        let config = sample_host_config();

        let manifest = render_static_existing_dataset_manifest(&config, "default");

        assert!(manifest.contains("name: nas-csi-repos"));
        assert!(manifest.contains("volumeHandle: \"repos\""));
        assert!(manifest.contains("nas-csi.dev/dataset: \"tank/repos\""));
        assert!(manifest.contains("nas-csi.dev/sourcePath: \"/mnt/tank/repos\""));
        assert!(manifest.contains("nas-csi.dev/readOnly: \"false\""));
        assert!(manifest.contains("key: kubernetes.io/hostname"));
        assert!(manifest.contains("- \"server-1\""));
        assert!(manifest.contains("- \"agent-1\""));
        assert!(manifest.contains("name: nas-csi-samples"));
        assert!(manifest.contains("volumeHandle: \"samples\""));
        assert!(manifest.contains("nas-csi.dev/dataset: \"tank/samples\""));
        assert!(manifest.contains("ReadOnlyMany"));
        assert!(manifest.contains("nas-csi.dev/readOnly: \"true\""));
    }

    #[test]
    fn renders_smoke_pods_pinned_to_nodes_with_export() {
        let config = sample_host_config();
        let options = sample_csi_install_options();

        let manifest = render_csi_smoke_pod_manifest(&config, &options);

        assert!(manifest.contains("name: nas-csi-smoke-repos"));
        assert!(manifest.contains("nodeName: \"server-1\""));
        assert!(manifest.contains("claimName: nas-csi-repos"));
        assert!(manifest.contains("mountPath: /mnt/nas-csi/repos"));
        assert!(manifest.contains("claimName: nas-csi-samples"));
        assert!(manifest.contains("mountPath: /mnt/nas-csi/samples"));
        assert!(manifest.contains("readOnly: true"));
    }

    #[test]
    fn renders_missing_export_probe_against_unconfigured_handle() {
        let config = sample_host_config();
        let options = sample_csi_install_options();

        let manifest = render_missing_export_manifest(&config, &options);

        assert!(manifest.contains("name: nas-csi-nas-csi-missing-export"));
        assert!(manifest.contains("volumeHandle: nas-csi-missing-export"));
        assert!(manifest.contains("nodeName: \"server-1\""));
        assert!(manifest.contains("claimName: nas-csi-nas-csi-missing-export"));
    }

    #[test]
    fn workload_validation_selects_read_write_repo_and_read_only_content_exports() {
        let config = sample_host_config();
        let options = sample_workload_validation_options();

        let selection =
            select_workload_validation_exports(&config, &options).expect("select exports");

        assert_eq!(
            selection,
            WorkloadValidationSelection {
                repo_export: "repos".to_string(),
                content_export: "samples".to_string()
            }
        );
    }

    #[test]
    fn renders_workload_validation_pods_for_selected_static_pvcs() {
        let config = sample_host_config();
        let options = sample_workload_validation_options();
        let selection =
            select_workload_validation_exports(&config, &options).expect("select exports");

        let manifest =
            render_workload_validation_manifest(&config, &options, &selection).expect("manifest");

        assert!(manifest.contains("name: nas-csi-workload-repo-repos"));
        assert!(manifest.contains("name: nas-csi-workload-content-samples"));
        assert!(manifest.contains("claimName: nas-csi-repos"));
        assert!(manifest.contains("claimName: nas-csi-samples"));
        assert!(manifest.contains("mountPath: /work/repo"));
        assert!(manifest.contains("mountPath: /content"));
        assert!(manifest.contains("readOnly: true"));
        assert!(manifest.contains("httpd -f -p 8080 -h /content"));
    }

    #[test]
    fn workload_scripts_cover_repo_and_streaming_operations() {
        let repo_script = repository_workload_script();
        let content_script = content_streaming_workload_script();

        assert!(repo_script.contains("git -C \"$repo\" status --short"));
        assert!(repo_script.contains("npm install --ignore-scripts --no-audit --no-fund"));
        assert!(repo_script.contains("smallFilesWritten"));
        assert!(content_script.contains("dd if=\"$first_file\" of=/dev/null"));
        assert!(content_script.contains("wget -qO- http://127.0.0.1:8080/"));
    }

    #[test]
    fn extracts_virtiofs_cache_policy_from_domain_xml() {
        let xml = r#"
<domain>
  <devices>
    <filesystem type='mount' accessmode='passthrough'>
      <driver type='virtiofs' cache='always'/>
      <source socket='/run/nas-csi/repos.sock'/>
      <target dir='nascsi_repos'/>
    </filesystem>
    <filesystem type='mount' accessmode='passthrough'>
      <driver type='virtiofs'/>
      <target dir="nascsi_samples"/>
    </filesystem>
  </devices>
</domain>
"#;

        assert_eq!(extract_virtiofs_cache_policy(xml, "nascsi_repos"), "always");
        assert_eq!(
            extract_virtiofs_cache_policy(xml, "nascsi_samples"),
            "not-set"
        );
        assert_eq!(
            extract_virtiofs_cache_policy(xml, "nascsi_missing"),
            "not-found"
        );
    }

    #[test]
    fn csi_install_input_guard_rejects_unassigned_exports() {
        let mut config = sample_host_config();
        for node in &mut config.nodes {
            node.exports.retain(|export| export != "samples");
        }
        let options = sample_csi_install_options();

        let error = verify_csi_install_inputs(&config, &options).expect_err("unassigned export");

        assert!(error.to_string().contains("samples"));
    }

    #[test]
    fn safe_k8s_name_stays_in_dns_label_limit() {
        let name = safe_k8s_name("nas-csi", &"repo_".repeat(40));

        assert!(name.len() <= 63);
        assert!(name.starts_with("nas-csi-"));
        assert!(
            name.chars()
                .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
        );
    }

    #[test]
    fn parse_listing_output_ignores_empty_lines() {
        assert_eq!(
            parse_listing_output("alpha\n\nbeta\n"),
            vec!["alpha".to_string(), "beta".to_string()]
        );
    }

    #[test]
    fn command_log_args_redact_guest_agent_payloads_and_sensitive_flags() {
        let command = nas_csi_vm_manager::CommandSpec::new(
            "/usr/bin/virsh",
            [
                "-c".to_string(),
                "qemu:///system".to_string(),
                "qemu-agent-command".to_string(),
                "nascsi-node-1".to_string(),
                r#"{"execute":"guest-exec","arguments":{"arg":["secret-file-contents"]}}"#
                    .to_string(),
            ],
        );

        assert_eq!(
            sanitized_command_args(&command),
            vec![
                "-c",
                "qemu:///system",
                "qemu-agent-command",
                "nascsi-node-1",
                "[REDACTED_QEMU_AGENT_PAYLOAD]"
            ]
        );

        let command = nas_csi_vm_manager::CommandSpec::new(
            "kubectl",
            [
                "--kubeconfig".to_string(),
                "/etc/nas-csi/kubeconfig".to_string(),
                "--token=super-secret".to_string(),
                "get".to_string(),
                "nodes".to_string(),
            ],
        );

        assert_eq!(
            sanitized_command_args(&command),
            vec![
                "--kubeconfig",
                "[REDACTED]",
                "--token=[REDACTED]",
                "get",
                "nodes"
            ]
        );
    }

    fn sample_host_config() -> HostConfig {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("examples/configs/host.sample.yaml");
        load_yaml(&path).expect("load sample host config")
    }

    fn sample_csi_install_options() -> CsiInstallOptions {
        CsiInstallOptions {
            artifact_dir: PathBuf::from("/var/lib/nas-csi/rendered"),
            manifest_root: PathBuf::from("/usr/local/share/nas-csi/deploy"),
            kubectl: "kubectl".to_string(),
            namespace: "default".to_string(),
            smoke_image: "busybox:1.36".to_string(),
            wait_timeout: Duration::from_secs(600),
            execute: false,
        }
    }

    fn sample_workload_validation_options() -> WorkloadValidationOptions {
        WorkloadValidationOptions {
            artifact_dir: PathBuf::from("/var/lib/nas-csi/rendered"),
            kubectl: "kubectl".to_string(),
            namespace: "default".to_string(),
            repo_export: None,
            content_export: None,
            repo_image: "node:22-bookworm".to_string(),
            content_image: "busybox:1.36".to_string(),
            content_command: "httpd -f -p 8080 -h /content".to_string(),
            wait_timeout: Duration::from_secs(600),
            small_file_count: 200,
            keep_pods: false,
            execute: false,
        }
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
