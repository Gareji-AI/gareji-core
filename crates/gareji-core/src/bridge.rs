//! Trusted local bridge over the Registry and Progress Recorder Interfaces.

use std::path::Path;

use gareji_contracts::{
    CheckpointStatusResult, CoreBridgeError, CoreBridgeOperation, CoreBridgeRequest,
    CoreBridgeResponse, CoreErrorCode, DeliveryView, ListProgressResult, ListProjectsResult,
    ProjectContextResult, ProjectGrant, RecordProgressResult, CORE_BRIDGE_PROTOCOL_VERSION,
};
use serde::Serialize;
use serde_json::Value;

use crate::board::{BoardActiveWorkAssessment, BoardPort, BoardPortError, UnavailableBoard};
use crate::progress::{
    CheckpointQuery, CheckpointStatus, DeliveryStatus, DeliveryTarget, ProgressCheckpoint,
    ProgressError, ProgressRecorder,
};
use crate::registry::{project_view, ProjectRegistry, RegistryError};

/// Deep Module serving bounded local transport operations from one durable database.
pub struct CoreBridge {
    board: Box<dyn BoardPort>,
    registry: ProjectRegistry,
    progress: ProgressRecorder,
}

impl CoreBridge {
    /// Open the Core bridge over one application-data SQLite file.
    pub fn open_sqlite(path: impl AsRef<Path>) -> Result<Self, CoreBridgeOpenError> {
        Self::open_sqlite_with_board(path, Box::<UnavailableBoard>::default())
    }

    /// Open Core with an explicit Board Adapter at the active-work assessment seam.
    pub fn open_sqlite_with_board(
        path: impl AsRef<Path>,
        board: Box<dyn BoardPort>,
    ) -> Result<Self, CoreBridgeOpenError> {
        let path = path.as_ref();
        Ok(Self {
            board,
            registry: ProjectRegistry::open_sqlite(path)?,
            progress: ProgressRecorder::open_sqlite(path)?,
        })
    }

    /// Handle one validated wire request without exposing internal errors or state.
    #[must_use]
    pub fn handle(&mut self, request: CoreBridgeRequest) -> CoreBridgeResponse {
        let request_id = request.request_id;
        if request.protocol_version != CORE_BRIDGE_PROTOCOL_VERSION {
            return error_response(
                request_id,
                CoreErrorCode::InvalidRequest,
                "unsupported Core bridge protocol",
            );
        }
        if request_id.is_empty() || request_id.chars().count() > 128 {
            return error_response(
                request_id,
                CoreErrorCode::InvalidRequest,
                "invalid request identity",
            );
        }

        match self.execute(request.operation) {
            Ok(result) => CoreBridgeResponse::Ok {
                protocol_version: CORE_BRIDGE_PROTOCOL_VERSION.to_owned(),
                request_id,
                result,
            },
            Err(failure) => error_response(request_id, failure.code, failure.message),
        }
    }

