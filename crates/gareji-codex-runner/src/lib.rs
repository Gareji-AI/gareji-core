//! Codex CLI implementation of the Gareji Core Runner Adapter seam.

use std::env;
use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use gareji_core::runner::{
    AuthorizedRunnerRequest, RunnerAdapter, RunnerFailure, RunnerFailureCategory, RunnerOutcome,
    RunnerReport,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use thiserror::Error;

const MAX_PROMPT_CHARS: usize = 524_288;
const MAX_OPTION_CHARS: usize = 256;
const MAX_STDOUT_BYTES: usize = 1_048_576;
const MAX_STDERR_BYTES: usize = 262_144;
const MAX_JSONL_EVENTS: usize = 10_000;
const MAX_FINAL_OUTPUT_CHARS: usize = 50_000;
const MAX_SUMMARY_CHARS: usize = 4_096;
const POLL_INTERVAL: Duration = Duration::from_millis(20);
const PIPE_DRAIN_GRACE: Duration = Duration::from_secs(1);

/// Stable capability that permits Codex to modify its primary workspace.
pub const WORKSPACE_WRITE_CAPABILITY: &str = "workspace.write";
/// Stable capability accepted for read-only workspace and instruction access.
pub const CONTEXT_READ_CAPABILITY: &str = "context.read";

/// Runtime-owned input accepted by the Codex Runner Adapter.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CodexRunPayload {
    /// Complete instruction passed to one non-interactive Codex turn.
    pub prompt: String,
    /// Existing prepared workspace used as the Codex working root.
    pub workspace: PathBuf,
    /// Optional explicit model selection owned by the caller.
    pub model: Option<String>,
}

/// Executes one authorized request through the stable non-interactive Codex CLI.
pub struct CodexRunnerAdapter {
    process: Box<dyn CodexProcessInvoker>,
}

impl CodexRunnerAdapter {
    /// Use the Codex executable from GAREJI_CODEX_BIN, or codex from PATH.
    #[must_use]
    pub fn from_environment() -> Self {
        let executable =
            env::var_os("GAREJI_CODEX_BIN").map_or_else(|| PathBuf::from("codex"), PathBuf::from);
        Self::new(executable)
    }

    /// Use one explicit Codex executable or command name.
    #[must_use]
    pub fn new(executable: impl Into<PathBuf>) -> Self {
        Self {
            process: Box::new(SystemCodexProcess {
                executable: executable.into(),
            }),
        }
    }

    #[cfg(test)]
    fn with_process(process: impl CodexProcessInvoker + 'static) -> Self {
        Self {
            process: Box::new(process),
        }
    }
}

impl Default for CodexRunnerAdapter {
    fn default() -> Self {
        Self::from_environment()
    }
}

impl RunnerAdapter for CodexRunnerAdapter {
    fn execute(&mut self, request: &AuthorizedRunnerRequest) -> RunnerReport {
        let payload: CodexRunPayload = match serde_json::from_value(request.payload().clone()) {
            Ok(payload) => payload,
            Err(_) => return configuration_failure("invalid_codex_payload", "invalid Codex input"),
        };
        if let Err(violation) = validate_payload(&payload) {
            return configuration_failure(violation.code, violation.message);
        }
        if let Some(capability) = request
            .authorized_capabilities()
            .iter()
            .find(|capability| !is_supported_capability(capability))
        {
            return RunnerReport {
                outcome: RunnerOutcome::Failed,
                summary: "Codex cannot enforce an authorized capability".to_owned(),
                payload: json!({"unsupported_capability": capability}),
                evidence: Vec::new(),
                failure: Some(RunnerFailure {
                    category: RunnerFailureCategory::Configuration,
                    code: "unsupported_capability".to_owned(),
                    message: "the Codex Adapter does not map this capability".to_owned(),
                }),
            };
        }

        let sandbox = if request
            .authorized_capabilities()
            .iter()
            .any(|capability| capability == WORKSPACE_WRITE_CAPABILITY)
        {
            CodexSandbox::WorkspaceWrite
        } else {
            CodexSandbox::ReadOnly
        };
        let invocation = CodexInvocation::new(payload, sandbox);
        let output = match self.process.invoke(
            &invocation,
            Duration::from_secs(u64::from(request.timeout_seconds())),
        ) {
            Ok(output) => output,
            Err(ProcessInvokeError::Spawn) => {
                return configuration_failure("codex_unavailable", "Codex could not be started");
            }
            Err(ProcessInvokeError::Io) => {
                return internal_failure("codex_process_io", "Codex process communication failed");
            }
        };
        translate_output(output, request.timeout_seconds())
    }
}

