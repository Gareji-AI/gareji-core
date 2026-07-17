//! Shared fixture plugin stdio helpers. Depends on `agentmesh-proto` only.

use agentmesh_proto::json_strict::from_slice_strict;
use agentmesh_proto::limits::Limits;
use agentmesh_proto::rpc::{
    methods, InitializeParams, InitializeResult, JsonRpcError, JsonRpcRequest, JsonRpcResponse,
    JsonRpcVersion, RunParams, RunResult,
};
use agentmesh_proto::PROTOCOL_VERSION;
use serde_json::Value;
use std::io::{self, Write};
use std::process::ExitCode;

mod framing {
    use std::io::{self, Read};

    pub fn encode(body: &[u8]) -> Vec<u8> {
        let header = format!("Content-Length: {}\r\n\r\n", body.len());
        let mut out = Vec::with_capacity(header.len() + body.len());
        out.extend_from_slice(header.as_bytes());
        out.extend_from_slice(body);
        out
    }

    pub fn decode_one(reader: &mut impl Read) -> io::Result<Vec<u8>> {
        let mut headers = Vec::new();
        let mut matched = 0usize;
        let mut buf = [0u8; 1];
        while matched < 4 {
            let n = reader.read(&mut buf)?;
            if n == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "eof in headers",
                ));
            }
            headers.push(buf[0]);
            matched = match (matched, buf[0]) {
                (0, b'\r') => 1,
                (1, b'\n') => 2,
                (2, b'\r') => 3,
                (3, b'\n') => 4,
                (_, b'\r') => 1,
                _ => 0,
            };
            if headers.len() > 8 * 1024 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "header block too large",
                ));
            }
        }
        let text = String::from_utf8_lossy(&headers);
        let mut len = None;
        for line in text.split("\r\n") {
            if let Some(rest) = line
                .strip_prefix("Content-Length:")
                .or_else(|| line.strip_prefix("content-length:"))
            {
                len = Some(
                    rest.trim()
                        .parse::<usize>()
                        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?,
                );
            }
        }
        let len =
            len.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing length"))?;
        let mut body = vec![0u8; len];
        reader.read_exact(&mut body)?;
        Ok(body)
    }
}

/// Behavior callback for a single run after successful initialize.
pub type RunHandler = Box<dyn FnOnce(RunParams) -> Result<RunResult, (i64, String, Option<Value>)>>;

/// Run the standard initialize → run lifecycle reading/writing stdio.
pub fn run_fixture(plugin_version: &str, capabilities: &[&str], on_run: RunHandler) -> ExitCode {
    let mut stdin = io::stdin().lock();
    let mut stdout = io::stdout().lock();
    let limits = Limits::default();

    let init_body = match framing::decode_one(&mut stdin) {
        Ok(b) => b,
        Err(_) => return ExitCode::from(1),
    };
    let init_req: JsonRpcRequest<InitializeParams> = match from_slice_strict(&init_body, &limits) {
        Ok(r) => r,
        Err(_) => return ExitCode::from(1),
    };
    if init_req.method != methods::INITIALIZE {
        return ExitCode::from(1);
    }
    let selected = init_req
        .params
        .protocol_versions
        .iter()
        .find(|v| v.as_str() == PROTOCOL_VERSION)
        .cloned()
        .unwrap_or_else(|| "unsupported".into());
    let init_result = InitializeResult {
        protocol_version: selected,
        plugin_version: plugin_version.into(),
        capabilities: capabilities.iter().map(|s| (*s).to_string()).collect(),
    };
    let init_resp = JsonRpcResponse {
        jsonrpc: JsonRpcVersion::V2,
        result: Some(init_result),
        error: None,
        id: init_req.id,
    };
    let init_bytes = serde_json::to_vec(&init_resp).expect("init resp");
    let _ = stdout.write_all(&framing::encode(&init_bytes));
    let _ = stdout.flush();

    let run_body = match framing::decode_one(&mut stdin) {
        Ok(b) => b,
        Err(_) => return ExitCode::from(1),
    };
    let run_req: JsonRpcRequest<RunParams> = match from_slice_strict(&run_body, &limits) {
        Ok(r) => r,
        Err(_) => return ExitCode::from(1),
    };
    if run_req.method != methods::RUN {
        return ExitCode::from(1);
    }
    let resp = match on_run(run_req.params) {
        Ok(result) => JsonRpcResponse {
            jsonrpc: JsonRpcVersion::V2,
            result: Some(result),
            error: None,
            id: run_req.id,
        },
        Err((code, message, data)) => JsonRpcResponse {
            jsonrpc: JsonRpcVersion::V2,
            result: None,
            error: Some(JsonRpcError {
                code,
                message,
                data,
            }),
            id: run_req.id,
        },
    };
    let run_bytes = serde_json::to_vec(&resp).expect("run resp");
    let _ = stdout.write_all(&framing::encode(&run_bytes));
    let _ = stdout.flush();
    ExitCode::SUCCESS
}

/// Write a raw (possibly malformed) frame to stdout.
pub fn write_raw_stdout(bytes: &[u8]) {
    let mut stdout = io::stdout().lock();
    let _ = stdout.write_all(bytes);
    let _ = stdout.flush();
}

/// Read and discard one initialize request, then write raw bytes.
pub fn absorb_initialize_then_raw(raw_response: &[u8]) -> ExitCode {
    let mut stdin = io::stdin().lock();
    let _ = framing::decode_one(&mut stdin);
    write_raw_stdout(raw_response);
    ExitCode::SUCCESS
}

pub use framing::{decode_one, encode};
