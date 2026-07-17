//! Absolute native plugin path validation and process spawn helpers.

use std::path::{Path, PathBuf};
use thiserror::Error;
use tokio::process::Command;

/// Errors rejecting a plugin path before spawn.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum PluginPathError {
    /// Path was not absolute.
    #[error("plugin path must be absolute")]
    NotAbsolute,
    /// Path does not exist.
    #[error("plugin path does not exist")]
    NotFound,
    /// Path exists but is not a file.
    #[error("plugin path is not a file")]
    NotFile,
}

/// Validated absolute native plugin executable path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginPath {
    path: PathBuf,
}

impl PluginPath {
    /// Validate an absolute native executable path (existence + file).
    pub fn resolve(path: impl AsRef<Path>) -> Result<Self, PluginPathError> {
        let path = path.as_ref();
        if !path.is_absolute() {
            return Err(PluginPathError::NotAbsolute);
        }
        let meta = std::fs::metadata(path).map_err(|_| PluginPathError::NotFound)?;
        if !meta.is_file() {
            return Err(PluginPathError::NotFile);
        }
        Ok(Self {
            path: path.to_path_buf(),
        })
    }

    /// Borrow the absolute path.
    pub fn as_path(&self) -> &Path {
        &self.path
    }
}

/// Spawn failures after path validation.
#[derive(Debug, Error)]
pub enum SpawnError {
    /// OS process creation failed.
    #[error("plugin spawn failed: {0}")]
    Os(#[from] std::io::Error),
}

/// Build a Command for a validated plugin with cleared environment and allowlist restores.
pub fn build_plugin_command(
    plugin: &PluginPath,
    allowlisted_env_keys: &[String],
) -> Result<Command, SpawnError> {
    let mut cmd = Command::new(plugin.as_path());
    cmd.env_clear();
    // Minimal platform baseline needed to launch a child.
    #[cfg(windows)]
    {
        for key in ["SystemRoot", "windir", "SystemDrive", "COMSPEC", "PATHEXT"] {
            if let Ok(val) = std::env::var(key) {
                cmd.env(key, val);
            }
        }
        // PATH is intentionally not restored unless explicitly allowlisted.
    }
    #[cfg(unix)]
    {
        for key in ["PATH", "HOME", "LANG", "LC_ALL", "TZ"] {
            if let Ok(val) = std::env::var(key) {
                cmd.env(key, val);
            }
        }
    }
    for key in allowlisted_env_keys {
        if let Ok(val) = std::env::var(key) {
            cmd.env(key, val);
        }
    }
    cmd.stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    Ok(cmd)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn rejects_relative_paths() {
        let err = PluginPath::resolve("relative/plugin").unwrap_err();
        assert_eq!(err, PluginPathError::NotAbsolute);
    }

    #[test]
    fn accepts_absolute_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("plugin.bin");
        let mut f = std::fs::File::create(&file).unwrap();
        writeln!(f, "x").unwrap();
        let abs = file.canonicalize().unwrap();
        let path = PluginPath::resolve(&abs).unwrap();
        assert_eq!(path.as_path(), abs.as_path());
    }
}