struct PayloadViolation {
    code: &'static str,
    message: &'static str,
}

fn validate_payload(payload: &CodexRunPayload) -> Result<(), PayloadViolation> {
    if !valid_text(&payload.prompt, MAX_PROMPT_CHARS) {
        return Err(PayloadViolation {
            code: "invalid_prompt",
            message: "Codex prompt is empty or invalid",
        });
    }
    if !payload.workspace.is_absolute() || !payload.workspace.is_dir() {
        return Err(PayloadViolation {
            code: "invalid_workspace",
            message: "Codex workspace must be an existing absolute directory",
        });
    }
    if payload
        .model
        .as_deref()
        .is_some_and(|model| !valid_single_line(model, MAX_OPTION_CHARS))
    {
        return Err(PayloadViolation {
            code: "invalid_model",
            message: "Codex model selection is invalid",
        });
    }
    Ok(())
}

fn is_supported_capability(capability: &str) -> bool {
    matches!(
        capability,
        CONTEXT_READ_CAPABILITY | WORKSPACE_WRITE_CAPABILITY
    )
}

fn valid_text(value: &str, max_chars: usize) -> bool {
    !value.trim().is_empty()
        && value.chars().count() <= max_chars
        && value
            .chars()
            .all(|character| !character.is_control() || matches!(character, '\n' | '\r' | '\t'))
}

fn valid_single_line(value: &str, max_chars: usize) -> bool {
    !value.trim().is_empty()
        && value.chars().count() <= max_chars
        && !value.chars().any(char::is_control)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CodexSandbox {
    ReadOnly,
    WorkspaceWrite,
}

impl CodexSandbox {
    const fn as_arg(self) -> &'static str {
        match self {
            Self::ReadOnly => "read-only",
            Self::WorkspaceWrite => "workspace-write",
        }
    }
}

#[derive(Debug)]
struct CodexInvocation {
    args: Vec<OsString>,
    prompt: String,
    workspace: PathBuf,
}

impl CodexInvocation {
    fn new(payload: CodexRunPayload, sandbox: CodexSandbox) -> Self {
        let mut args = vec![
            OsString::from("exec"),
            OsString::from("--json"),
            OsString::from("--ephemeral"),
            OsString::from("--color"),
            OsString::from("never"),
            OsString::from("--ignore-user-config"),
            OsString::from("--disable"),
            OsString::from("hooks"),
            OsString::from("--sandbox"),
            OsString::from(sandbox.as_arg()),
            OsString::from("--cd"),
            payload.workspace.as_os_str().to_owned(),
            OsString::from("--config"),
            OsString::from("approval_policy=\"never\""),
            OsString::from("--config"),
            OsString::from("web_search=\"disabled\""),
            OsString::from("--config"),
            OsString::from("mcp_servers={}"),
        ];
        if let Some(model) = payload.model {
            args.extend([OsString::from("--model"), OsString::from(model)]);
        }
        args.push(OsString::from("-"));
        Self {
            args,
            prompt: payload.prompt,
            workspace: payload.workspace,
        }
    }
}

trait CodexProcessInvoker {
    fn invoke(
        &mut self,
        invocation: &CodexInvocation,
        timeout: Duration,
    ) -> Result<ProcessOutput, ProcessInvokeError>;
}

struct SystemCodexProcess {
    executable: PathBuf,
}

impl CodexProcessInvoker for SystemCodexProcess {
    fn invoke(
        &mut self,
        invocation: &CodexInvocation,
        timeout: Duration,
    ) -> Result<ProcessOutput, ProcessInvokeError> {
        let mut command = Command::new(&self.executable);
        command
            .args(&invocation.args)
            .current_dir(&invocation.workspace)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        configure_background_process(&mut command);
        let mut child = command.spawn().map_err(|_| ProcessInvokeError::Spawn)?;
        capture_child(&mut child, invocation.prompt.as_bytes(), timeout)
    }
}

