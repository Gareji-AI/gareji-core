//! Secure logical-binary resolution under a pinned toolchain cache.

use crate::manifest::{validate_sha256_hex, AppManifest};
use crate::pin::ToolchainPin;
use agentmesh_proto::PROTOCOL_VERSION;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Component, Path, PathBuf};
use thiserror::Error;

/// Release-manifest schema written into each installed toolchain cache.
pub const RELEASE_MANIFEST_SCHEMA_VERSION: &str = "agentmesh-release-manifest.v0";

/// Errors while resolving a logical plugin binary.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ResolveError {
    /// Structural / policy failure.
    #[error("{0}")]
    Invalid(String),
}

/// How the plugin binary was selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolveMode {
    /// Resolved via toolchain pin + local cache.
    Pinned,
    /// Explicit developer override (`--dev-plugin`).
    UnpinnedDev,
}

/// Successful secure resolution result (secret-free).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPlugin {
    /// Absolute canonical plugin path.
    pub plugin_path: PathBuf,
    /// Lowercase SHA-256 of the plugin bytes.
    pub plugin_sha256: String,
    /// Resolution mode.
    pub mode: ResolveMode,
    /// Logical plugin name from the app manifest.
    pub logical_name: String,
    /// Pin tag when pinned; `None` for unpinned overrides.
    pub pin_tag: Option<String>,
    /// Pin target when pinned.
    pub pin_target: Option<String>,
    /// Host protocol version enforced for this resolve.
    pub protocol_version: String,
}

/// Local release-manifest entry for one packaged binary.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ReleaseBinary {
    /// Path relative to the toolchain root (no `..`).
    pub relative_path: String,
    /// Expected SHA-256 of the binary file.
    pub sha256: String,
}

/// Verified release-manifest document stored under the toolchain cache.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ReleaseManifest {
    /// Must be [`RELEASE_MANIFEST_SCHEMA_VERSION`].
    pub schema_version: String,
    /// Release tag.
    pub tag: String,
    /// Full commit SHA.
    pub commit_sha: String,
    /// Target triple.
    pub target: String,
    /// Protocol version of packaged binaries.
    pub protocol_version: String,
    /// Logical-name → binary metadata.
    pub binaries: std::collections::BTreeMap<String, ReleaseBinary>,
}

impl ReleaseManifest {
    /// Parse release-manifest JSON.
    pub fn parse(text: &str) -> Result<Self, String> {
        serde_json::from_str(text).map_err(|e| format!("release-manifest parse error: {e}"))
    }

    /// Structural validation against a consumer pin.
    pub fn validate_against_pin(&self, pin: &ToolchainPin) -> Result<(), String> {
        if self.schema_version != RELEASE_MANIFEST_SCHEMA_VERSION {
            return Err(format!(
                "unsupported release-manifest schema_version: {}",
                self.schema_version
            ));
        }
        if self.tag != pin.tag {
            return Err(format!(
                "release-manifest tag mismatch: manifest=`{}` pin=`{}`",
                self.tag, pin.tag
            ));
        }
        if self.commit_sha != pin.commit_sha {
            return Err("release-manifest commit_sha mismatch".into());
        }
        if self.target != pin.target {
            return Err(format!(
                "release-manifest target mismatch: manifest=`{}` pin=`{}`",
                self.target, pin.target
            ));
        }
        if self.protocol_version != PROTOCOL_VERSION {
            return Err(format!(
                "release-manifest protocol_version mismatch: manifest=`{}` host=`{PROTOCOL_VERSION}`",
                self.protocol_version
            ));
        }
        for (name, bin) in &self.binaries {
            if name.is_empty() || name.contains('/') || name.contains('\\') || name.contains("..") {
                return Err(format!("invalid binary logical name `{name}`"));
            }
            validate_relative_path(&bin.relative_path, "binaries.relative_path")?;
            validate_sha256_hex(&bin.sha256, "binaries.sha256")?;
        }
        Ok(())
    }
}

