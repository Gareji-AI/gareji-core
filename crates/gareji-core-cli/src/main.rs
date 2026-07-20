//! Local Gareji Core bridge and project registration CLI.

mod board_adapter;

use std::env;
use std::fs;
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use gareji_contracts::{
    CoreBridgeError, CoreBridgeOperation, CoreBridgeRequest, CoreBridgeResponse, CoreErrorCode,
    ListProgressResult, ProjectRegistration, CORE_BRIDGE_PROTOCOL_VERSION,
};
use gareji_core::bridge::CoreBridge;
use gareji_core::registry::ProjectRegistry;

use crate::board_adapter::ProcessBoardAdapter;

const MAX_BRIDGE_LINE_BYTES: usize = 1_048_576;

#[derive(Debug, Parser)]
#[command(
    name = "gareji-core",
    version,
    about = "Gareji local trust-kernel bridge and project registry"
)]
struct Cli {
    /// Override the local application-data SQLite file.
    #[arg(long, global = true, env = "GAREJI_CORE_DB")]
    database: Option<PathBuf>,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Serve bounded newline-delimited JSON requests for one parent process.
    Bridge,
    /// Manage explicit local project registrations.
    Project {
        #[command(subcommand)]
        command: ProjectCommands,
    },
    /// Read bounded project progress as JSON.
    Progress {
        #[command(subcommand)]
        command: ProgressCommands,
    },
}

#[derive(Debug, Subcommand)]
enum ProjectCommands {
    /// Validate a registration JSON file without opening or mutating the registry.
    Validate {
        /// Path to a ProjectRegistration JSON object.
        #[arg(long)]
        file: PathBuf,
    },
    /// Create or replace a registration from a JSON file.
    Register {
        /// Path to a ProjectRegistration JSON object.
        #[arg(long)]
        file: PathBuf,
    },
    /// List complete local registrations as JSON.
    List,
}

#[derive(Debug, Subcommand)]
enum ProgressCommands {
    /// List the newest accepted Progress Checkpoints for one project.
    List {
        /// Stable registered project identity.
        #[arg(long)]
        project_id: String,
        /// Maximum number of checkpoints to return, from 1 through 100.
        #[arg(long, default_value_t = 5)]
        limit: u16,
        /// Return checkpoints older than this checkpoint identity.
        #[arg(long)]
        before: Option<String>,
    },
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("gareji-core: {error:#}");
            ExitCode::from(2)
        }
    }
}

fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Commands::Bridge => run_bridge(&database_path(cli.database.as_ref())?),
        Commands::Project { command } => match command {
            ProjectCommands::Validate { file } => validate_project(&file),
            ProjectCommands::Register { file } => {
                register_project(&database_path(cli.database.as_ref())?, &file)
            }
            ProjectCommands::List => list_projects(&database_path(cli.database.as_ref())?),
        },
        Commands::Progress { command } => match command {
            ProgressCommands::List {
                project_id,
                limit,
                before,
            } => list_progress(
                &database_path(cli.database.as_ref())?,
                project_id,
                limit,
                before,
            ),
        },
    }
}

fn database_path(configured: Option<&PathBuf>) -> Result<PathBuf> {
    configured.cloned().map_or_else(default_database_path, Ok)
}

fn validate_project(file: &PathBuf) -> Result<()> {
    let bytes = fs::read(file).context("could not read the project registration file")?;
    let registration: ProjectRegistration =
        serde_json::from_slice(&bytes).context("invalid project registration JSON")?;
    ProjectRegistry::validate(&registration)?;
    println!("valid project={}", registration.project_id);
    Ok(())
}

fn default_database_path() -> Result<PathBuf> {
    let base = if cfg!(windows) {
        environment_path("LOCALAPPDATA")
    } else if cfg!(target_os = "macos") {
        environment_path("HOME").map(|path| path.join("Library/Application Support"))
    } else {
        environment_path("XDG_DATA_HOME")
            .or_else(|| environment_path("HOME").map(|path| path.join(".local/share")))
    }
    .context("could not resolve the local application-data directory")?;
    Ok(base.join("Gareji").join("core.sqlite3"))
}