#[cfg(windows)]
fn configure_background_process(command: &mut Command) {
    use std::os::windows::process::CommandExt;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn configure_background_process(_command: &mut Command) {}

fn capture_child(
    child: &mut Child,
    prompt: &[u8],
    timeout: Duration,
) -> Result<ProcessOutput, ProcessInvokeError> {
    let mut stdin = child.stdin.take().ok_or(ProcessInvokeError::Io)?;
    let stdout = child.stdout.take().ok_or(ProcessInvokeError::Io)?;
    let stderr = child.stderr.take().ok_or(ProcessInvokeError::Io)?;
    let prompt = prompt.to_vec();

    let (input_sender, input_receiver) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let result = stdin
            .write_all(&prompt)
            .and_then(|()| stdin.flush())
            .map_err(|_| ProcessInvokeError::Io);
        let _ = input_sender.send(result);
    });
    let (stdout_sender, stdout_receiver) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let _ = stdout_sender.send(read_bounded(stdout, MAX_STDOUT_BYTES));
    });
    let (stderr_sender, stderr_receiver) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let _ = stderr_sender.send(read_bounded(stderr, MAX_STDERR_BYTES));
    });

    let deadline = Instant::now() + timeout;
    let (status, timed_out) = loop {
        match child.try_wait().map_err(|_| ProcessInvokeError::Io)? {
            Some(status) => break (status, false),
            None if Instant::now() >= deadline => {
                child.kill().map_err(|_| ProcessInvokeError::Io)?;
                let status = child.wait().map_err(|_| ProcessInvokeError::Io)?;
                break (status, true);
            }
            None => thread::sleep(POLL_INTERVAL),
        }
    };

    if timed_out {
        return Ok(ProcessOutput {
            exit_code: status.code(),
            timed_out: true,
            stdout: BoundedOutput::default(),
            stderr: BoundedOutput::default(),
        });
    }
    let drain_deadline = Instant::now() + PIPE_DRAIN_GRACE;
    let _input_result = receive_before(&input_receiver, drain_deadline)?;
    let stdout = receive_before(&stdout_receiver, drain_deadline)??;
    let stderr = receive_before(&stderr_receiver, drain_deadline)??;

    Ok(ProcessOutput {
        exit_code: status.code(),
        timed_out,
        stdout,
        stderr,
    })
}

fn receive_before<T>(
    receiver: &mpsc::Receiver<T>,
    deadline: Instant,
) -> Result<T, ProcessInvokeError> {
    receiver
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .map_err(|_| ProcessInvokeError::Io)
}

fn read_bounded(mut reader: impl Read, limit: usize) -> Result<BoundedOutput, ProcessInvokeError> {
    let mut retained = Vec::new();
    let mut exceeded = false;
    let mut buffer = [0_u8; 8_192];
    loop {
        let count = reader
            .read(&mut buffer)
            .map_err(|_| ProcessInvokeError::Io)?;
        if count == 0 {
            break;
        }
        let remaining = limit.saturating_sub(retained.len());
        let retained_count = remaining.min(count);
        retained.extend_from_slice(&buffer[..retained_count]);
        if retained_count < count {
            exceeded = true;
        }
    }
    Ok(BoundedOutput {
        bytes: retained,
        exceeded,
    })
}

#[derive(Debug, Error)]
enum ProcessInvokeError {
    #[error("Codex process could not be spawned")]
    Spawn,
    #[error("Codex process communication failed")]
    Io,
}

#[derive(Debug, Default)]
struct BoundedOutput {
    bytes: Vec<u8>,
    exceeded: bool,
}

#[derive(Debug)]
struct ProcessOutput {
    exit_code: Option<i32>,
    timed_out: bool,
    stdout: BoundedOutput,
    #[allow(dead_code)]
    stderr: BoundedOutput,
}

