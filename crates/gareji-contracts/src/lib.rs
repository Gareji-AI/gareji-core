//! Dependency-light wire contracts shared across Gareji Core transports.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Version discriminator for the local Core bridge.
pub const CORE_BRIDGE_PROTOCOL_VERSION: &str = "gareji.core-bridge.v0";

/// One request sent to the local Core bridge.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CoreBridgeRequest {
    /// Bridge protocol discriminator.
    pub protocol_version: String,
    /// Caller-assigned correlation identity.
    pub request_id: String,
    /// One bounded operation.
    #[serde(flatten)]
    pub operation: CoreBridgeOperation,
}

/// Operations available to trusted local transports.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "operation", content = "payload", rename_all = "snake_case")]
pub enum CoreBridgeOperation {
    /// List registered projects visible through Core.
    ListProjects,
    /// Read configured sourced context for one project.
    GetProjectContext {
        /// Stable project identity.
        project_id: String,
        /// Optional explicit Board Work item reference.
        work_item_id: Option<String>,
    },
    /// Select an operational Work item reference.
    SetActiveWorkItem {
        /// Stable project identity.
        project_id: String,
        /// Opaque Board-owned Work item identity.
        work_item_id: String,
    },
    /// Submit one immutable Progress Checkpoint.
    RecordProgress {
        /// A `gareji.progress-checkpoint.v0` object.
        checkpoint: Value,
    },
    /// Inspect one stored Progress Checkpoint.
    GetCheckpointStatus {
        /// Stable checkpoint identity.
        checkpoint_id: String,
    },
}

/// One response returned by the local Core bridge.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CoreBridgeResponse {
    /// The operation completed successfully.
    Ok {
        /// Bridge protocol discriminator.
        protocol_version: String,
        /// Correlation identity copied from the request.
        request_id: String,
        /// Operation-specific bounded result.
        result: Value,
    },
    /// The operation failed with a stable public category.
    Error {
        /// Bridge protocol discriminator.
        protocol_version: String,
        /// Correlation identity copied from the request when readable.
        request_id: String,
        /// Bounded failure description.
        error: CoreBridgeError,
    },
}

/// Bounded error returned across the Core bridge.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CoreBridgeError {
    /// Stable machine-readable category.
    pub code: CoreErrorCode,
    /// Safe, bounded human-readable summary.
    pub message: String,
}

/// Stable Core bridge failure categories.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CoreErrorCode {
    /// The request or payload violates the contract.
    InvalidRequest,
    /// The project does not grant the requested operation.
    PermissionDenied,
    /// The project is not registered.
    ProjectNotFound,
    /// The Work item is unknown to an attached Board Adapter.
    WorkItemNotFound,
    /// Board owns the item but its state cannot be selected as active work.
    WorkItemNotEligible,
    /// The configured local Board Adapter is unavailable.
    BoardUnavailable,
    /// The execution workspace is not connected.
    WorkspaceNotConnected,
    /// The checkpoint does not exist.
    CheckpointNotFound,
    /// An immutable identity was reused with different content.
    Conflict,
    /// A local implementation failure occurred.
    InternalError,
}

/// User-granted operation for one registered project.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectGrant {
    /// Read configured sourced context.
    ReadContext,
    /// Select an opaque active Work item reference.
    SelectActiveWork,
    /// Record and inspect Progress Checkpoints.
    WriteProgress,
}

impl ProjectGrant {
    /// Stable string used by MCP and configuration views.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ReadContext => "read_context",
            Self::SelectActiveWork => "select_active_work",
            Self::WriteProgress => "write_progress",
        }
    }
}

/// One attributed context entry configured for a project.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SourcedContext {
    /// Knowledge workspace identity.
    pub workspace_id: String,
    /// Source identity inside the workspace.
    pub source_id: String,
    /// Bounded freshness description.
    pub freshness: String,
    /// Optional bounded inline content.
    pub content: Option<String>,
    /// Optional reference for content kept outside Core.
    pub evidence_ref: Option<String>,
}

/// Complete operational registration stored by Core.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectRegistration {
    /// Stable Board-owned project identity.
    pub project_id: String,
    /// Human-readable project name.
    pub name: String,
    /// Stable execution workspace identity or local reference.
    pub execution_workspace: String,
    /// Configured source identifiers displayed to clients.
    pub context_sources: Vec<String>,
    /// Explicit user grants.
    pub grants: Vec<ProjectGrant>,
    /// Context entries returned until provider Adapters are connected.
    pub sourced_context: Vec<SourcedContext>,
    /// Projection destination identities captured at first record.
    pub delivery_targets: Vec<String>,
}

/// Project summary exposed to trusted local transports.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProjectView {
    /// Stable project identity.
    pub id: String,
    /// Human-readable project name.
    pub name: String,
    /// Execution workspace identity or local reference.
    pub execution_workspace: String,
    /// Configured context source identities.
    pub context_sources: Vec<String>,
    /// Explicit grants as stable strings.
    pub grants: Vec<String>,
}

/// Result of listing registered projects.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ListProjectsResult {
    /// Registered projects sorted by identity.
    pub projects: Vec<ProjectView>,
}

/// Result of reading configured project context.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProjectContextResult {
    /// Stable project identity.
    pub project_id: String,
    /// Explicit Work item reference, when supplied.
    pub work_item_id: Option<String>,
    /// Attributed entries from every configured source.
    pub sources: Vec<SourcedContext>,
}

/// Result of selecting active work.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ActiveWorkItemResult {
    /// Stable project identity.
    pub project_id: String,
    /// Previously selected Work item, when present.
    pub previous_work_item_id: Option<String>,
    /// Newly selected Work item.
    pub current_work_item_id: String,
}

/// Current projection delivery summary.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DeliveryView {
    /// Stable destination identity.
    pub destination_id: String,
    /// Current delivery state.
    pub status: String,
    /// Projection attempt count.
    pub attempts: u32,
}

/// Result of durable Progress Checkpoint intake.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RecordProgressResult {
    /// Stable checkpoint identity.
    pub checkpoint_id: String,
    /// True when the same immutable record already existed.
    pub duplicate: bool,
    /// Independent destination delivery states.
    pub deliveries: Vec<DeliveryView>,
}

/// Result of inspecting one Progress Checkpoint.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct CheckpointStatusResult {
    /// Stable checkpoint identity.
    pub checkpoint_id: String,
    /// Immutable checkpoint payload.
    pub checkpoint: Value,
    /// Independent destination delivery states.
    pub deliveries: Vec<DeliveryView>,
}
