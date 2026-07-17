//! One-shot host run orchestration.

use crate::audit::{AuditError, AuditStore, FsAuditStore};
use crate::failure_coord::FailureCoordinator;
use crate::framing::{FrameDecodeError, FrameDecoder, FrameEncoder, FrameLimits};
use crate::lifecycle::{CancellationToken, RunConfig, RunOutcome};
use crate::process::{build_plugin_command, PluginPath, PluginPathError};
use crate::redaction::{RedactionError, RedactionPolicy};
use crate::sidecar::{
    CommitMeta, CompactSink, InterruptionMeta, MessageRecord, RedactionMeta, SidecarDocument,
    StderrCapture, StdoutCompactSink,
};
use agentmesh_proto::compact::{CompactArtifact, CompactEnvelope};
use agentmesh_proto::failure::{FailureCode, FailureRecord};
use agentmesh_proto::json_strict::{from_slice_strict, parse_value_strict};
use agentmesh_proto::rpc::{
    methods, InitializeParams, InitializeResult, JsonRpcId, JsonRpcRequest, JsonRpcResponse,
    JsonRpcVersion, RunParams, RunResult,
};
use agentmesh_proto::{HOST_VERSION, PROTOCOL_VERSION};
use chrono::Utc;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Child;
use uuid::Uuid;

/// Execute a complete one-shot plugin run using the filesystem audit store and stdout.
pub async fn execute_run(config: RunConfig) -> RunOutcome {
    execute_run_with(
        config,
        &FsAuditStore,
        &mut StdoutCompactSink,
        CancellationToken::new(),
    )
    .await
}

