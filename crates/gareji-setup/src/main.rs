//! `gareji` first-run setup and diagnosis CLI.

use std::env;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use gareji_bootstrap::{
    default_data_dir, discover_marketplace_root, doctor, setup, DoctorRequest, SetupReport,
    SetupRequest, StepStatus, SystemCommandRunner,
};

#[derive(Debug, Parser)]
#[command(
    name = "gareji",
    version,
    about = "Install, configure, and diagnose a local Gareji environment"
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
    let report = match &cli.command {
        Commands::Setup(args) => {
            let request = setup_request(args)?;
            setup(&request, &mut runner)?
        }
        Commands::Doctor(args) => doctor(
            &DoctorRequest {
                install_dir: args
                    .install_dir
                    .clone()
                    .map_or_else(default_install_dir, Ok)?,
                codex_binary: resolve_command(&args.codex_bin)?,
                project_id: args.project_id.clone(),
            },
            &mut runner,
        )?,
    };
    print_report(&report, cli.json)?;
    Ok(report.healthy)
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
        install_dir: args
            .install_dir
            .clone()
            .map_or_else(default_install_dir, Ok)?,
        codex_binary: resolve_command(&args.codex_bin)?,
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

fn print_report(report: &SetupReport, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string(report)?);
        return Ok(());
    }
    if let Some(project_id) = &report.project_id {
        println!("Gareji project: {project_id}");
    }
    for step in &report.steps {
        let status = match step.status {
            StepStatus::Ready => "ready",
            StepStatus::Changed => "changed",
            StepStatus::Planned => "planned",
            StepStatus::Missing => "missing",
        };
        println!("[{status}] {}: {}", step.step, step.message);
    }
    if report.restart_codex {
        println!("Next: restart Codex, then review and trust the Gareji Progress Stop Hook.");
    }
    Ok(())
}

fn core_binary_name() -> &'static str {
    if cfg!(windows) {
        "gareji-core.exe"
    } else {
        "gareji-core"
    }
}