    fn execute(&mut self, operation: CoreBridgeOperation) -> Result<Value, CoreFailure> {
        match operation {
            CoreBridgeOperation::ListProjects => {
                let projects = self
                    .registry
                    .list()
                    .map_err(CoreFailure::from_registry)?
                    .iter()
                    .map(project_view)
                    .collect();
                encode(&ListProjectsResult { projects })
            }
            CoreBridgeOperation::GetProjectContext {
                project_id,
                work_item_id,
            } => {
                validate_optional_id(work_item_id.as_deref())?;
                let project = self
                    .registry
                    .get(&project_id)
                    .map_err(CoreFailure::from_registry)?;
                require_grant(&project.grants, ProjectGrant::ReadContext)?;
                encode(&ProjectContextResult {
                    project_id,
                    work_item_id,
                    sources: project.sourced_context,
                })
            }
            CoreBridgeOperation::SetActiveWorkItem {
                project_id,
                work_item_id,
            } => {
                let project = self
                    .registry
                    .get(&project_id)
                    .map_err(CoreFailure::from_registry)?;
                require_grant(&project.grants, ProjectGrant::SelectActiveWork)?;
                match self
                    .board
                    .assess_active_work(&project_id, &work_item_id)
                    .map_err(CoreFailure::from_board)?
                {
                    BoardActiveWorkAssessment::Eligible => {}
                    BoardActiveWorkAssessment::Ineligible => {
                        return Err(CoreFailure {
                            code: CoreErrorCode::WorkItemNotEligible,
                            message: "Work item state is not eligible for active work",
                        });
                    }
                }
                let result = self
                    .registry
                    .set_active_work_item(&project_id, &work_item_id)
                    .map_err(CoreFailure::from_registry)?;
                encode(&result)
            }
            CoreBridgeOperation::RecordProgress { checkpoint } => {
                let checkpoint: ProgressCheckpoint = serde_json::from_value(checkpoint)
                    .map_err(|_| CoreFailure::invalid("invalid Progress Checkpoint"))?;
                let project = self
                    .registry
                    .get(&checkpoint.project_id)
                    .map_err(CoreFailure::from_registry)?;
                require_grant(&project.grants, ProjectGrant::WriteProgress)?;
                if project.execution_workspace != checkpoint.execution_workspace_id {
                    return Err(CoreFailure {
                        code: CoreErrorCode::WorkspaceNotConnected,
                        message: "checkpoint workspace is not connected to the project",
                    });
                }
                let targets: Vec<_> = project
                    .delivery_targets
                    .into_iter()
                    .map(|destination_id| DeliveryTarget { destination_id })
                    .collect();
                let receipt = self
                    .progress
                    .record(&checkpoint, &targets)
                    .map_err(CoreFailure::from_progress)?;
                encode(&RecordProgressResult {
                    checkpoint_id: receipt.checkpoint_id,
                    duplicate: receipt.duplicate,
                    deliveries: receipt
                        .deliveries
                        .into_iter()
                        .map(|delivery| DeliveryView {
                            destination_id: delivery.destination_id,
                            status: delivery_status(delivery.status).to_owned(),
                            attempts: delivery.attempts,
                            last_error: delivery.last_error,
                        })
                        .collect(),
                })
            }
            CoreBridgeOperation::GetCheckpointStatus { checkpoint_id } => {
                let status = self
                    .progress
                    .status(&checkpoint_id)
                    .map_err(CoreFailure::from_progress)?;
                let project = self
                    .registry
                    .get(&status.checkpoint.project_id)
                    .map_err(CoreFailure::from_registry)?;
                require_grant(&project.grants, ProjectGrant::WriteProgress)?;
                let result = checkpoint_status_result(status)?;
                debug_assert_eq!(result.checkpoint_id, checkpoint_id);
                encode(&result)
            }
            CoreBridgeOperation::ListProgress {
                project_id,
                work_item_id,
                before_checkpoint_id,
                limit,
            } => {
                let project = self
                    .registry
                    .get(&project_id)
                    .map_err(CoreFailure::from_registry)?;
                require_grant(&project.grants, ProjectGrant::WriteProgress)?;
                let page = self
                    .progress
                    .list(&CheckpointQuery {
                        project_id,
                        work_item_id,
                        before_checkpoint_id,
                        limit,
                    })
                    .map_err(CoreFailure::from_progress)?;
                let checkpoints = page
                    .checkpoints
                    .into_iter()
                    .map(checkpoint_status_result)
                    .collect::<Result<Vec<_>, _>>()?;
                encode(&ListProgressResult {
                    checkpoints,
                    next_cursor: page.next_cursor,
                })
            }
        }
    }
}

