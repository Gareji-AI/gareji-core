//! Durable, idempotent Progress Checkpoint recording and projection delivery.

mod types;

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::time::Duration;

use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use sha2::{Digest, Sha256};
use thiserror::Error;

pub use types::{
    ActorType, CheckpointActor, CheckpointOutcome, CheckpointPage, CheckpointQuery,
    CheckpointSource, CheckpointStatus, CheckpointValidationError, DeliveryReceipt, DeliveryStatus,
    DeliveryTarget, GitState, ProgressCheckpoint, ProjectionOutcome, RecordReceipt, SyncSummary,
    VerificationRecord, VerificationStatus, WorkItemState, PROGRESS_CHECKPOINT_SCHEMA_VERSION,
};

const MAX_DELIVERY_MESSAGE: usize = 1_000;

/// Knowledge projection seam used by `sync_pending`.
pub trait CheckpointProjector {
    /// Project one immutable checkpoint to one configured destination.
    ///
    /// Returned messages must already be safe to persist and display. The Recorder
    /// bounds their length and removes unsafe control characters, but does not know
    /// how to redact destination-specific secrets.
    fn project(
        &mut self,
        target: &DeliveryTarget,
        checkpoint: &ProgressCheckpoint,
    ) -> ProjectionOutcome;
}

/// Deep Module for local durability, idempotency, and independent deliveries.
pub struct ProgressRecorder {
    connection: Connection,
}

