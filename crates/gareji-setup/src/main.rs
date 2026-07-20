//! `gareji` first-run setup and diagnosis CLI.

use std::env;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use gareji_bootstrap::{
    core_binary_name, default_data_dir, discover_marketplace_root, doctor, setup, status,
    DoctorRequest, InstallationContext, SetupReport, SetupRequest, StatusReport, StatusRequest,
    StepStatus, SystemCommandRunner,
};

#[derive(Debug, Parser)]
#[command(
    name = "gareji",
    version,
    about = "Install, configure, and inspect a local Gareji environment"
)]
struct Cli {
    /// Emit one machine-readable JSON report.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Install Core, register a project, and install the Codex Plugin.
    Setup(SetupArgs),
    /// Diagnose the installed Core, registration, and Codex Plugin.
    Doctor(DoctorArgs),
    /// Show project health, context references, and recent Codex progress.
    Status(StatusArgs),
}

#[derive(Debug, Args)]
struct SetupArgs {
    /// Absolute project workspace directory.
    #[arg(long)]
    workspace: PathBuf,
    /// Absolute Markdown context path. Repeat for multiple files.
    #[arg(long, required = true)]
    context: Vec<PathBuf>,
    /// Stable project identity. Defaults to a slug of the workspace name.
    #[arg(long)]
    project_id: Option<String>,
    /// Human-readable project name. Defaults to the workspace name.
    #[arg(long)]
    name: Option<String>,
    /// Gareji Core binary to install. Defaults to the sibling binary or PATH.
    #[arg(long, env = "GAREJI_CORE_SOURCE")]
    core_source: Option<PathBuf>,
    /// Override the Gareji application-data install directory.
    #[arg(long)]
    install_dir: Option<PathBuf>,
    /// Codex CLI executable.
    #[arg(long, default_value = "codex")]
    codex_bin: PathBuf,
    /// Local marketplace root. Defaults to the nearest Gareji distribution root.
    #[arg(long)]
    marketplace_root: Option<PathBuf>,
    /// Report intended changes without writing or installing anything.
    #[arg(long)]
    dry_run: bool,
    /// Replace a differing registration with the same project identity.
    #[arg(long)]
    replace_project: bool,
}

#[derive(Debug, Args)]
struct DoctorArgs {
    /// Optional project identity that should be registered.
    #[arg(long)]
    project_id: Option<String>,
    /// Override the Gareji application-data install directory.
    #[arg(long)]
    install_dir: Option<PathBuf>,
    /// Codex CLI executable.
    #[arg(long, default_value = "codex")]
    codex_bin: PathBuf,
}

#[derive(Debug, Args)]
struct StatusArgs {
    /// Registered project workspace. Defaults to the current directory.
    #[arg(long, default_value = ".")]
    workspace: PathBuf,
    /// Override the Gareji application-data install directory.
    #[arg(long)]
    install_dir: Option<PathBuf>,
    /// Codex CLI executable.
    #[arg(long, default_value = "codex")]
    codex_bin: PathBuf,
    /// Maximum number of recent Progress Checkpoints to show.
    #[arg(long, default_value_t = 5)]
    limit: u16,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(&cli) {
        Ok(healthy) => {
            if healthy {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            }
        }
        Err(error) => {
            eprintln!("gareji: {error:#}");
            ExitCode::from(2)
        }
    }
}

fn run(cli: &Cli) -> Result<bool> {
    let mut runner = SystemCommandRunner;
    match &cli.command {
        Commands::Setup(args) => {
            let request = setup_request(args)?;
            let report = setup(&request, &mut runner)?;
            print_setup_report(&report, cli.json)?;
            Ok(report.healthy)
        }
        Commands::Doctor(args) => {
            let report = doctor(
                &DoctorRequest {
                    installation: InstallationContext {
                        install_dir: args
                            .install_dir
                            .clone()
                            .map_or_else(default_install_dir, Ok)?,
                        codex_binary: resolve_command(&args.codex_bin)?,
                    },
                    project_id: args.project_id.clone(),
                },
                &mut runner,
            )?;
            print_setup_report(&report, cli.json)?;
            Ok(report.healthy)
        }
        Commands::Status(args) => {
            let report = status(
                &StatusRequest {
                    workspace: args.workspace.clone(),
                    installation: InstallationContext {
                        install_dir: args
                            .install_dir
                            .clone()
                            .map_or_else(default_install_dir, Ok)?,
                        codex_binary: resolve_command(&args.codex_bin)?,
                    },
                    limit: args.limit,
                },
                &mut runner,
            )?;
            print_status_report(&report, cli.json)?;
            Ok(report.health.healthy)
        }
    }
}

