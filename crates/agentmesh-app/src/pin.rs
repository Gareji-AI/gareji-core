//! Version-controlled toolchain pin (no machine-local install paths).

use crate::manifest::validate_sha256_hex;
use serde::Deserialize;
use std::path::Path;

/// Schema identity for toolchain pins.
pub const PIN_SCHEMA_VERSION: &str = "agentmesh-toolchain-pin.v0";

/// Supported release target triples for immutable prereleases.
pub const SUPPORTED_TARGETS: &[&str] = &[
    "x86_64-pc-windows-msvc",
    "x86_64-unknown-linux-gnu",
    "aarch64-apple-darwin",
];

/// Consumer pin recording exact prerelease identity.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ToolchainPin {
    /// Must be [`PIN_SCHEMA_VERSION`].
    pub schema_version: String,
    /// Prerelease tag, e.g. `v0.2.0-dev.1`.
    pub tag: String,
    /// Full 40-character commit SHA.
    pub commit_sha: String,
    /// Target triple.
    pub target: String,
    /// SHA-256 of the release manifest JSON for this tag/target.
    pub release_manifest_sha256: String,
    /// Optional previous known-good pin identity (tag only for v0 documentation).
    #[serde(default)]
    pub previous_tag: Option<String>,
}

impl ToolchainPin {
    /// Parse a pin TOML document.
    pub fn parse(text: &str) -> Result<Self, String> {
        // Reject machine-local install path keys early by inspecting the raw table.
        let value: toml::Value =
            toml::from_str(text).map_err(|e| format!("toolchain pin parse error: {e}"))?;
        if let Some(table) = value.as_table() {
            for forbidden in [
                "install_path",
                "cache_path",
                "local_path",
                "path",
                "plugin_path",
            ] {
                if table.contains_key(forbidden) {
                    return Err(format!(
                        "toolchain pin must not contain machine-local field `{forbidden}`"
                    ));
                }
            }
        }
        toml::from_str::<Self>(text).map_err(|e| format!("toolchain pin parse error: {e}"))
    }

    /// Load and parse a pin file.
    pub fn load(path: &Path) -> Result<Self, String> {
        let text =
            std::fs::read_to_string(path).map_err(|e| format!("toolchain pin read error: {e}"))?;
        Self::parse(&text)
    }

    /// Structural validation.
    pub fn validate_structure(&self) -> Result<(), String> {
        if self.schema_version != PIN_SCHEMA_VERSION {
            return Err(format!(
                "unsupported pin schema_version: {} (expected {PIN_SCHEMA_VERSION})",
                self.schema_version
            ));
        }
        validate_tag(&self.tag)?;
        validate_commit_sha(&self.commit_sha)?;
        if !SUPPORTED_TARGETS.contains(&self.target.as_str()) {
            return Err(format!(
                "unsupported target `{}` (supported: {})",
                self.target,
                SUPPORTED_TARGETS.join(", ")
            ));
        }
        validate_sha256_hex(&self.release_manifest_sha256, "release_manifest_sha256")?;
        if let Some(prev) = &self.previous_tag {
            validate_tag(prev)?;
        }
        Ok(())
    }
}

fn validate_tag(tag: &str) -> Result<(), String> {
    if tag.is_empty() || tag.len() > 64 {
        return Err("tag must be 1..=64 characters".into());
    }
    if !(tag.starts_with('v') || tag.starts_with("agentmesh-")) {
        return Err("tag must start with `v` or `agentmesh-`".into());
    }
    if tag.contains('/') || tag.contains('\\') || tag.contains("..") {
        return Err("tag must not contain path elements".into());
    }
    Ok(())
}

fn validate_commit_sha(sha: &str) -> Result<(), String> {
    let lower = sha.to_ascii_lowercase();
    if lower.len() != 40 || !lower.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("commit_sha must be full 40-character hex SHA".into());
    }
    if sha.chars().any(|c| c.is_ascii_uppercase()) {
        return Err("commit_sha must be lowercase hex".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_pin() {
        let pin = ToolchainPin::parse(
            r#"
schema_version = "agentmesh-toolchain-pin.v0"
tag = "v0.2.0-dev.1"
commit_sha = "376f849893654a8d2de868e79f0c5d0aefb4308c"
target = "x86_64-pc-windows-msvc"
release_manifest_sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
"#,
        )
        .unwrap();
        pin.validate_structure().unwrap();
    }

    #[test]
    fn rejects_install_path() {
        let err = ToolchainPin::parse(
            r#"
schema_version = "agentmesh-toolchain-pin.v0"
tag = "v0.2.0-dev.1"
commit_sha = "376f849893654a8d2de868e79f0c5d0aefb4308c"
target = "x86_64-pc-windows-msvc"
release_manifest_sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
install_path = "C:/Users/me/.agentmesh"
"#,
        )
        .unwrap_err();
        assert!(err.contains("install_path"));
    }
}