/// Execute with injectable seams (tests).
pub async fn execute_run_with<A: AuditStore, S: CompactSink>(
    config: RunConfig,
    audit: &A,
    sink: &mut S,
    cancel: CancellationToken,
) -> RunOutcome {
    let run_id = config
        .run_id
        .clone()
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let failures = FailureCoordinator::new();
    let mut timings = BTreeMap::new();
    let started = Utc::now();
    let mut messages = Vec::new();
    let mut unknown_headers = Vec::new();
    let mut plugin_version = None;
    let mut interruption = None;
    let mut exit_status = None;
    let mut hashes = BTreeMap::new();
    let mut payload = Value::Object(serde_json::Map::new());
    let mut stderr_cap = StderrCapture {
        byte_count: 0,
        sha256: sha256_hex(b""),
        truncated: false,
        raw_utf8_lossy: None,
        sensitive_content: false,
    };

    let redaction = match RedactionPolicy::from_pointers(config.redact_pointers.clone()) {
        Ok(p) => p,
        Err(RedactionError::InvalidPointer(msg)) => {
            failures.record(FailureRecord::new(
                FailureCode::InputSchemaViolation,
                format!("invalid redact pointer: {msg}"),
            ));
            return finish(
                &run_id,
                &config,
                &failures,
                None,
                messages,
                unknown_headers,
                stderr_cap,
                timings,
                exit_status,
                hashes,
                plugin_version,
                interruption,
                audit,
                sink,
                payload,
            );
        }
    };

    // Input validation before spawn.
    if config.input.is_empty() {
        failures.record(FailureRecord::new(
            FailureCode::InputEmpty,
            "input has zero bytes",
        ));
        return finish(
            &run_id,
            &config,
            &failures,
            None,
            messages,
            unknown_headers,
            stderr_cap,
            timings,
            exit_status,
            hashes,
            plugin_version,
            interruption,
            audit,
            sink,
            payload,
        );
    }
    if config.input.len() > config.limits.input_max_bytes {
        failures.record(FailureRecord::new(
            FailureCode::InputTooLarge,
            format!(
                "input {} bytes exceeds limit {}",
                config.input.len(),
                config.limits.input_max_bytes
            ),
        ));
        return finish(
            &run_id,
            &config,
            &failures,
            None,
            messages,
            unknown_headers,
            stderr_cap,
            timings,
            exit_status,
            hashes,
            plugin_version,
            interruption,
            audit,
            sink,
            payload,
        );
    }
    hashes.insert("input".into(), sha256_hex(&config.input));
    let input_value = match parse_value_strict(&config.input, &config.limits) {
        Ok(v) => v,
        Err(e) => {
            let code = match e {
                agentmesh_proto::ProtoError::InvalidJson(_)
                | agentmesh_proto::ProtoError::DuplicateKey(_) => FailureCode::InputInvalidJson,
                agentmesh_proto::ProtoError::TreeBound(_)
                | agentmesh_proto::ProtoError::SchemaViolation(_) => {
                    FailureCode::InputSchemaViolation
                }
                other => {
                    failures.record(FailureRecord::new(
                        FailureCode::InputInvalidJson,
                        other.to_string(),
                    ));
                    return finish(
                        &run_id,
                        &config,
                        &failures,
                        None,
                        messages,
                        unknown_headers,
                        stderr_cap,
                        timings,
                        exit_status,
                        hashes,
                        plugin_version,
                        interruption,
                        audit,
                        sink,
                        payload,
                    );
                }
            };
            failures.record(FailureRecord::new(code, e.to_string()));
            return finish(
                &run_id,
                &config,
                &failures,
                None,
                messages,
                unknown_headers,
                stderr_cap,
                timings,
                exit_status,
                hashes,
                plugin_version,
                interruption,
                audit,
                sink,
                payload,
            );
        }
    };

    let plugin = match PluginPath::resolve(&config.plugin) {
        Ok(p) => p,
        Err(PluginPathError::NotAbsolute) => {
            failures.record(FailureRecord::new(
                FailureCode::PluginSpawnFailed,
                "plugin path must be an absolute native executable",
            ));
            return finish(
                &run_id,
                &config,
                &failures,
                None,
                messages,
                unknown_headers,
                stderr_cap,
                timings,
                exit_status,
                hashes,
                plugin_version,
                interruption,
                audit,
                sink,
                payload,
            );
        }
        Err(PluginPathError::NotFound) => {
            failures.record(FailureRecord::new(
                FailureCode::PluginNotFound,
                "plugin absolute path does not exist",
            ));
            return finish(
                &run_id,
                &config,
                &failures,
                None,
                messages,
                unknown_headers,
                stderr_cap,
                timings,
                exit_status,
                hashes,
                plugin_version,
                interruption,
                audit,
                sink,
                payload,
            );
        }
        Err(PluginPathError::NotFile) => {
            failures.record(FailureRecord::new(
                FailureCode::PluginSpawnFailed,
                "plugin path is not a file",
            ));
            return finish(
                &run_id,
                &config,
                &failures,
                None,
                messages,
                unknown_headers,
                stderr_cap,
                timings,
                exit_status,
                hashes,
                plugin_version,
                interruption,
                audit,
                sink,
                payload,
            );
        }
    };

    let mut command = match build_plugin_command(&plugin, &config.plugin_env_keys) {
        Ok(c) => c,
        Err(e) => {
            failures.record(FailureRecord::new(
                FailureCode::PluginSpawnFailed,
                e.to_string(),
            ));
            return finish(
                &run_id,
                &config,
                &failures,
                None,
                messages,
                unknown_headers,
                stderr_cap,
                timings,
                exit_status,
                hashes,
                plugin_version,
                interruption,
                audit,
                sink,
                payload,
            );
        }
    };

    let mut child = match command.spawn() {
        Ok(c) => c,
        Err(e) => {
            failures.record(FailureRecord::new(
                FailureCode::PluginSpawnFailed,
                e.to_string(),
            ));
            return finish(
                &run_id,
                &config,
                &failures,
                None,
                messages,
                unknown_headers,
                stderr_cap,
                timings,
                exit_status,
                hashes,
                plugin_version,
                interruption,
                audit,
                sink,
                payload,
            );
        }
    };

    let mut stdin = child.stdin.take();
    let mut stdout = child.stdout.take();
    let stderr = child.stderr.take();

    // Concurrent stderr drain.
    let stderr_limit = config.limits.stderr_retain_bytes;
    let capture_raw = config.capture_plugin_stderr;
    let stderr_task = tokio::spawn(async move {
        let mut retained = Vec::new();
        let mut total: u64 = 0;
        let mut truncated = false;
        let mut hasher = Sha256::new();
        if let Some(mut err) = stderr {
            let mut buf = [0u8; 8192];
            loop {
                match err.read(&mut buf).await {
                    Ok(0) => break,
                    Ok(n) => {
                        total += n as u64;
                        hasher.update(&buf[..n]);
                        if retained.len() < stderr_limit {
                            let room = stderr_limit - retained.len();
                            let take = n.min(room);
                            retained.extend_from_slice(&buf[..take]);
                            if take < n {
                                truncated = true;
                            }
                        } else {
                            truncated = true;
                        }
                    }
                    Err(_) => break,
                }
            }
        }
        let sha = hex::encode(hasher.finalize());
        let raw = if capture_raw {
            Some(String::from_utf8_lossy(&retained).into_owned())
        } else {
            None
        };
        StderrCapture {
            byte_count: total,
            sha256: sha,
            truncated,
            raw_utf8_lossy: raw,
            sensitive_content: capture_raw,
        }
    });

    let frame_limits = FrameLimits::from(&config.limits);
    let init_id = JsonRpcId::new(format!("init-{run_id}")).expect("valid id");
    let init_req = JsonRpcRequest {
        jsonrpc: JsonRpcVersion::V2,
        method: methods::INITIALIZE.to_string(),
        params: InitializeParams::phase0(HOST_VERSION),
        id: init_id.clone(),
    };
    let init_body = serde_json::to_vec(&init_req).expect("serialize initialize");
    let init_frame = FrameEncoder::encode(&init_body);
    push_message(
        &mut messages,
        &redaction,
        "host_to_plugin",
        Some(methods::INITIALIZE),
        &init_body,
    );

    let t_init = Utc::now();
    if let Some(ref mut si) = stdin {
        if let Err(e) = si.write_all(&init_frame).await {
            failures.record(FailureRecord::new(
                FailureCode::PluginWriteFailed,
                e.to_string(),
            ));
            terminate_child(&mut child, &cancel, &mut interruption, &mut exit_status).await;
            stderr_cap = stderr_task.await.unwrap_or(stderr_cap);
            timings.insert(
                "initialize_ms".into(),
                (Utc::now() - t_init).num_milliseconds().max(0) as u64,
            );
            return finish(
                &run_id,
                &config,
                &failures,
                None,
                messages,
                unknown_headers,
                stderr_cap,
                timings,
                exit_status,
                hashes,
                plugin_version,
                interruption,
                audit,
                sink,
                payload,
            );
        }
    }

    let init_timeout = Duration::from_millis(config.limits.initialize_timeout_ms);
    let init_read = read_one_frame(&mut stdout, frame_limits, init_timeout, &cancel);
    let init_frame_res = init_read.await;
    timings.insert(
        "initialize_ms".into(),
        (Utc::now() - t_init).num_milliseconds().max(0) as u64,
    );

    let init_decoded = match init_frame_res {
        Ok(frame) => frame,
        Err(e) => {
            map_frame_err(&failures, e, true);
            terminate_child(&mut child, &cancel, &mut interruption, &mut exit_status).await;
            stderr_cap = stderr_task.await.unwrap_or(stderr_cap);
            return finish(
                &run_id,
                &config,
                &failures,
                None,
                messages,
                unknown_headers,
                stderr_cap,
                timings,
                exit_status,
                hashes,
                plugin_version,
                interruption,
                audit,
                sink,
                payload,
            );
        }
    };
    if !init_decoded.unknown_headers.is_empty() {
        unknown_headers.push(init_decoded.unknown_headers.clone());
    }
    hashes.insert("initialize_response".into(), sha256_hex(&init_decoded.body));
    push_message(
        &mut messages,
        &redaction,
        "plugin_to_host",
        Some(methods::INITIALIZE),
        &init_decoded.body,
    );

    let init_resp: JsonRpcResponse<InitializeResult> =
        match from_slice_strict(&init_decoded.body, &config.limits) {
            Ok(r) => r,
            Err(e) => {
                failures.record(FailureRecord::new(
                    FailureCode::SchemaViolation,
                    e.to_string(),
                ));
                terminate_child(&mut child, &cancel, &mut interruption, &mut exit_status).await;
                stderr_cap = stderr_task.await.unwrap_or(stderr_cap);
                return finish(
                    &run_id,
                    &config,
                    &failures,
                    None,
                    messages,
                    unknown_headers,
                    stderr_cap,
                    timings,
                    exit_status,
                    hashes,
                    plugin_version,
                    interruption,
                    audit,
                    sink,
                    payload,
                );
            }
        };
    if init_resp.id != init_id {
        failures.record(FailureRecord::new(
            FailureCode::RpcIdMismatch,
            "initialize response id mismatch",
        ));
        terminate_child(&mut child, &cancel, &mut interruption, &mut exit_status).await;
        stderr_cap = stderr_task.await.unwrap_or(stderr_cap);
        return finish(
            &run_id,
            &config,
            &failures,
            None,
            messages,
            unknown_headers,
            stderr_cap,
            timings,
            exit_status,
            hashes,
            plugin_version,
            interruption,
            audit,
            sink,
            payload,
        );
    }
    if let Some(err) = init_resp.error {
        failures.record(FailureRecord::new(
            FailureCode::PluginApplicationError,
            format!("initialize error {}: {}", err.code, err.message),
        ));
        terminate_child(&mut child, &cancel, &mut interruption, &mut exit_status).await;
        stderr_cap = stderr_task.await.unwrap_or(stderr_cap);
        return finish(
            &run_id,
            &config,
            &failures,
            None,
            messages,
            unknown_headers,
            stderr_cap,
            timings,
            exit_status,
            hashes,
            plugin_version,
            interruption,
            audit,
            sink,
            payload,
        );
    }
    let Some(init_result) = init_resp.result else {
        failures.record(FailureRecord::new(
            FailureCode::SchemaViolation,
            "initialize response missing result",
        ));
        terminate_child(&mut child, &cancel, &mut interruption, &mut exit_status).await;
        stderr_cap = stderr_task.await.unwrap_or(stderr_cap);
        return finish(
            &run_id,
            &config,
            &failures,
            None,
            messages,
            unknown_headers,
            stderr_cap,
            timings,
            exit_status,
            hashes,
            plugin_version,
            interruption,
            audit,
            sink,
            payload,
        );
    };
    if init_result.protocol_version != PROTOCOL_VERSION {
        failures.record(FailureRecord::new(
            FailureCode::ProtocolVersionMismatch,
            format!(
                "no shared protocol version: plugin selected {}",
                init_result.protocol_version
            ),
        ));
        terminate_child(&mut child, &cancel, &mut interruption, &mut exit_status).await;
        stderr_cap = stderr_task.await.unwrap_or(stderr_cap);
        return finish(
            &run_id,
            &config,
            &failures,
            None,
            messages,
            unknown_headers,
            stderr_cap,
            timings,
            exit_status,
            hashes,
            plugin_version,
            interruption,
            audit,
            sink,
            payload,
        );
    }
    plugin_version = Some(init_result.plugin_version);

    // Run request.
    let run_id_rpc = JsonRpcId::new(format!("run-{run_id}")).expect("valid id");
    let run_req = JsonRpcRequest {
        jsonrpc: JsonRpcVersion::V2,
        method: methods::RUN.to_string(),
        params: RunParams {
            run_id: run_id.clone(),
            input: input_value,
        },
        id: run_id_rpc.clone(),
    };
    let run_body = serde_json::to_vec(&run_req).expect("serialize run");
    let run_frame = FrameEncoder::encode(&run_body);
    push_message(
        &mut messages,
        &redaction,
        "host_to_plugin",
        Some(methods::RUN),
        &run_body,
    );

    let t_run = Utc::now();
    if let Some(ref mut si) = stdin {
        if let Err(e) = si.write_all(&run_frame).await {
            failures.record(FailureRecord::new(
                FailureCode::PluginWriteFailed,
                e.to_string(),
            ));
            terminate_child(&mut child, &cancel, &mut interruption, &mut exit_status).await;
            stderr_cap = stderr_task.await.unwrap_or(stderr_cap);
            timings.insert(
                "run_ms".into(),
                (Utc::now() - t_run).num_milliseconds().max(0) as u64,
            );
            return finish(
                &run_id,
                &config,
                &failures,
                None,
                messages,
                unknown_headers,
                stderr_cap,
                timings,
                exit_status,
                hashes,
                plugin_version,
                interruption,
                audit,
                sink,
                payload,
            );
        }
    }

    let run_timeout = Duration::from_millis(config.limits.run_timeout_ms);
    let run_frame_res = read_one_frame(&mut stdout, frame_limits, run_timeout, &cancel).await;
    timings.insert(
        "run_ms".into(),
        (Utc::now() - t_run).num_milliseconds().max(0) as u64,
    );
    let run_decoded = match run_frame_res {
        Ok(frame) => frame,
        Err(e) => {
            map_frame_err(&failures, e, false);
            terminate_child(&mut child, &cancel, &mut interruption, &mut exit_status).await;
            stderr_cap = stderr_task.await.unwrap_or(stderr_cap);
            return finish(
                &run_id,
                &config,
                &failures,
                None,
                messages,
                unknown_headers,
                stderr_cap,
                timings,
                exit_status,
                hashes,
                plugin_version,
                interruption,
                audit,
                sink,
                payload,
            );
        }
    };
    if !run_decoded.unknown_headers.is_empty() {
        unknown_headers.push(run_decoded.unknown_headers.clone());
    }
    hashes.insert("run_response".into(), sha256_hex(&run_decoded.body));
    push_message(
        &mut messages,
        &redaction,
        "plugin_to_host",
        Some(methods::RUN),
        &run_decoded.body,
    );

    let run_resp: JsonRpcResponse<RunResult> =
        match from_slice_strict(&run_decoded.body, &config.limits) {
            Ok(r) => r,
            Err(e) => {
                failures.record(FailureRecord::new(
                    FailureCode::SchemaViolation,
                    e.to_string(),
                ));
                terminate_child(&mut child, &cancel, &mut interruption, &mut exit_status).await;
                stderr_cap = stderr_task.await.unwrap_or(stderr_cap);
                return finish(
                    &run_id,
                    &config,
                    &failures,
                    None,
                    messages,
                    unknown_headers,
                    stderr_cap,
                    timings,
                    exit_status,
                    hashes,
                    plugin_version,
                    interruption,
                    audit,
                    sink,
                    payload,
                );
            }
        };
    if run_resp.id != run_id_rpc {
        failures.record(FailureRecord::new(
            FailureCode::RpcIdMismatch,
            "run response id mismatch",
        ));
        terminate_child(&mut child, &cancel, &mut interruption, &mut exit_status).await;
        stderr_cap = stderr_task.await.unwrap_or(stderr_cap);
        return finish(
            &run_id,
            &config,
            &failures,
            None,
            messages,
            unknown_headers,
            stderr_cap,
            timings,
            exit_status,
            hashes,
            plugin_version,
            interruption,
            audit,
            sink,
            payload,
        );
    }
    if let Some(err) = run_resp.error {
        failures.record(FailureRecord::new(
            FailureCode::PluginApplicationError,
            format!("plugin application error {}: {}", err.code, err.message),
        ));
        // continue to close/exit validation still
    } else if let Some(result) = run_resp.result {
        payload = result.payload;
    } else {
        failures.record(FailureRecord::new(
            FailureCode::SchemaViolation,
            "run response missing result and error",
        ));
    }

    // Close stdin and exit grace: probe stdout to EOF; first additional byte => unexpected_output.
    drop(stdin);
    let grace = Duration::from_millis(config.limits.exit_grace_ms);
    let t_close = Utc::now();
    close_and_reap(
        &mut child,
        &mut stdout,
        grace,
        &cancel,
        &mut interruption,
        &mut exit_status,
        &failures,
    )
    .await;
    timings.insert(
        "close_ms".into(),
        (Utc::now() - t_close).num_milliseconds().max(0) as u64,
    );
    stderr_cap = stderr_task.await.unwrap_or(stderr_cap);
    timings.insert(
        "total_ms".into(),
        (Utc::now() - started).num_milliseconds().max(0) as u64,
    );

    finish(
        &run_id,
        &config,
        &failures,
        Some(&redaction),
        messages,
        unknown_headers,
        stderr_cap,
        timings,
        exit_status,
        hashes,
        plugin_version,
        interruption,
        audit,
        sink,
        payload,
    )
}

