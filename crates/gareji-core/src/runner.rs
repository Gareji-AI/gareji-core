//! Policy-enforcing execution seam for replaceable Runner Adapters.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::policy::{
    ApprovalSource, CapabilityGate, CapabilityPolicy, ExecutionIntent, GateError, PolicyDecision,
    PolicyRejection,
};

const MAX_RUN_ID_CHARS: usize = 128;
const MAX_TIMEOUT_SECONDS: u32 = 86_400;
const MAX_REQUEST_BYTES: usize = 1024 * 1024;
const MAX_RESULT_BYTES: usize = 256 * 1024;
const MAX_JSON_DEPTH: usize = 64;
const MAX_JSON_NODES: usize = 100_000;
const MAX_SUMMARY_CHARS: usize = 4_096;
const MAX_FAILURE_MESSAGE_CHARS: usize = 2_048;
const MAX_EVIDENCE_REFERENCES: usize = 32;
const MAX_EVIDENCE_REFERENCE_CHARS: usize = 2_048;
const MAX_TOKEN_BYTES: usize = 128;

/// Deep Core Module that validates, authorizes, and executes one bounded Run.
pub struct RunExecutor<S, R> {
    gate: CapabilityGate<S>,
    runner: R,
}

impl<S: ApprovalSource, R: RunnerAdapter> RunExecutor<S, R> {
    /// Create an executor from local policy, a trusted approval source, and one Runner Adapter.
    pub fn new(policy: CapabilityPolicy, approvals: S, runner: R) -> Self {
        Self {
            gate: CapabilityGate::new(policy, approvals),
            runner,
        }
    }

    /// Validate and authorize one request before invoking the Runner Adapter exactly once.
    pub fn execute(&mut self, request: CoreRunRequest) -> Result<CoreRunResult, CoreRunError> {
        validate_request(&request).map_err(CoreRunError::InvalidRequest)?;

        let intent = ExecutionIntent {
            operation_id: request.run_id.clone(),
            requested: request.requested_capabilities.clone(),
        };
        match self.gate.authorize(&intent)? {
            PolicyDecision::Rejected { reasons } => Ok(CoreRunResult::Rejected {
                run_id: request.run_id,
                reasons,
            }),
            PolicyDecision::Approved { capabilities } => {
                let authorized = AuthorizedRunnerRequest {
                    run_id: request.run_id.clone(),
                    timeout_seconds: request.timeout_seconds,
                    authorized_capabilities: capabilities.clone(),
                    payload: request.payload,
                };
                let report = self.runner.execute(&authorized);
                validate_report(&report).map_err(CoreRunError::InvalidRunnerReport)?;

                Ok(CoreRunResult::Finished {
                    run_id: request.run_id,
                    authorized_capabilities: capabilities,
                    report,
                })
            }
        }
    }
}

/// Runtime-neutral Adapter that translates one authorized request into one invocation.
pub trait RunnerAdapter {
    /// Execute exactly one bounded runtime invocation and return a compact report.
    fn execute(&mut self, request: &AuthorizedRunnerRequest) -> RunnerReport;
}

/// Caller-supplied request before Core capability authorization.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoreRunRequest {
    /// Stable identity for this Core Run and its operation-bound approvals.
    pub run_id: String,
    /// Positive execution deadline supplied to the trusted Runner Adapter.
    pub timeout_seconds: u32,
    /// Capabilities the requested invocation will exercise.
    pub requested_capabilities: BTreeSet<String>,
    /// Runtime-owned structured input kept opaque to Core.
    pub payload: Value,
}

/// Request delivered to a Runner Adapter only after Core authorization.
#[derive(Clone, Debug, PartialEq)]
pub struct AuthorizedRunnerRequest {
    /// Stable Core Run identity.
    run_id: String,
    /// Positive execution deadline validated by Core.
    timeout_seconds: u32,
    /// Sorted capabilities authorized by Core for this exact Run.
    authorized_capabilities: Vec<String>,
    /// Runtime-owned structured input kept opaque to Core.
    payload: Value,
}

impl AuthorizedRunnerRequest {
    /// Stable Core Run identity.
    #[must_use]
    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    /// Positive execution deadline in seconds.
    #[must_use]
    pub const fn timeout_seconds(&self) -> u32 {
        self.timeout_seconds
    }

