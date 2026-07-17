//! Named failure categories and detailed codes.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Stable failure category used for exit-code mapping during 0.x.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FailureCategory {
    /// CLI / input validation failures (exit 2).
    Input,
    /// Framing / RPC / schema protocol failures (exit 10).
    Protocol,
    /// Plugin path/spawn/application failures (exit 11).
    Plugin,
    /// Initialize/run/exit-grace timeouts (exit 12).
    Timeout,
    /// Host cancellation / Ctrl-C (exit 12).
    Cancelled,
    /// Audit sidecar persistence failures (exit 13).
    Audit,
    /// Unexpected host invariants (exit 70).
    Internal,
}

impl FailureCategory {
    /// Map category to the Phase 0 process exit code.
    pub const fn exit_code(self) -> i32 {
        match self {
            Self::Input => 2,
            Self::Protocol => 10,
            Self::Plugin => 11,
            Self::Timeout | Self::Cancelled => 12,
            Self::Audit => 13,
            Self::Internal => 70,
        }
    }
}

impl fmt::Display for FailureCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Input => "input",
            Self::Protocol => "protocol",
            Self::Plugin => "plugin",
            Self::Timeout => "timeout",
            Self::Cancelled => "cancelled",
            Self::Audit => "audit",
            Self::Internal => "internal",
        };
        f.write_str(s)
    }
}

/// Detailed failure code for operator diagnosis. May evolve with changelog before 1.0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FailureCode {
    InputMissing,
    InputEmpty,
    InputTooLarge,
    InputReadFailed,
    InputInvalidJson,
    InputSchemaViolation,
    PluginNotFound,
    PluginSpawnFailed,
    PluginWriteFailed,
    InitializeTimeout,
    RunTimeout,
    ProtocolVersionMismatch,
    InvalidFraming,
    InvalidJson,
    SchemaViolation,
    PluginApplicationError,
    UnexpectedEof,
    FrameTooLarge,
    RpcIdMismatch,
    UnexpectedOutput,
    PluginExited,
    PluginExitTimeout,
    HostInterrupted,
    SidecarTooLarge,
    SidecarWriteFailed,
    StdoutWriteFailed,
    HostInternalError,
}

impl FailureCode {
    /// Stable category for this detailed code.
    pub const fn category(self) -> FailureCategory {
        match self {
            Self::InputMissing
            | Self::InputEmpty
            | Self::InputTooLarge
            | Self::InputReadFailed
            | Self::InputInvalidJson
            | Self::InputSchemaViolation => FailureCategory::Input,
            Self::PluginNotFound
            | Self::PluginSpawnFailed
            | Self::PluginWriteFailed
            | Self::PluginApplicationError
            | Self::PluginExited => FailureCategory::Plugin,
            Self::InitializeTimeout | Self::RunTimeout | Self::PluginExitTimeout => {
                FailureCategory::Timeout
            }
            Self::HostInterrupted => FailureCategory::Cancelled,
            Self::ProtocolVersionMismatch
            | Self::InvalidFraming
            | Self::InvalidJson
            | Self::SchemaViolation
            | Self::UnexpectedEof
            | Self::FrameTooLarge
            | Self::RpcIdMismatch
            | Self::UnexpectedOutput => FailureCategory::Protocol,
            Self::SidecarTooLarge | Self::SidecarWriteFailed => FailureCategory::Audit,
            Self::StdoutWriteFailed | Self::HostInternalError => FailureCategory::Internal,
        }
    }

    /// Wire string for compact envelopes / diagnostics.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InputMissing => "input_missing",
            Self::InputEmpty => "input_empty",
            Self::InputTooLarge => "input_too_large",
            Self::InputReadFailed => "input_read_failed",
            Self::InputInvalidJson => "input_invalid_json",
            Self::InputSchemaViolation => "input_schema_violation",
            Self::PluginNotFound => "plugin_not_found",
            Self::PluginSpawnFailed => "plugin_spawn_failed",
            Self::PluginWriteFailed => "plugin_write_failed",
            Self::InitializeTimeout => "initialize_timeout",
            Self::RunTimeout => "run_timeout",
            Self::ProtocolVersionMismatch => "protocol_version_mismatch",
            Self::InvalidFraming => "invalid_framing",
            Self::InvalidJson => "invalid_json",
            Self::SchemaViolation => "schema_violation",
            Self::PluginApplicationError => "plugin_application_error",
            Self::UnexpectedEof => "unexpected_eof",
            Self::FrameTooLarge => "frame_too_large",
            Self::RpcIdMismatch => "rpc_id_mismatch",
            Self::UnexpectedOutput => "unexpected_output",
            Self::PluginExited => "plugin_exited",
            Self::PluginExitTimeout => "plugin_exit_timeout",
            Self::HostInterrupted => "host_interrupted",
            Self::SidecarTooLarge => "sidecar_too_large",
            Self::SidecarWriteFailed => "sidecar_write_failed",
            Self::StdoutWriteFailed => "stdout_write_failed",
            Self::HostInternalError => "host_internal_error",
        }
    }
}

impl fmt::Display for FailureCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Primary failure recorded by the lifecycle coordinator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailureRecord {
    /// Stable category.
    pub category: FailureCategory,
    /// Detailed code.
    pub code: FailureCode,
    /// Host-authored safe message (never raw plugin stderr).
    pub message: String,
}

impl FailureRecord {
    /// Construct a record from a detailed code and safe message.
    pub fn new(code: FailureCode, message: impl Into<String>) -> Self {
        Self {
            category: code.category(),
            code,
            message: message.into(),
        }
    }
}

/// Ordered secondary failure observed after the primary terminal cause.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecondaryFailure {
    /// Stable category.
    pub category: FailureCategory,
    /// Detailed code.
    pub code: FailureCode,
    /// Host-authored safe message.
    pub message: String,
}

impl From<FailureRecord> for SecondaryFailure {
    fn from(value: FailureRecord) -> Self {
        Self {
            category: value.category,
            code: value.code,
            message: value.message,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_codes_never_map_only_to_internal_category() {
        let codes = [
            FailureCode::InputMissing,
            FailureCode::PluginSpawnFailed,
            FailureCode::InitializeTimeout,
            FailureCode::InvalidFraming,
            FailureCode::PluginApplicationError,
            FailureCode::HostInterrupted,
            FailureCode::SidecarWriteFailed,
        ];
        for code in codes {
            assert_ne!(code.category(), FailureCategory::Internal);
            assert_ne!(code, FailureCode::HostInternalError);
        }
    }

    #[test]
    fn exit_codes_are_stable() {
        assert_eq!(FailureCategory::Input.exit_code(), 2);
        assert_eq!(FailureCategory::Protocol.exit_code(), 10);
        assert_eq!(FailureCategory::Plugin.exit_code(), 11);
        assert_eq!(FailureCategory::Timeout.exit_code(), 12);
        assert_eq!(FailureCategory::Cancelled.exit_code(), 12);
        assert_eq!(FailureCategory::Audit.exit_code(), 13);
        assert_eq!(FailureCategory::Internal.exit_code(), 70);
    }
}
