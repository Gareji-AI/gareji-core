//! Audit persistence seams: real filesystem + fault-injection fake.

use chrono::Local;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Persistence errors used by lifecycle.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum AuditError {
    /// Directory could not be created.
    #[error("sidecar directory create failed: {0}")]
    DirCreate(String),
    /// Temporary write failed.
    #[error("sidecar temporary write failed: {0}")]
    TempWrite(String),
    /// Serialized sidecar exceeded the hard cap.
    #[error("sidecar too large: {0} bytes")]
    TooLarge(usize),
    /// Sync failed.
    #[error("sidecar sync failed: {0}")]
    Sync(String),
    /// Rename/commit failed.
    #[error("sidecar rename failed: {0}")]
    Rename(String),
    /// Destination already exists (no-overwrite).
    #[error("sidecar destination already exists")]
    AlreadyExists,
    /// Path escaped the sidecar parent.
    #[error("sidecar path escape rejected")]
    PathEscape,
}

/// Narrow audit store seam.
pub trait AuditStore: Send + Sync {
    /// Persist bytes with write-once semantics under parent/day/run_id/full.json.
    fn persist(
        &self,
        parent: &Path,
        run_id: &str,
        bytes: &[u8],
        max_bytes: usize,
    ) -> Result<PersistResult, AuditError>;
}

/// Result metadata for sidecar commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistResult {
    /// Final path of the written sidecar.
    pub path: PathBuf,
    /// Sync level achieved.
    pub sync_level: String,
    /// Commit method used.
    pub commit_method: String,
}

/// Real local-filesystem audit store.
#[derive(Debug, Default)]
pub struct FsAuditStore;

impl AuditStore for FsAuditStore {
    fn persist(
        &self,
        parent: &Path,
        run_id: &str,
        bytes: &[u8],
        max_bytes: usize,
    ) -> Result<PersistResult, AuditError> {
        if bytes.len() > max_bytes {
            return Err(AuditError::TooLarge(bytes.len()));
        }
        std::fs::create_dir_all(parent).map_err(|e| AuditError::DirCreate(e.to_string()))?;
        let parent_canon = parent
            .canonicalize()
            .map_err(|e| AuditError::DirCreate(e.to_string()))?;
        if run_id.contains('/') || run_id.contains('\\') || run_id.contains("..") {
            return Err(AuditError::PathEscape);
        }
        let day = Local::now().format("%Y-%m-%d").to_string();
        let dest_dir = parent_canon.join(&day).join(run_id);
        std::fs::create_dir_all(&dest_dir).map_err(|e| AuditError::DirCreate(e.to_string()))?;
        let dest = dest_dir.join("full.json");
        let dest_canon_parent = dest_dir
            .canonicalize()
            .map_err(|e| AuditError::DirCreate(e.to_string()))?;
        if !dest_canon_parent.starts_with(&parent_canon) {
            return Err(AuditError::PathEscape);
        }
        if dest.exists() {
            return Err(AuditError::AlreadyExists);
        }
        let tmp = dest_dir.join(format!(".full.{run_id}.tmp"));
        {
            use std::io::Write;
            let mut file =
                std::fs::File::create(&tmp).map_err(|e| AuditError::TempWrite(e.to_string()))?;
            file.write_all(bytes)
                .map_err(|e| AuditError::TempWrite(e.to_string()))?;
            file.sync_all()
                .map_err(|e| AuditError::Sync(e.to_string()))?;
        }
        let sync_level = "file_sync".to_string();
        std::fs::rename(&tmp, &dest).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            AuditError::Rename(e.to_string())
        })?;
        set_owner_only(&dest);
        Ok(PersistResult {
            path: dest,
            sync_level,
            commit_method: "same_dir_rename_no_overwrite".into(),
        })
    }
}

fn set_owner_only(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(windows)]
    {
        let _ = path;
    }
}

/// In-memory / fault-injection audit store for tests.
#[derive(Debug, Default)]
pub struct InMemoryAuditStore {
    /// Optional injected fault.
    pub fault: Option<AuditError>,
    /// Captured last successful bytes.
    pub last_bytes: std::sync::Mutex<Option<Vec<u8>>>,
}

impl AuditStore for InMemoryAuditStore {
    fn persist(
        &self,
        parent: &Path,
        run_id: &str,
        bytes: &[u8],
        max_bytes: usize,
    ) -> Result<PersistResult, AuditError> {
        if let Some(fault) = &self.fault {
            return Err(fault.clone());
        }
        if bytes.len() > max_bytes {
            return Err(AuditError::TooLarge(bytes.len()));
        }
        *self.last_bytes.lock().expect("lock") = Some(bytes.to_vec());
        Ok(PersistResult {
            path: parent.join(run_id).join("full.json"),
            sync_level: "memory".into(),
            commit_method: "in_memory".into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn too_large_before_write() {
        let store = FsAuditStore;
        let dir = tempfile::tempdir().unwrap();
        let err = store
            .persist(dir.path(), "run1", &vec![0u8; 10], 5)
            .unwrap_err();
        assert!(matches!(err, AuditError::TooLarge(10)));
    }

    #[test]
    fn in_memory_fault_injection() {
        let store = InMemoryAuditStore {
            fault: Some(AuditError::Rename("injected".into())),
            last_bytes: std::sync::Mutex::new(None),
        };
        let err = store
            .persist(Path::new("/tmp"), "run", b"{}", 100)
            .unwrap_err();
        assert!(matches!(err, AuditError::Rename(_)));
    }
}