    /// Sorted capabilities authorized by Core for this exact Run.
    #[must_use]
    pub fn authorized_capabilities(&self) -> &[String] {
        &self.authorized_capabilities
    }

    /// Runtime-owned structured input kept opaque to Core.
    #[must_use]
    pub const fn payload(&self) -> &Value {
        &self.payload
    }
}

/// Stable result of policy evaluation and optional Runner invocation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CoreRunResult {
    /// Core policy rejected the request before Runner invocation.
    Rejected {
        /// Stable Core Run identity.
        run_id: String,
        /// Sorted policy rejection reasons.
        reasons: Vec<PolicyRejection>,
    },
    /// The Runner returned one bounded report after authorization.
    Finished {
        /// Stable Core Run identity.
        run_id: String,
        /// Sorted capabilities authorized for the invocation.
        authorized_capabilities: Vec<String>,
        /// Validated compact Runner report.
        report: RunnerReport,
    },
}

/// Compact runtime-neutral report returned by a Runner Adapter.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunnerReport {
    /// Terminal outcome of the invocation.
    pub outcome: RunnerOutcome,
    /// Bounded human-readable result summary without a raw transcript.
    pub summary: String,
    /// Runtime-owned compact structured result kept opaque to Core.
    pub payload: Value,
    /// Bounded references to evidence retained outside the compact report.
    pub evidence: Vec<EvidenceReference>,
    /// Stable failure detail required for failed and cancelled outcomes.
    pub failure: Option<RunnerFailure>,
}

/// Terminal Runner outcome.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunnerOutcome {
    /// The runtime completed the requested work successfully.
    Succeeded,
    /// The runtime completed with a non-cancellation failure.
    Failed,
    /// The invocation was cancelled before successful completion.
    Cancelled,
}

/// Reference to bounded Run evidence stored outside the compact result.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceReference {
    /// Stable lowercase evidence kind, such as `audit_sidecar` or `git_revision`.
    pub kind: String,
    /// Bounded Adapter-produced reference; never raw evidence content.
    pub reference: String,
}

/// Stable bounded Runner failure.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunnerFailure {
    /// Broad runtime-neutral failure category.
    pub category: RunnerFailureCategory,
    /// Stable lowercase Adapter-defined failure code.
    pub code: String,
    /// Safe bounded message without raw runtime output.
    pub message: String,
}

/// Runtime-neutral Runner failure category.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunnerFailureCategory {
    /// Runtime, model, profile, or Adapter configuration is invalid.
    Configuration,
    /// The runtime reported a work failure.
    Execution,
    /// The bounded invocation exceeded its deadline.
    Timeout,
    /// The invocation was cancelled.
    Cancelled,
    /// Runtime output violated the Adapter's expected protocol.
    Protocol,
    /// Required compact evidence could not be preserved.
    Evidence,
    /// An unexpected local invariant failed.
    Internal,
}