/// Failure opening the bridge's durable Modules.
#[derive(Debug, thiserror::Error)]
pub enum CoreBridgeOpenError {
    /// Project registry initialization failed.
    #[error(transparent)]
    Registry(#[from] RegistryError),
    /// Progress Recorder initialization failed.
    #[error(transparent)]
    Progress(#[from] ProgressError),
}

#[derive(Clone, Copy)]
struct CoreFailure {
    code: CoreErrorCode,
    message: &'static str,
}

impl CoreFailure {
    const fn invalid(message: &'static str) -> Self {
        Self {
            code: CoreErrorCode::InvalidRequest,
            message,
        }
    }

    const fn internal() -> Self {
        Self {
            code: CoreErrorCode::InternalError,
            message: "local Core operation failed",
        }
    }

    fn from_registry(error: RegistryError) -> Self {
        match error {
            RegistryError::Invalid { .. } => Self::invalid("invalid project operation"),
            RegistryError::ProjectNotFound { .. } => Self {
                code: CoreErrorCode::ProjectNotFound,
                message: "project was not found or is not visible",
            },
            RegistryError::CreateDirectory { .. }
            | RegistryError::Storage { .. }
            | RegistryError::Serialization { .. } => Self::internal(),
        }
    }

    const fn from_board(error: BoardPortError) -> Self {
        match error {
            BoardPortError::ProjectNotFound => Self {
                code: CoreErrorCode::ProjectNotFound,
                message: "project was not found or is not visible in Board",
            },
            BoardPortError::WorkItemNotFound => Self {
                code: CoreErrorCode::WorkItemNotFound,
                message: "Work item was not found in the requested project",
            },
            BoardPortError::Unavailable => Self {
                code: CoreErrorCode::BoardUnavailable,
                message: "Gareji Board is unavailable",
            },
            BoardPortError::InvalidResponse => Self::internal(),
        }
    }

    fn from_progress(error: ProgressError) -> Self {
        match error {
            ProgressError::InvalidCheckpoint(_) => Self::invalid("invalid Progress Checkpoint"),
            ProgressError::CheckpointConflict { .. }
            | ProgressError::DeliverySetMismatch { .. } => Self {
                code: CoreErrorCode::Conflict,
                message: "checkpoint identity conflicts with stored content or destinations",
            },
            ProgressError::CheckpointNotFound { .. } => Self {
                code: CoreErrorCode::CheckpointNotFound,
                message: "checkpoint was not found",
            },
            ProgressError::CreateDirectory { .. }
            | ProgressError::Storage { .. }
            | ProgressError::Serialization { .. }
            | ProgressError::CorruptState { .. } => Self::internal(),
        }
    }
}

fn require_grant(grants: &[ProjectGrant], required: ProjectGrant) -> Result<(), CoreFailure> {
    if grants.contains(&required) {
        Ok(())
    } else {
        Err(CoreFailure {
            code: CoreErrorCode::PermissionDenied,
            message: "project does not grant this operation",
        })
    }
}

fn checkpoint_status_result(
    status: CheckpointStatus,
) -> Result<CheckpointStatusResult, CoreFailure> {
    let checkpoint_id = status.checkpoint.checkpoint_id.clone();
    let checkpoint =
        serde_json::to_value(status.checkpoint).map_err(|_| CoreFailure::internal())?;
    Ok(CheckpointStatusResult {
        checkpoint_id,
        checkpoint,
        deliveries: status
            .deliveries
            .into_iter()
            .map(|delivery| DeliveryView {
                destination_id: delivery.destination_id,
                status: delivery_status(delivery.status).to_owned(),
                attempts: delivery.attempts,
                last_error: delivery.last_error,
            })
            .collect(),
    })
}

fn validate_optional_id(value: Option<&str>) -> Result<(), CoreFailure> {
    if value.is_some_and(|value| value.is_empty() || value.chars().count() > 128) {
        return Err(CoreFailure::invalid("invalid Work item identity"));
    }
    Ok(())
}

fn encode(value: &impl Serialize) -> Result<Value, CoreFailure> {
    serde_json::to_value(value).map_err(|_| CoreFailure::internal())
}

const fn delivery_status(status: DeliveryStatus) -> &'static str {
    match status {
        DeliveryStatus::Pending => "pending",
        DeliveryStatus::Synced => "synced",
        DeliveryStatus::Conflict => "conflict",
        DeliveryStatus::Failed => "failed",
    }
}

fn error_response(request_id: String, code: CoreErrorCode, message: &str) -> CoreBridgeResponse {
    CoreBridgeResponse::Error {
        protocol_version: CORE_BRIDGE_PROTOCOL_VERSION.to_owned(),
        request_id,
        error: CoreBridgeError {
            code,
            message: message.chars().take(256).collect(),
        },
    }
}

#[cfg(test)]
mod tests {
    use gareji_contracts::{
        CoreBridgeOperation, ProjectRegistration, SourcedContext, CORE_BRIDGE_PROTOCOL_VERSION,
    };
    use tempfile::tempdir;

    use super::*;
    use crate::progress::{
        ActorType, CheckpointActor, CheckpointOutcome, CheckpointSource, VerificationRecord,
        VerificationStatus, PROGRESS_CHECKPOINT_SCHEMA_VERSION,
    };