impl ProgressRecorder {
    /// Open or create a SQLite recorder at an application-data path.
    pub fn open_sqlite(path: impl AsRef<Path>) -> Result<Self, ProgressError> {
        let path = path.as_ref();
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)
                .map_err(|source| ProgressError::CreateDirectory { source })?;
        }
        let connection =
            Connection::open(path).map_err(|source| ProgressError::Storage { source })?;
        Self::from_connection(connection)
    }

    /// Open an in-memory SQLite recorder for conformance tests and disposable demos.
    pub fn open_in_memory() -> Result<Self, ProgressError> {
        let connection =
            Connection::open_in_memory().map_err(|source| ProgressError::Storage { source })?;
        Self::from_connection(connection)
    }

    fn from_connection(connection: Connection) -> Result<Self, ProgressError> {
        connection
            .busy_timeout(Duration::from_secs(5))
            .map_err(|source| ProgressError::Storage { source })?;
        connection
            .execute_batch(
                "PRAGMA foreign_keys = ON;
                 PRAGMA journal_mode = WAL;
                 CREATE TABLE IF NOT EXISTS gareji_checkpoints (
                   checkpoint_id TEXT PRIMARY KEY,
                   fingerprint TEXT NOT NULL,
                   payload_json TEXT NOT NULL,
                   recorded_at TEXT NOT NULL,
                   project_id TEXT,
                   work_item_id TEXT
                 );
                 CREATE TABLE IF NOT EXISTS gareji_checkpoint_deliveries (
                   checkpoint_id TEXT NOT NULL,
                   destination_id TEXT NOT NULL,
                   status TEXT NOT NULL CHECK (status IN ('pending', 'synced', 'conflict', 'failed')),
                   attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
                   last_error TEXT,
                   PRIMARY KEY (checkpoint_id, destination_id),
                   FOREIGN KEY (checkpoint_id) REFERENCES gareji_checkpoints(checkpoint_id)
                 );
                 CREATE INDEX IF NOT EXISTS gareji_delivery_retry
                   ON gareji_checkpoint_deliveries(status, checkpoint_id, destination_id);",
            )
            .map_err(|source| ProgressError::Storage { source })?;
        ensure_checkpoint_scope_columns(&connection)?;
        connection
            .execute_batch(
                "UPDATE gareji_checkpoints
                   SET project_id = json_extract(payload_json, '$.project_id'),
                       work_item_id = json_extract(payload_json, '$.work_item_id')
                 WHERE project_id IS NULL;
                 CREATE INDEX IF NOT EXISTS gareji_checkpoints_by_scope
                   ON gareji_checkpoints(project_id, work_item_id);
                 PRAGMA user_version = 2;",
            )
            .map_err(|source| ProgressError::Storage { source })?;
        Ok(Self { connection })
    }

    /// Durably record one checkpoint and initial per-destination deliveries.
    pub fn record(
        &mut self,
        checkpoint: &ProgressCheckpoint,
        targets: &[DeliveryTarget],
    ) -> Result<RecordReceipt, ProgressError> {
        checkpoint.validate()?;
        validate_targets(targets)?;

        let payload_json = serde_json::to_string(checkpoint)
            .map_err(|source| ProgressError::Serialization { source })?;
        let fingerprint = hex_sha256(payload_json.as_bytes());
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|source| ProgressError::Storage { source })?;

        let existing_fingerprint: Option<String> = transaction
            .query_row(
                "SELECT fingerprint FROM gareji_checkpoints WHERE checkpoint_id = ?1",
                [&checkpoint.checkpoint_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|source| ProgressError::Storage { source })?;

        let duplicate = if let Some(existing_fingerprint) = existing_fingerprint {
            if existing_fingerprint != fingerprint {
                return Err(ProgressError::CheckpointConflict {
                    checkpoint_id: checkpoint.checkpoint_id.clone(),
                });
            }
            let existing_targets = {
                let mut statement = transaction
                    .prepare(
                        "SELECT destination_id FROM gareji_checkpoint_deliveries
                         WHERE checkpoint_id = ?1 ORDER BY destination_id",
                    )
                    .map_err(|source| ProgressError::Storage { source })?;
                let rows = statement
                    .query_map([&checkpoint.checkpoint_id], |row| row.get::<_, String>(0))
                    .map_err(|source| ProgressError::Storage { source })?;
                let mut existing_targets = BTreeSet::new();
                for row in rows {
                    existing_targets
                        .insert(row.map_err(|source| ProgressError::Storage { source })?);
                }
                existing_targets
            };
            let requested_targets: BTreeSet<String> = targets
                .iter()
                .map(|target| target.destination_id.clone())
                .collect();
            if existing_targets != requested_targets {
                return Err(ProgressError::DeliverySetMismatch {
                    checkpoint_id: checkpoint.checkpoint_id.clone(),
                });
            }
            true
        } else {
            transaction
                .execute(
                    "INSERT INTO gareji_checkpoints
                       (checkpoint_id, fingerprint, payload_json, recorded_at, project_id, work_item_id)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        checkpoint.checkpoint_id,
                        fingerprint,
                        payload_json,
                        checkpoint.recorded_at,
                        checkpoint.project_id,
                        checkpoint.work_item_id
                    ],
                )
                .map_err(|source| ProgressError::Storage { source })?;
            for target in targets {
                transaction
                    .execute(
                        "INSERT INTO gareji_checkpoint_deliveries
                           (checkpoint_id, destination_id, status, attempts, last_error)
                         VALUES (?1, ?2, 'pending', 0, NULL)",
                        params![checkpoint.checkpoint_id, target.destination_id],
                    )
                    .map_err(|source| ProgressError::Storage { source })?;
            }
            false
        };

        transaction
            .commit()
            .map_err(|source| ProgressError::Storage { source })?;
        let status = self.status(&checkpoint.checkpoint_id)?;
        Ok(RecordReceipt {
            checkpoint_id: checkpoint.checkpoint_id.clone(),
            duplicate,
            deliveries: status.deliveries,
        })
    }

    /// Read one immutable checkpoint and its current independent deliveries.
    pub fn status(&self, checkpoint_id: &str) -> Result<CheckpointStatus, ProgressError> {
        validate_lookup_id(checkpoint_id)?;
        let payload_json: Option<String> = self
            .connection
            .query_row(
                "SELECT payload_json FROM gareji_checkpoints WHERE checkpoint_id = ?1",
                [checkpoint_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|source| ProgressError::Storage { source })?;
        let payload_json = payload_json.ok_or_else(|| ProgressError::CheckpointNotFound {
            checkpoint_id: checkpoint_id.to_owned(),
        })?;
        let checkpoint = serde_json::from_str(&payload_json)
            .map_err(|source| ProgressError::Serialization { source })?;
        let deliveries = self.delivery_receipts(checkpoint_id)?;
        Ok(CheckpointStatus {
            checkpoint,
            deliveries,
        })
    }

    /// Read a bounded newest-first page for one project and optional Work item.
    pub fn list(&self, query: &CheckpointQuery) -> Result<CheckpointPage, ProgressError> {
        validate_bounded_id("project_id", &query.project_id)?;
        if let Some(work_item_id) = &query.work_item_id {
            validate_bounded_id("work_item_id", work_item_id)?;
        }
        if let Some(checkpoint_id) = &query.before_checkpoint_id {
            validate_bounded_id("before_checkpoint_id", checkpoint_id)?;
        }
        if !(1..=100).contains(&query.limit) {
            return Err(CheckpointValidationError {
                field: "limit",
                message: "must be from 1 through 100",
            }
            .into());
        }

        let fetch_limit = i64::from(query.limit) + 1;
        let mut statement = self
            .connection
            .prepare(
                "WITH page AS (
                   SELECT rowid AS ledger_rowid, checkpoint_id, payload_json
                   FROM gareji_checkpoints
                   WHERE project_id = ?1
                     AND (?2 IS NULL OR work_item_id = ?2)
                     AND (
                       ?3 IS NULL OR rowid < (
                         SELECT rowid FROM gareji_checkpoints WHERE checkpoint_id = ?3
                       )
                     )
                   ORDER BY rowid DESC
                   LIMIT ?4
                 )
                 SELECT
                   p.ledger_rowid,
                   p.checkpoint_id,
                   p.payload_json,
                   d.destination_id,
                   d.status,
                   d.attempts,
                   d.last_error
                 FROM page p
                 LEFT JOIN gareji_checkpoint_deliveries d
                   ON d.checkpoint_id = p.checkpoint_id
                 ORDER BY p.ledger_rowid DESC, d.destination_id",
            )
            .map_err(|source| ProgressError::Storage { source })?;
        let rows = statement
            .query_map(
                params![
                    query.project_id,
                    query.work_item_id,
                    query.before_checkpoint_id,
                    fetch_limit
                ],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, Option<i64>>(5)?,
                        row.get::<_, Option<String>>(6)?,
                    ))
                },
            )
            .map_err(|source| ProgressError::Storage { source })?;

        let mut checkpoints: Vec<CheckpointStatus> = Vec::new();
        let mut current_rowid = None;
        for row in rows {
            let (
                ledger_rowid,
                checkpoint_id,
                payload_json,
                destination_id,
                raw_status,
                attempts,
                last_error,
            ) = row.map_err(|source| ProgressError::Storage { source })?;
            if current_rowid != Some(ledger_rowid) {
                let checkpoint: ProgressCheckpoint = serde_json::from_str(&payload_json)
                    .map_err(|source| ProgressError::Serialization { source })?;
                if checkpoint.checkpoint_id != checkpoint_id {
                    return Err(ProgressError::CorruptState {
                        message: "checkpoint identity does not match stored payload",
                    });
                }
                checkpoints.push(CheckpointStatus {
                    checkpoint,
                    deliveries: Vec::new(),
                });
                current_rowid = Some(ledger_rowid);
            }
            if let (Some(destination_id), Some(raw_status), Some(attempts)) =
                (destination_id, raw_status, attempts)
            {
                let status = DeliveryStatus::from_db(&raw_status).ok_or_else(|| {
                    ProgressError::CorruptState {
                        message: "unknown delivery status",
                    }
                })?;
                let attempts =
                    u32::try_from(attempts).map_err(|_| ProgressError::CorruptState {
                        message: "delivery attempts out of range",
                    })?;
                checkpoints
                    .last_mut()
                    .ok_or(ProgressError::CorruptState {
                        message: "delivery exists without a checkpoint",
                    })?
                    .deliveries
                    .push(DeliveryReceipt {
                        destination_id,
                        status,
                        attempts,
                        last_error,
                    });
            }
        }

        let has_more = checkpoints.len() > usize::from(query.limit);
        checkpoints.truncate(usize::from(query.limit));
        let next_cursor = has_more
            .then(|| {
                checkpoints
                    .last()
                    .map(|status| status.checkpoint.checkpoint_id.clone())
            })
            .flatten();
        Ok(CheckpointPage {
            checkpoints,
            next_cursor,
        })
    }

    /// Retry every pending or failed delivery once through the supplied Adapter router.
    pub fn sync_pending(
        &mut self,
        projector: &mut impl CheckpointProjector,
    ) -> Result<SyncSummary, ProgressError> {
        let pending = self.pending_deliveries()?;
        let mut summary = SyncSummary::default();
        for delivery in pending {
            summary.attempted = summary.attempted.saturating_add(1);
            let outcome = projector.project(&delivery.target, &delivery.checkpoint);
            let (status, last_error) = match outcome {
                ProjectionOutcome::Synced => {
                    summary.synced = summary.synced.saturating_add(1);
                    (DeliveryStatus::Synced, None)
                }
                ProjectionOutcome::Conflict { message } => {
                    summary.conflict = summary.conflict.saturating_add(1);
                    (
                        DeliveryStatus::Conflict,
                        Some(sanitize_message(&message, MAX_DELIVERY_MESSAGE)),
                    )
                }
                ProjectionOutcome::Failed { message } => {
                    summary.failed = summary.failed.saturating_add(1);
                    (
                        DeliveryStatus::Failed,
                        Some(sanitize_message(&message, MAX_DELIVERY_MESSAGE)),
                    )
                }
            };
            self.connection
                .execute(
                    "UPDATE gareji_checkpoint_deliveries
                     SET status = ?3, attempts = attempts + 1, last_error = ?4
                     WHERE checkpoint_id = ?1 AND destination_id = ?2",
                    params![
                        delivery.checkpoint.checkpoint_id,
                        delivery.target.destination_id,
                        status.as_db(),
                        last_error
                    ],
                )
                .map_err(|source| ProgressError::Storage { source })?;
        }
        Ok(summary)
    }

    fn delivery_receipts(
        &self,
        checkpoint_id: &str,
    ) -> Result<Vec<DeliveryReceipt>, ProgressError> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT destination_id, status, attempts, last_error
                 FROM gareji_checkpoint_deliveries
                 WHERE checkpoint_id = ?1
                 ORDER BY destination_id",
            )
            .map_err(|source| ProgressError::Storage { source })?;
        let rows = statement
            .query_map([checkpoint_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            })
            .map_err(|source| ProgressError::Storage { source })?;

        let mut receipts = Vec::new();
        for row in rows {
            let (destination_id, raw_status, attempts, last_error) =
                row.map_err(|source| ProgressError::Storage { source })?;
            let status = DeliveryStatus::from_db(&raw_status).ok_or_else(|| {
                ProgressError::CorruptState {
                    message: "unknown delivery status",
                }
            })?;
            let attempts = u32::try_from(attempts).map_err(|_| ProgressError::CorruptState {
                message: "delivery attempts out of range",
            })?;
            receipts.push(DeliveryReceipt {
                destination_id,
                status,
                attempts,
                last_error,
            });
        }
        Ok(receipts)
    }

    fn pending_deliveries(&self) -> Result<Vec<PendingDelivery>, ProgressError> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT c.payload_json, d.destination_id
                 FROM gareji_checkpoint_deliveries d
                 JOIN gareji_checkpoints c ON c.checkpoint_id = d.checkpoint_id
                 WHERE d.status IN ('pending', 'failed')
                 ORDER BY c.rowid, d.destination_id",
            )
            .map_err(|source| ProgressError::Storage { source })?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|source| ProgressError::Storage { source })?;

        let mut deliveries = Vec::new();
        for row in rows {
            let (payload_json, destination_id) =
                row.map_err(|source| ProgressError::Storage { source })?;
            let checkpoint = serde_json::from_str(&payload_json)
                .map_err(|source| ProgressError::Serialization { source })?;
            deliveries.push(PendingDelivery {
                checkpoint,
                target: DeliveryTarget { destination_id },
            });
        }
        Ok(deliveries)
    }
}

