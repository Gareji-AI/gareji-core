//! AgentMesh Phase 0 protocol types: strict JSON-RPC 2.0 profile and host envelopes.

pub mod compact;
pub mod error;
pub mod failure;
pub mod json_strict;
pub mod limits;
pub mod rpc;
pub mod schema;
pub mod versions;

pub use compact::{CompactArtifact, CompactDiagnostic, CompactEnvelope, CompactOutcome};
pub use error::ProtoError;
pub use failure::{FailureCategory, FailureCode, FailureRecord, SecondaryFailure};
pub use json_strict::{from_slice_strict, from_str_strict};
pub use limits::Limits;
pub use rpc::{
    ApplicationErrorData, InitializeParams, InitializeResult, JsonRpcError, JsonRpcId,
    JsonRpcRequest, JsonRpcResponse, JsonRpcVersion, ProtocolCapability, RunParams, RunResult,
};
pub use versions::{HOST_VERSION, PLUGIN_PROTOCOL_DATE, PROTOCOL_VERSION};