fn map_frame_err(failures: &FailureCoordinator, err: ReadFrameError, is_init: bool) {
    match err {
        ReadFrameError::Timeout => {
            failures.record(FailureRecord::new(
                if is_init {
                    FailureCode::InitializeTimeout
                } else {
                    FailureCode::RunTimeout
                },
                if is_init {
                    "initialize timed out"
                } else {
                    "run timed out"
                },
            ));
        }
        ReadFrameError::Cancelled => {
            failures.record(FailureRecord::new(
                FailureCode::HostInterrupted,
                "host cancelled during frame read",
            ));
        }
        ReadFrameError::Decode(FrameDecodeError::UnexpectedEof) => {
            failures.record(FailureRecord::new(
                FailureCode::UnexpectedEof,
                "stdout closed before complete frame",
            ));
        }
        ReadFrameError::Decode(FrameDecodeError::FrameTooLarge(m)) => {
            failures.record(FailureRecord::new(FailureCode::FrameTooLarge, m));
        }
        ReadFrameError::Decode(FrameDecodeError::InvalidFraming(m)) => {
            failures.record(FailureRecord::new(FailureCode::InvalidFraming, m));
        }
        ReadFrameError::Io(m) => {
            failures.record(FailureRecord::new(FailureCode::UnexpectedEof, m));
        }
        ReadFrameError::PluginExited => {
            failures.record(FailureRecord::new(
                FailureCode::PluginExited,
                "plugin exited before valid response",
            ));
        }
    }
}