fn translate_output(output: ProcessOutput, timeout_seconds: u32) -> RunnerReport {
    if output.timed_out {
        return RunnerReport {
            outcome: RunnerOutcome::Failed,
            summary: "Codex exceeded the authorized execution deadline".to_owned(),
            payload: json!({"timeout_seconds": timeout_seconds}),
            evidence: Vec::new(),
            failure: Some(RunnerFailure {
                category: RunnerFailureCategory::Timeout,
                code: "codex_timeout".to_owned(),
                message: "Codex exceeded the execution deadline".to_owned(),
            }),
        };
    }
    if output.stdout.exceeded {
        return protocol_failure(
            "codex_output_too_large",
            "Codex JSONL output exceeded its bound",
        );
    }
    let stream = match std::str::from_utf8(&output.stdout.bytes) {
        Ok(stream) => stream,
        Err(_) => return protocol_failure("invalid_codex_utf8", "Codex output was not UTF-8"),
    };
    let events = match parse_events(stream) {
        Ok(events) => events,
        Err(code) => return protocol_failure(code, "Codex returned invalid JSONL events"),
    };

    if output.exit_code == Some(130) {
        return RunnerReport {
            outcome: RunnerOutcome::Cancelled,
            summary: "Codex invocation was cancelled".to_owned(),
            payload: compact_event_payload(&events),
            evidence: Vec::new(),
            failure: Some(RunnerFailure {
                category: RunnerFailureCategory::Cancelled,
                code: "codex_cancelled".to_owned(),
                message: "Codex exited after cancellation".to_owned(),
            }),
        };
    }
    if output.exit_code != Some(0) {
        return RunnerReport {
            outcome: RunnerOutcome::Failed,
            summary: "Codex invocation failed".to_owned(),
            payload: json!({
                "exit_code": output.exit_code,
                "thread_id": events.thread_id,
            }),
            evidence: Vec::new(),
            failure: Some(RunnerFailure {
                category: RunnerFailureCategory::Execution,
                code: "codex_exit_nonzero".to_owned(),
                message: "Codex exited without successful completion".to_owned(),
            }),
        };
    }
    if events.turn_failed || events.error_seen {
        return RunnerReport {
            outcome: RunnerOutcome::Failed,
            summary: "Codex reported a failed turn".to_owned(),
            payload: compact_event_payload(&events),
            evidence: Vec::new(),
            failure: Some(RunnerFailure {
                category: RunnerFailureCategory::Execution,
                code: "codex_turn_failed".to_owned(),
                message: "Codex reported a terminal execution failure".to_owned(),
            }),
        };
    }
    if !events.turn_completed {
        return protocol_failure(
            "missing_terminal_event",
            "Codex exited without a terminal turn event",
        );
    }

    let summary = events.final_output.as_deref().map_or_else(
        || "Codex completed without a final agent message".to_owned(),
        |message| bounded_text(message, MAX_SUMMARY_CHARS),
    );
    RunnerReport {
        outcome: RunnerOutcome::Succeeded,
        summary,
        payload: compact_event_payload(&events),
        evidence: Vec::new(),
        failure: None,
    }
}

#[derive(Default)]
struct ParsedEvents {
    thread_id: Option<String>,
    final_output: Option<String>,
    usage: Option<Value>,
    turn_completed: bool,
    turn_failed: bool,
    error_seen: bool,
}

fn parse_events(stream: &str) -> Result<ParsedEvents, &'static str> {
    let mut parsed = ParsedEvents::default();
    let mut count = 0_usize;
    for line in stream.lines().filter(|line| !line.trim().is_empty()) {
        count = count.saturating_add(1);
        if count > MAX_JSONL_EVENTS {
            return Err("too_many_codex_events");
        }
        let event: Value = serde_json::from_str(line).map_err(|_| "invalid_codex_jsonl")?;
        let event_type = event
            .get("type")
            .and_then(Value::as_str)
            .ok_or("missing_codex_event_type")?;
        match event_type {
            "thread.started" => {
                let thread_id = event
                    .get("thread_id")
                    .and_then(Value::as_str)
                    .ok_or("invalid_thread_started")?;
                if !valid_single_line(thread_id, MAX_OPTION_CHARS) {
                    return Err("invalid_thread_started");
                }
                parsed.thread_id = Some(thread_id.to_owned());
            }
            "item.completed" => {
                let item = event.get("item").ok_or("invalid_item_completed")?;
                let item_type = item
                    .get("type")
                    .and_then(Value::as_str)
                    .ok_or("invalid_item_completed")?;
                if item_type == "agent_message" {
                    let text = item
                        .get("text")
                        .and_then(Value::as_str)
                        .ok_or("invalid_agent_message")?;
                    parsed.final_output = Some(bounded_text(text, MAX_FINAL_OUTPUT_CHARS));
                }
            }
            "turn.completed" => {
                parsed.turn_completed = true;
                parsed.usage = compact_usage(event.get("usage"))?;
            }
            "turn.failed" => parsed.turn_failed = true,
            "error" => parsed.error_seen = true,
            _ => {}
        }
    }
    Ok(parsed)
}

