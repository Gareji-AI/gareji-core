//! AgentMesh CLI entrypoint (Phase 0 host + App v0 validate/run).

use agentmesh_app::{
    default_toolchain_cache_root, install_toolchain_bundle, prepare_app_run, validate_app_bundle,
    write_run_marker, AppRunMode, AppRunRequest, ResolveMode,
};
use agentmesh_host::audit::FsAuditStore;
use agentmesh_host::lifecycle::{CancellationToken, RunConfig};
use agentmesh_host::sidecar::{CompactSink, CompactSinkError};
use agentmesh_host::{execute_run, execute_run_with};
use agentmesh_proto::{CompactDiagnostic, CompactEnvelope, FailureCode, Limits};
use clap::{Parser, Subcommand};
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Debug, Default)]
struct BufferCompactSink {
    bytes: Vec<u8>,
}

impl CompactSink for BufferCompactSink {
    fn write_all(&mut self, bytes: &[u8]) -> Result<(), CompactSinkError> {
        self.bytes.extend_from_slice(bytes);
        Ok(())
    }
}

#[derive(Debug, Parser)]
#[command(
    name = "agentmesh",
    version,
    about = "AgentMesh one-shot host and App tooling"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
    /// Increase host tracing on stderr (never prints plugin stderr).
    #[arg(long, global = true)]
    verbose: bool,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Execute a one-shot plugin initialize/run/close lifecycle.
    Run {
        /// Absolute path to a native plugin executable.
        #[arg(long)]
        plugin: PathBuf,
        /// Path to input JSON (required).
        #[arg(long)]
        input: PathBuf,
        /// Parent directory for audit sidecars.
        #[arg(long)]
        sidecar_dir: PathBuf,
        /// Allowlisted environment variable NAMES (values from parent env).
        #[arg(long = "plugin-env", value_name = "KEY")]
        plugin_env: Vec<String>,
        /// RFC 6901 JSON Pointers to redact in audit records.
        #[arg(long = "redact-pointer", value_name = "POINTER")]
        redact_pointer: Vec<String>,
        /// Store bounded raw plugin stderr in the owner-only sidecar.
        #[arg(long)]
        capture_plugin_stderr: bool,
        /// Run timeout in milliseconds (1000..=3600000).
        #[arg(long)]
        run_timeout_ms: Option<u64>,
    },
    /// AgentMesh App authoring / packaging commands.
    App {
        #[command(subcommand)]
        command: AppCommands,
    },
    /// Local toolchain cache install commands.
    Toolchain {
        #[command(subcommand)]
        command: ToolchainCommands,
    },
}

#[derive(Debug, Subcommand)]
enum AppCommands {
    /// Validate `agentmesh-app.toml` with a version-controlled toolchain pin.
    Validate {
        /// Path to `agentmesh-app.toml`.
        #[arg(long)]
        manifest: PathBuf,
        /// Path to toolchain pin TOML (tag/commit/target/manifest hash).
        #[arg(long = "toolchain-pin")]
        toolchain_pin: PathBuf,
        /// Emit machine-readable JSON report on stdout.
        #[arg(long)]
        json: bool,
    },
    /// Run an App via pin→cache resolve (or development `--dev-plugin` override).
    Run {
        /// Path to `agentmesh-app.toml`.
        #[arg(long)]
        manifest: PathBuf,
        /// Path to toolchain pin TOML.
        #[arg(long = "toolchain-pin")]
        toolchain_pin: PathBuf,
        /// Path to input JSON.
        #[arg(long)]
        input: PathBuf,
        /// Parent directory for audit sidecars.
        #[arg(long)]
        sidecar_dir: PathBuf,
        /// Run mode: `production` (default) or `development`.
        #[arg(long, default_value = "production")]
        mode: String,
        /// Absolute path override for local plugin development (rejected in production).
        #[arg(long = "dev-plugin")]
        dev_plugin: Option<PathBuf>,
        /// Override toolchain cache root (default: `~/.agentmesh/toolchains` or env).
        #[arg(long = "toolchain-cache")]
        toolchain_cache: Option<PathBuf>,
        /// RFC 6901 JSON Pointers to redact in audit records.
        #[arg(long = "redact-pointer", value_name = "POINTER")]
        redact_pointer: Vec<String>,
        /// Run timeout override in milliseconds (1000..=3600000).
        #[arg(long)]
        run_timeout_ms: Option<u64>,
    },
}

#[derive(Debug, Subcommand)]
enum ToolchainCommands {
    /// Atomically install a verified toolchain bundle into the local cache.
    Install {
        /// Path to an extracted toolchain bundle directory (`release-manifest.json` + `bin/`).
        #[arg(long)]
        bundle: PathBuf,
        /// Override toolchain cache root (default: `~/.agentmesh/toolchains` or env).
        #[arg(long = "toolchain-cache")]
        toolchain_cache: Option<PathBuf>,
        /// Emit machine-readable JSON report on stdout.
        #[arg(long)]
        json: bool,
    },
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    init_tracing(cli.verbose);