struct PendingDelivery {
    checkpoint: ProgressCheckpoint,
    target: DeliveryTarget,
}

/// Recorder failure with bounded public messages and inspectable sources.
#[derive(Debug, Error)]
pub enum ProgressError {
    /// Checkpoint or destination validation failed.
    #[error(transparent)]
    InvalidCheckpoint(#[from] CheckpointValidationError),
    /// The same checkpoint identity was presented with different content.
    #[error("checkpoint `{checkpoint_id}` conflicts with stored content")]
    CheckpointConflict { checkpoint_id: String },
    /// An idempotent retry changed the destinations captured by the first record.
    #[error("checkpoint `{checkpoint_id}` was retried with a different delivery set")]
    DeliverySetMismatch { checkpoint_id: String },
    /// The requested checkpoint does not exist.
    #[error("checkpoint `{checkpoint_id}` was not found")]
    CheckpointNotFound { checkpoint_id: String },
    /// The application-data directory could not be created.
    #[error("progress storage directory could not be created")]
    CreateDirectory {
        #[source]
        source: std::io::Error,
    },
    /// SQLite operation failed.
    #[error("progress storage operation failed")]
    Storage {
        #[source]
        source: rusqlite::Error,
    },
    /// Stored JSON could not be encoded or decoded.
    #[error("progress payload serialization failed")]
    Serialization {
        #[source]
        source: serde_json::Error,
    },
    /// Durable state violated an internal invariant.
    #[error("progress storage is corrupt: {message}")]
    CorruptState { message: &'static str },
}

fn validate_targets(targets: &[DeliveryTarget]) -> Result<(), CheckpointValidationError> {
    let mut unique = BTreeSet::new();
    for target in targets {
        target.validate()?;
        if !unique.insert(&target.destination_id) {
            return Err(CheckpointValidationError {
                field: "destinations",
                message: "contains duplicate destination IDs",
            });
        }
    }
    Ok(())
}

fn validate_lookup_id(checkpoint_id: &str) -> Result<(), CheckpointValidationError> {
    validate_bounded_id("checkpoint_id", checkpoint_id)
}

fn validate_bounded_id(field: &'static str, value: &str) -> Result<(), CheckpointValidationError> {
    if value.is_empty() {
        return Err(CheckpointValidationError {
            field,
            message: "is shorter than minimum length",
        });
    }
    if value.chars().count() > 128 {
        return Err(CheckpointValidationError {
            field,
            message: "exceeds maximum length",
        });
    }
    Ok(())
}

fn ensure_checkpoint_scope_columns(connection: &Connection) -> Result<(), ProgressError> {
    for column in ["project_id", "work_item_id"] {
        let exists: bool = connection
            .query_row(
                "SELECT EXISTS(
                   SELECT 1 FROM pragma_table_info('gareji_checkpoints') WHERE name = ?1
                 )",
                [column],
                |row| row.get(0),
            )
            .map_err(|source| ProgressError::Storage { source })?;
        if !exists {
            connection
                .execute(
                    &format!("ALTER TABLE gareji_checkpoints ADD COLUMN {column} TEXT"),
                    [],
                )
                .map_err(|source| ProgressError::Storage { source })?;
        }
    }
    Ok(())
}

fn hex_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn sanitize_message(message: &str, max: usize) -> String {
    message
        .chars()
        .filter(|character| !character.is_control() || matches!(character, '\n' | '\t'))
        .take(max)
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use tempfile::tempdir;

    use super::*;

    fn checkpoint(id: &str) -> ProgressCheckpoint {
        ProgressCheckpoint {
            schema_version: PROGRESS_CHECKPOINT_SCHEMA_VERSION.to_owned(),
            checkpoint_id: id.to_owned(),
            recorded_at: "2026-07-17T12:00:00+09:00".to_owned(),
            project_id: "core".to_owned(),
            work_item_id: Some("CORE-1".to_owned()),
            execution_workspace_id: "core-local".to_owned(),
            source: CheckpointSource::Runner,
            actor: CheckpointActor {
                kind: ActorType::Agent,
                id: "implementer".to_owned(),
            },
            outcome: CheckpointOutcome::Progress,
            summary: "Implemented durable progress recording.".to_owned(),
            changed_paths: vec!["crates/gareji-core/src/progress/mod.rs".to_owned()],
            git: Some(GitState {
                head: None,
                branch: Some("main".to_owned()),
                dirty: true,
            }),
            verification: vec![VerificationRecord {
                name: "cargo test".to_owned(),
                status: VerificationStatus::Passed,
                evidence_ref: Some("run://test/core".to_owned()),
            }],
            evidence_refs: vec!["file://crates/gareji-core".to_owned()],
            recommended_state: Some(WorkItemState::InReview),
        }
    }

    fn targets() -> Vec<DeliveryTarget> {
        vec![
            DeliveryTarget {
                destination_id: "json".to_owned(),
            },
            DeliveryTarget {
                destination_id: "zettelkasten".to_owned(),
            },
        ]
    }

    #[test]
    fn record_is_durable_and_creates_independent_pending_deliveries() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("gareji.sqlite");
        let mut recorder = ProgressRecorder::open_sqlite(&path).unwrap();

        let receipt = recorder.record(&checkpoint("cp-1"), &targets()).unwrap();
        assert!(!receipt.duplicate);
        assert_eq!(receipt.deliveries.len(), 2);
        assert!(receipt
            .deliveries
            .iter()
            .all(|delivery| delivery.status == DeliveryStatus::Pending));
        drop(recorder);

        let reopened = ProgressRecorder::open_sqlite(path).unwrap();
        assert_eq!(reopened.status("cp-1").unwrap().deliveries.len(), 2);
    }

