//! Stable Progress Checkpoint and delivery types.

use std::collections::HashSet;

use chrono::DateTime;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The only Progress Checkpoint schema accepted by this implementation.
pub const PROGRESS_CHECKPOINT_SCHEMA_VERSION: &str = "gareji.progress-checkpoint.v0";

/// One immutable, evidence-linked progress event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProgressCheckpoint {
    /// Schema discriminator.
    pub schema_version: String,
    /// Caller-assigned idempotency identity.
    pub checkpoint_id: String,
    /// RFC 3339 event timestamp.
    pub recorded_at: String,
    /// Stable Board project identity.
    pub project_id: String,
    /// Optional explicit Work item identity.
    pub work_item_id: Option<String>,
    /// Execution workspace in which the work occurred.
    pub execution_workspace_id: String,
    /// Capture source.
    pub source: CheckpointSource,
    /// Human, agent, or system actor.
    pub actor: CheckpointActor,
    /// Progress outcome, separate from Work item state.
    pub outcome: CheckpointOutcome,
    /// Compact human-readable description.
    pub summary: String,
    /// Relative paths materially changed by the work.
    pub changed_paths: Vec<String>,
    /// Optional bounded Git evidence.
    pub git: Option<GitState>,
    /// Verification performed during the work.
    pub verification: Vec<VerificationRecord>,
    /// References to evidence stored outside the checkpoint.
    pub evidence_refs: Vec<String>,
    /// State recommendation for Board to evaluate.
    pub recommended_state: Option<WorkItemState>,
}

impl ProgressCheckpoint {
    /// Validate the stable v0 bounds before durable storage.
    pub fn validate(&self) -> Result<(), CheckpointValidationError> {
        if self.schema_version != PROGRESS_CHECKPOINT_SCHEMA_VERSION {
            return Err(invalid("schema_version", "unsupported schema version"));
        }
        validate_required_id("checkpoint_id", &self.checkpoint_id)?;
        validate_required_id("project_id", &self.project_id)?;
        validate_optional_id("work_item_id", self.work_item_id.as_deref())?;
        validate_required_id("execution_workspace_id", &self.execution_workspace_id)?;
        validate_required_id("actor.id", &self.actor.id)?;

        if DateTime::parse_from_rfc3339(&self.recorded_at).is_err() {
            return Err(invalid("recorded_at", "must be RFC 3339 date-time"));
        }
        validate_text("summary", &self.summary, 1, 2_000)?;
        validate_collection_len("changed_paths", self.changed_paths.len(), 200)?;
        validate_unique("changed_paths", &self.changed_paths)?;
        for path in &self.changed_paths {
            validate_text("changed_paths[]", path, 1, 512)?;
            if !is_safe_relative_path(path) {
                return Err(invalid(
                    "changed_paths[]",
                    "must be a relative path without parent traversal",
                ));
            }
        }

        if let Some(git) = &self.git {
            validate_optional_text("git.head", git.head.as_deref(), 128)?;
            validate_optional_text("git.branch", git.branch.as_deref(), 256)?;
        }

        validate_collection_len("verification", self.verification.len(), 100)?;
        for verification in &self.verification {
            validate_text("verification[].name", &verification.name, 1, 256)?;
            validate_optional_text(
                "verification[].evidence_ref",
                verification.evidence_ref.as_deref(),
                1_024,
            )?;
        }

        validate_collection_len("evidence_refs", self.evidence_refs.len(), 100)?;
        validate_unique("evidence_refs", &self.evidence_refs)?;
        for evidence_ref in &self.evidence_refs {
            validate_text("evidence_refs[]", evidence_ref, 1, 1_024)?;
        }
        Ok(())
    }
}

/// Where a checkpoint originated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckpointSource {
    /// Gareji Runner completed a bounded Run or reconciliation.
    Runner,
    /// An enabled MCP client recorded progress.
    Mcp,
    /// A person used the manual CLI.
    ManualCli,
    /// A trusted Codex Stop Hook captured turn-end progress.
    CodexStopHook,
    /// An optional Git post-commit Hook captured a milestone.
    GitPostCommit,
}

/// Actor attached to a checkpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckpointActor {
    /// Actor class.
    #[serde(rename = "type")]
    pub kind: ActorType,
    /// Stable local actor identity.
    pub id: String,
}

/// Supported actor classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActorType {
    /// A person performed or recorded the work.
    Human,
    /// An agent performed the work.
    Agent,
    /// A system reconciliation produced the checkpoint.
    System,
}

/// Outcome of work represented by a checkpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckpointOutcome {
    /// Meaningful progress occurred.
    Progress,
    /// The bounded work appears complete.
    Completed,
    /// A human or another agent should review the result.
    NeedsReview,
    /// Work cannot proceed without a blocker being resolved.
    Blocked,
    /// The execution failed.
    Failed,
    /// The bounded inspection found no action to perform.
    NoAction,
}

/// Bounded Git evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitState {
    /// Current revision, when available.
    pub head: Option<String>,
    /// Current branch, when available.
    pub branch: Option<String>,
    /// Whether tracked or untracked changes remain.
    pub dirty: bool,
}

/// One verification result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerificationRecord {
    /// Human-readable check name.
    pub name: String,
    /// Bounded status.
    pub status: VerificationStatus,
    /// Optional external evidence reference.
    pub evidence_ref: Option<String>,
}

/// Verification status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationStatus {
    /// The check passed.
    Passed,
    /// The check ran and failed.
    Failed,
    /// The check was intentionally not run.
    NotRun,
    /// The recorder cannot determine the status.
    Unknown,
}

