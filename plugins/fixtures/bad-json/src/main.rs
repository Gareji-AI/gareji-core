//! Intentionally malformed JSON body with valid framing (independent raw writer).

use agentmesh_fixture_support::{absorb_initialize_then_raw, encode};
use std::process::ExitCode;

fn main() -> ExitCode {
    let raw = encode(b"{not-json");
    absorb_initialize_then_raw(&raw)
}