    match cli.command {
        Commands::Run {
            plugin,
            input,
            sidecar_dir,
            plugin_env,
            redact_pointer,
            capture_plugin_stderr,
            run_timeout_ms,
        } => match run_command(
            plugin,
            input,
            sidecar_dir,
            plugin_env,
            redact_pointer,
            capture_plugin_stderr,
            run_timeout_ms,
        )
        .await
        {
            Ok(code) => ExitCode::from(u8::try_from(code).unwrap_or(70)),
            Err(code) => ExitCode::from(u8::try_from(code).unwrap_or(70)),
        },
        Commands::App { command } => match command {
            AppCommands::Validate {
                manifest,
                toolchain_pin,
                json,
            } => app_validate_command(manifest, toolchain_pin, json),
            AppCommands::Run {
                manifest,
                toolchain_pin,
                input,
                sidecar_dir,
                mode,
                dev_plugin,
                toolchain_cache,
                redact_pointer,
                run_timeout_ms,
            } => {
                match app_run_command(
                    manifest,
                    toolchain_pin,
                    input,
                    sidecar_dir,
                    mode,
                    dev_plugin,
                    toolchain_cache,
                    redact_pointer,
                    run_timeout_ms,
                )
                .await
                {
                    Ok(code) => ExitCode::from(u8::try_from(code).unwrap_or(70)),
                    Err(code) => ExitCode::from(u8::try_from(code).unwrap_or(70)),
                }
            }
        },
        Commands::Toolchain { command } => match command {
            ToolchainCommands::Install {
                bundle,
                toolchain_cache,
                json,
            } => toolchain_install_command(bundle, toolchain_cache, json),
        },
    }
}

fn init_tracing(verbose: bool) {
    let filter = if verbose { "info" } else { "warn" };
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}

fn toolchain_install_command(
    bundle: PathBuf,
    toolchain_cache: Option<PathBuf>,
    json: bool,
) -> ExitCode {
    let cache = match toolchain_cache {
        Some(path) => path,
        None => match default_toolchain_cache_root() {
            Ok(path) => path,
            Err(err) => {
                eprintln!("agentmesh toolchain install: {err}");
                return ExitCode::from(2);
            }
        },
    };
    match install_toolchain_bundle(&bundle, &cache) {
        Ok(report) => {
            if json {
                let payload = serde_json::json!({
                    "ok": true,
                    "tag": report.tag,
                    "target": report.target,
                    "install_dir": report.install_dir,
                    "release_manifest_sha256": report.release_manifest_sha256,
                    "binary_count": report.binary_count,
                });
                println!("{payload}");
            } else {
                println!(
                    "ok installed tag={} target={} binaries={} release_manifest_sha256={}",
                    report.tag, report.target, report.binary_count, report.release_manifest_sha256
                );
            }
            ExitCode::SUCCESS
        }
        Err(err) => {
            if json {
                let payload = serde_json::json!({
                    "ok": false,
                    "error": err.to_string(),
                });
                println!("{payload}");
            } else {
                eprintln!("agentmesh toolchain install: {err}");
            }
            ExitCode::from(2)
        }
    }
}

