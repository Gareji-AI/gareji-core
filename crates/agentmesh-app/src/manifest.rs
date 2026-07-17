//! `agentmesh-app.toml` v0 parsing and structural checks.

use serde::Deserialize;
use std::path::{Component, Path};

/// Schema identity for app manifests.
pub const APP_MANIFEST_SCHEMA_VERSION: &str = "agentmesh-app.v0";

/// Top-level AgentMesh App manifest v0.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AppManifest {
    /// Must be [`APP_MANIFEST_SCHEMA_VERSION`].
    pub schema_version: String,
    /// Stable app name (directory / deployable unit identity).
    pub name: String,
    /// Host protocol date/version this app expects (exact match for v0).
    pub protocol_version: String,
    /// Logical plugin binding.
    pub plugin: AppPlugin,
    /// Optional limit overrides.
    #[serde(default)]
    pub limits: AppLimits,
    /// Sidecar policy.
    #[serde(default)]
    pub sidecar: AppSidecar,
    /// Allowlisted environment variable names only.
    #[serde(default)]
    pub env: AppEnv,
    /// Schema references (relative paths under the app root).
    #[serde(default)]
    pub schemas: AppSchemas,
    /// Non-shell conformance reference.
    #[serde(default)]
    pub conformance: AppConformance,
}

/// Plugin logical identity inside a pinned toolchain.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AppPlugin {
    /// Logical binary name resolved via toolchain pin/cache (never a path).
    pub logical_name: String,
    /// Optional expected SHA-256 of the packaged plugin binary.
    #[serde(default)]
    pub sha256: Option<String>,
}

/// Optional runtime limit overrides.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default)]
pub struct AppLimits {
    /// Run timeout override in milliseconds.
    #[serde(default)]
    pub run_timeout_ms: Option<u64>,
    /// Input max bytes override.
    #[serde(default)]
    pub input_max_bytes: Option<usize>,
    /// Sidecar max bytes override.
    #[serde(default)]
    pub sidecar_max_bytes: Option<usize>,
}

/// Sidecar retention / capture policy.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AppSidecar {
    /// Retention class label (`owner_local`, `ephemeral`, ...).
    #[serde(default = "default_retention")]
    pub retention_class: String,
    /// Whether host may store bounded raw plugin stderr.
    #[serde(default)]
    pub capture_plugin_stderr: bool,
}

fn default_retention() -> String {
    "owner_local".into()
}

impl Default for AppSidecar {
    fn default() -> Self {
        Self {
            retention_class: default_retention(),
            capture_plugin_stderr: false,
        }
    }
}

/// Environment allowlist — names only, never values.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default)]
pub struct AppEnv {
    /// Parent-env variable names the host may forward.
    #[serde(default)]
    pub allowlist: Vec<String>,
}

/// Schema path references relative to the app directory.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default)]
pub struct AppSchemas {
    /// Input schema reference (relative path).
    #[serde(default)]
    pub input: Option<String>,
    /// Output schema reference (relative path).
    #[serde(default)]
    pub output: Option<String>,
}

/// Conformance binding without shell command strings.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default)]
pub struct AppConformance {
    /// Cargo package name to test (`cargo test -p <name>`).
    #[serde(default)]
    pub cargo_package: Option<String>,
}

impl AppManifest {
    /// Parse a manifest TOML document.
    pub fn parse(text: &str) -> Result<Self, String> {
        toml::from_str::<Self>(text).map_err(|e| format!("manifest parse error: {e}"))
    }

    /// Load and parse a manifest file.
    pub fn load(path: &Path) -> Result<Self, String> {
        let text =
            std::fs::read_to_string(path).map_err(|e| format!("manifest read error: {e}"))?;
        Self::parse(&text)
    }

    /// Structural validation independent of toolchain pin.
    pub fn validate_structure(&self) -> Result<(), String> {
        if self.schema_version != APP_MANIFEST_SCHEMA_VERSION {
            return Err(format!(
                "unsupported schema_version: {} (expected {APP_MANIFEST_SCHEMA_VERSION})",
                self.schema_version
            ));
        }
        validate_app_name(&self.name)?;
        if self.protocol_version.trim().is_empty() {
            return Err("protocol_version is empty".into());
        }
        validate_logical_name(&self.plugin.logical_name)?;
        if let Some(hash) = &self.plugin.sha256 {
            validate_sha256_hex(hash, "plugin.sha256")?;
        }
        if let Some(ms) = self.limits.run_timeout_ms {
            agentmesh_proto::Limits::validate_run_timeout_ms(ms)
                .map_err(|e| format!("limits.run_timeout_ms: {e}"))?;
        }
        validate_retention(&self.sidecar.retention_class)?;
        for key in &self.env.allowlist {
            validate_env_name(key)?;
        }
        if let Some(path) = &self.schemas.input {
            validate_relative_schema_ref(path, "schemas.input")?;
        }
        if let Some(path) = &self.schemas.output {
            validate_relative_schema_ref(path, "schemas.output")?;
        }
        if let Some(pkg) = &self.conformance.cargo_package {
            validate_cargo_package(pkg)?;
        }
        Ok(())
    }
}