    #[test]
    fn same_id_and_payload_is_idempotent() {
        let mut recorder = ProgressRecorder::open_in_memory().unwrap();
        let checkpoint = checkpoint("cp-2");
        recorder.record(&checkpoint, &targets()).unwrap();

        let duplicate = recorder.record(&checkpoint, &targets()).unwrap();
        assert!(duplicate.duplicate);
        assert_eq!(duplicate.deliveries.len(), 2);
    }

    #[test]
    fn idempotent_retry_must_preserve_the_initial_delivery_set() {
        let mut recorder = ProgressRecorder::open_in_memory().unwrap();
        let checkpoint = checkpoint("cp-2b");
        recorder.record(&checkpoint, &targets()).unwrap();

        assert!(matches!(
            recorder.record(
                &checkpoint,
                &[DeliveryTarget {
                    destination_id: "json".to_owned()
                }]
            ),
            Err(ProgressError::DeliverySetMismatch { .. })
        ));
    }

    #[test]
    fn same_id_with_different_payload_is_a_conflict() {
        let mut recorder = ProgressRecorder::open_in_memory().unwrap();
        recorder.record(&checkpoint("cp-3"), &targets()).unwrap();
        let mut changed = checkpoint("cp-3");
        changed.summary = "Different immutable content.".to_owned();

        assert!(matches!(
            recorder.record(&changed, &targets()),
            Err(ProgressError::CheckpointConflict { .. })
        ));
    }