/// Resolve default toolchain cache root (`$AGENTMESH_TOOLCHAIN_CACHE` or `~/.agentmesh/toolchains`).
pub fn default_toolchain_cache_root() -> Result<PathBuf, ResolveError> {
    if let Ok(explicit) = std::env::var("AGENTMESH_TOOLCHAIN_CACHE") {
        let path = PathBuf::from(explicit);
        if path.as_os_str().is_empty() {
            return Err(ResolveError::Invalid(
                "AGENTMESH_TOOLCHAIN_CACHE is empty".into(),
            ));
        }
        return Ok(path);
    }
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .ok_or_else(|| {
            ResolveError::Invalid(
                "cannot resolve home directory for toolchain cache (set AGENTMESH_TOOLCHAIN_CACHE)"
                    .into(),
            )
        })?;
    Ok(PathBuf::from(home).join(".agentmesh").join("toolchains"))
}

/// Resolve a logical plugin under a pinned local toolchain cache.
pub fn resolve_pinned_plugin(
    manifest: &AppManifest,
    pin: &ToolchainPin,
    cache_root: &Path,
) -> Result<ResolvedPlugin, ResolveError> {
    if manifest.protocol_version != PROTOCOL_VERSION {
        return Err(ResolveError::Invalid(format!(
            "protocol_version mismatch: app=`{}` host=`{PROTOCOL_VERSION}`",
            manifest.protocol_version
        )));
    }

    let toolchain_dir = cache_root.join(&pin.tag).join(&pin.target);
    let manifest_path = toolchain_dir.join("release-manifest.json");
    let manifest_bytes = fs::read(&manifest_path).map_err(|e| {
        ResolveError::Invalid(format!(
            "release-manifest missing under {}: {e}",
            manifest_path.display()
        ))
    })?;
    let digest = sha256_hex(&manifest_bytes);
    if digest != pin.release_manifest_sha256 {
        return Err(ResolveError::Invalid(
            "release-manifest SHA-256 does not match toolchain pin".into(),
        ));
    }
    let text = String::from_utf8(manifest_bytes)
        .map_err(|_| ResolveError::Invalid("release-manifest is not valid UTF-8".into()))?;
    let release = ReleaseManifest::parse(&text).map_err(ResolveError::Invalid)?;
    release
        .validate_against_pin(pin)
        .map_err(ResolveError::Invalid)?;

    let logical = &manifest.plugin.logical_name;
    let entry = release.binaries.get(logical).ok_or_else(|| {
        ResolveError::Invalid(format!(
            "logical plugin `{logical}` not present in release-manifest"
        ))
    })?;

    let candidate = toolchain_dir.join(&entry.relative_path);
    let canonical_root = canonicalize_dir(cache_root)?;
    let canonical_plugin = canonicalize_file(&candidate)?;
    ensure_under_root(&canonical_plugin, &canonical_root)?;

    let plugin_sha = hash_file(&canonical_plugin)?;
    if plugin_sha != entry.sha256 {
        return Err(ResolveError::Invalid(format!(
            "plugin binary SHA-256 mismatch for `{logical}`"
        )));
    }
    if let Some(expected) = &manifest.plugin.sha256 {
        if &plugin_sha != expected {
            return Err(ResolveError::Invalid(
                "plugin binary SHA-256 does not match app manifest plugin.sha256".into(),
            ));
        }
    }

    reject_windows_reparse_outside_root(&canonical_plugin, &canonical_root)?;

    Ok(ResolvedPlugin {
        plugin_path: canonical_plugin,
        plugin_sha256: plugin_sha,
        mode: ResolveMode::Pinned,
        logical_name: logical.clone(),
        pin_tag: Some(pin.tag.clone()),
        pin_target: Some(pin.target.clone()),
        protocol_version: PROTOCOL_VERSION.to_string(),
    })
}

