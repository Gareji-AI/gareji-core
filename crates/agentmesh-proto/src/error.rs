//! Proto-level errors.

use thiserror::Error;

/// Errors from protocol parsing / validation.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ProtoError {
    /// Input was not valid JSON.
    #[error("invalid JSON: {0}")]
    InvalidJson(String),
    /// Duplicate object keys were rejected.
    #[error("duplicate object key: {0}")]
    DuplicateKey(String),
    /// JSON tree exceeded depth/node bounds.
    #[error("JSON tree bound exceeded: {0}")]
    TreeBound(String),
    /// Schema / shape violation for a host-owned envelope.
    #[error("schema violation: {0}")]
    SchemaViolation(String),
    /// JSON-RPC ID was invalid under the strict Phase 0 profile.
    #[error("invalid JSON-RPC id: {0}")]
    InvalidId(String),
    /// Batch arrays are rejected in Phase 0.
    #[error("JSON-RPC batches are not supported in protocol v0")]
    BatchRejected,
}