fn environment_path(name: &str) -> Option<PathBuf> {
    env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn register_project(database: &PathBuf, file: &PathBuf) -> Result<()> {
    let bytes = fs::read(file).context("could not read the project registration file")?;
    let registration: ProjectRegistration =
        serde_json::from_slice(&bytes).context("invalid project registration JSON")?;
    let mut registry = ProjectRegistry::open_sqlite(database)?;
    let replaced = registry.register(&registration)?;
    let action = if replaced { "updated" } else { "created" };
    println!("{action} project={}", registration.project_id);
    Ok(())
}

fn list_projects(database: &PathBuf) -> Result<()> {
    let registry = ProjectRegistry::open_sqlite(database)?;
    println!("{}", serde_json::to_string(&registry.list()?)?);
    Ok(())
}

fn list_progress(
    database: &PathBuf,
    project_id: String,
    limit: u16,
    before_checkpoint_id: Option<String>,
) -> Result<()> {
    if !(1..=100).contains(&limit) {
        bail!("progress limit must be from 1 through 100");
    }
    let mut bridge = CoreBridge::open_sqlite_with_board(
        database,
        Box::new(ProcessBoardAdapter::from_environment()),
    )?;
    let response = bridge.handle(CoreBridgeRequest {
        protocol_version: CORE_BRIDGE_PROTOCOL_VERSION.to_owned(),
        request_id: "gareji-core-progress-list".to_owned(),
        operation: CoreBridgeOperation::ListProgress {
            project_id,
            work_item_id: None,
            before_checkpoint_id,
            limit,
        },
    });
    match response {
        CoreBridgeResponse::Ok { result, .. } => {
            let result: ListProgressResult =
                serde_json::from_value(result).context("Core returned an invalid progress list")?;
            println!("{}", serde_json::to_string(&result)?);
            Ok(())
        }
        CoreBridgeResponse::Error { error, .. } => {
            bail!(
                "Core rejected progress list: {:?}: {}",
                error.code,
                error.message
            )
        }
    }
}

fn run_bridge(database: &PathBuf) -> Result<()> {
    let mut bridge = CoreBridge::open_sqlite_with_board(
        database,
        Box::new(ProcessBoardAdapter::from_environment()),
    )?;
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut reader = BufReader::new(stdin.lock());
    let mut writer = BufWriter::new(stdout.lock());
    let mut line = Vec::new();

    while read_bounded_line(&mut reader, &mut line)? {
        let response = match serde_json::from_slice::<CoreBridgeRequest>(&line) {
            Ok(request) => bridge.handle(request),
            Err(_) => CoreBridgeResponse::Error {
                protocol_version: CORE_BRIDGE_PROTOCOL_VERSION.to_owned(),
                request_id: String::new(),
                error: CoreBridgeError {
                    code: CoreErrorCode::InvalidRequest,
                    message: "invalid Core bridge request".to_owned(),
                },
            },
        };
        serde_json::to_writer(&mut writer, &response)?;
        writer.write_all(b"\n")?;
        writer.flush()?;
    }
    Ok(())
}

fn read_bounded_line(reader: &mut impl BufRead, output: &mut Vec<u8>) -> io::Result<bool> {
    output.clear();
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return Ok(!output.is_empty());
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let consumed = newline.map_or(available.len(), |position| position + 1);
        let payload_len = newline.unwrap_or(consumed);
        if output.len().saturating_add(payload_len) > MAX_BRIDGE_LINE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Core bridge request exceeds maximum size",
            ));
        }
        output.extend_from_slice(&available[..payload_len]);
        reader.consume(consumed);
        if newline.is_some() {
            return Ok(true);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    #[test]
    fn bounded_reader_accepts_multiple_lines_without_newline_bytes() {
        let mut input = Cursor::new(b"first\nsecond\n".to_vec());
        let mut output = Vec::new();
        assert!(read_bounded_line(&mut input, &mut output).unwrap());
        assert_eq!(output, b"first");
        assert!(read_bounded_line(&mut input, &mut output).unwrap());
        assert_eq!(output, b"second");
        assert!(!read_bounded_line(&mut input, &mut output).unwrap());
    }

    #[test]
    fn bounded_reader_rejects_an_oversized_request() {
        let mut input = Cursor::new(vec![b'x'; MAX_BRIDGE_LINE_BYTES + 1]);
        let mut output = Vec::new();
        assert_eq!(
            read_bounded_line(&mut input, &mut output)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn progress_list_rejects_an_out_of_range_limit() {
        let directory = tempfile::tempdir().unwrap();
        let error = list_progress(
            &directory.path().join("core.sqlite3"),
            "core".to_owned(),
            0,
            None,
        )
        .unwrap_err();
        assert!(error.to_string().contains("from 1 through 100"));
    }
}