/// Failure before a trustworthy Core Run result can be returned.
#[derive(Debug, Error)]
pub enum CoreRunError {
    /// The caller supplied a request outside the bounded contract.
    #[error("invalid Core Run request: {0}")]
    InvalidRequest(RunContractViolation),
    /// Capability policy or trusted approval lookup failed closed.
    #[error(transparent)]
    Authorization(#[from] GateError),
    /// The Runner Adapter returned an untrustworthy report.
    #[error("invalid Runner report: {0}")]
    InvalidRunnerReport(RunContractViolation),
}

/// Stable reason a request or report violated the Runner contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Error)]
pub enum RunContractViolation {
    /// The Run identity is empty, too long, or contains control characters.
    #[error("invalid run identity")]
    InvalidRunId,
    /// The deadline is zero or exceeds the v0 maximum.
    #[error("invalid execution deadline")]
    InvalidTimeout,
    /// The JSON payload exceeds its serialized byte limit.
    #[error("payload exceeds the byte limit")]
    PayloadTooLarge,
    /// The JSON payload exceeds the nesting limit.
    #[error("payload exceeds the nesting limit")]
    PayloadTooDeep,
    /// The JSON payload exceeds the node-count limit.
    #[error("payload exceeds the node-count limit")]
    PayloadTooComplex,
    /// The compact summary is empty, too long, or contains unsafe controls.
    #[error("invalid summary")]
    InvalidSummary,
    /// The report contains more evidence references than permitted.
    #[error("too many evidence references")]
    TooManyEvidenceReferences,
    /// An evidence kind or reference violates the bounded contract.
    #[error("invalid evidence reference")]
    InvalidEvidenceReference,
    /// A failed or cancelled report omitted its failure.
    #[error("terminal failure detail is required")]
    MissingFailure,
    /// A successful report unexpectedly included a failure.
    #[error("successful report cannot include failure detail")]
    UnexpectedFailure,
    /// Failure category and outcome are inconsistent.
    #[error("failure category does not match the outcome")]
    FailureOutcomeMismatch,
    /// Failure code or message violates the bounded contract.
    #[error("invalid failure detail")]
    InvalidFailure,
}

fn validate_request(request: &CoreRunRequest) -> Result<(), RunContractViolation> {
    if request.run_id.is_empty()
        || request.run_id.chars().count() > MAX_RUN_ID_CHARS
        || request.run_id.chars().any(char::is_control)
    {
        return Err(RunContractViolation::InvalidRunId);
    }
    if request.timeout_seconds == 0 || request.timeout_seconds > MAX_TIMEOUT_SECONDS {
        return Err(RunContractViolation::InvalidTimeout);
    }
    validate_json(&request.payload, MAX_REQUEST_BYTES)
}

fn validate_report(report: &RunnerReport) -> Result<(), RunContractViolation> {
    if !valid_safe_text(&report.summary, MAX_SUMMARY_CHARS) {
        return Err(RunContractViolation::InvalidSummary);
    }
    validate_json(&report.payload, MAX_RESULT_BYTES)?;

    if report.evidence.len() > MAX_EVIDENCE_REFERENCES {
        return Err(RunContractViolation::TooManyEvidenceReferences);
    }
    for evidence in &report.evidence {
        if !valid_token(&evidence.kind)
            || evidence.reference.is_empty()
            || evidence.reference.chars().count() > MAX_EVIDENCE_REFERENCE_CHARS
            || evidence.reference.chars().any(char::is_control)
        {
            return Err(RunContractViolation::InvalidEvidenceReference);
        }
    }

    match (report.outcome, &report.failure) {
        (RunnerOutcome::Succeeded, None) => Ok(()),
        (RunnerOutcome::Succeeded, Some(_)) => Err(RunContractViolation::UnexpectedFailure),
        (RunnerOutcome::Failed | RunnerOutcome::Cancelled, None) => {
            Err(RunContractViolation::MissingFailure)
        }
        (outcome, Some(failure)) => validate_failure(outcome, failure),
    }
}

fn validate_failure(
    outcome: RunnerOutcome,
    failure: &RunnerFailure,
) -> Result<(), RunContractViolation> {
    if !valid_token(&failure.code) || !valid_safe_text(&failure.message, MAX_FAILURE_MESSAGE_CHARS)
    {
        return Err(RunContractViolation::InvalidFailure);
    }
    if matches!(outcome, RunnerOutcome::Cancelled)
        != matches!(failure.category, RunnerFailureCategory::Cancelled)
    {
        return Err(RunContractViolation::FailureOutcomeMismatch);
    }
    Ok(())
}

fn validate_json(value: &Value, max_bytes: usize) -> Result<(), RunContractViolation> {
    let mut pending = vec![(value, 1_usize)];
    let mut nodes = 0_usize;
    while let Some((current, depth)) = pending.pop() {
        if depth > MAX_JSON_DEPTH {
            return Err(RunContractViolation::PayloadTooDeep);
        }
        nodes = nodes.saturating_add(1);
        if nodes > MAX_JSON_NODES {
            return Err(RunContractViolation::PayloadTooComplex);
        }

        match current {
            Value::Array(values) => {
                pending.extend(values.iter().map(|item| (item, depth + 1)));
            }
            Value::Object(values) => {
                pending.extend(values.values().map(|item| (item, depth + 1)));
            }
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
        }
    }

    let encoded = serde_json::to_vec(value).map_err(|_| RunContractViolation::PayloadTooComplex)?;
    if encoded.len() > max_bytes {
        return Err(RunContractViolation::PayloadTooLarge);
    }
    Ok(())
}

fn valid_token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_TOKEN_BYTES
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
}

