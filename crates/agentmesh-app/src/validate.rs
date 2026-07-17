//! Combined manifest + toolchain pin validation.

use crate::manifest::AppManifest;
use crate::pin::ToolchainPin;
use agentmesh_proto::PROTOCOL_VERSION;
use std::path::Path;
use thiserror::Error;

/// Validation failure categories.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ValidationError {
    /// IO / parse / structural failure.
    #[error("{0}")]
    Invalid(String),
}

/// Successful validation report (secret-free).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationReport {
    /// App name from manifest.
    pub app_name: String,
    /// Logical plugin name.
    pub plugin_logical_name: String,
    /// Pin tag.
    pub pin_tag: String,
    /// Pin commit SHA.
    pub pin_commit_sha: String,
    /// Pin target triple.
    pub pin_target: String,
    /// Host protocol version checked against the app.
    pub protocol_version: String,
}

/// Validate an app manifest together with a version-controlled toolchain pin.
pub fn validate_app_bundle(
    manifest_path: &Path,
    pin_path: &Path,
) -> Result<ValidationReport, ValidationError> {
    let manifest = AppManifest::load(manifest_path).map_err(ValidationError::Invalid)?;
    manifest
        .validate_structure()
        .map_err(ValidationError::Invalid)?;

    let pin = ToolchainPin::load(pin_path).map_err(ValidationError::Invalid)?;
    pin.validate_structure().map_err(ValidationError::Invalid)?;

    if manifest.protocol_version != PROTOCOL_VERSION {
        return Err(ValidationError::Invalid(format!(
            "protocol_version mismatch: app=`{}` host=`{PROTOCOL_VERSION}`",
            manifest.protocol_version
        )));
    }

    // Reject shell-command-like leftover keys by scanning raw TOML again for forbidden keys.
    reject_forbidden_manifest_keys(manifest_path)?;
    reject_forbidden_pin_keys(pin_path)?;

    // If schema refs exist, ensure they resolve under the app root without escape.
    let app_root = manifest_path
        .parent()
        .ok_or_else(|| ValidationError::Invalid("manifest path has no parent".into()))?;
    for (field, rel) in [
        ("schemas.input", manifest.schemas.input.as_deref()),
        ("schemas.output", manifest.schemas.output.as_deref()),
    ] {
        if let Some(rel) = rel {
            let joined = app_root.join(rel);
            let canonical_root =
                std::fs::canonicalize(app_root).unwrap_or_else(|_| app_root.to_path_buf());
            // Soft existence check: warn-as-error for validate when file missing.
            if !joined.exists() {
                return Err(ValidationError::Invalid(format!(
                    "{field} path does not exist: {rel}"
                )));
            }
            let canonical = std::fs::canonicalize(&joined).map_err(|e| {
                ValidationError::Invalid(format!("{field} canonicalize failed: {e}"))
            })?;
            if !canonical.starts_with(&canonical_root) {
                return Err(ValidationError::Invalid(format!(
                    "{field} escapes app root"
                )));
            }
        }
    }

    Ok(ValidationReport {
        app_name: manifest.name,
        plugin_logical_name: manifest.plugin.logical_name,
        pin_tag: pin.tag,
        pin_commit_sha: pin.commit_sha,
        pin_target: pin.target,
        protocol_version: PROTOCOL_VERSION.to_string(),
    })
}

fn reject_forbidden_manifest_keys(path: &Path) -> Result<(), ValidationError> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| ValidationError::Invalid(format!("manifest read error: {e}")))?;
    let value: toml::Value = toml::from_str(&text)
        .map_err(|e| ValidationError::Invalid(format!("manifest parse error: {e}")))?;
    let forbidden = [
        "command",
        "shell",
        "exec",
        "script",
        "run",
        "args",
        "cwd",
        "install_path",
        "plugin_path",
    ];
    scan_forbidden(&value, "", &forbidden)
}

fn reject_forbidden_pin_keys(path: &Path) -> Result<(), ValidationError> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| ValidationError::Invalid(format!("pin read error: {e}")))?;
    let value: toml::Value = toml::from_str(&text)
        .map_err(|e| ValidationError::Invalid(format!("pin parse error: {e}")))?;
    let forbidden = [
        "command",
        "shell",
        "exec",
        "install_path",
        "cache_path",
        "local_path",
        "path",
        "plugin_path",
    ];
    scan_forbidden(&value, "", &forbidden)
}

fn scan_forbidden(
    value: &toml::Value,
    prefix: &str,
    forbidden: &[&str],
) -> Result<(), ValidationError> {
    let Some(table) = value.as_table() else {
        return Ok(());
    };
    for (key, child) in table {
        let path = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        if forbidden.iter().any(|f| key == f) {
            return Err(ValidationError::Invalid(format!(
                "forbidden key `{path}` (shell/path fields are rejected)"
            )));
        }
        scan_forbidden(child, &path, forbidden)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn write(dir: &Path, name: &str, body: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn validate_ok_with_schema_ref() {
        let dir = tempdir().unwrap();
        let schema = write(dir.path(), "input.schema.json", "{}\n");
        let _ = schema;
        let manifest = write(
            dir.path(),
            "agentmesh-app.toml",
            r#"
schema_version = "agentmesh-app.v0"
name = "backlog-promoter"
protocol_version = "2026-07-15"

[plugin]
logical_name = "agentmesh-example-plugin"

[schemas]
input = "input.schema.json"

[conformance]
cargo_package = "agentmesh-example-plugin"
"#,
        );
        let pin = write(
            dir.path(),
            "pin.toml",
            r#"
schema_version = "agentmesh-toolchain-pin.v0"
tag = "v0.2.0-dev.1"
commit_sha = "376f849893654a8d2de868e79f0c5d0aefb4308c"
target = "x86_64-pc-windows-msvc"
release_manifest_sha256 = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
"#,
        );
        let report = validate_app_bundle(&manifest, &pin).unwrap();
        assert_eq!(report.app_name, "backlog-promoter");
        assert_eq!(report.pin_tag, "v0.2.0-dev.1");
    }

    #[test]
    fn reject_command_key() {
        let dir = tempdir().unwrap();
        let manifest = write(
            dir.path(),
            "agentmesh-app.toml",
            r#"
schema_version = "agentmesh-app.v0"
name = "backlog-promoter"
protocol_version = "2026-07-15"
command = "rm -rf /"

[plugin]
logical_name = "agentmesh-example-plugin"
"#,
        );
        let pin = write(
            dir.path(),
            "pin.toml",
            r#"
schema_version = "agentmesh-toolchain-pin.v0"
tag = "v0.2.0-dev.1"
commit_sha = "376f849893654a8d2de868e79f0c5d0aefb4308c"
target = "x86_64-pc-windows-msvc"
release_manifest_sha256 = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
"#,
        );
        let err = validate_app_bundle(&manifest, &pin).unwrap_err();
        assert!(err.to_string().contains("command"));
    }
}
