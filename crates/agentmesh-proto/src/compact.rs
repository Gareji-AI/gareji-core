//! Compact stdout envelope owned by the host.

use crate::failure::{FailureCategory, FailureCode};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Compact run outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CompactOutcome {
    /// Successful run.
    Ok,
    /// Failed run with category/code in diagnostics.
    Error,
}

/// Artifact path reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CompactArtifact {
    /// Relative or host-generated sidecar path.
    pub path: String,
}

impl CompactArtifact {
    /// Construct from a path string.
    pub fn new(path: impl Into<String>) -> Self {
        Self { path: path.into() }
    }
}

/// Host diagnostic entry (safe; never raw plugin stderr).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CompactDiagnostic {
    /// Failure category when present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<FailureCategory>,
    /// Detailed failure code when present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<FailureCode>,
    /// Host-authored message.
    pub message: String,
}

/// Exactly one compact JSON object printed to stdout.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CompactEnvelope {
    /// Host envelope schema/date version.
    pub schema_version: String,
    /// Host-generated run id.
    pub run_id: String,
    /// Success or error.
    pub outcome: CompactOutcome,
    /// Opaque plugin payload on success; empty object on failure.
    pub payload: Value,
    /// Sidecar references.
    pub artifacts: Vec<CompactArtifact>,
    /// Safe diagnostics.
    pub diagnostics: Vec<CompactDiagnostic>,
}

impl CompactEnvelope {
    /// Success envelope.
    pub fn ok(
        schema_version: impl Into<String>,
        run_id: impl Into<String>,
        payload: Value,
        artifacts: Vec<CompactArtifact>,
    ) -> Self {
        Self {
            schema_version: schema_version.into(),
            run_id: run_id.into(),
            outcome: CompactOutcome::Ok,
            payload,
            artifacts,
            diagnostics: Vec::new(),
        }
    }

    /// Failure envelope.
    pub fn error(
        schema_version: impl Into<String>,
        run_id: impl Into<String>,
        category: FailureCategory,
        code: FailureCode,
        message: impl Into<String>,
        artifacts: Vec<CompactArtifact>,
    ) -> Self {
        Self {
            schema_version: schema_version.into(),
            run_id: run_id.into(),
            outcome: CompactOutcome::Error,
            payload: Value::Object(serde_json::Map::new()),
            artifacts,
            diagnostics: vec![CompactDiagnostic {
                category: Some(category),
                code: Some(code),
                message: message.into(),
            }],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::versions::PROTOCOL_VERSION;

    #[test]
    fn success_envelope_has_no_decision_field() {
        let env = CompactEnvelope::ok(
            PROTOCOL_VERSION,
            "run-1",
            serde_json::json!({"echo": true}),
            vec![CompactArtifact::new(".agentmesh/runs/x/full.json")],
        );
        let v = serde_json::to_value(&env).unwrap();
        assert!(v.get("decision").is_none());
        assert_eq!(v["outcome"], "ok");
        assert_eq!(v["schema_version"], PROTOCOL_VERSION);
    }
}