    #[test]
    fn list_is_scoped_newest_first_and_cursor_paginated() {
        let mut recorder = ProgressRecorder::open_in_memory().unwrap();
        for id in ["cp-page-1", "cp-page-2", "cp-page-3"] {
            recorder.record(&checkpoint(id), &targets()).unwrap();
        }
        let mut other_work_item = checkpoint("cp-other-work");
        other_work_item.work_item_id = Some("CORE-2".to_owned());
        recorder.record(&other_work_item, &targets()).unwrap();
        let mut other_project = checkpoint("cp-other-project");
        other_project.project_id = "board".to_owned();
        recorder.record(&other_project, &targets()).unwrap();

        let first = recorder
            .list(&CheckpointQuery {
                project_id: "core".to_owned(),
                work_item_id: Some("CORE-1".to_owned()),
                before_checkpoint_id: None,
                limit: 2,
            })
            .unwrap();
        assert_eq!(
            first
                .checkpoints
                .iter()
                .map(|status| status.checkpoint.checkpoint_id.as_str())
                .collect::<Vec<_>>(),
            ["cp-page-3", "cp-page-2"]
        );
        assert_eq!(first.next_cursor.as_deref(), Some("cp-page-2"));
        assert!(first
            .checkpoints
            .iter()
            .all(|status| status.deliveries.len() == 2));

        let second = recorder
            .list(&CheckpointQuery {
                project_id: "core".to_owned(),
                work_item_id: Some("CORE-1".to_owned()),
                before_checkpoint_id: first.next_cursor,
                limit: 2,
            })
            .unwrap();
        assert_eq!(second.checkpoints.len(), 1);
        assert_eq!(second.checkpoints[0].checkpoint.checkpoint_id, "cp-page-1");
        assert!(second.next_cursor.is_none());
    }

