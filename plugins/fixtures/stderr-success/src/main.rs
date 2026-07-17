//! Writes stderr noise but succeeds.

use agentmesh_fixture_support::run_fixture;
use agentmesh_proto::rpc::RunResult;
use std::io::{self, Write};
use std::process::ExitCode;

fn main() -> ExitCode {
    run_fixture(
        "0.1.0",
        &["compact_output", "sidecar_refs"],
        Box::new(|params| {
            let mut err = io::stderr();
            let _ = writeln!(
                err,
                "fixture-stderr-line secret=SHOULD_NOT_LEAK_TO_TERMINAL"
            );
            let _ = err.flush();
            Ok(RunResult {
                payload: serde_json::json!({"ok": true, "input": params.input}),
            })
        }),
    )
}
