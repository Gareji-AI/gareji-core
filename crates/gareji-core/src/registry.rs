//! Durable project registrations and operational active-work references.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::time::Duration;

use gareji_contracts::{ActiveWorkItemResult, ProjectRegistration, ProjectView};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use thiserror::Error;

const MAX_PROJECTS: usize = 10_000;
const MAX_CONTEXT_ENTRIES: usize = 100;
const MAX_COLLECTION_ENTRIES: usize = 100;

/// Deep Module for registered execution workspaces and active-work references.
pub struct ProjectRegistry {
    connection: Connection,
}

impl ProjectRegistry {
    /// Validate one registration without opening or mutating registry storage.
    pub fn validate(registration: &ProjectRegistration) -> Result<(), RegistryError> {
        validate_registration(registration)
    }

    /// Open or create a registry at an application-data path.
    pub fn open_sqlite(path: impl AsRef<Path>) -> Result<Self, RegistryError> {
        let path = path.as_ref();
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)
                .map_err(|source| RegistryError::CreateDirectory { source })?;
        }
        let connection =
            Connection::open(path).map_err(|source| RegistryError::Storage { source })?;
        Self::from_connection(connection)
    }

    /// Open an in-memory registry for conformance tests and disposable demos.
    pub fn open_in_memory() -> Result<Self, RegistryError> {
        let connection =
            Connection::open_in_memory().map_err(|source| RegistryError::Storage { source })?;
        Self::from_connection(connection)
    }

    fn from_connection(connection: Connection) -> Result<Self, RegistryError> {
        connection
            .busy_timeout(Duration::from_secs(5))
            .map_err(|source| RegistryError::Storage { source })?;
        connection
            .execute_batch(
                "PRAGMA foreign_keys = ON;
                 PRAGMA journal_mode = WAL;
                 CREATE TABLE IF NOT EXISTS gareji_projects (
                   project_id TEXT PRIMARY KEY,
                   registration_json TEXT NOT NULL
                 );
                 CREATE TABLE IF NOT EXISTS gareji_active_work (
                   project_id TEXT PRIMARY KEY,
                   work_item_id TEXT NOT NULL,
                   FOREIGN KEY (project_id) REFERENCES gareji_projects(project_id) ON DELETE CASCADE
                 );",
            )
            .map_err(|source| RegistryError::Storage { source })?;
        Ok(Self { connection })
    }

    /// Create or replace one complete operational registration.
    pub fn register(&mut self, registration: &ProjectRegistration) -> Result<bool, RegistryError> {
        validate_registration(registration)?;
        let payload = serde_json::to_string(registration)
            .map_err(|source| RegistryError::Serialization { source })?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|source| RegistryError::Storage { source })?;
        let existed = transaction
            .query_row(
                "SELECT 1 FROM gareji_projects WHERE project_id = ?1",
                [&registration.project_id],
                |_| Ok(()),
            )
            .optional()
            .map_err(|source| RegistryError::Storage { source })?
            .is_some();
        transaction
            .execute(
                "INSERT INTO gareji_projects (project_id, registration_json)
                 VALUES (?1, ?2)
                 ON CONFLICT(project_id) DO UPDATE SET registration_json = excluded.registration_json",
                params![registration.project_id, payload],
            )
            .map_err(|source| RegistryError::Storage { source })?;
        transaction
            .commit()
            .map_err(|source| RegistryError::Storage { source })?;
        Ok(existed)
    }

    /// List complete registrations sorted by project identity.
    pub fn list(&self) -> Result<Vec<ProjectRegistration>, RegistryError> {
        let mut statement = self
            .connection
            .prepare("SELECT registration_json FROM gareji_projects ORDER BY project_id LIMIT ?1")
            .map_err(|source| RegistryError::Storage { source })?;
        let rows = statement
            .query_map([i64::try_from(MAX_PROJECTS).unwrap_or(i64::MAX)], |row| {
                row.get::<_, String>(0)
            })
            .map_err(|source| RegistryError::Storage { source })?;
        let mut projects = Vec::new();
        for row in rows {
            let payload = row.map_err(|source| RegistryError::Storage { source })?;
            projects.push(
                serde_json::from_str(&payload)
                    .map_err(|source| RegistryError::Serialization { source })?,
            );
        }
        Ok(projects)
    }

    /// Read one complete project registration.
    pub fn get(&self, project_id: &str) -> Result<ProjectRegistration, RegistryError> {
        validate_id("project_id", project_id)?;
        let payload = self
            .connection
            .query_row(
                "SELECT registration_json FROM gareji_projects WHERE project_id = ?1",
                [project_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|source| RegistryError::Storage { source })?
            .ok_or_else(|| RegistryError::ProjectNotFound {
                project_id: project_id.to_owned(),
            })?;
        serde_json::from_str(&payload).map_err(|source| RegistryError::Serialization { source })
    }

    /// Select an opaque Board-owned Work item reference for one project.
    pub fn set_active_work_item(
        &mut self,
        project_id: &str,
        work_item_id: &str,
    ) -> Result<ActiveWorkItemResult, RegistryError> {
        validate_id("project_id", project_id)?;
        validate_id("work_item_id", work_item_id)?;
        self.get(project_id)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|source| RegistryError::Storage { source })?;
        let previous_work_item_id = transaction
            .query_row(
                "SELECT work_item_id FROM gareji_active_work WHERE project_id = ?1",
                [project_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|source| RegistryError::Storage { source })?;
        transaction
            .execute(
                "INSERT INTO gareji_active_work (project_id, work_item_id)
                 VALUES (?1, ?2)
                 ON CONFLICT(project_id) DO UPDATE SET work_item_id = excluded.work_item_id",
                params![project_id, work_item_id],
            )
            .map_err(|source| RegistryError::Storage { source })?;
        transaction
            .commit()
            .map_err(|source| RegistryError::Storage { source })?;
        Ok(ActiveWorkItemResult {
            project_id: project_id.to_owned(),
            previous_work_item_id,
            current_work_item_id: work_item_id.to_owned(),
        })
    }

    /// Read the current operational Work item reference, when selected.
    pub fn active_work_item(&self, project_id: &str) -> Result<Option<String>, RegistryError> {
        validate_id("project_id", project_id)?;
        self.get(project_id)?;
        self.connection
            .query_row(
                "SELECT work_item_id FROM gareji_active_work WHERE project_id = ?1",
                [project_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|source| RegistryError::Storage { source })
    }
}

/// Convert one complete registration into its bounded transport view.
#[must_use]
pub fn project_view(registration: &ProjectRegistration) -> ProjectView {
    ProjectView {
        id: registration.project_id.clone(),
        name: registration.name.clone(),
        execution_workspace: registration.execution_workspace.clone(),
        context_sources: registration.context_sources.clone(),
        grants: registration
            .grants
            .iter()
            .map(|grant| grant.as_str().to_owned())
            .collect(),
    }
}

/// Registry failure with bounded public messages and inspectable sources.
#[derive(Debug, Error)]
pub enum RegistryError {
    /// A configured value violates stable bounds.
    #[error("invalid {field}: {message}")]
    Invalid {
        /// Field containing the invalid value.
        field: &'static str,
        /// Stable bounded explanation.
        message: &'static str,
    },
    /// The requested project is not registered.
    #[error("project `{project_id}` was not found")]
    ProjectNotFound {
        /// Stable requested project identity.
        project_id: String,
    },
    /// The application-data directory could not be created.
    #[error("registry storage directory could not be created")]
    CreateDirectory {
        /// Underlying filesystem failure.
        #[source]
        source: std::io::Error,
    },
    /// SQLite operation failed.
    #[error("registry storage operation failed")]
    Storage {
        /// Underlying SQLite failure.
        #[source]
        source: rusqlite::Error,
    },
    /// Stored registration JSON could not be encoded or decoded.
    #[error("registry payload serialization failed")]
    Serialization {
        /// Underlying JSON failure.
        #[source]
        source: serde_json::Error,
    },
}

fn validate_registration(registration: &ProjectRegistration) -> Result<(), RegistryError> {
    validate_id("project_id", &registration.project_id)?;
    validate_text("name", &registration.name, 1, 256)?;
    validate_text(
        "execution_workspace",
        &registration.execution_workspace,
        1,
        1_024,
    )?;
    validate_collection("context_sources", &registration.context_sources)?;
    validate_collection("delivery_targets", &registration.delivery_targets)?;
    if registration.grants.len() > MAX_COLLECTION_ENTRIES {
        return Err(invalid("grants", "contains too many items"));
    }
    if registration.grants.iter().collect::<BTreeSet<_>>().len() != registration.grants.len() {
        return Err(invalid("grants", "contains duplicate items"));
    }
    if registration.sourced_context.len() > MAX_CONTEXT_ENTRIES {
        return Err(invalid("sourced_context", "contains too many items"));
    }
    let mut context_ids = BTreeSet::new();
    for source in &registration.sourced_context {
        validate_id("sourced_context[].workspace_id", &source.workspace_id)?;
        validate_id("sourced_context[].source_id", &source.source_id)?;
        validate_text("sourced_context[].freshness", &source.freshness, 1, 128)?;
        validate_optional_text(
            "sourced_context[].content",
            source.content.as_deref(),
            64_000,
        )?;
        validate_optional_text(
            "sourced_context[].evidence_ref",
            source.evidence_ref.as_deref(),
            1_024,
        )?;
        if source.content.is_none() && source.evidence_ref.is_none() {
            return Err(invalid(
                "sourced_context[]",
                "requires content or evidence_ref",
            ));
        }
        if !context_ids.insert((&source.workspace_id, &source.source_id)) {
            return Err(invalid("sourced_context", "contains duplicate source IDs"));
        }
    }
    Ok(())
}

fn validate_collection(field: &'static str, values: &[String]) -> Result<(), RegistryError> {
    if values.len() > MAX_COLLECTION_ENTRIES {
        return Err(invalid(field, "contains too many items"));
    }
    let mut unique = BTreeSet::new();
    for value in values {
        validate_id(field, value)?;
        if !unique.insert(value) {
            return Err(invalid(field, "contains duplicate items"));
        }
    }
    Ok(())
}

fn validate_id(field: &'static str, value: &str) -> Result<(), RegistryError> {
    validate_text(field, value, 1, 128)
}

fn validate_optional_text(
    field: &'static str,
    value: Option<&str>,
    max: usize,
) -> Result<(), RegistryError> {
    if let Some(value) = value {
        validate_text(field, value, 1, max)?;
    }
    Ok(())
}

fn validate_text(
    field: &'static str,
    value: &str,
    min: usize,
    max: usize,
) -> Result<(), RegistryError> {
    let length = value.chars().count();
    if length < min {
        return Err(invalid(field, "is shorter than minimum length"));
    }
    if length > max {
        return Err(invalid(field, "exceeds maximum length"));
    }
    Ok(())
}

const fn invalid(field: &'static str, message: &'static str) -> RegistryError {
    RegistryError::Invalid { field, message }
}

#[cfg(test)]
mod tests {
    use gareji_contracts::{ProjectGrant, SourcedContext};
    use tempfile::tempdir;

    use super::*;

    fn registration() -> ProjectRegistration {
        ProjectRegistration {
            project_id: "gareji-core".to_owned(),
            name: "Gareji Core".to_owned(),
            execution_workspace: "workspace://gareji-core".to_owned(),
            context_sources: vec!["local-markdown".to_owned()],
            grants: vec![
                ProjectGrant::ReadContext,
                ProjectGrant::SelectActiveWork,
                ProjectGrant::WriteProgress,
            ],
            sourced_context: vec![SourcedContext {
                workspace_id: "notes".to_owned(),
                source_id: "project-brief".to_owned(),
                freshness: "configured".to_owned(),
                content: Some("Build the local trust kernel.".to_owned()),
                evidence_ref: None,
            }],
            delivery_targets: vec!["local-json".to_owned(), "knowledge-projection".to_owned()],
        }
    }

    #[test]
    fn registration_and_active_work_survive_reopen() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("gareji.sqlite");
        let mut registry = ProjectRegistry::open_sqlite(&path).unwrap();
        assert!(!registry.register(&registration()).unwrap());
        assert!(registry.register(&registration()).unwrap());
        let selection = registry
            .set_active_work_item("gareji-core", "CORE-1")
            .unwrap();
        assert_eq!(selection.previous_work_item_id, None);
        drop(registry);

        let reopened = ProjectRegistry::open_sqlite(path).unwrap();
        assert_eq!(reopened.list().unwrap(), vec![registration()]);
        assert_eq!(
            reopened.active_work_item("gareji-core").unwrap(),
            Some("CORE-1".to_owned())
        );
    }

    #[test]
    fn active_work_is_scoped_per_project_and_returns_previous_selection() {
        let mut registry = ProjectRegistry::open_in_memory().unwrap();
        registry.register(&registration()).unwrap();
        registry
            .set_active_work_item("gareji-core", "CORE-1")
            .unwrap();

        let changed = registry
            .set_active_work_item("gareji-core", "CORE-2")
            .unwrap();
        assert_eq!(changed.previous_work_item_id.as_deref(), Some("CORE-1"));
        assert_eq!(changed.current_work_item_id, "CORE-2");
    }

    #[test]
    fn registration_requires_attributed_context_content_or_reference() {
        let mut invalid_registration = registration();
        invalid_registration.sourced_context[0].content = None;
        let mut registry = ProjectRegistry::open_in_memory().unwrap();

        assert!(matches!(
            registry.register(&invalid_registration),
            Err(RegistryError::Invalid {
                field: "sourced_context[]",
                ..
            })
        ));
    }

    #[test]
    fn registration_accepts_reference_only_context_without_delivery_targets() {
        let mut reference_only = registration();
        reference_only.sourced_context[0].content = None;
        reference_only.sourced_context[0].evidence_ref =
            Some(r"C:\absolute\path\to\project\PROJECT.md".to_owned());
        reference_only.delivery_targets.clear();
        let mut registry = ProjectRegistry::open_in_memory().unwrap();

        assert!(!registry.register(&reference_only).unwrap());
        assert_eq!(registry.get("gareji-core").unwrap(), reference_only);
    }
}
