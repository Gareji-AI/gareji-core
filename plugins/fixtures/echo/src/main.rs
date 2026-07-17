//! Echo fixture: returns input as payload.

use agentmesh_fixture_support::run_fixture;
use agentmesh_proto::rpc::RunResult;
use std::process::ExitCode;

fn main() -> ExitCode {
    run_fixture(
        "0.1.0",
        &["compact_output", "sidecar_refs"],
        Box::new(|params| {
            Ok(RunResult {
                payload: serde_json::json!({
                    "echo": params.input,
                    "run_id": params.run_id,
                }),
            })
        }),
    )
}