#[derive(Debug)]
enum ReadFrameError {
    Timeout,
    Cancelled,
    Decode(FrameDecodeError),
    Io(String),
    PluginExited,
}

async fn read_one_frame(
    stdout: &mut Option<tokio::process::ChildStdout>,
    limits: FrameLimits,
    timeout: Duration,
    cancel: &CancellationToken,
) -> Result<crate::framing::DecodedFrame, ReadFrameError> {
    let Some(out) = stdout.as_mut() else {
        return Err(ReadFrameError::PluginExited);
    };
    let mut decoder = FrameDecoder::new(limits);
    let mut buf = [0u8; 8192];
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if cancel.is_cancelled() {
            return Err(ReadFrameError::Cancelled);
        }
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err(ReadFrameError::Timeout);
        }
        let read_res = tokio::time::timeout(remaining, out.read(&mut buf)).await;
        match read_res {
            Err(_) => return Err(ReadFrameError::Timeout),
            Ok(Ok(0)) => {
                return match decoder.finish() {
                    Ok(Some(f)) => Ok(f),
                    Ok(None) | Err(FrameDecodeError::UnexpectedEof) => {
                        Err(ReadFrameError::Decode(FrameDecodeError::UnexpectedEof))
                    }
                    Err(e) => Err(ReadFrameError::Decode(e)),
                };
            }
            Ok(Ok(n)) => match decoder.push(&buf[..n]) {
                Ok(Some(frame)) => return Ok(frame),
                Ok(None) => continue,
                Err(e) => return Err(ReadFrameError::Decode(e)),
            },
            Ok(Err(e)) => return Err(ReadFrameError::Io(e.to_string())),
        }
    }
}