/// Board state that a checkpoint may recommend but cannot apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkItemState {
    /// Not yet selected for near-term work.
    Backlog,
    /// Eligible for selection.
    Todo,
    /// Work is active.
    InProgress,
    /// Awaiting review or reconciliation.
    InReview,
    /// Waiting on an explicit blocker.
    Blocked,
    /// Accepted as complete.
    Done,
    /// Intentionally stopped without completion.
    Cancelled,
}

/// A configured projection destination.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveryTarget {
    /// Stable destination identity resolved by Board configuration.
    pub destination_id: String,
}

impl DeliveryTarget {
    pub(crate) fn validate(&self) -> Result<(), CheckpointValidationError> {
        validate_required_id("destination_id", &self.destination_id)
    }
}

/// Projection state for one checkpoint and destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryStatus {
    /// Durable locally and waiting for projection.
    Pending,
    /// Destination acknowledged the same checkpoint.
    Synced,
    /// The destination has the same ID with different content.
    Conflict,
    /// The latest projection attempt failed and may be retried.
    Failed,
}

impl DeliveryStatus {
    pub(crate) const fn as_db(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Synced => "synced",
            Self::Conflict => "conflict",
            Self::Failed => "failed",
        }
    }

    pub(crate) fn from_db(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "synced" => Some(Self::Synced),
            "conflict" => Some(Self::Conflict),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

/// Current delivery information for one destination.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryReceipt {
    /// Stable destination identity.
    pub destination_id: String,
    /// Current delivery state.
    pub status: DeliveryStatus,
    /// Number of projection attempts.
    pub attempts: u32,
    /// Bounded latest error or conflict summary.
    pub last_error: Option<String>,
}

/// Durable response from `record`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordReceipt {
    /// Stable checkpoint identity.
    pub checkpoint_id: String,
    /// True when the same immutable payload was already present.
    pub duplicate: bool,
    /// Independent delivery states, sorted by destination identity.
    pub deliveries: Vec<DeliveryReceipt>,
}

/// Checkpoint plus current per-destination delivery information.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointStatus {
    /// Immutable stored checkpoint.
    pub checkpoint: ProgressCheckpoint,
    /// Independent delivery states, sorted by destination identity.
    pub deliveries: Vec<DeliveryReceipt>,
}

/// Bounded newest-first query over the immutable checkpoint ledger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckpointQuery {
    /// Stable Board project identity.
    pub project_id: String,
    /// Optional explicit Work item filter.
    pub work_item_id: Option<String>,
    /// Return records older than this checkpoint identity.
    pub before_checkpoint_id: Option<String>,
    /// Maximum number of checkpoints to return, from 1 through 100.
    pub limit: u16,
}

/// One bounded page from the immutable checkpoint ledger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckpointPage {
    /// Checkpoints in newest-first intake order.
    pub checkpoints: Vec<CheckpointStatus>,
    /// Cursor for the next older page when more records exist.
    pub next_cursor: Option<String>,
}

/// Result returned by a Knowledge projection Adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectionOutcome {
    /// The destination contains the same checkpoint.
    Synced,
    /// The destination contains conflicting content for the checkpoint ID.
    Conflict { message: String },
    /// Delivery failed and may be retried.
    Failed { message: String },
}

/// Counts returned from one explicit delivery pass.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncSummary {
    /// Pending or failed deliveries inspected.
    pub attempted: u32,
    /// Deliveries that became synced.
    pub synced: u32,
    /// Deliveries that became conflicts.
    pub conflict: u32,
    /// Deliveries whose latest attempt failed.
    pub failed: u32,
}

/// Invalid checkpoint or destination input.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("invalid {field}: {message}")]
pub struct CheckpointValidationError {
    /// Field or collection containing the invalid value.
    pub field: &'static str,
    /// Stable bounded explanation.
    pub message: &'static str,
}

fn invalid(field: &'static str, message: &'static str) -> CheckpointValidationError {
    CheckpointValidationError { field, message }
}

fn validate_required_id(field: &'static str, value: &str) -> Result<(), CheckpointValidationError> {
    validate_text(field, value, 1, 128)
}

fn validate_optional_id(
    field: &'static str,
    value: Option<&str>,
) -> Result<(), CheckpointValidationError> {
    validate_optional_text(field, value, 128)
}

fn validate_optional_text(
    field: &'static str,
    value: Option<&str>,
    max: usize,
) -> Result<(), CheckpointValidationError> {
    if let Some(value) = value {
        if value.chars().count() > max {
            return Err(invalid(field, "exceeds maximum length"));
        }
    }
    Ok(())
}

fn validate_text(
    field: &'static str,
    value: &str,
    min: usize,
    max: usize,
) -> Result<(), CheckpointValidationError> {
    let length = value.chars().count();
    if length < min {
        return Err(invalid(field, "is shorter than minimum length"));
    }
    if length > max {
        return Err(invalid(field, "exceeds maximum length"));
    }
    Ok(())
}

fn validate_collection_len(
    field: &'static str,
    length: usize,
    max: usize,
) -> Result<(), CheckpointValidationError> {
    if length > max {
        return Err(invalid(field, "contains too many items"));
    }
    Ok(())
}

fn validate_unique(
    field: &'static str,
    values: &[String],
) -> Result<(), CheckpointValidationError> {
    let unique: HashSet<&str> = values.iter().map(String::as_str).collect();
    if unique.len() != values.len() {
        return Err(invalid(field, "contains duplicate items"));
    }
    Ok(())
}

fn is_safe_relative_path(value: &str) -> bool {
    let normalized = value.replace('\\', "/");
    if normalized.starts_with('/')
        || normalized.starts_with("//")
        || normalized
            .as_bytes()
            .get(1)
            .is_some_and(|second| *second == b':')
    {
        return false;
    }
    normalized.split('/').all(|part| part != "..")
}
