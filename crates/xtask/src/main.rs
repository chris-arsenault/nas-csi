use std::env;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const DEFAULT_IMAGE_REGISTRY: &str = "ghcr.io/chris-arsenault";
const DEFAULT_IMAGE_TAG: &str = "0.1.0-lab1";

fn main() -> ExitCode {
    let args = env::args().skip(1).collect::<Vec<_>>();
    let result = match args.first().map(String::as_str) {
        Some("check") if args.len() == 1 => run_check(),
        Some("package-host-agent") if args.len() == 1 => package_host_agent(),
        Some("build-images") => build_images(&args[1..]),
        Some("push-images") => push_images(&args[1..]),
        None => {
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
    eprintln!(
        "usage: cargo run -p nas-csi-xtask -- <check|package-host-agent|build-images|push-images> [--runtime docker|podman] [--registry REGISTRY] [--tag TAG]"
    );
}

fn run_check() -> Result<(), String> {
    run("cargo", &["fmt", "--all", "--check"])?;
    run("cargo", &["check", "--workspace"])?;
    run("cargo", &["test", "--workspace"])?;
    run("python3", &["hack/validate-examples.py"])?;
    Ok(())
}

fn run(program: &str, args: &[&str]) -> Result<(), String> {
    run_in(
        Path::new("."),
        program,
        &args.iter().map(|arg| arg.to_string()).collect::<Vec<_>>(),
    )
}

fn run_in(workdir: &Path, program: &str, args: &[String]) -> Result<(), String> {
    let status = Command::new(program)
        .current_dir(workdir)
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

#[derive(Clone, Debug, PartialEq, Eq)]
struct ImageOptions {
    runtime: String,
    registry: String,
    tag: String,
}

impl Default for ImageOptions {
    fn default() -> Self {
        Self {
            runtime: env::var("NAS_CSI_CONTAINER_RUNTIME").unwrap_or_else(|_| "docker".to_string()),
            registry: env::var("NAS_CSI_IMAGE_REGISTRY")
                .unwrap_or_else(|_| DEFAULT_IMAGE_REGISTRY.to_string()),
            tag: env::var("NAS_CSI_IMAGE_TAG").unwrap_or_else(|_| DEFAULT_IMAGE_TAG.to_string()),
        }
    }
}

impl ImageOptions {
    fn controller_image(&self) -> String {
        format!("{}/nas-csi-controller:{}", self.registry, self.tag)
    }

    fn node_image(&self) -> String {
        format!("{}/nas-csi-node:{}", self.registry, self.tag)
    }
}

fn parse_image_options(args: &[String]) -> Result<ImageOptions, String> {
    let mut options = ImageOptions::default();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--runtime" => {
                index += 1;
                options.runtime = args
                    .get(index)
                    .cloned()
                    .ok_or_else(|| "--runtime requires a value".to_string())?;
            }
            "--registry" => {
                index += 1;
                options.registry = args
                    .get(index)
                    .cloned()
                    .ok_or_else(|| "--registry requires a value".to_string())?;
            }
            "--tag" => {
                index += 1;
                options.tag = args
                    .get(index)
                    .cloned()
                    .ok_or_else(|| "--tag requires a value".to_string())?;
            }
            value => return Err(format!("unknown image option: {value}")),
        }
        index += 1;
    }

    if options.runtime.is_empty() {
        return Err("container runtime must not be empty".to_string());
    }
    if options.registry.is_empty() {
        return Err("image registry must not be empty".to_string());
    }
    if options.tag.is_empty() {
        return Err("image tag must not be empty".to_string());
    }

    Ok(options)
}

fn build_images(args: &[String]) -> Result<(), String> {
    let options = parse_image_options(args)?;
    let root = workspace_root()?;

    build_image(
        &root,
        &options.runtime,
        "deploy/images/controller.Dockerfile",
        &options.controller_image(),
    )?;
    build_image(
        &root,
        &options.runtime,
        "deploy/images/node.Dockerfile",
        &options.node_image(),
    )?;

    println!("built {}", options.controller_image());
    println!("built {}", options.node_image());
    Ok(())
}

fn build_image(root: &Path, runtime: &str, dockerfile: &str, image: &str) -> Result<(), String> {
    run_in(
        root,
        runtime,
        &[
            "build".to_string(),
            "-f".to_string(),
            dockerfile.to_string(),
            "-t".to_string(),
            image.to_string(),
            ".".to_string(),
        ],
    )
}

fn push_images(args: &[String]) -> Result<(), String> {
    let options = parse_image_options(args)?;
    push_image(&options.runtime, &options.controller_image())?;
    push_image(&options.runtime, &options.node_image())?;

    println!("pushed {}", options.controller_image());
    println!("pushed {}", options.node_image());
    Ok(())
}

fn push_image(runtime: &str, image: &str) -> Result<(), String> {
    run_in(
        Path::new("."),
        runtime,
        &["push".to_string(), image.to_string()],
    )
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

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn parses_image_options_overrides() {
        let options = parse_image_options(&strings(&[
            "--runtime",
            "podman",
            "--registry",
            "registry.example/nas",
            "--tag",
            "dev-test",
        ]))
        .expect("parse image options");

        assert_eq!(
            options,
            ImageOptions {
                runtime: "podman".to_string(),
                registry: "registry.example/nas".to_string(),
                tag: "dev-test".to_string(),
            }
        );
        assert_eq!(
            options.controller_image(),
            "registry.example/nas/nas-csi-controller:dev-test"
        );
        assert_eq!(
            options.node_image(),
            "registry.example/nas/nas-csi-node:dev-test"
        );
    }

    #[test]
    fn rejects_missing_image_option_value() {
        let error = parse_image_options(&strings(&["--tag"])).expect_err("missing tag value");

        assert_eq!(error, "--tag requires a value");
    }

    #[test]
    fn rejects_unknown_image_option() {
        let error =
            parse_image_options(&strings(&["--unknown", "value"])).expect_err("unknown option");

        assert_eq!(error, "unknown image option: --unknown");
    }
}
