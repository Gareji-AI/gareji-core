//! Atomic toolchain cache install with per-tag/target lock.

use crate::pin::SUPPORTED_TARGETS;
use crate::resolve::{sha256_hex, ReleaseManifest, ResolveError, RELEASE_MANIFEST_SCHEMA_VERSION};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

/// Errors during toolchain install.
#[derive(Debug, Error)]
pub enum InstallError {
    /// Policy or validation failure.
    #[error("{0}")]
    Invalid(String),
    /// Filesystem failure.
    #[error("{0}")]
    Io(String),
}

impl From<ResolveError> for InstallError {
    fn from(value: ResolveError) -> Self {
        Self::Invalid(value.to_string())
    }
}

/// Successful install report (secret-free).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallReport {
    /// Installed absolute cache directory.
    pub install_dir: PathBuf,
    /// Release tag.
    pub tag: String,
    /// Target triple.
    pub target: String,
    /// SHA-256 of installed `release-manifest.json`.
    pub release_manifest_sha256: String,
    /// Number of binaries installed.
    pub binary_count: usize,
}

/// Install a verified toolchain bundle into the local cache.
///
/// Layout expected under `bundle_dir`:
/// - `release-manifest.json` (`agentmesh-release-manifest.v0`)
/// - files referenced by `binaries.*.relative_path` (typically under `bin/`)
///
/// Install algorithm:
/// 1. Acquire exclusive per-tag/target lock (`create_new` lock file).
/// 2. Refuse if final `cache/<tag>/<target>/` already exists (immutable directory).
/// 3. Copy into sibling staging dir, verify every hash, set executable bits.
/// 4. Atomically rename staging → final.
/// 5. Best-effort mark files read-only (Windows-safe; never overwrite running binaries).
pub fn install_toolchain_bundle(
    bundle_dir: &Path,
    cache_root: &Path,
) -> Result<InstallReport, InstallError> {
    let manifest_path = bundle_dir.join("release-manifest.json");
    let manifest_bytes = fs::read(&manifest_path).map_err(|e| {
        InstallError::Invalid(format!(
            "bundle missing release-manifest.json at {}: {e}",
            manifest_path.display()
        ))
    })?;
    let text = String::from_utf8(manifest_bytes.clone())
        .map_err(|_| InstallError::Invalid("release-manifest.json is not UTF-8".into()))?;
    let release = ReleaseManifest::parse(&text).map_err(InstallError::Invalid)?;
    if release.schema_version != RELEASE_MANIFEST_SCHEMA_VERSION {
        return Err(InstallError::Invalid(format!(
            "unsupported release-manifest schema_version: {}",
            release.schema_version
        )));
    }
    if !SUPPORTED_TARGETS.contains(&release.target.as_str()) {
        return Err(InstallError::Invalid(format!(
            "unsupported target in release-manifest: {}",
            release.target
        )));
    }
    // Pre-verify source bundle hashes before touching the cache.
    verify_bundle_hashes(bundle_dir, &release)?;

    fs::create_dir_all(cache_root).map_err(|e| {
        InstallError::Io(format!(
            "cannot create toolchain cache root {}: {e}",
            cache_root.display()
        ))
    })?;
    let tag_dir = cache_root.join(&release.tag);
    fs::create_dir_all(&tag_dir).map_err(|e| {
        InstallError::Io(format!("cannot create tag dir {}: {e}", tag_dir.display()))
    })?;

    let final_dir = tag_dir.join(&release.target);
    if final_dir.exists() {
        return Err(InstallError::Invalid(format!(
            "immutable toolchain directory already exists: {} (refuse overwrite; use a new tag/target)",
            final_dir.display()
        )));
    }

    let _lock = InstallLock::acquire(&tag_dir, &release.target)?;

    // Re-check after lock acquisition (another installer may have finished).
    if final_dir.exists() {
        return Err(InstallError::Invalid(format!(
            "immutable toolchain directory already exists: {}",
            final_dir.display()
        )));
    }

    let staging = staging_dir(&tag_dir, &release.target)?;
    if staging.exists() {
        fs::remove_dir_all(&staging).map_err(|e| {
            InstallError::Io(format!(
                "failed clearing stale staging {}: {e}",
                staging.display()
            ))
        })?;
    }
    fs::create_dir_all(&staging).map_err(|e| {
        InstallError::Io(format!("cannot create staging {}: {e}", staging.display()))
    })?;

    // Copy release-manifest first, then each binary under its relative path.
    let staged_manifest = staging.join("release-manifest.json");
    fs::write(&staged_manifest, &manifest_bytes)
        .map_err(|e| InstallError::Io(format!("failed writing staged release-manifest: {e}")))?;

    for (logical, bin) in &release.binaries {
        let src = bundle_dir.join(&bin.relative_path);
        let dst = staging.join(&bin.relative_path);
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                InstallError::Io(format!("cannot create staged parent for `{logical}`: {e}"))
            })?;
        }
        fs::copy(&src, &dst).map_err(|e| {
            InstallError::Io(format!(
                "failed copying `{logical}` from {}: {e}",
                src.display()
            ))
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&dst)
                .map_err(|e| InstallError::Io(format!("stat staged `{logical}`: {e}")))?
                .permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&dst, perms)
                .map_err(|e| InstallError::Io(format!("chmod staged `{logical}` failed: {e}")))?;
        }
    }

    // Verify staged copies before promote.
    verify_bundle_hashes(&staging, &release)?;

    // Atomic promote: staging → final (final must not exist).
    fs::rename(&staging, &final_dir).map_err(|e| {
        // Best-effort cleanup of staging on failure.
        let _ = fs::remove_dir_all(&staging);
        InstallError::Io(format!(
            "atomic rename into cache failed ({} → {}): {e}",
            staging.display(),
            final_dir.display()
        ))
    })?;

    mark_tree_readonly(&final_dir);

    let installed_manifest = fs::read(final_dir.join("release-manifest.json"))
        .map_err(|e| InstallError::Io(format!("failed reading installed release-manifest: {e}")))?;

    Ok(InstallReport {
        install_dir: final_dir,
        tag: release.tag,
        target: release.target,
        release_manifest_sha256: sha256_hex(&installed_manifest),
        binary_count: release.binaries.len(),
    })
}