/// Resolve an explicit absolute `--dev-plugin` path (development only).
pub fn resolve_dev_plugin(
    manifest: &AppManifest,
    absolute_plugin: &Path,
) -> Result<ResolvedPlugin, ResolveError> {
    if !absolute_plugin.is_absolute() {
        return Err(ResolveError::Invalid(
            "--dev-plugin path must be absolute".into(),
        ));
    }
    for component in absolute_plugin.components() {
        if matches!(component, Component::ParentDir) {
            return Err(ResolveError::Invalid(
                "--dev-plugin path must not contain '..'".into(),
            ));
        }
    }
    let canonical = canonicalize_file(absolute_plugin)?;
    let plugin_sha = hash_file(&canonical)?;
    if let Some(expected) = &manifest.plugin.sha256 {
        // Dev overrides intentionally skip cache identity, but if the app pins an
        // expected hash, enforce it as a developer guardrail.
        if &plugin_sha != expected {
            return Err(ResolveError::Invalid(
                "dev-plugin SHA-256 does not match app manifest plugin.sha256".into(),
            ));
        }
    }
    Ok(ResolvedPlugin {
        plugin_path: canonical,
        plugin_sha256: plugin_sha,
        mode: ResolveMode::UnpinnedDev,
        logical_name: manifest.plugin.logical_name.clone(),
        pin_tag: None,
        pin_target: None,
        protocol_version: PROTOCOL_VERSION.to_string(),
    })
}

fn validate_relative_path(path: &str, field: &str) -> Result<(), String> {
    if path.is_empty() {
        return Err(format!("{field} is empty"));
    }
    if Path::new(path).is_absolute() {
        return Err(format!("{field} must be relative"));
    }
    for component in Path::new(path).components() {
        match component {
            Component::Normal(_) | Component::CurDir => {}
            Component::ParentDir => return Err(format!("{field} must not contain '..'")),
            _ => return Err(format!("{field} has unsupported path component")),
        }
    }
    Ok(())
}

fn canonicalize_dir(path: &Path) -> Result<PathBuf, ResolveError> {
    fs::create_dir_all(path).map_err(|e| {
        ResolveError::Invalid(format!(
            "cannot ensure toolchain cache root {}: {e}",
            path.display()
        ))
    })?;
    fs::canonicalize(path).map_err(|e| {
        ResolveError::Invalid(format!(
            "cannot canonicalize toolchain cache root {}: {e}",
            path.display()
        ))
    })
}

fn canonicalize_file(path: &Path) -> Result<PathBuf, ResolveError> {
    let meta = fs::symlink_metadata(path).map_err(|e| {
        ResolveError::Invalid(format!("plugin path missing {}: {e}", path.display()))
    })?;
    if !meta.is_file() && !meta.file_type().is_symlink() {
        return Err(ResolveError::Invalid(format!(
            "plugin path is not a file: {}",
            path.display()
        )));
    }
    let canonical = fs::canonicalize(path).map_err(|e| {
        ResolveError::Invalid(format!(
            "cannot canonicalize plugin path {}: {e}",
            path.display()
        ))
    })?;
    let file_meta = fs::metadata(&canonical).map_err(|e| {
        ResolveError::Invalid(format!(
            "canonical plugin metadata failed {}: {e}",
            canonical.display()
        ))
    })?;
    if !file_meta.is_file() {
        return Err(ResolveError::Invalid(format!(
            "canonical plugin path is not a file: {}",
            canonical.display()
        )));
    }
    Ok(canonical)
}

fn ensure_under_root(path: &Path, root: &Path) -> Result<(), ResolveError> {
    if !path.starts_with(root) {
        return Err(ResolveError::Invalid(
            "resolved plugin path escapes toolchain cache root".into(),
        ));
    }
    Ok(())
}

fn reject_windows_reparse_outside_root(path: &Path, root: &Path) -> Result<(), ResolveError> {
    // canonicalize() already resolved reparse points / symlinks into an absolute path.
    // Re-check containment as a defense-in-depth for Windows junction targets.
    ensure_under_root(path, root)
}

