//! Build orchestration, as a plain Rust binary so no extra tool is needed on any
//! platform. Run it through the cargo alias: `cargo xtask <task>`.
//!
//! Deliberately has no dependencies: it should compile in a second.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const IMAGE: &str = "raptureoss/mock-s3";

fn main() -> ExitCode {
    let task = std::env::args().nth(1);

    let result = match task.as_deref() {
        Some("dist") => dist(),
        Some("docker") => docker(),
        Some("console") => console(),
        Some(other) => Err(format!("unknown task: {other}\n\n{}", usage())),
        None => Err(usage()),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

fn usage() -> String {
    format!(
        "tasks:\n  \
         dist      build the console, then the release binary with it embedded\n  \
         console   build the console only\n  \
         docker    build the container image ({IMAGE}:dev)\n"
    )
}

fn workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is xtask/, so the root is one level up.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask is never the workspace root")
        .to_path_buf()
}

fn run(program: &str, args: &[&str], cwd: &Path) -> Result<(), String> {
    println!("==> {program} {}", args.join(" "));

    let status = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .status()
        .map_err(|error| format!("could not start {program}: {error}"))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("{program} exited with {status}"))
    }
}

/// The console is optional while it is being built out, so a missing `web/` is a notice
/// rather than a failure.
fn console() -> Result<(), String> {
    let root = workspace_root();
    let web = root.join("web");

    if !web.join("package.json").exists() {
        println!("note: web/package.json not found, skipping the console build");
        return Ok(());
    }

    let pnpm = if cfg!(windows) { "pnpm.cmd" } else { "pnpm" };
    run(pnpm, &["install", "--frozen-lockfile"], &web)?;
    run(pnpm, &["run", "build"], &web)
}

fn dist() -> Result<(), String> {
    console()?;
    run("cargo", &["build", "--release"], &workspace_root())
}

fn docker() -> Result<(), String> {
    run(
        "docker",
        &["build", "-t", &format!("{IMAGE}:dev"), "."],
        &workspace_root(),
    )
}