fn valid_safe_text(value: &str, max_chars: usize) -> bool {
    !value.trim().is_empty()
        && value.chars().count() <= max_chars
        && value
            .chars()
            .all(|character| !character.is_control() || matches!(character, '\n' | '\r' | '\t'))
}

#[cfg(test)]
mod tests {
    use std::{
        cell::{Cell, RefCell},
        rc::Rc,
    };

    use crate::policy::ApprovalSourceError;

    use super::*;

    fn set(values: &[&str]) -> BTreeSet<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    struct StaticApprovals {
        approved: BTreeSet<String>,
        fail: bool,
    }

    impl ApprovalSource for StaticApprovals {
        fn approved_capabilities(
            &self,
            _operation_id: &str,
        ) -> Result<BTreeSet<String>, ApprovalSourceError> {
            if self.fail {
                Err(ApprovalSourceError::new("board_unavailable"))
            } else {
                Ok(self.approved.clone())
            }
        }
    }

    struct RecordingRunner {
        calls: Rc<Cell<usize>>,
        request: Rc<RefCell<Option<AuthorizedRunnerRequest>>>,
        report: RunnerReport,
    }

    type TestExecutor = RunExecutor<StaticApprovals, RecordingRunner>;
    type ExecutorFixture = (
        TestExecutor,
        Rc<Cell<usize>>,
        Rc<RefCell<Option<AuthorizedRunnerRequest>>>,
    );

    impl RunnerAdapter for RecordingRunner {
        fn execute(&mut self, request: &AuthorizedRunnerRequest) -> RunnerReport {
            self.calls.set(self.calls.get() + 1);
            self.request.replace(Some(request.clone()));
            self.report.clone()
        }
    }

    fn success_report() -> RunnerReport {
        RunnerReport {
            outcome: RunnerOutcome::Succeeded,
            summary: "implementation verified".to_owned(),
            payload: serde_json::json!({"revision": "abc123"}),
            evidence: vec![EvidenceReference {
                kind: "git_revision".to_owned(),
                reference: "abc123".to_owned(),
            }],
            failure: None,
        }
    }

    fn executor(
        allowed: &[&str],
        approval_required: &[&str],
        approved: &[&str],
        fail_approvals: bool,
        report: RunnerReport,
    ) -> ExecutorFixture {
        let calls = Rc::new(Cell::new(0));
        let captured = Rc::new(RefCell::new(None));
        let policy = CapabilityPolicy::new(set(allowed), set(approval_required)).unwrap();
        let executor = RunExecutor::new(
            policy,
            StaticApprovals {
                approved: set(approved),
                fail: fail_approvals,
            },
            RecordingRunner {
                calls: Rc::clone(&calls),
                request: Rc::clone(&captured),
                report,
            },
        );
        (executor, calls, captured)
    }

    fn request(capabilities: &[&str]) -> CoreRunRequest {
        CoreRunRequest {
            run_id: "run-1".to_owned(),
            timeout_seconds: 1_200,
            requested_capabilities: set(capabilities),
            payload: serde_json::json!({"task": "opaque"}),
        }
    }

    #[test]
    fn invokes_runner_once_with_only_sorted_authorized_capabilities() {
        let (mut executor, calls, captured) = executor(
            &["workspace.write", "context.read"],
            &[],
            &[],
            false,
            success_report(),
        );

        let result = executor
            .execute(request(&["workspace.write", "context.read"]))
            .unwrap();

        assert_eq!(calls.get(), 1);
        assert_eq!(
            captured.borrow().as_ref().unwrap(),
            &AuthorizedRunnerRequest {
                run_id: "run-1".to_owned(),
                timeout_seconds: 1_200,
                authorized_capabilities: vec![
                    "context.read".to_owned(),
                    "workspace.write".to_owned()
                ],
                payload: serde_json::json!({"task": "opaque"}),
            }
        );
        assert!(matches!(
            result,
            CoreRunResult::Finished {
                report: RunnerReport {
                    outcome: RunnerOutcome::Succeeded,
                    ..
                },
                ..
            }
        ));
    }

    #[test]
    fn policy_rejection_never_invokes_runner() {
        let (mut executor, calls, _) =
            executor(&["context.read"], &[], &[], false, success_report());

        let result = executor.execute(request(&["workspace.write"])).unwrap();

        assert_eq!(calls.get(), 0);
        assert!(matches!(result, CoreRunResult::Rejected { .. }));
    }

