//! JSON Schema generation helpers for protocol-v0.md drift checks.

use crate::compact::CompactEnvelope;
use crate::rpc::{InitializeParams, InitializeResult, RunParams, RunResult};
use schemars::schema_for;
use serde_json::Value;

/// Generate the Phase 0 host-owned schemas as a single JSON object.
pub fn generate_schemas() -> Value {
    serde_json::json!({
        "protocol_version": crate::versions::PROTOCOL_VERSION,
        "initialize_params": schema_for!(InitializeParams),
        "initialize_result": schema_for!(InitializeResult),
        "run_params": schema_for!(RunParams),
        "run_result": schema_for!(RunResult),
        "compact_envelope": schema_for!(CompactEnvelope),
    })
}

/// Pretty-printed schema document bytes.
pub fn generate_schemas_pretty() -> String {
    serde_json::to_string_pretty(&generate_schemas()).expect("schema serialization")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn schema_snapshot_is_current() {
        let generated = generate_schemas_pretty();
        let path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../schemas/protocol-v0.schema.json");
        if path.exists() {
            let on_disk = std::fs::read_to_string(&path).expect("read schema");
            assert_eq!(
                on_disk.replace("\r\n", "\n").trim(),
                generated.trim(),
                "schemas/protocol-v0.schema.json drifted; regenerate via tests or scripts"
            );
        }
    }
}
