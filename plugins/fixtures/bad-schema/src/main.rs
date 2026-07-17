//! Valid JSON but wrong schema for initialize result.

use agentmesh_fixture_support::{absorb_initialize_then_raw, encode};
use std::process::ExitCode;

fn main() -> ExitCode {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "result": {"not": "an initialize result"},
        "id": "ignored"
    });
    let raw = encode(&serde_json::to_vec(&body).unwrap());
    absorb_initialize_then_raw(&raw)
}
