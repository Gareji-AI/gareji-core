//! Returns a valid run result then exits non-zero.

use agentmesh_fixture_support::{decode_one, encode};
use agentmesh_proto::json_strict::from_slice_strict;
use agentmesh_proto::limits::Limits;
use agentmesh_proto::rpc::{
    methods, InitializeParams, InitializeResult, JsonRpcRequest, JsonRpcResponse, JsonRpcVersion,
    RunParams, RunResult,
};
use agentmesh_proto::PROTOCOL_VERSION;
use std::io::{self, Write};
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut stdin = io::stdin().lock();
    let mut stdout = io::stdout().lock();
    let limits = Limits::default();

    let init_body = decode_one(&mut stdin).expect("init");
    let init_req: JsonRpcRequest<InitializeParams> =
        from_slice_strict(&init_body, &limits).expect("init parse");
    let init_resp = JsonRpcResponse {
        jsonrpc: JsonRpcVersion::V2,
        result: Some(InitializeResult {
            protocol_version: PROTOCOL_VERSION.into(),
            plugin_version: "0.1.0".into(),
            capabilities: vec!["compact_output".into()],
        }),
        error: None,
        id: init_req.id,
    };
    stdout
        .write_all(&encode(&serde_json::to_vec(&init_resp).unwrap()))
        .unwrap();
    stdout.flush().unwrap();

    let run_body = decode_one(&mut stdin).expect("run");
    let run_req: JsonRpcRequest<RunParams> =
        from_slice_strict(&run_body, &limits).expect("run parse");
    assert_eq!(run_req.method, methods::RUN);
    let run_resp = JsonRpcResponse {
        jsonrpc: JsonRpcVersion::V2,
        result: Some(RunResult {
            payload: serde_json::json!({"before_exit": true}),
        }),
        error: None,
        id: run_req.id,
    };
    stdout
        .write_all(&encode(&serde_json::to_vec(&run_resp).unwrap()))
        .unwrap();
    stdout.flush().unwrap();
    ExitCode::from(7)
}