fn validate_app_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.len() > 64 {
        return Err("name must be 1..=64 characters".into());
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
    {
        return Err("name must be lowercase ASCII letters/digits/_/-".into());
    }
    Ok(())
}

fn validate_logical_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.len() > 128 {
        return Err("plugin.logical_name must be 1..=128 characters".into());
    }
    if name.contains('/') || name.contains('\\') || name.contains(':') {
        return Err("plugin.logical_name must not contain path separators".into());
    }
    if name.contains("..") {
        return Err("plugin.logical_name must not contain '..'".into());
    }
    // Reject shell-looking tokens.
    for needle in ['$', '`', '|', ';', '&', '<', '>', '(', ')', '{', '}'] {
        if name.contains(needle) {
            return Err("plugin.logical_name contains forbidden shell metacharacters".into());
        }
    }
    if Path::new(name)
        .components()
        .any(|c| matches!(c, Component::ParentDir))
    {
        return Err("plugin.logical_name path escape rejected".into());
    }
    Ok(())
}

pub(crate) fn validate_sha256_hex(value: &str, field: &str) -> Result<(), String> {
    let lower = value.to_ascii_lowercase();
    if lower.len() != 64 || !lower.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("{field} must be 64 lowercase hex characters"));
    }
    if value.chars().any(|c| c.is_ascii_uppercase()) {
        return Err(format!("{field} must be lowercase hex"));
    }
    Ok(())
}

fn validate_retention(value: &str) -> Result<(), String> {
    match value {
        "owner_local" | "ephemeral" | "none" => Ok(()),
        other => Err(format!("sidecar.retention_class unsupported: {other}")),
    }
}

fn validate_env_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("env.allowlist entry is empty".into());
    }
    if name.contains('=') {
        return Err(format!(
            "env.allowlist must be names only (got value-like entry {name:?})"
        ));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
    {
        return Err(format!(
            "env.allowlist entry must be UPPER_SNAKE_CASE name: {name}"
        ));
    }
    // Reject common secret *values* mistaken as names is already handled by '='.
    // Reject names that embed inline values / URLs.
    if name.contains("://") || name.contains('/') || name.contains('\\') {
        return Err(format!("env.allowlist rejects path/url-like entry: {name}"));
    }
    Ok(())
}

fn validate_relative_schema_ref(path: &str, field: &str) -> Result<(), String> {
    if path.is_empty() {
        return Err(format!("{field} is empty"));
    }
    if Path::new(path).is_absolute() {
        return Err(format!("{field} must be a relative path"));
    }
    for component in Path::new(path).components() {
        match component {
            Component::Normal(_) | Component::CurDir => {}
            Component::ParentDir => {
                return Err(format!("{field} must not contain '..'"));
            }
            _ => return Err(format!("{field} has unsupported path component")),
        }
    }
    for needle in ['$', '`', '|', ';', '&'] {
        if path.contains(needle) {
            return Err(format!("{field} contains forbidden shell metacharacters"));
        }
    }
    Ok(())
}

fn validate_cargo_package(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("conformance.cargo_package is empty".into());
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err("conformance.cargo_package has invalid characters".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_manifest() {
        let m = AppManifest::parse(
            r#"
schema_version = "agentmesh-app.v0"
name = "backlog-promoter"
protocol_version = "2026-07-15"

[plugin]
logical_name = "agentmesh-example-plugin"
"#,
        )
        .unwrap();
        m.validate_structure().unwrap();
        assert_eq!(m.plugin.logical_name, "agentmesh-example-plugin");
    }

    #[test]
    fn rejects_plugin_path() {
        let m = AppManifest::parse(
            r#"
schema_version = "agentmesh-app.v0"
name = "backlog-promoter"
protocol_version = "2026-07-15"
[plugin]
logical_name = "../evil"
"#,
        )
        .unwrap();
        assert!(m.validate_structure().unwrap_err().contains("logical_name"));
    }

    #[test]
    fn rejects_env_value() {
        let m = AppManifest::parse(
            r#"
schema_version = "agentmesh-app.v0"
name = "backlog-promoter"
protocol_version = "2026-07-15"
[plugin]
logical_name = "agentmesh-example-plugin"
[env]
allowlist = ["TOKEN=secret"]
"#,
        )
        .unwrap();
        assert!(m.validate_structure().unwrap_err().contains("names only"));
    }
}
