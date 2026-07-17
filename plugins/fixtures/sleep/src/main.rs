//! Sleeps during run to trigger run_timeout; floods stderr for chaos tests.

use agentmesh_fixture_support::run_fixture;
use agentmesh_proto::rpc::RunResult;
use std::io::{self, Write};
use std::process::ExitCode;
use std::thread;
use std::time::Duration;

fn main() -> ExitCode {
    run_fixture(
        "0.1.0",
        &["compact_output"],
        Box::new(|_params| {
            let mut stderr = io::stderr();
            for i in 0..10_000 {
                let _ = writeln!(stderr, "noise-{i}");
                let _ = stderr.flush();
            }
            thread::sleep(Duration::from_secs(30));
            Ok(RunResult {
                payload: serde_json::json!({"slept": true}),
            })
        }),
    )
}