    #[test]
    fn opening_a_v1_database_backfills_checkpoint_scope() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("gareji-v1.sqlite");
        let legacy_checkpoint = checkpoint("cp-legacy");
        let payload_json = serde_json::to_string(&legacy_checkpoint).unwrap();
        let fingerprint = hex_sha256(payload_json.as_bytes());
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE gareji_checkpoints (
                   checkpoint_id TEXT PRIMARY KEY,
                   fingerprint TEXT NOT NULL,
                   payload_json TEXT NOT NULL,
                   recorded_at TEXT NOT NULL
                 );
                 CREATE TABLE gareji_checkpoint_deliveries (
                   checkpoint_id TEXT NOT NULL,
                   destination_id TEXT NOT NULL,
                   status TEXT NOT NULL,
                   attempts INTEGER NOT NULL,
                   last_error TEXT,
                   PRIMARY KEY (checkpoint_id, destination_id)
                 );
                 PRAGMA user_version = 1;",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO gareji_checkpoints
                   (checkpoint_id, fingerprint, payload_json, recorded_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    legacy_checkpoint.checkpoint_id,
                    fingerprint,
                    payload_json,
                    legacy_checkpoint.recorded_at
                ],
            )
            .unwrap();
        drop(connection);

        let recorder = ProgressRecorder::open_sqlite(path).unwrap();
        let page = recorder
            .list(&CheckpointQuery {
                project_id: "core".to_owned(),
                work_item_id: None,
                before_checkpoint_id: None,
                limit: 10,
            })
            .unwrap();
        assert_eq!(page.checkpoints.len(), 1);
        assert_eq!(page.checkpoints[0].checkpoint.checkpoint_id, "cp-legacy");
    }

    #[test]
    fn rejects_absolute_or_parent_traversal_changed_paths() {
        let mut recorder = ProgressRecorder::open_in_memory().unwrap();
        let mut absolute = checkpoint("cp-4");
        absolute.changed_paths = vec!["C:\\Users\\demo\\secret.txt".to_owned()];
        assert!(recorder.record(&absolute, &[]).is_err());

        let mut traversal = checkpoint("cp-5");
        traversal.changed_paths = vec!["../secret.txt".to_owned()];
        assert!(recorder.record(&traversal, &[]).is_err());
    }

    struct QueueProjector {
        outcomes: VecDeque<ProjectionOutcome>,
    }

    impl CheckpointProjector for QueueProjector {
        fn project(
            &mut self,
            _target: &DeliveryTarget,
            _checkpoint: &ProgressCheckpoint,
        ) -> ProjectionOutcome {
            self.outcomes.pop_front().unwrap()
        }
    }

    #[test]
    fn sync_preserves_partial_success_and_retries_failed_delivery() {
        let mut recorder = ProgressRecorder::open_in_memory().unwrap();
        recorder.record(&checkpoint("cp-6"), &targets()).unwrap();
        let mut first_pass = QueueProjector {
            outcomes: VecDeque::from([
                ProjectionOutcome::Synced,
                ProjectionOutcome::Failed {
                    message: "workspace unavailable".to_owned(),
                },
            ]),
        };

        assert_eq!(
            recorder.sync_pending(&mut first_pass).unwrap(),
            SyncSummary {
                attempted: 2,
                synced: 1,
                conflict: 0,
                failed: 1,
            }
        );
        let partial = recorder.status("cp-6").unwrap();
        assert_eq!(partial.deliveries[0].status, DeliveryStatus::Synced);
        assert_eq!(partial.deliveries[1].status, DeliveryStatus::Failed);

        let mut retry = QueueProjector {
            outcomes: VecDeque::from([ProjectionOutcome::Synced]),
        };
        assert_eq!(recorder.sync_pending(&mut retry).unwrap().synced, 1);
        let complete = recorder.status("cp-6").unwrap();
        assert!(complete
            .deliveries
            .iter()
            .all(|delivery| delivery.status == DeliveryStatus::Synced));
        assert_eq!(complete.deliveries[1].attempts, 2);
    }

    #[test]
    fn conflict_delivery_is_not_retried() {
        let mut recorder = ProgressRecorder::open_in_memory().unwrap();
        recorder
            .record(
                &checkpoint("cp-7"),
                &[DeliveryTarget {
                    destination_id: "json".to_owned(),
                }],
            )
            .unwrap();
        let mut projector = QueueProjector {
            outcomes: VecDeque::from([ProjectionOutcome::Conflict {
                message: "different payload".to_owned(),
            }]),
        };
        assert_eq!(recorder.sync_pending(&mut projector).unwrap().conflict, 1);

        let mut should_not_run = QueueProjector {
            outcomes: VecDeque::new(),
        };
        assert_eq!(
            recorder
                .sync_pending(&mut should_not_run)
                .unwrap()
                .attempted,
            0
        );
    }

    #[test]
    fn public_checkpoint_example_matches_the_rust_contract() {
        let checkpoint: ProgressCheckpoint = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../examples/progress-checkpoint-v0.json"
        )))
        .unwrap();
        checkpoint.validate().unwrap();
    }
}
