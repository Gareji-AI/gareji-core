//! Strict JSON-RPC 2.0 profile for AgentMesh Phase 0.

use crate::error::ProtoError;
use crate::versions::PROTOCOL_VERSION;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;

/// JSON-RPC version marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum JsonRpcVersion {
    /// Exactly `"2.0"`.
    #[serde(rename = "2.0")]
    V2,
}

/// Request/response ID: non-empty visible-ASCII string up to 128 bytes.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct JsonRpcId(String);

impl JsonRpcId {
    /// Validate and construct an ID.
    pub fn new(value: impl Into<String>) -> Result<Self, ProtoError> {
        let value = value.into();
        if value.is_empty() {
            return Err(ProtoError::InvalidId("empty id".into()));
        }
        if value.len() > 128 {
            return Err(ProtoError::InvalidId("id longer than 128 bytes".into()));
        }
        if !value.bytes().all(|b| (0x21..=0x7E).contains(&b)) {
            return Err(ProtoError::InvalidId(
                "id must be non-empty visible ASCII (0x21-0x7E)".into(),
            ));
        }
        Ok(Self(value))
    }

    /// Borrow the ID string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for JsonRpcId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Host → plugin or plugin response envelope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct JsonRpcRequest<T> {
    /// Must be `"2.0"`.
    pub jsonrpc: JsonRpcVersion,
    /// Method name.
    pub method: String,
    /// Typed params.
    pub params: T,
    /// Strict string ID.
    pub id: JsonRpcId,
}

/// Success or error response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct JsonRpcResponse<T> {
    /// Must be `"2.0"`.
    pub jsonrpc: JsonRpcVersion,
    /// Result on success.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<T>,
    /// Error on failure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
    /// Matching request ID.
    pub id: JsonRpcId,
}

/// JSON-RPC error object.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct JsonRpcError {
    /// Numeric code.
    pub code: i64,
    /// Short message.
    pub message: String,
    /// Optional opaque bounded data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// Opaque application-error data slot (plugin-owned).
pub type ApplicationErrorData = Value;

/// Known Phase 0 capability names. Unknown names are ignored but audited.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProtocolCapability {
    /// Compact stdout envelope support.
    CompactOutput,
    /// Sidecar artifact references.
    SidecarRefs,
    /// Unknown capability preserved as raw string in negotiation auditing.
    #[serde(untagged)]
    Unknown(String),
}

/// `initialize` request params.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct InitializeParams {
    /// Host-supported protocol versions.
    pub protocol_versions: Vec<String>,
    /// Host SemVer.
    pub host_version: String,
    /// Host capability names (opaque extras allowed as strings).
    pub capabilities: Vec<String>,
}

impl InitializeParams {
    /// Construct default Phase 0 initialize params.
    pub fn phase0(host_version: impl Into<String>) -> Self {
        Self {
            protocol_versions: vec![PROTOCOL_VERSION.to_string()],
            host_version: host_version.into(),
            capabilities: vec!["compact_output".into(), "sidecar_refs".into()],
        }
    }
}

/// `initialize` result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct InitializeResult {
    /// Selected wire protocol version.
    pub protocol_version: String,
    /// Plugin SemVer.
    pub plugin_version: String,
    /// Plugin capability names.
    pub capabilities: Vec<String>,
}

/// `agentmesh.run` params.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RunParams {
    /// Host-generated run id.
    pub run_id: String,
    /// Opaque plugin-owned input JSON.
    pub input: Value,
}

/// `agentmesh.run` result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RunResult {
    /// Opaque plugin-owned payload JSON.
    pub payload: Value,
}

/// Phase 0 method names.
pub mod methods {
    /// Capability negotiation.
    pub const INITIALIZE: &str = "initialize";
    /// Single run request.
    pub const RUN: &str = "agentmesh.run";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_and_non_ascii_ids() {
        assert!(JsonRpcId::new("").is_err());
        assert!(JsonRpcId::new("has space").is_err());
        assert!(JsonRpcId::new("ok-id_1").is_ok());
    }

    #[test]
    fn initialize_params_roundtrip() {
        let p = InitializeParams::phase0("0.1.0");
        let v = serde_json::to_value(&p).unwrap();
        let back: InitializeParams = serde_json::from_value(v).unwrap();
        assert_eq!(back.protocol_versions[0], PROTOCOL_VERSION);
    }
}
