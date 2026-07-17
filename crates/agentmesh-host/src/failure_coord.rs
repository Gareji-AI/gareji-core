//! Deterministic primary/secondary failure precedence.

use agentmesh_proto::failure::{FailureRecord, SecondaryFailure};
use std::sync::Mutex;

/// Single compare-and-set transition for the first terminal cause.
#[derive(Debug, Default)]
pub struct FailureCoordinator {
    inner: Mutex<Inner>,
}

#[derive(Debug, Default)]
struct Inner {
    primary: Option<FailureRecord>,
    secondary: Vec<SecondaryFailure>,
}

impl FailureCoordinator {
    /// Create an empty coordinator.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a failure. First wins as primary; later become secondary.
    pub fn record(&self, failure: FailureRecord) {
        let mut g = self.inner.lock().expect("failure coord lock");
        if g.primary.is_none() {
            g.primary = Some(failure);
        } else {
            g.secondary.push(failure.into());
        }
    }

    /// Promote an audit failure to primary only when outcome was otherwise success.
    pub fn record_audit_failure(&self, failure: FailureRecord) {
        let mut g = self.inner.lock().expect("failure coord lock");
        if g.primary.is_none() {
            g.primary = Some(failure);
        } else {
            g.secondary.push(failure.into());
        }
    }

    /// Compact stdout delivery failure becomes final primary; previous primary moves secondary.
    pub fn record_stdout_failure(&self, failure: FailureRecord) {
        let mut g = self.inner.lock().expect("failure coord lock");
        if let Some(prev) = g.primary.take() {
            g.secondary.insert(0, prev.into());
        }
        g.primary = Some(failure);
    }

    /// Borrow primary failure.
    pub fn primary(&self) -> Option<FailureRecord> {
        self.inner.lock().expect("lock").primary.clone()
    }

    /// Clone secondary failures.
    pub fn secondary(&self) -> Vec<SecondaryFailure> {
        self.inner.lock().expect("lock").secondary.clone()
    }

    /// Whether any primary exists.
    pub fn has_primary(&self) -> bool {
        self.inner.lock().expect("lock").primary.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentmesh_proto::failure::FailureCode;

    #[test]
    fn first_failure_is_primary() {
        let c = FailureCoordinator::new();
        c.record(FailureRecord::new(FailureCode::RunTimeout, "t"));
        c.record(FailureRecord::new(FailureCode::PluginExited, "e"));
        assert_eq!(c.primary().unwrap().code, FailureCode::RunTimeout);
        assert_eq!(c.secondary().len(), 1);
        assert_eq!(c.secondary()[0].code, FailureCode::PluginExited);
    }

    #[test]
    fn stdout_failure_becomes_final_primary() {
        let c = FailureCoordinator::new();
        c.record(FailureRecord::new(FailureCode::SchemaViolation, "s"));
        c.record_stdout_failure(FailureRecord::new(FailureCode::StdoutWriteFailed, "pipe"));
        assert_eq!(c.primary().unwrap().code, FailureCode::StdoutWriteFailed);
        assert_eq!(c.secondary()[0].code, FailureCode::SchemaViolation);
    }

    #[test]
    fn audit_after_plugin_failure_is_secondary() {
        let c = FailureCoordinator::new();
        c.record(FailureRecord::new(FailureCode::PluginExited, "p"));
        c.record_audit_failure(FailureRecord::new(FailureCode::SidecarWriteFailed, "disk"));
        assert_eq!(c.primary().unwrap().code, FailureCode::PluginExited);
        assert_eq!(c.secondary()[0].code, FailureCode::SidecarWriteFailed);
    }
}