    fn request(id: &str, operation: CoreBridgeOperation) -> CoreBridgeRequest {
        CoreBridgeRequest {
            protocol_version: CORE_BRIDGE_PROTOCOL_VERSION.to_owned(),
            request_id: id.to_owned(),
            operation,
        }
    }

    fn registration() -> ProjectRegistration {
        ProjectRegistration {
            project_id: "core".to_owned(),
            name: "Core".to_owned(),
            execution_workspace: "core-local".to_owned(),
            context_sources: vec!["local-markdown".to_owned()],
            grants: vec![
                ProjectGrant::ReadContext,
                ProjectGrant::SelectActiveWork,
                ProjectGrant::WriteProgress,
            ],
            sourced_context: vec![SourcedContext {
                workspace_id: "notes".to_owned(),
                source_id: "brief".to_owned(),
                freshness: "configured".to_owned(),
                content: Some("Build Core.".to_owned()),
                evidence_ref: None,
            }],
            delivery_targets: vec!["local-json".to_owned()],
        }
    }

    fn checkpoint() -> ProgressCheckpoint {
        ProgressCheckpoint {
            schema_version: PROGRESS_CHECKPOINT_SCHEMA_VERSION.to_owned(),
            checkpoint_id: "cp-bridge-1".to_owned(),
            recorded_at: "2026-07-17T18:00:00+09:00".to_owned(),
            project_id: "core".to_owned(),
            work_item_id: Some("CORE-1".to_owned()),
            execution_workspace_id: "core-local".to_owned(),
            source: CheckpointSource::Mcp,
            actor: CheckpointActor {
                kind: ActorType::Agent,
                id: "codex".to_owned(),
            },
            outcome: CheckpointOutcome::Progress,
            summary: "Connected the local Core bridge.".to_owned(),
            changed_paths: vec!["crates/gareji-core/src/bridge.rs".to_owned()],
            git: None,
            verification: vec![VerificationRecord {
                name: "cargo test".to_owned(),
                status: VerificationStatus::Passed,
                evidence_ref: None,
            }],
            evidence_refs: vec![],
            recommended_state: None,
        }
    }

    struct StaticBoard {
        assessment: Result<BoardActiveWorkAssessment, BoardPortError>,
    }

    impl BoardPort for StaticBoard {
        fn assess_active_work(
            &self,
            _project_id: &str,
            _work_item_id: &str,
        ) -> Result<BoardActiveWorkAssessment, BoardPortError> {
            self.assessment
        }
    }

    fn bridge_with_project() -> (tempfile::TempDir, CoreBridge) {
        let directory = tempdir().unwrap();
        let path = directory.path().join("gareji.sqlite");
        let mut registry = ProjectRegistry::open_sqlite(&path).unwrap();
        registry.register(&registration()).unwrap();
        drop(registry);
        let bridge = CoreBridge::open_sqlite_with_board(
            path,
            Box::new(StaticBoard {
                assessment: Ok(BoardActiveWorkAssessment::Eligible),
            }),
        )
        .unwrap();
        (directory, bridge)
    }

    #[test]
    fn bridge_lists_registered_projects_and_persists_active_work() {
        let (_directory, mut bridge) = bridge_with_project();
        let listed = bridge.handle(request("req-1", CoreBridgeOperation::ListProjects));
        let CoreBridgeResponse::Ok { result, .. } = listed else {
            panic!("expected successful list");
        };
        let listed: ListProjectsResult = serde_json::from_value(result).unwrap();
        assert_eq!(listed.projects[0].id, "core");

        let selected = bridge.handle(request(
            "req-2",
            CoreBridgeOperation::SetActiveWorkItem {
                project_id: "core".to_owned(),
                work_item_id: "CORE-1".to_owned(),
            },
        ));
        assert!(matches!(selected, CoreBridgeResponse::Ok { .. }));
    }

