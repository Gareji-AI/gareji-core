//! Sidecar document shape and compact stdout sink seam.

use agentmesh_proto::failure::{FailureRecord, SecondaryFailure};
use agentmesh_proto::limits::Limits;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::{self, Write};
use thiserror::Error;

/// Compact stdout sink seam.
pub trait CompactSink: Send {
    /// Write the final compact JSON bytes.
    fn write_all(&mut self, bytes: &[u8]) -> Result<(), CompactSinkError>;
}

/// Broken-pipe / write failure.
#[derive(Debug, Error)]
pub enum CompactSinkError {
    /// Underlying IO failure.
    #[error("stdout write failed: {0}")]
    Io(#[from] io::Error),
}

/// Real stdout sink.
#[derive(Debug, Default)]
pub struct StdoutCompactSink;

impl CompactSink for StdoutCompactSink {
    fn write_all(&mut self, bytes: &[u8]) -> Result<(), CompactSinkError> {
        let mut out = io::stdout().lock();
        out.write_all(bytes)?;
        out.flush()?;
        Ok(())
    }
}

/// Test sink that can inject broken-pipe failures.
#[derive(Debug, Default)]
pub struct VecCompactSink {
    /// Captured bytes.
    pub bytes: Vec<u8>,
    /// When true, writes fail.
    pub fail: bool,
}

impl CompactSink for VecCompactSink {
    fn write_all(&mut self, bytes: &[u8]) -> Result<(), CompactSinkError> {
        if self.fail {
            return Err(CompactSinkError::Io(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "injected broken pipe",
            )));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(())
    }
}

/// Alias used in docs / exports.
pub type WriteOnceCommit = crate::audit::PersistResult;

/// Ordered normalized protocol message record (redacted).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageRecord {
    /// Direction: host_to_plugin / plugin_to_host.
    pub direction: String,
    /// Method if applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    /// Redacted JSON value.
    pub message: Value,
    /// SHA-256 of the original raw bytes.
    pub raw_sha256: String,
}

/// Plugin stderr capture metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StderrCapture {
    /// Total bytes observed.
    pub byte_count: u64,
    /// SHA-256 of retained+observed stream up to retention (of retained buffer).
    pub sha256: String,
    /// Whether truncated at retention cap.
    pub truncated: bool,
    /// Optional raw bytes when `--capture-plugin-stderr` is set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_utf8_lossy: Option<String>,
    /// Sensitive marker when raw captured.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub sensitive_content: bool,
}

/// Full audit sidecar document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SidecarDocument {
    /// Protocol date version.
    pub protocol_version: String,
    /// Host SemVer.
    pub host_version: String,
    /// Plugin SemVer if known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin_version: Option<String>,
    /// Run id.
    pub run_id: String,
    /// Effective limits.
    pub limits: Limits,
    /// Allowlisted env key names only.
    pub plugin_env_keys: Vec<String>,
    /// Redaction metadata.
    pub redaction: RedactionMeta,
    /// Ordered normalized messages.
    pub messages: Vec<MessageRecord>,
    /// Unknown framing headers observed.
    pub unknown_headers: Vec<BTreeMap<String, String>>,
    /// Stderr capture.
    pub stderr: StderrCapture,
    /// Phase timings in milliseconds.
    pub timings_ms: BTreeMap<String, u64>,
    /// Process exit code if observed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_status: Option<i32>,
    /// Primary failure if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub primary_failure: Option<FailureRecord>,
    /// Ordered secondary failures.
    pub secondary_failures: Vec<SecondaryFailure>,
    /// Input/response/compact hashes.
    pub hashes: BTreeMap<String, String>,
    /// Interrupted flags.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interruption: Option<InterruptionMeta>,
    /// Commit metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<CommitMeta>,
}

/// Redaction policy audit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedactionMeta {
    /// Configured pointers.
    pub pointers: Vec<String>,
    /// Explicit no-redaction when empty.
    pub no_redaction_policy: bool,
    /// Count of redacted fields.
    pub redacted_field_count: usize,
}

/// Direct-child interruption metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterruptionMeta {
    /// Host recorded interruption.
    pub host_interrupted: bool,
    /// Termination of the direct child was attempted.
    pub direct_child_termination_attempted: bool,
    /// Whether the direct child exit was observed.
    pub direct_child_exit_observed: bool,
}

/// Write-once commit metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitMeta {
    /// Sync level.
    pub sync_level: String,
    /// Commit method.
    pub commit_method: String,
}

impl SidecarDocument {
    /// Serialize exactly once to bytes.
    pub fn to_vec(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }
}