fn setup_request(args: &SetupArgs) -> Result<SetupRequest> {
    let executable = env::current_exe().context("could not resolve the gareji executable")?;
    let marketplace_root = match &args.marketplace_root {
        Some(path) => path.clone(),
        None => discover_marketplace_root(&executable)
            .or_else(|| {
                env::current_dir()
                    .ok()
                    .and_then(|directory| discover_marketplace_root(&directory))
            })
            .context("could not discover the Gareji marketplace; pass --marketplace-root")?,
    };
    Ok(SetupRequest {
        project_id: args.project_id.clone(),
        project_name: args.name.clone(),
        workspace: args.workspace.clone(),
        context_paths: args.context.clone(),
        core_source: match &args.core_source {
            Some(path) => path.clone(),
            None => discover_core_source(&executable)?,
        },
        installation: InstallationContext {
            install_dir: args
                .install_dir
                .clone()
                .map_or_else(default_install_dir, Ok)?,
            codex_binary: resolve_command(&args.codex_bin)?,
        },
        marketplace_root,
        dry_run: args.dry_run,
        replace_project: args.replace_project,
    })
}

fn default_install_dir() -> Result<PathBuf> {
    Ok(default_data_dir()?.join("bin"))
}

fn discover_core_source(setup_executable: &Path) -> Result<PathBuf> {
    let sibling = setup_executable
        .parent()
        .context("gareji executable has no parent directory")?
        .join(core_binary_name());
    if sibling.is_file() {
        return Ok(sibling);
    }
    which::which(core_binary_name())
        .context("could not find gareji-core beside gareji or on PATH; pass --core-source")
}

fn resolve_command(command: &Path) -> Result<PathBuf> {
    if command.components().count() > 1 || command.is_absolute() {
        if !command.is_file() {
            bail!("command path does not exist: {}", command.display());
        }
        return command
            .canonicalize()
            .context("could not canonicalize command path");
    }
    which::which(command).with_context(|| format!("could not find {} on PATH", command.display()))
}

fn print_setup_report(report: &SetupReport, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string(report)?);
        return Ok(());
    }
    if let Some(project_id) = &report.project_id {
        println!("Gareji project: {project_id}");
    }
    for step in &report.steps {
        let status = step_status_label(step.status.clone());
        println!("[{status}] {}: {}", step.step, step.message);
    }
    if report.restart_codex {
        println!("Next: restart Codex, then review and trust the Gareji Progress Stop Hook.");
    } else if report
        .steps
        .iter()
        .any(|step| step.status == StepStatus::Planned)
    {
        println!("Next: rerun this command without --dry-run to apply the setup.");
    } else if report.healthy && report.project_id.is_some() {
        println!("Next: work in Codex, then run `gareji status` from the project workspace.");
    }
    Ok(())
}

fn print_status_report(report: &StatusReport, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string(report)?);
        return Ok(());
    }
    println!("{}", report.project_name);
    println!("Project: {}", report.project_id);
    println!("Workspace: {}", human_windows_path(&report.workspace));
    println!("Context references: {}", report.context_references);
    println!();
    println!("System");
    for step in &report.health.steps {
        let status = step_status_label(step.status.clone());
        println!("  [{status}] {}", step.message);
    }
    println!();
    println!("Recent Codex progress");
    if report.recent_activity.is_empty() {
        println!("  No progress recorded yet.");
        println!("  Next: complete a Codex turn in this workspace.");
        return Ok(());
    }
    for activity in &report.recent_activity {
        println!(
            "  {} [{}] {}",
            activity.recorded_at, activity.outcome, activity.summary
        );
        for path in activity.changed_paths.iter().take(5) {
            println!("    - {path}");
        }
        if activity.changed_paths.len() > 5 {
            println!("    - ... and {} more", activity.changed_paths.len() - 5);
        }
    }
    Ok(())
}

fn step_status_label(status: StepStatus) -> &'static str {
    match status {
        StepStatus::Ready => "ready",
        StepStatus::Changed => "changed",
        StepStatus::Planned => "planned",
        StepStatus::Missing => "missing",
    }
}

fn human_windows_path(path: &str) -> String {
    if let Some(path) = path.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{path}");
    }
    path.strip_prefix(r"\\?\").unwrap_or(path).to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_windows_path_removes_verbatim_prefixes() {
        assert_eq!(
            human_windows_path(r"\\?\C:\Garage\Project"),
            r"C:\Garage\Project"
        );
        assert_eq!(
            human_windows_path(r"\\?\UNC\server\share\Project"),
            r"\\server\share\Project"
        );
        assert_eq!(human_windows_path("/work/project"), "/work/project");
    }
}