async fn close_and_reap(
    child: &mut Child,
    stdout: &mut Option<tokio::process::ChildStdout>,
    grace: Duration,
    cancel: &CancellationToken,
    interruption: &mut Option<InterruptionMeta>,
    exit_status: &mut Option<i32>,
    failures: &FailureCoordinator,
) {
    let deadline = tokio::time::Instant::now() + grace;
    if let Some(out) = stdout.as_mut() {
        let mut buf = [0u8; 1];
        while tokio::time::Instant::now() < deadline {
            if cancel.is_cancelled() {
                failures.record(FailureRecord::new(
                    FailureCode::HostInterrupted,
                    "interrupted during close",
                ));
                break;
            }
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            match tokio::time::timeout(remaining, out.read(&mut buf)).await {
                Ok(Ok(0)) | Ok(Err(_)) | Err(_) => break,
                Ok(Ok(_)) => {
                    failures.record(FailureRecord::new(
                        FailureCode::UnexpectedOutput,
                        "additional stdout after valid response",
                    ));
                    break;
                }
            }
        }
    }

    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
    match tokio::time::timeout(remaining.max(Duration::from_millis(1)), child.wait()).await {
        Ok(Ok(status)) => {
            *exit_status = status.code();
            if !status.success() && !failures.has_primary() {
                failures.record(FailureRecord::new(
                    FailureCode::PluginExited,
                    format!("plugin exited with status {status}"),
                ));
            } else if !status.success() {
                failures.record(FailureRecord::new(
                    FailureCode::PluginExited,
                    format!("plugin exited with status {status}"),
                ));
            }
        }
        Ok(Err(_)) | Err(_) => {
            failures.record(FailureRecord::new(
                FailureCode::PluginExitTimeout,
                "plugin did not exit within exit grace",
            ));
            terminate_child(child, cancel, interruption, exit_status).await;
        }
    }
}

