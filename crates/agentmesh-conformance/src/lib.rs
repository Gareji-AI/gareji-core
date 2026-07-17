//! Internal conformance helpers and workspace boundary checks.

use std::path::PathBuf;
use std::process::Command;

/// Resolve a built fixture binary from `CARGO_TARGET_DIR` / relative target.
pub fn fixture_bin(name: &str) -> PathBuf {
    let exe = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    };
    let mut candidates = Vec::new();
    if let Ok(dir) = std::env::var("CARGO_TARGET_DIR") {
        candidates.push(PathBuf::from(dir).join("debug").join(&exe));
        candidates.push(
            PathBuf::from(std::env::var("CARGO_TARGET_DIR").unwrap())
                .join("release")
                .join(&exe),
        );
    }
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    candidates.push(manifest_dir.join("../../target/debug").join(&exe));
    candidates.push(manifest_dir.join("../../target/release").join(&exe));
    candidates
        .into_iter()
        .find(|p| p.exists())
        .unwrap_or_else(|| PathBuf::from(&exe))
}

/// Run `cargo metadata` and assert fixture-support does not depend on host/conformance.
pub fn assert_fixture_dependency_boundary() {
    let output = Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .current_dir(workspace_root())
        .output()
        .expect("cargo metadata");
    assert!(output.status.success(), "cargo metadata failed");
    let meta: serde_json::Value = serde_json::from_slice(&output.stdout).expect("json");
    let packages = meta["packages"].as_array().expect("packages");
    for pkg in packages {
        let name = pkg["name"].as_str().unwrap_or("");
        let is_plugin_side =
            name == "agentmesh-fixture-support" || name.starts_with("agentmesh-fixture-");
        if is_plugin_side {
            let deps = pkg["dependencies"].as_array().cloned().unwrap_or_default();
            for dep in deps {
                let dep_name = dep["name"].as_str().unwrap_or("");
                assert_ne!(
                    dep_name, "agentmesh-host",
                    "{name} must not depend on agentmesh-host"
                );
                assert_ne!(
                    dep_name, "agentmesh-conformance",
                    "{name} must not depend on agentmesh-conformance"
                );
            }
        }
    }
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentmesh_host::execute_run_with;
    use agentmesh_host::lifecycle::RunConfig;
    use agentmesh_host::sidecar::VecCompactSink;
    use agentmesh_host::{CancellationToken, FsAuditStore};
    use agentmesh_proto::failure::FailureCode;
    use agentmesh_proto::{CompactOutcome, Limits};
    use std::time::Duration;

    #[test]
    fn fixture_crates_do_not_depend_on_host_or_conformance() {
        assert_fixture_dependency_boundary();
    }

    fn abs_fixture(name: &str) -> PathBuf {
        let p = fixture_bin(name);
        std::fs::canonicalize(&p).unwrap_or(p)
    }

    #[tokio::test]
    async fn echo_roundtrip_success() {
        let plugin = abs_fixture("agentmesh-fixture-echo");
        if !plugin.exists() {
            eprintln!("skip: fixture not built at {}", plugin.display());
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let mut sink = VecCompactSink::default();
        let mut limits = Limits::default();
        limits.run_timeout_ms = 5_000;
        let outcome = execute_run_with(
            RunConfig {
                plugin,
                input: br#"{"hello":"world"}"#.to_vec(),
                sidecar_dir: dir.path().to_path_buf(),
                plugin_env_keys: vec![],
                redact_pointers: vec![],
                capture_plugin_stderr: false,
                limits,
                run_id: Some("test-echo-1".into()),
            },
            &FsAuditStore,
            &mut sink,
            CancellationToken::new(),
        )
        .await;
        assert_eq!(
            outcome.exit_code,
            0,
            "stdout={}",
            String::from_utf8_lossy(&sink.bytes)
        );
        assert_eq!(outcome.envelope.outcome, CompactOutcome::Ok);
        assert!(outcome.sidecar_path.is_some());
    }

    #[tokio::test]
    async fn bad_json_maps_to_invalid_json_or_schema() {
        let plugin = abs_fixture("agentmesh-fixture-bad-json");
        if !plugin.exists() {
            eprintln!("skip: fixture not built");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let mut sink = VecCompactSink::default();
        let mut limits = Limits::default();
        limits.initialize_timeout_ms = 3_000;
        let outcome = execute_run_with(
            RunConfig {
                plugin,
                input: br#"{}"#.to_vec(),
                sidecar_dir: dir.path().to_path_buf(),
                plugin_env_keys: vec![],
                redact_pointers: vec![],
                capture_plugin_stderr: false,
                limits,
                run_id: Some("test-bad-json".into()),
            },
            &FsAuditStore,
            &mut sink,
            CancellationToken::new(),
        )
        .await;
        assert_ne!(outcome.exit_code, 0);
        let code = outcome.envelope.diagnostics[0].code.unwrap();
        assert!(
            matches!(
                code,
                FailureCode::InvalidJson
                    | FailureCode::SchemaViolation
                    | FailureCode::RpcIdMismatch
            ),
            "got {code}"
        );
    }

    #[tokio::test]
    async fn sleep_fixture_times_out_while_draining_stderr() {
        let plugin = abs_fixture("agentmesh-fixture-sleep");
        if !plugin.exists() {
            eprintln!("skip: fixture not built");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let mut sink = VecCompactSink::default();
        let mut limits = Limits::default();
        limits.initialize_timeout_ms = 5_000;
        limits.run_timeout_ms = 1_000;
        limits.exit_grace_ms = 500;
        let started = std::time::Instant::now();
        let outcome = execute_run_with(
            RunConfig {
                plugin,
                input: br#"{}"#.to_vec(),
                sidecar_dir: dir.path().to_path_buf(),
                plugin_env_keys: vec![],
                redact_pointers: vec![],
                capture_plugin_stderr: false,
                limits,
                run_id: Some("test-sleep".into()),
            },
            &FsAuditStore,
            &mut sink,
            CancellationToken::new(),
        )
        .await;
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "deadlock suspected"
        );
        assert_eq!(
            outcome.envelope.diagnostics[0].code,
            Some(FailureCode::RunTimeout)
        );
    }

    #[tokio::test]
    async fn stderr_success_does_not_fail_run() {
        let plugin = abs_fixture("agentmesh-fixture-stderr-success");
        if !plugin.exists() {
            eprintln!("skip: fixture not built");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let mut sink = VecCompactSink::default();
        let outcome = execute_run_with(
            RunConfig {
                plugin,
                input: br#"{"x":1}"#.to_vec(),
                sidecar_dir: dir.path().to_path_buf(),
                plugin_env_keys: vec![],
                redact_pointers: vec![],
                capture_plugin_stderr: false,
                limits: Limits::default(),
                run_id: Some("test-stderr-ok".into()),
            },
            &FsAuditStore,
            &mut sink,
            CancellationToken::new(),
        )
        .await;
        assert_eq!(outcome.exit_code, 0);
        let text = String::from_utf8_lossy(&sink.bytes);
        assert!(!text.contains("SHOULD_NOT_LEAK_TO_TERMINAL"));
    }

    #[test]
    fn path_helper_smoke() {
        let _ = fixture_bin("agentmesh-fixture-echo");
    }
}