fn verify_bundle_hashes(root: &Path, release: &ReleaseManifest) -> Result<(), InstallError> {
    for (logical, bin) in &release.binaries {
        let path = root.join(&bin.relative_path);
        let bytes = fs::read(&path).map_err(|e| {
            InstallError::Invalid(format!(
                "bundle binary `{logical}` missing at {}: {e}",
                path.display()
            ))
        })?;
        let actual = sha256_hex(&bytes);
        if actual != bin.sha256 {
            return Err(InstallError::Invalid(format!(
                "bundle binary `{logical}` SHA-256 mismatch"
            )));
        }
    }
    Ok(())
}

fn staging_dir(tag_dir: &Path, target: &str) -> Result<PathBuf, InstallError> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| InstallError::Io(format!("clock error: {e}")))?
        .as_nanos();
    Ok(tag_dir.join(format!(".staging-{target}-{nanos}")))
}

fn mark_tree_readonly(root: &Path) {
    // Best-effort immutability: mark files read-only so casual overwrite fails.
    // Directory replacement remains blocked by existence checks + rename semantics.
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            mark_tree_readonly(&path);
            continue;
        }
        if let Ok(meta) = fs::metadata(&path) {
            let mut perms = meta.permissions();
            perms.set_readonly(true);
            let _ = fs::set_permissions(&path, perms);
        }
    }
}

struct InstallLock {
    file: Option<File>,
    path: PathBuf,
}

impl InstallLock {
    fn acquire(tag_dir: &Path, target: &str) -> Result<Self, InstallError> {
        let path = tag_dir.join(format!("{target}.install.lock"));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| {
                InstallError::Invalid(format!(
                    "install lock held or unavailable at {}: {e}",
                    path.display()
                ))
            })?;
        let _ = writeln!(file, "pid={}", std::process::id());
        let _ = file.flush();
        Ok(Self {
            file: Some(file),
            path,
        })
    }
}

impl Drop for InstallLock {
    fn drop(&mut self) {
        drop(self.file.take());
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn write_bundle(dir: &Path, plugin_bytes: &[u8]) -> PathBuf {
        let bin_dir = dir.join("bin");
        fs::create_dir_all(&bin_dir).unwrap();
        let plugin_name = if cfg!(windows) {
            "agentmesh-example-plugin.exe"
        } else {
            "agentmesh-example-plugin"
        };
        fs::write(bin_dir.join(plugin_name), plugin_bytes).unwrap();
        let body = serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": RELEASE_MANIFEST_SCHEMA_VERSION,
            "tag": "v0.2.0-dev.1",
            "commit_sha": "376f849893654a8d2de868e79f0c5d0aefb4308c",
            "target": "x86_64-pc-windows-msvc",
            "protocol_version": "2026-07-15",
            "binaries": {
                "agentmesh-example-plugin": {
                    "relative_path": format!("bin/{plugin_name}"),
                    "sha256": sha256_hex(plugin_bytes),
                }
            }
        }))
        .unwrap();
        fs::write(dir.join("release-manifest.json"), body).unwrap();
        dir.to_path_buf()
    }

    #[test]
    fn install_atomic_and_refuse_overwrite() {
        let dir = tempdir().unwrap();
        let bundle = dir.path().join("bundle");
        fs::create_dir_all(&bundle).unwrap();
        write_bundle(&bundle, b"plugin-bytes");
        let cache = dir.path().join("cache");

        let report = install_toolchain_bundle(&bundle, &cache).unwrap();
        assert_eq!(report.tag, "v0.2.0-dev.1");
        assert!(report.install_dir.join("release-manifest.json").is_file());
        assert_eq!(report.binary_count, 1);

        let err = install_toolchain_bundle(&bundle, &cache).unwrap_err();
        assert!(err.to_string().contains("already exists"));

        // Allow TempDir cleanup on Windows after read-only marking.
        clear_readonly_tree(&report.install_dir);
    }

    fn clear_readonly_tree(root: &Path) {
        let Ok(entries) = fs::read_dir(root) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                clear_readonly_tree(&path);
            }
            if let Ok(meta) = fs::metadata(&path) {
                let mut perms = meta.permissions();
                #[allow(clippy::permissions_set_readonly_false)]
                perms.set_readonly(false);
                let _ = fs::set_permissions(&path, perms);
            }
        }
    }

    #[test]
    fn reject_tampered_bundle_hash() {
        let dir = tempdir().unwrap();
        let bundle = dir.path().join("bundle");
        fs::create_dir_all(&bundle).unwrap();
        write_bundle(&bundle, b"plugin-bytes");
        // Tamper binary after writing matching manifest.
        let plugin_name = if cfg!(windows) {
            "agentmesh-example-plugin.exe"
        } else {
            "agentmesh-example-plugin"
        };
        fs::write(bundle.join("bin").join(plugin_name), b"tampered").unwrap();
        let err = install_toolchain_bundle(&bundle, &dir.path().join("cache")).unwrap_err();
        assert!(err.to_string().contains("SHA-256 mismatch"));
    }
}