fn hash_file(path: &Path) -> Result<String, ResolveError> {
    let bytes = fs::read(path).map_err(|e| {
        ResolveError::Invalid(format!("failed reading plugin {}: {e}", path.display()))
    })?;
    Ok(sha256_hex(&bytes))
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::AppManifest;
    use crate::pin::ToolchainPin;
    use std::io::Write;
    use tempfile::tempdir;

    fn sample_app() -> AppManifest {
        AppManifest::parse(
            r#"
schema_version = "agentmesh-app.v0"
name = "backlog-promoter"
protocol_version = "2026-07-15"
[plugin]
logical_name = "agentmesh-example-plugin"
"#,
        )
        .unwrap()
    }

    fn sample_pin(manifest_sha: &str) -> ToolchainPin {
        ToolchainPin::parse(&format!(
            r#"
schema_version = "agentmesh-toolchain-pin.v0"
tag = "v0.2.0-dev.1"
commit_sha = "376f849893654a8d2de868e79f0c5d0aefb4308c"
target = "x86_64-pc-windows-msvc"
release_manifest_sha256 = "{manifest_sha}"
"#
        ))
        .unwrap()
    }

    fn write_toolchain(cache: &Path, plugin_bytes: &[u8]) -> (ToolchainPin, PathBuf) {
        let root = cache.join("v0.2.0-dev.1").join("x86_64-pc-windows-msvc");
        let bin_dir = root.join("bin");
        fs::create_dir_all(&bin_dir).unwrap();
        let plugin_name = if cfg!(windows) {
            "agentmesh-example-plugin.exe"
        } else {
            "agentmesh-example-plugin"
        };
        let plugin_path = bin_dir.join(plugin_name);
        fs::write(&plugin_path, plugin_bytes).unwrap();
        let plugin_sha = sha256_hex(plugin_bytes);
        let release = serde_json::json!({
            "schema_version": RELEASE_MANIFEST_SCHEMA_VERSION,
            "tag": "v0.2.0-dev.1",
            "commit_sha": "376f849893654a8d2de868e79f0c5d0aefb4308c",
            "target": "x86_64-pc-windows-msvc",
            "protocol_version": "2026-07-15",
            "binaries": {
                "agentmesh-example-plugin": {
                    "relative_path": format!("bin/{plugin_name}"),
                    "sha256": plugin_sha
                }
            }
        });
        let release_text = serde_json::to_vec_pretty(&release).unwrap();
        let release_sha = sha256_hex(&release_text);
        fs::write(root.join("release-manifest.json"), &release_text).unwrap();
        (sample_pin(&release_sha), plugin_path)
    }

    #[test]
    fn resolve_pinned_ok() {
        let dir = tempdir().unwrap();
        let (pin, _) = write_toolchain(dir.path(), b"plugin-bytes");
        let resolved = resolve_pinned_plugin(&sample_app(), &pin, dir.path()).unwrap();
        assert_eq!(resolved.mode, ResolveMode::Pinned);
        assert_eq!(resolved.plugin_sha256, sha256_hex(b"plugin-bytes"));
        assert!(resolved
            .plugin_path
            .starts_with(dir.path().canonicalize().unwrap()));
    }

    #[test]
    fn reject_hash_mismatch() {
        let dir = tempdir().unwrap();
        let (pin, plugin_path) = write_toolchain(dir.path(), b"plugin-bytes");
        // Tamper after pinning the release-manifest digest.
        fs::write(&plugin_path, b"tampered").unwrap();
        let err = resolve_pinned_plugin(&sample_app(), &pin, dir.path()).unwrap_err();
        assert!(err.to_string().contains("SHA-256 mismatch"));
    }

    #[test]
    fn reject_path_escape_relative() {
        let err = validate_relative_path("../evil", "binaries.relative_path").unwrap_err();
        assert!(err.contains(".."));
    }

    #[test]
    fn resolve_dev_requires_absolute() {
        let err = resolve_dev_plugin(&sample_app(), Path::new("relative.exe")).unwrap_err();
        assert!(err.to_string().contains("absolute"));
    }

    #[test]
    fn resolve_dev_ok() {
        let dir = tempdir().unwrap();
        let plugin = dir.path().join("dev-plugin.bin");
        let mut f = fs::File::create(&plugin).unwrap();
        write!(f, "dev").unwrap();
        let abs = plugin.canonicalize().unwrap();
        let resolved = resolve_dev_plugin(&sample_app(), &abs).unwrap();
        assert_eq!(resolved.mode, ResolveMode::UnpinnedDev);
        assert_eq!(resolved.plugin_sha256, sha256_hex(b"dev"));
    }
}
