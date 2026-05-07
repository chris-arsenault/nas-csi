use std::env;
use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let args = env::args().skip(1).collect::<Vec<_>>();
    let result = match args.as_slice() {
        [command] if command == "check" => run_check(),
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
    eprintln!("usage: cargo run -p nas-csi-xtask -- check");
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
