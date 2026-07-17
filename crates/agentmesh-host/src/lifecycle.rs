//! Cancellation token seam (OS signal adapter lives outside the state machine).

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

/// Lightweight cancellation token.
#[derive(Debug, Clone, Default)]
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
}

impl CancellationToken {
    /// Create a new token.
    pub fn new() -> Self {
        Self::default()
    }

    /// Mark cancelled.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    /// Check cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

/// Configuration for a single one-shot run.
#[derive(Debug, Clone)]
pub struct RunConfig {
    /// Absolute plugin path (string form for config; validated later).
    pub plugin: std::path::PathBuf,
    /// Raw input JSON bytes.
    pub input: Vec<u8>,
    /// Sidecar parent directory.
    pub sidecar_dir: std::path::PathBuf,
    /// Allowlisted plugin env keys.
    pub plugin_env_keys: Vec<String>,
    /// Redaction pointers.
    pub redact_pointers: Vec<String>,
    /// Capture raw plugin stderr.
    pub capture_plugin_stderr: bool,
    /// Host limits (run timeout may be overridden).
    pub limits: agentmesh_proto::Limits,
    /// Optional host-generated run id; random UUID if None.
    pub run_id: Option<String>,
}

/// Final host outcome after lifecycle.
#[derive(Debug)]
pub struct RunOutcome {
    /// Compact envelope ready for stdout.
    pub envelope: agentmesh_proto::CompactEnvelope,
    /// Process exit category code.
    pub exit_code: i32,
    /// Sidecar path if persisted.
    pub sidecar_path: Option<std::path::PathBuf>,
}