    #[test]
    fn unavailable_approval_source_fails_closed_before_runner() {
        let (mut executor, calls, _) = executor(
            &["workspace.write"],
            &["workspace.write"],
            &["workspace.write"],
            true,
            success_report(),
        );

        let error = executor.execute(request(&["workspace.write"])).unwrap_err();

        assert_eq!(calls.get(), 0);
        assert!(matches!(error, CoreRunError::Authorization(_)));
    }

    #[test]
    fn invalid_request_fails_before_approval_or_runner() {
        let (mut executor, calls, _) =
            executor(&["context.read"], &[], &[], false, success_report());
        let mut invalid = request(&["context.read"]);
        invalid.run_id = "bad\nrun".to_owned();

        let error = executor.execute(invalid).unwrap_err();

        assert_eq!(calls.get(), 0);
        assert!(matches!(
            error,
            CoreRunError::InvalidRequest(RunContractViolation::InvalidRunId)
        ));
    }

    #[test]
    fn invalid_runner_report_is_not_forwarded() {
        let mut report = success_report();
        report.summary.clear();
        let (mut executor, calls, _) = executor(&["context.read"], &[], &[], false, report);

        let error = executor.execute(request(&["context.read"])).unwrap_err();

        assert_eq!(calls.get(), 1);
        assert!(matches!(
            error,
            CoreRunError::InvalidRunnerReport(RunContractViolation::InvalidSummary)
        ));
    }

    #[test]
    fn accepts_bounded_failed_report() {
        let report = RunnerReport {
            outcome: RunnerOutcome::Failed,
            summary: "runtime timed out".to_owned(),
            payload: Value::Object(serde_json::Map::new()),
            evidence: Vec::new(),
            failure: Some(RunnerFailure {
                category: RunnerFailureCategory::Timeout,
                code: "run_timeout".to_owned(),
                message: "runtime exceeded its deadline".to_owned(),
            }),
        };
        let (mut executor, _, _) = executor(&["context.read"], &[], &[], false, report.clone());

        assert_eq!(
            executor.execute(request(&["context.read"])).unwrap(),
            CoreRunResult::Finished {
                run_id: "run-1".to_owned(),
                authorized_capabilities: vec!["context.read".to_owned()],
                report,
            }
        );
    }

    #[test]
    fn enforces_json_depth_and_result_size_limits() {
        let mut deep = Value::Null;
        for _ in 0..MAX_JSON_DEPTH {
            deep = Value::Array(vec![deep]);
        }
        let (mut deep_executor, calls, _) =
            executor(&["context.read"], &[], &[], false, success_report());
        let mut deep_request = request(&["context.read"]);
        deep_request.payload = deep;

        assert!(matches!(
            deep_executor.execute(deep_request),
            Err(CoreRunError::InvalidRequest(
                RunContractViolation::PayloadTooDeep
            ))
        ));
        assert_eq!(calls.get(), 0);

        let mut oversized_report = success_report();
        oversized_report.payload = Value::String("x".repeat(MAX_RESULT_BYTES));
        let (mut oversized_executor, calls, _) =
            executor(&["context.read"], &[], &[], false, oversized_report);

        assert!(matches!(
            oversized_executor.execute(request(&["context.read"])),
            Err(CoreRunError::InvalidRunnerReport(
                RunContractViolation::PayloadTooLarge
            ))
        ));
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn cancellation_requires_cancelled_failure_category() {
        let report = RunnerReport {
            outcome: RunnerOutcome::Cancelled,
            summary: "cancelled by operator".to_owned(),
            payload: Value::Null,
            evidence: Vec::new(),
            failure: Some(RunnerFailure {
                category: RunnerFailureCategory::Execution,
                code: "operator_cancelled".to_owned(),
                message: "operator cancelled the run".to_owned(),
            }),
        };

        let (mut executor, calls, _) = executor(&["context.read"], &[], &[], false, report);

        assert!(matches!(
            executor.execute(request(&["context.read"])),
            Err(CoreRunError::InvalidRunnerReport(
                RunContractViolation::FailureOutcomeMismatch
            ))
        ));
        assert_eq!(calls.get(), 1);
    }
}