async fn terminate_child(
    child: &mut Child,
    cancel: &CancellationToken,
    interruption: &mut Option<InterruptionMeta>,
    exit_status: &mut Option<i32>,
) {
    let attempted = child.start_kill().is_ok();
    let observed = match tokio::time::timeout(Duration::from_secs(2), child.wait()).await {
        Ok(Ok(status)) => {
            *exit_status = status.code();
            true
        }
        _ => false,
    };
    if cancel.is_cancelled() {
        *interruption = Some(InterruptionMeta {
            host_interrupted: true,
            direct_child_termination_attempted: attempted,
            direct_child_exit_observed: observed,
        });
    } else if attempted {
        *interruption = Some(InterruptionMeta {
            host_interrupted: false,
            direct_child_termination_attempted: attempted,
            direct_child_exit_observed: observed,
        });
    }
}

fn push_message(
    messages: &mut Vec<MessageRecord>,
    redaction: &RedactionPolicy,
    direction: &str,
    method: Option<&str>,
    raw: &[u8],
) {
    let mut value = serde_json::from_slice::<Value>(raw).unwrap_or(Value::Null);
    let count = redaction.apply(&mut value);
    let _ = count;
    messages.push(MessageRecord {
        direction: direction.into(),
        method: method.map(str::to_string),
        message: value,
        raw_sha256: sha256_hex(raw),
    });
}