fn compact_usage(value: Option<&Value>) -> Result<Option<Value>, &'static str> {
    let Some(value) = value else {
        return Ok(None);
    };
    let source = value.as_object().ok_or("invalid_turn_completed")?;
    let mut usage = serde_json::Map::new();
    for field in [
        "input_tokens",
        "cached_input_tokens",
        "output_tokens",
        "reasoning_output_tokens",
    ] {
        if let Some(value) = source.get(field) {
            if !value.is_u64() {
                return Err("invalid_turn_completed");
            }
            usage.insert(field.to_owned(), value.clone());
        }
    }
    Ok(Some(Value::Object(usage)))
}

fn compact_event_payload(events: &ParsedEvents) -> Value {
    json!({
        "thread_id": events.thread_id,
        "final_output": events.final_output,
        "usage": events.usage,
    })
}

fn bounded_text(value: &str, max_chars: usize) -> String {
    let mut bounded = String::new();
    for character in value.chars().take(max_chars) {
        if character.is_control() && !matches!(character, '\n' | '\r' | '\t') {
            bounded.push(' ');
        } else {
            bounded.push(character);
        }
    }
    if bounded.trim().is_empty() {
        "Codex returned an empty result".to_owned()
    } else {
        bounded
    }
}

fn configuration_failure(code: &str, message: &str) -> RunnerReport {
    failure_report(RunnerFailureCategory::Configuration, code, message)
}

fn internal_failure(code: &str, message: &str) -> RunnerReport {
    failure_report(RunnerFailureCategory::Internal, code, message)
}

fn protocol_failure(code: &str, message: &str) -> RunnerReport {
    failure_report(RunnerFailureCategory::Protocol, code, message)
}

