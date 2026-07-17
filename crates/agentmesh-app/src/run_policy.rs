//! `agentmesh app run` policy: production rejects `--dev-plugin`.

use crate::manifest::AppManifest;
use crate::pin::ToolchainPin;
use crate::resolve::{
    default_toolchain_cache_root, resolve_dev_plugin, resolve_pinned_plugin, ResolveError,
    ResolveMode, ResolvedPlugin,
};
use crate::validate::{validate_app_bundle, ValidationError};
use agentmesh_proto::Limits;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// App run mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppRunMode {
    /// Production/canary — pin + cache only; `--dev-plugin` rejected.
    Production,
    /// Local development — may use `--dev-plugin` (marks run unpinned).
    Development,
}

impl AppRunMode {
    /// Parse from CLI token.
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "production" | "prod" | "canary" => Ok(Self::Production),
            "development" | "dev" => Ok(Self::Development),
            other => Err(format!(
                "unsupported --mode `{other}` (expected production|development)"
            )),
        }
    }
}

/// Errors preparing an app run.
#[derive(Debug, Error)]
pub enum AppRunError {
    /// Manifest/pin validation failed.
    #[error("{0}")]
    Validate(#[from] ValidationError),
    /// Binary resolution failed.
    #[error("{0}")]
    Resolve(#[from] ResolveError),
    /// Policy refusal (e.g. production + `--dev-plugin`).
    #[error("{0}")]
    Policy(String),
    /// Input / IO failure.
    #[error("{0}")]
    Io(String),
}

/// Prepared host binding for `execute_run`.
#[derive(Debug, Clone)]
pub struct PreparedAppRun {
    /// Securely resolved plugin.
    pub resolved: ResolvedPlugin,
    /// Host limits with optional app overrides applied.
    pub limits: Limits,
    /// Environment allowlist names from the app manifest.
    pub plugin_env_keys: Vec<String>,
    /// Whether plugin stderr capture is requested by the app.
    pub capture_plugin_stderr: bool,
    /// Secret-free marker string for diagnostics / sidecar notes.
    pub run_marker: String,
}

/// Inputs for preparing `agentmesh app run`.
#[derive(Debug, Clone)]
pub struct AppRunRequest<'a> {
    /// Path to `agentmesh-app.toml`.
    pub manifest_path: &'a Path,
    /// Path to toolchain pin TOML.
    pub pin_path: &'a Path,
    /// Production vs development.
    pub mode: AppRunMode,
    /// Optional absolute `--dev-plugin` override.
    pub dev_plugin: Option<&'a Path>,
    /// Optional toolchain cache root override.
    pub toolchain_cache: Option<&'a Path>,
}

/// Validate + resolve according to mode/policy (does not spawn the host).
pub fn prepare_app_run(
    request: AppRunRequest<'_>,
) -> Result<(AppManifest, PreparedAppRun), AppRunError> {
    let _report = validate_app_bundle(request.manifest_path, request.pin_path)?;
    let manifest = AppManifest::load(request.manifest_path).map_err(AppRunError::Io)?;
    let pin = ToolchainPin::load(request.pin_path).map_err(AppRunError::Io)?;

    if let Some(dev) = request.dev_plugin {
        if request.mode == AppRunMode::Production {
            return Err(AppRunError::Policy(
                "production/canary mode rejects --dev-plugin (use --mode development for local overrides)"
                    .into(),
            ));
        }
        let resolved = resolve_dev_plugin(&manifest, dev)?;
        return Ok((
            manifest.clone(),
            prepared_from_manifest(&manifest, resolved),
        ));
    }

    let cache_root = match request.toolchain_cache {
        Some(path) => path.to_path_buf(),
        None => default_toolchain_cache_root()?,
    };
    let resolved = resolve_pinned_plugin(&manifest, &pin, &cache_root)?;
    Ok((
        manifest.clone(),
        prepared_from_manifest(&manifest, resolved),
    ))
}

fn prepared_from_manifest(manifest: &AppManifest, resolved: ResolvedPlugin) -> PreparedAppRun {
    let mut limits = Limits::default();
    if let Some(ms) = manifest.limits.run_timeout_ms {
        limits.run_timeout_ms = ms;
    }
    if let Some(n) = manifest.limits.input_max_bytes {
        limits.input_max_bytes = n;
    }
    if let Some(n) = manifest.limits.sidecar_max_bytes {
        limits.sidecar_max_bytes = n;
    }

    let run_marker = match resolved.mode {
        ResolveMode::Pinned => format!(
            "app_run_mode=pinned pin_tag={} pin_target={} plugin={} plugin_sha256={} protocol={}",
            resolved.pin_tag.as_deref().unwrap_or(""),
            resolved.pin_target.as_deref().unwrap_or(""),
            resolved.logical_name,
            resolved.plugin_sha256,
            resolved.protocol_version
        ),
        ResolveMode::UnpinnedDev => format!(
            "app_run_mode=unpinned plugin={} override_sha256={} protocol={}",
            resolved.logical_name, resolved.plugin_sha256, resolved.protocol_version
        ),
    };

    PreparedAppRun {
        resolved,
        limits,
        plugin_env_keys: manifest.env.allowlist.clone(),
        capture_plugin_stderr: manifest.sidecar.capture_plugin_stderr,
        run_marker,
    }
}

/// Write a small secret-free marker file beside sidecars for unpinned/pinned auditing.
pub fn write_run_marker(sidecar_dir: &Path, marker: &str) -> Result<PathBuf, AppRunError> {
    std::fs::create_dir_all(sidecar_dir)
        .map_err(|e| AppRunError::Io(format!("sidecar_dir create failed: {e}")))?;
    let path = sidecar_dir.join("agentmesh-app-run-marker.txt");
    std::fs::write(&path, format!("{marker}\n"))
        .map_err(|e| AppRunError::Io(format!("marker write failed: {e}")))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolve::{sha256_hex, RELEASE_MANIFEST_SCHEMA_VERSION};
    use std::fs;
    use tempfile::tempdir;

    fn write_files(dir: &Path) -> (PathBuf, PathBuf, PathBuf) {
        let app_dir = dir.join("app");
        fs::create_dir_all(&app_dir).unwrap();
        let manifest = app_dir.join("agentmesh-app.toml");
        fs::write(
            &manifest,
            r#"
schema_version = "agentmesh-app.v0"
name = "backlog-promoter"
protocol_version = "2026-07-15"
[plugin]
logical_name = "agentmesh-example-plugin"
"#,
        )
        .unwrap();

        let cache = dir.join("cache");
        let root = cache.join("v0.2.0-dev.1").join("x86_64-pc-windows-msvc");
        let bin = root.join("bin");
        fs::create_dir_all(&bin).unwrap();
        let plugin_name = if cfg!(windows) {
            "agentmesh-example-plugin.exe"
        } else {
            "agentmesh-example-plugin"
        };
        let plugin_path = bin.join(plugin_name);
        fs::write(&plugin_path, b"plugin-bytes").unwrap();
        let plugin_sha = sha256_hex(b"plugin-bytes");
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
        let release_bytes = serde_json::to_vec_pretty(&release).unwrap();
        let release_sha = sha256_hex(&release_bytes);
        fs::write(root.join("release-manifest.json"), &release_bytes).unwrap();

        let pin = dir.join("pin.toml");
        fs::write(
            &pin,
            format!(
                r#"
schema_version = "agentmesh-toolchain-pin.v0"
tag = "v0.2.0-dev.1"
commit_sha = "376f849893654a8d2de868e79f0c5d0aefb4308c"
target = "x86_64-pc-windows-msvc"
release_manifest_sha256 = "{release_sha}"
"#
            ),
        )
        .unwrap();
        (manifest, pin, cache)
    }

    #[test]
    fn production_rejects_dev_plugin() {
        let dir = tempdir().unwrap();
        let (manifest, pin, cache) = write_files(dir.path());
        let dev = dir.path().join("dev.bin");
        fs::write(&dev, b"dev").unwrap();
        let abs = fs::canonicalize(&dev).unwrap();
        let err = prepare_app_run(AppRunRequest {
            manifest_path: &manifest,
            pin_path: &pin,
            mode: AppRunMode::Production,
            dev_plugin: Some(&abs),
            toolchain_cache: Some(&cache),
        })
        .unwrap_err();
        assert!(err.to_string().contains("rejects --dev-plugin"));
    }

    #[test]
    fn development_allows_dev_plugin_unpinned() {
        let dir = tempdir().unwrap();
        let (manifest, pin, cache) = write_files(dir.path());
        let _ = cache;
        let dev = dir.path().join("dev.bin");
        fs::write(&dev, b"dev").unwrap();
        let abs = fs::canonicalize(&dev).unwrap();
        let (_app, prepared) = prepare_app_run(AppRunRequest {
            manifest_path: &manifest,
            pin_path: &pin,
            mode: AppRunMode::Development,
            dev_plugin: Some(&abs),
            toolchain_cache: None,
        })
        .unwrap();
        assert_eq!(prepared.resolved.mode, ResolveMode::UnpinnedDev);
        assert!(prepared.run_marker.contains("app_run_mode=unpinned"));
    }

    #[test]
    fn production_pinned_ok() {
        let dir = tempdir().unwrap();
        let (manifest, pin, cache) = write_files(dir.path());
        let (_app, prepared) = prepare_app_run(AppRunRequest {
            manifest_path: &manifest,
            pin_path: &pin,
            mode: AppRunMode::Production,
            dev_plugin: None,
            toolchain_cache: Some(&cache),
        })
        .unwrap();
        assert_eq!(prepared.resolved.mode, ResolveMode::Pinned);
        assert!(prepared.run_marker.contains("app_run_mode=pinned"));
    }
}
