use std::env;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let args = env::args().skip(1).collect::<Vec<_>>();
    let result = match args.as_slice() {
        [command] if command == "check" => run_check(),
        [command] if command == "package-host-agent" => package_host_agent(),
        [] => {
            print_usage();
            Ok(())
        }
        _ => {
            print_usage();
            Err("unknown xtask command".to_string())
        }
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn print_usage() {
    eprintln!("usage: cargo run -p nas-csi-xtask -- <check|package-host-agent>");
}

fn run_check() -> Result<(), String> {
    run("cargo", &["fmt", "--all", "--check"])?;
    run("cargo", &["check", "--workspace"])?;
    run("cargo", &["test", "--workspace"])?;
    run("python3", &["hack/validate-examples.py"])?;
    Ok(())
}

fn run(program: &str, args: &[&str]) -> Result<(), String> {
    let status = Command::new(program)
        .args(args)
        .status()
        .map_err(|error| format!("failed to run {program}: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "{} {} failed with {status}",
            program,
            args.join(" ")
        ))
    }
}

fn package_host_agent() -> Result<(), String> {
    run("cargo", &["build", "--release", "-p", "nas-csi-host-agent"])?;

    let root = workspace_root()?;
    let package_dir = root.join("dist/host-agent");
    if package_dir.exists() {
        fs::remove_dir_all(&package_dir)
            .map_err(|error| format!("failed to remove {}: {error}", package_dir.display()))?;
    }

    fs::create_dir_all(package_dir.join("bin"))
        .map_err(|error| format!("failed to create package directory: {error}"))?;
    fs::copy(
        root.join("target/release/nas-csi-host-agent"),
        package_dir.join("bin/nas-csi-host-agent"),
    )
    .map_err(|error| format!("failed to copy host-agent binary: {error}"))?;

    for file in [
        "nas-csi-host-agent.service",
        "nas-csi-host-agent.env",
        "install.sh",
        "uninstall.sh",
        "README.md",
    ] {
        fs::copy(
            root.join("deploy/systemd").join(file),
            package_dir.join(file),
        )
        .map_err(|error| format!("failed to copy {file}: {error}"))?;
    }
    copy_dir(
        &root.join("deploy/addons"),
        &package_dir.join("deploy/addons"),
    )?;
    copy_dir(
        &root.join("deploy/kubernetes"),
        &package_dir.join("deploy/kubernetes"),
    )?;

    set_executable(package_dir.join("bin/nas-csi-host-agent"))?;
    set_executable(package_dir.join("install.sh"))?;
    set_executable(package_dir.join("uninstall.sh"))?;

    println!("packaged host agent under {}", package_dir.display());
    Ok(())
}

fn copy_dir(source: &Path, destination: &Path) -> Result<(), String> {
    fs::create_dir_all(destination)
        .map_err(|error| format!("failed to create {}: {error}", destination.display()))?;
    for entry in fs::read_dir(source)
        .map_err(|error| format!("failed to read {}: {error}", source.display()))?
    {
        let entry =
            entry.map_err(|error| format!("failed to read {} entry: {error}", source.display()))?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if source_path.is_dir() {
            copy_dir(&source_path, &destination_path)?;
        } else {
            fs::copy(&source_path, &destination_path).map_err(|error| {
                format!(
                    "failed to copy {} to {}: {error}",
                    source_path.display(),
                    destination_path.display()
                )
            })?;
        }
    }
    Ok(())
}

fn workspace_root() -> Result<PathBuf, String> {
    let output = Command::new("cargo")
        .args(["metadata", "--no-deps", "--format-version=1"])
        .output()
        .map_err(|error| format!("failed to run cargo metadata: {error}"))?;
    if !output.status.success() {
        return Err(format!("cargo metadata failed with {}", output.status));
    }
    let value: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("failed to parse cargo metadata: {error}"))?;
    value
        .get("workspace_root")
        .and_then(serde_json::Value::as_str)
        .map(PathBuf::from)
        .ok_or_else(|| "cargo metadata did not include workspace_root".to_string())
}

fn set_executable(path: impl AsRef<Path>) -> Result<(), String> {
    let path = path.as_ref();
    #[cfg(unix)]
    {
        let mut permissions = fs::metadata(path)
            .map_err(|error| format!("failed to stat {}: {error}", path.display()))?
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions)
            .map_err(|error| format!("failed to chmod {}: {error}", path.display()))?;
    }
    Ok(())
}