fn app_validate_command(manifest: PathBuf, toolchain_pin: PathBuf, json: bool) -> ExitCode {
    match validate_app_bundle(&manifest, &toolchain_pin) {
        Ok(report) => {
            if json {
                let payload = serde_json::json!({
                    "ok": true,
                    "app_name": report.app_name,
                    "plugin_logical_name": report.plugin_logical_name,
                    "pin_tag": report.pin_tag,
                    "pin_commit_sha": report.pin_commit_sha,
                    "pin_target": report.pin_target,
                    "protocol_version": report.protocol_version,
                });
                println!("{}", payload);
            } else {
                println!(
                    "ok app={} plugin={} pin={}@{} protocol={}",
                    report.app_name,
                    report.plugin_logical_name,
                    report.pin_tag,
                    report.pin_target,
                    report.protocol_version
                );
            }
            ExitCode::SUCCESS
        }
        Err(err) => {
            if json {
                let payload = serde_json::json!({
                    "ok": false,
                    "error": err.to_string(),
                });
                println!("{payload}");
            } else {
                eprintln!("agentmesh app validate: {err}");
            }
            ExitCode::from(2)
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn app_run_command(
    manifest: PathBuf,
    toolchain_pin: PathBuf,
    input: PathBuf,
    sidecar_dir: PathBuf,
    mode: String,
    dev_plugin: Option<PathBuf>,
    toolchain_cache: Option<PathBuf>,
    redact_pointer: Vec<String>,
    run_timeout_ms: Option<u64>,
) -> Result<i32, i32> {
    let mode = match AppRunMode::parse(&mode) {
        Ok(m) => m,
        Err(msg) => {
            eprintln!("agentmesh app run: {msg}");
            return Err(2);
        }
    };

    let prepared = match prepare_app_run(AppRunRequest {
        manifest_path: &manifest,
        pin_path: &toolchain_pin,
        mode,
        dev_plugin: dev_plugin.as_deref(),
        toolchain_cache: toolchain_cache.as_deref(),
    }) {
        Ok((_app, prepared)) => prepared,
        Err(err) => {
            eprintln!("agentmesh app run: {err}");
            return Err(2);
        }
    };

    if let Err(err) = write_run_marker(&sidecar_dir, &prepared.run_marker) {
        eprintln!("agentmesh app run: {err}");
        return Err(2);
    }

    let mut limits = prepared.limits;
    if let Some(ms) = run_timeout_ms {
        match Limits::validate_run_timeout_ms(ms) {
            Ok(v) => limits.run_timeout_ms = v,
            Err(msg) => {
                emit_input_failure("invalid-timeout", FailureCode::InputSchemaViolation, msg);
                return Err(2);
            }
        }
    }

    let input_bytes = match std::fs::read(&input) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            emit_input_failure(
                "missing-input",
                FailureCode::InputMissing,
                "input path missing",
            );
            return Err(2);
        }
        Err(e) => {
            emit_input_failure(
                "input-read",
                FailureCode::InputReadFailed,
                &format!("input read failed: {e}"),
            );
            return Err(2);
        }
    };

    if input_bytes.len() > limits.input_max_bytes {
        emit_input_failure(
            "input-too-large",
            FailureCode::InputSchemaViolation,
            "input exceeds app/host input_max_bytes",
        );
        return Err(2);
    }

    let config = RunConfig {
        plugin: prepared.resolved.plugin_path.clone(),
        input: input_bytes,
        sidecar_dir,
        plugin_env_keys: prepared.plugin_env_keys,
        redact_pointers: redact_pointer,
        capture_plugin_stderr: prepared.capture_plugin_stderr,
        limits,
        run_id: None,
    };

    let mut sink = BufferCompactSink::default();
    let mut outcome =
        execute_run_with(config, &FsAuditStore, &mut sink, CancellationToken::new()).await;
    annotate_app_run_envelope(
        &mut outcome.envelope,
        &prepared.run_marker,
        prepared.resolved.mode,
    );
    let env_bytes = serde_json::to_vec(&outcome.envelope).unwrap_or_default();
    if let Err(e) = std::io::stdout().lock().write_all(&env_bytes) {
        eprintln!("agentmesh app run: stdout write failed: {e}");
        return Err(70);
    }
    Ok(outcome.exit_code)
}

fn annotate_app_run_envelope(envelope: &mut CompactEnvelope, marker: &str, mode: ResolveMode) {
    envelope.diagnostics.push(CompactDiagnostic {
        category: None,
        code: None,
        message: marker.to_string(),
    });
    let _ = mode; // marker already encodes pinned vs unpinned
}

async fn run_command(
    plugin: PathBuf,
    input: PathBuf,
    sidecar_dir: PathBuf,
    plugin_env: Vec<String>,
    redact_pointer: Vec<String>,
    capture_plugin_stderr: bool,
    run_timeout_ms: Option<u64>,
) -> Result<i32, i32> {
    let mut limits = Limits::default();
    if let Some(ms) = run_timeout_ms {
        match Limits::validate_run_timeout_ms(ms) {
            Ok(v) => limits.run_timeout_ms = v,
            Err(msg) => {
                emit_input_failure("invalid-timeout", FailureCode::InputSchemaViolation, msg);
                return Err(2);
            }
        }
    }

    let input_bytes = match std::fs::read(&input) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            emit_input_failure(
                "missing-input",
                FailureCode::InputMissing,
                "input path missing",
            );
            return Err(2);
        }
        Err(e) => {
            emit_input_failure(
                "input-read",
                FailureCode::InputReadFailed,
                &format!("input read failed: {e}"),
            );
            return Err(2);
        }
    };

    let config = RunConfig {
        plugin,
        input: input_bytes,
        sidecar_dir,
        plugin_env_keys: plugin_env,
        redact_pointers: redact_pointer,
        capture_plugin_stderr,
        limits,
        run_id: None,
    };

    let outcome = execute_run(config).await;
    Ok(outcome.exit_code)
}

fn emit_input_failure(run_id: &str, code: FailureCode, message: &str) {
    let env = CompactEnvelope::error(
        agentmesh_proto::PROTOCOL_VERSION,
        run_id,
        code.category(),
        code,
        message,
        vec![],
    );
    if let Ok(bytes) = serde_json::to_vec(&env) {
        let _ = std::io::Write::write_all(&mut std::io::stdout(), &bytes);
    }
    eprintln!(
        "agentmesh: run_id={run_id} code={} category={} message={message}",
        code,
        code.category()
    );
}