    #[test]
    fn bridge_records_and_reads_progress_through_the_same_interface() {
        let (_directory, mut bridge) = bridge_with_project();
        let recorded = bridge.handle(request(
            "req-3",
            CoreBridgeOperation::RecordProgress {
                checkpoint: serde_json::to_value(checkpoint()).unwrap(),
            },
        ));
        let CoreBridgeResponse::Ok { result, .. } = recorded else {
            panic!("expected successful record");
        };
        let receipt: RecordProgressResult = serde_json::from_value(result).unwrap();
        assert_eq!(receipt.deliveries[0].status, "pending");

        let status = bridge.handle(request(
            "req-4",
            CoreBridgeOperation::GetCheckpointStatus {
                checkpoint_id: "cp-bridge-1".to_owned(),
            },
        ));
        assert!(matches!(status, CoreBridgeResponse::Ok { .. }));

        let listed = bridge.handle(request(
            "req-5",
            CoreBridgeOperation::ListProgress {
                project_id: "core".to_owned(),
                work_item_id: Some("CORE-1".to_owned()),
                before_checkpoint_id: None,
                limit: 10,
            },
        ));
        let CoreBridgeResponse::Ok { result, .. } = listed else {
            panic!("expected successful progress list");
        };
        let listed: ListProgressResult = serde_json::from_value(result).unwrap();
        assert_eq!(listed.checkpoints.len(), 1);
        assert_eq!(listed.checkpoints[0].checkpoint_id, "cp-bridge-1");
        assert!(listed.next_cursor.is_none());
    }

    #[test]
    fn bridge_enforces_project_grants() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("gareji.sqlite");
        let mut denied = registration();
        denied.grants.clear();
        let mut registry = ProjectRegistry::open_sqlite(&path).unwrap();
        registry.register(&denied).unwrap();
        drop(registry);
        let mut bridge = CoreBridge::open_sqlite(path).unwrap();

        let response = bridge.handle(request(
            "req-denied",
            CoreBridgeOperation::GetProjectContext {
                project_id: "core".to_owned(),
                work_item_id: None,
            },
        ));
        assert!(matches!(
            response,
            CoreBridgeResponse::Error {
                error: CoreBridgeError {
                    code: CoreErrorCode::PermissionDenied,
                    ..
                },
                ..
            }
        ));
    }

    #[test]
    fn bridge_does_not_store_board_ineligible_active_work() {
        let (directory, mut bridge) = bridge_with_project();
        let selected = bridge.handle(request(
            "req-eligible",
            CoreBridgeOperation::SetActiveWorkItem {
                project_id: "core".to_owned(),
                work_item_id: "CORE-1".to_owned(),
            },
        ));
        assert!(matches!(selected, CoreBridgeResponse::Ok { .. }));
        drop(bridge);

        let path = directory.path().join("gareji.sqlite");
        let mut denied_bridge = CoreBridge::open_sqlite_with_board(
            &path,
            Box::new(StaticBoard {
                assessment: Ok(BoardActiveWorkAssessment::Ineligible),
            }),
        )
        .unwrap();
        let denied = denied_bridge.handle(request(
            "req-ineligible",
            CoreBridgeOperation::SetActiveWorkItem {
                project_id: "core".to_owned(),
                work_item_id: "CORE-2".to_owned(),
            },
        ));
        assert!(matches!(
            denied,
            CoreBridgeResponse::Error {
                error: CoreBridgeError {
                    code: CoreErrorCode::WorkItemNotEligible,
                    ..
                },
                ..
            }
        ));
        drop(denied_bridge);

        let registry = ProjectRegistry::open_sqlite(path).unwrap();
        assert_eq!(
            registry.active_work_item("core").unwrap().as_deref(),
            Some("CORE-1")
        );
    }

    #[test]
    fn bridge_reports_board_unavailable_without_falling_back() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("gareji.sqlite");
        let mut registry = ProjectRegistry::open_sqlite(&path).unwrap();
        registry.register(&registration()).unwrap();
        drop(registry);
        let mut bridge = CoreBridge::open_sqlite(path).unwrap();

        let response = bridge.handle(request(
            "req-board-unavailable",
            CoreBridgeOperation::SetActiveWorkItem {
                project_id: "core".to_owned(),
                work_item_id: "CORE-1".to_owned(),
            },
        ));
        assert!(matches!(
            response,
            CoreBridgeResponse::Error {
                error: CoreBridgeError {
                    code: CoreErrorCode::BoardUnavailable,
                    ..
                },
                ..
            }
        ));
    }
}