#[allow(clippy::too_many_arguments)]
fn finish<A: AuditStore, S: CompactSink>(
    run_id: &str,
    config: &RunConfig,
    failures: &FailureCoordinator,
    redaction: Option<&RedactionPolicy>,
    messages: Vec<MessageRecord>,
    unknown_headers: Vec<BTreeMap<String, String>>,
    stderr: StderrCapture,
    timings_ms: BTreeMap<String, u64>,
    exit_status: Option<i32>,
    mut hashes: BTreeMap<String, String>,
    plugin_version: Option<String>,
    interruption: Option<InterruptionMeta>,
    audit: &A,
    sink: &mut S,
    payload: Value,
) -> RunOutcome {
    let redaction_meta = match redaction {
        Some(r) => RedactionMeta {
            pointers: r.pointers.clone(),
            no_redaction_policy: r.is_noop(),
            redacted_field_count: 0,
        },
        None => RedactionMeta {
            pointers: config.redact_pointers.clone(),
            no_redaction_policy: config.redact_pointers.is_empty(),
            redacted_field_count: 0,
        },
    };

    let primary = failures.primary();
    let secondary = failures.secondary();

    let mut doc = SidecarDocument {
        protocol_version: PROTOCOL_VERSION.into(),
        host_version: HOST_VERSION.into(),
        plugin_version,
        run_id: run_id.into(),
        limits: config.limits,
        plugin_env_keys: config.plugin_env_keys.clone(),
        redaction: redaction_meta,
        messages,
        unknown_headers,
        stderr,
        timings_ms,
        exit_status,
        primary_failure: primary.clone(),
        secondary_failures: secondary,
        hashes: hashes.clone(),
        interruption,
        commit: None,
    };

    let mut sidecar_path: Option<PathBuf> = None;
    let bytes = match doc.to_vec() {
        Ok(b) => b,
        Err(e) => {
            failures.record_audit_failure(FailureRecord::new(
                FailureCode::SidecarWriteFailed,
                e.to_string(),
            ));
            Vec::new()
        }
    };

    if !bytes.is_empty() {
        match audit.persist(
            &config.sidecar_dir,
            run_id,
            &bytes,
            config.limits.sidecar_max_bytes,
        ) {
            Ok(res) => {
                doc.commit = Some(CommitMeta {
                    sync_level: res.sync_level.clone(),
                    commit_method: res.commit_method.clone(),
                });
                // Re-serialize with commit metadata for completeness if small enough.
                if let Ok(with_commit) = doc.to_vec() {
                    if with_commit.len() <= config.limits.sidecar_max_bytes {
                        // Best-effort: already committed once; keep first path to honor no-overwrite.
                        let _ = with_commit;
                    }
                }
                sidecar_path = Some(res.path);
            }
            Err(AuditError::TooLarge(n)) => {
                failures.record_audit_failure(FailureRecord::new(
                    FailureCode::SidecarTooLarge,
                    format!("sidecar {n} bytes exceed cap"),
                ));
            }
            Err(e) => {
                failures.record_audit_failure(FailureRecord::new(
                    FailureCode::SidecarWriteFailed,
                    e.to_string(),
                ));
            }
        }
    }

    let primary = failures.primary();
    let artifacts = sidecar_path
        .as_ref()
        .map(|p| vec![CompactArtifact::new(p.to_string_lossy())])
        .unwrap_or_default();

    let envelope = if let Some(fail) = primary.clone() {
        CompactEnvelope::error(
            PROTOCOL_VERSION,
            run_id,
            fail.category,
            fail.code,
            fail.message,
            artifacts,
        )
    } else {
        CompactEnvelope::ok(PROTOCOL_VERSION, run_id, payload, artifacts)
    };

    let env_bytes = serde_json::to_vec(&envelope).unwrap_or_default();
    hashes.insert("compact".into(), sha256_hex(&env_bytes));
    if let Err(e) = sink.write_all(&env_bytes) {
        failures.record_stdout_failure(FailureRecord::new(
            FailureCode::StdoutWriteFailed,
            e.to_string(),
        ));
        tracing::error!(
            run_id = %run_id,
            code = %FailureCode::StdoutWriteFailed,
            "stdout_write_failed; see stderr correlation"
        );
        eprintln!(
            "agentmesh: run_id={run_id} code=stdout_write_failed category=internal message={e}"
        );
    } else {
        // newline for CLI friendliness is NOT allowed for machine stdout contract —
        // emit exactly one JSON object; consumers may accept no trailing newline.
        // Spec: exactly one compact JSON object. Prefer no extra bytes.
    }

    let exit_code = failures
        .primary()
        .map(|f| f.category.exit_code())
        .unwrap_or(0);

    RunOutcome {
        envelope: if failures.primary().map(|p| p.code) == Some(FailureCode::StdoutWriteFailed) {
            // Envelope may be incomplete on the wire; still return computed envelope for tests.
            CompactEnvelope::error(
                PROTOCOL_VERSION,
                run_id,
                agentmesh_proto::FailureCategory::Internal,
                FailureCode::StdoutWriteFailed,
                "stdout write failed",
                sidecar_path
                    .as_ref()
                    .map(|p| vec![CompactArtifact::new(p.to_string_lossy())])
                    .unwrap_or_default(),
            )
        } else {
            envelope
        },
        exit_code,
        sidecar_path,
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}