fn failure_report(category: RunnerFailureCategory, code: &str, message: &str) -> RunnerReport {
    RunnerReport {
        outcome: RunnerOutcome::Failed,
        summary: message.to_owned(),
        payload: Value::Object(serde_json::Map::new()),
        evidence: Vec::new(),
        failure: Some(RunnerFailure {
            category,
            code: code.to_owned(),
            message: message.to_owned(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::BTreeSet;
    use std::path::Path;
    use std::rc::Rc;

    use gareji_core::policy::{ApprovalSource, ApprovalSourceError, CapabilityPolicy};
    use gareji_core::runner::{CoreRunRequest, CoreRunResult, RunExecutor};
    use tempfile::tempdir;

    use super::*;

    struct NoApprovals;

    impl ApprovalSource for NoApprovals {
        fn approved_capabilities(
            &self,
            _operation_id: &str,
        ) -> Result<BTreeSet<String>, ApprovalSourceError> {
            Ok(BTreeSet::new())
        }
    }

    #[derive(Clone)]
    struct StubProcess {
        invocation: Rc<RefCell<Option<CapturedInvocation>>>,
        output: Rc<RefCell<Option<Result<ProcessOutput, ProcessInvokeError>>>>,
    }

    #[derive(Debug)]
    struct CapturedInvocation {
        args: Vec<String>,
        prompt: String,
        workspace: PathBuf,
        timeout: Duration,
    }

    impl CodexProcessInvoker for StubProcess {
        fn invoke(
            &mut self,
            invocation: &CodexInvocation,
            timeout: Duration,
        ) -> Result<ProcessOutput, ProcessInvokeError> {
            self.invocation.replace(Some(CapturedInvocation {
                args: invocation
                    .args
                    .iter()
                    .map(|argument| argument.to_string_lossy().into_owned())
                    .collect(),
                prompt: invocation.prompt.clone(),
                workspace: invocation.workspace.clone(),
                timeout,
            }));
            self.output
                .borrow_mut()
                .take()
                .expect("one configured process output")
        }
    }

    #[test]
    fn executes_jsonl_codex_with_the_authorized_workspace_sandbox() {
        let directory = tempdir().unwrap();
        let (adapter, invocation) = adapter_with_output(success_output());
        let mut executor = executor(
            adapter,
            &[CONTEXT_READ_CAPABILITY, WORKSPACE_WRITE_CAPABILITY],
        );

        let result = executor
            .execute(request(
                directory.path(),
                &[CONTEXT_READ_CAPABILITY, WORKSPACE_WRITE_CAPABILITY],
            ))
            .unwrap();

        let CoreRunResult::Finished { report, .. } = result else {
            panic!("expected finished result");
        };
        assert_eq!(report.outcome, RunnerOutcome::Succeeded);
        assert_eq!(report.summary, "Implemented the requested change.");
        assert_eq!(
            report.payload,
            json!({
                "thread_id": "thread-1",
                "final_output": "Implemented the requested change.",
                "usage": {"input_tokens": 10, "output_tokens": 5}
            })
        );
        let invocation = invocation.borrow();
        let invocation = invocation.as_ref().unwrap();
        assert!(has_pair(&invocation.args, "--sandbox", "workspace-write"));
        assert!(has_pair(&invocation.args, "--disable", "hooks"));
        assert!(has_pair(
            &invocation.args,
            "--config",
            "web_search=\"disabled\""
        ));
        assert!(has_pair(&invocation.args, "--config", "mcp_servers={}"));
        assert_eq!(invocation.args.last().map(String::as_str), Some("-"));
        assert_eq!(invocation.prompt, "Implement the feature.");
        assert_eq!(invocation.workspace, directory.path());
        assert_eq!(invocation.timeout, Duration::from_secs(30));
    }

    #[test]
    fn defaults_to_read_only_without_workspace_write() {
        let directory = tempdir().unwrap();
        let (adapter, invocation) = adapter_with_output(success_output());
        let mut executor = executor(adapter, &[CONTEXT_READ_CAPABILITY]);

        executor
            .execute(request(directory.path(), &[CONTEXT_READ_CAPABILITY]))
            .unwrap();

        assert!(has_pair(
            &invocation.borrow().as_ref().unwrap().args,
            "--sandbox",
            "read-only"
        ));
    }

    #[test]
    fn rejects_capabilities_it_cannot_enforce_without_invoking_codex() {
        let directory = tempdir().unwrap();
        let (adapter, invocation) = adapter_with_output(success_output());
        let mut executor = executor(adapter, &["production.write"]);

        let result = executor
            .execute(request(directory.path(), &["production.write"]))
            .unwrap();

        let CoreRunResult::Finished { report, .. } = result else {
            panic!("expected finished result");
        };
        assert_failure(
            &report,
            RunnerFailureCategory::Configuration,
            "unsupported_capability",
        );
        assert!(invocation.borrow().is_none());
    }

    #[test]
    fn maps_timeout_and_invalid_jsonl_to_stable_failures() {
        let directory = tempdir().unwrap();
        let (adapter, _) = adapter_with_output(ProcessOutput {
            exit_code: None,
            timed_out: true,
            stdout: bounded(""),
            stderr: bounded("secret stderr"),
        });
        let mut timeout_executor = executor(adapter, &[CONTEXT_READ_CAPABILITY]);
        let result = timeout_executor
            .execute(request(directory.path(), &[CONTEXT_READ_CAPABILITY]))
            .unwrap();
        let CoreRunResult::Finished { report, .. } = result else {
            panic!("expected finished result");
        };
        assert_failure(&report, RunnerFailureCategory::Timeout, "codex_timeout");
        assert!(!serde_json::to_string(&report).unwrap().contains("secret"));

        let (adapter, _) = adapter_with_output(ProcessOutput {
            exit_code: Some(0),
            timed_out: false,
            stdout: bounded("not-json\n"),
            stderr: bounded(""),
        });
        let mut protocol_executor = executor(adapter, &[CONTEXT_READ_CAPABILITY]);
        let result = protocol_executor
            .execute(request(directory.path(), &[CONTEXT_READ_CAPABILITY]))
            .unwrap();
        let CoreRunResult::Finished { report, .. } = result else {
            panic!("expected finished result");
        };
        assert_failure(
            &report,
            RunnerFailureCategory::Protocol,
            "invalid_codex_jsonl",
        );
    }

    #[test]
    fn rejects_malformed_usage_in_a_known_completion_event() {
        let directory = tempdir().unwrap();
        let (adapter, _) = adapter_with_output(ProcessOutput {
            exit_code: Some(0),
            timed_out: false,
            stdout: bounded("{\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":\"ten\"}}\n"),
            stderr: bounded(""),
        });
        let mut executor = executor(adapter, &[CONTEXT_READ_CAPABILITY]);

        let result = executor
            .execute(request(directory.path(), &[CONTEXT_READ_CAPABILITY]))
            .unwrap();

        let CoreRunResult::Finished { report, .. } = result else {
            panic!("expected finished result");
        };
        assert_failure(
            &report,
            RunnerFailureCategory::Protocol,
            "invalid_turn_completed",
        );
    }

    #[test]
    fn maps_nonzero_exit_without_persisting_stderr() {
        let directory = tempdir().unwrap();
        let (adapter, _) = adapter_with_output(ProcessOutput {
            exit_code: Some(2),
            timed_out: false,
            stdout: bounded("{\"type\":\"thread.started\",\"thread_id\":\"thread-1\"}\n"),
            stderr: bounded("credential-shaped stderr"),
        });
        let mut executor = executor(adapter, &[CONTEXT_READ_CAPABILITY]);

        let result = executor
            .execute(request(directory.path(), &[CONTEXT_READ_CAPABILITY]))
            .unwrap();

        let CoreRunResult::Finished { report, .. } = result else {
            panic!("expected finished result");
        };
        assert_failure(
            &report,
            RunnerFailureCategory::Execution,
            "codex_exit_nonzero",
        );
        assert!(!serde_json::to_string(&report)
            .unwrap()
            .contains("credential"));
    }

    #[test]
    fn rejects_invalid_adapter_payload_before_invoking_codex() {
        let (adapter, invocation) = adapter_with_output(success_output());
        let mut executor = executor(adapter, &[CONTEXT_READ_CAPABILITY]);
        let request = CoreRunRequest {
            run_id: "run-1".to_owned(),
            timeout_seconds: 30,
            requested_capabilities: set(&[CONTEXT_READ_CAPABILITY]),
            payload: json!({"prompt": "missing workspace"}),
        };

        let result = executor.execute(request).unwrap();

        let CoreRunResult::Finished { report, .. } = result else {
            panic!("expected finished result");
        };
        assert_failure(
            &report,
            RunnerFailureCategory::Configuration,
            "invalid_codex_payload",
        );
        assert!(invocation.borrow().is_none());
    }

    fn executor(
        adapter: CodexRunnerAdapter,
        allowed: &[&str],
    ) -> RunExecutor<NoApprovals, CodexRunnerAdapter> {
        RunExecutor::new(
            CapabilityPolicy::new(set(allowed), BTreeSet::new()).unwrap(),
            NoApprovals,
            adapter,
        )
    }

    fn request(workspace: &Path, capabilities: &[&str]) -> CoreRunRequest {
        CoreRunRequest {
            run_id: "run-1".to_owned(),
            timeout_seconds: 30,
            requested_capabilities: set(capabilities),
            payload: serde_json::to_value(CodexRunPayload {
                prompt: "Implement the feature.".to_owned(),
                workspace: workspace.to_owned(),
                model: Some("gpt-test".to_owned()),
            })
            .unwrap(),
        }
    }

    fn adapter_with_output(
        output: ProcessOutput,
    ) -> (CodexRunnerAdapter, Rc<RefCell<Option<CapturedInvocation>>>) {
        let invocation = Rc::new(RefCell::new(None));
        let process = StubProcess {
            invocation: Rc::clone(&invocation),
            output: Rc::new(RefCell::new(Some(Ok(output)))),
        };
        (CodexRunnerAdapter::with_process(process), invocation)
    }

    fn success_output() -> ProcessOutput {
        ProcessOutput {
            exit_code: Some(0),
            timed_out: false,
            stdout: bounded(
                "{\"type\":\"thread.started\",\"thread_id\":\"thread-1\"}\n\
                 {\"type\":\"turn.started\"}\n\
                 {\"type\":\"item.completed\",\"item\":{\"id\":\"item-1\",\"type\":\"agent_message\",\"text\":\"Implemented the requested change.\"}}\n\
                 {\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":10,\"output_tokens\":5}}\n",
            ),
            stderr: bounded("progress only"),
        }
    }

    fn bounded(value: &str) -> BoundedOutput {
        BoundedOutput {
            bytes: value.as_bytes().to_vec(),
            exceeded: false,
        }
    }

    fn set(values: &[&str]) -> BTreeSet<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    fn has_pair(args: &[String], first: &str, second: &str) -> bool {
        args.windows(2)
            .any(|pair| pair[0] == first && pair[1] == second)
    }

    fn assert_failure(report: &RunnerReport, category: RunnerFailureCategory, code: &str) {
        assert_eq!(report.outcome, RunnerOutcome::Failed);
        let failure = report.failure.as_ref().unwrap();
        assert_eq!(failure.category, category);
        assert_eq!(failure.code, code);
    }
}
