//! Deterministic local capability-policy evaluation.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;

const MAX_ID_LEN: usize = 128;

/// Locally configured capabilities and their approval requirements.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityPolicy {
    /// Capabilities this installation may execute at all.
    pub allowed: BTreeSet<String>,
    /// Allowed capabilities that also need explicit approval evidence.
    pub approval_required: BTreeSet<String>,
}

impl CapabilityPolicy {
    /// Build and validate a policy.
    pub fn new(
        allowed: impl IntoIterator<Item = String>,
        approval_required: impl IntoIterator<Item = String>,
    ) -> Result<Self, PolicyInputError> {
        let policy = Self {
            allowed: allowed.into_iter().collect(),
            approval_required: approval_required.into_iter().collect(),
        };
        policy.validate()?;
        Ok(policy)
    }

    fn validate(&self) -> Result<(), PolicyInputError> {
        validate_capabilities(&self.allowed)?;
        validate_capabilities(&self.approval_required)?;

        if let Some(capability) = self.approval_required.difference(&self.allowed).next() {
            return Err(PolicyInputError::ApprovalForDisallowedCapability {
                capability: capability.clone(),
            });
        }
        Ok(())
    }
}

/// One bounded operation Core is being asked to authorize.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionIntent {
    /// Stable caller-provided operation identity used by evidence and approvals.
    pub operation_id: String,
    /// Capabilities the operation will exercise.
    pub requested: BTreeSet<String>,
}

impl ExecutionIntent {
    /// Validate an intent received from an external caller.
    pub fn validate(&self) -> Result<(), PolicyInputError> {
        validate_id("operation_id", &self.operation_id)?;
        validate_capabilities(&self.requested)?;
        Ok(())
    }
}

/// Trusted source of approvals bound to one operation identity.
pub trait ApprovalSource {
    /// Return capabilities explicitly approved for this operation.
    fn approved_capabilities(
        &self,
        operation_id: &str,
    ) -> Result<BTreeSet<String>, ApprovalSourceError>;
}

/// Local capability gate invoked before a Runner.
pub struct CapabilityGate<S> {
    policy: CapabilityPolicy,
    approvals: S,
}

impl<S: ApprovalSource> CapabilityGate<S> {
    /// Create a gate from validated policy and a trusted approval source.
    pub fn new(policy: CapabilityPolicy, approvals: S) -> Self {
        Self { policy, approvals }
    }

    /// Authorize one execution intent without accepting caller-asserted approvals.
    pub fn authorize(&self, intent: &ExecutionIntent) -> Result<PolicyDecision, GateError> {
        self.policy.validate()?;
        intent.validate()?;

        let requires_approval = intent
            .requested
            .iter()
            .any(|capability| self.policy.approval_required.contains(capability));
        let approved = if requires_approval {
            let approved = self.approvals.approved_capabilities(&intent.operation_id)?;
            validate_capabilities(&approved)?;
            approved
        } else {
            BTreeSet::new()
        };
        Ok(evaluate_verified(&self.policy, intent, &approved))
    }
}

/// Deterministic local authorization result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum PolicyDecision {
    /// Every requested capability is locally allowed and approved when required.
    Approved {
        /// Sorted capabilities authorized for this operation.
        capabilities: Vec<String>,
    },
    /// Core rejected the operation before invoking a Runner.
    Rejected {
        /// Sorted stable rejection reasons.
        reasons: Vec<PolicyRejection>,
    },
}

/// A stable reason why Core rejected an execution intent.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum PolicyRejection {
    /// The installation does not permit this capability.
    CapabilityNotAllowed { capability: String },
    /// The capability is allowed only with explicit approval evidence.
    ApprovalRequired { capability: String },
}

/// Invalid policy or intent input.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PolicyInputError {
    /// A stable identifier failed validation.
    #[error("invalid {field}: {message}")]
    InvalidId {
        field: &'static str,
        message: &'static str,
    },
    /// A capability name failed validation.
    #[error("invalid capability `{capability}`")]
    InvalidCapability { capability: String },
    /// A policy requested approval for a capability it does not allow.
    #[error("approval required for disallowed capability `{capability}`")]
    ApprovalForDisallowedCapability { capability: String },
}

/// Approval lookup failure with a bounded client-safe code.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("approval source unavailable: {code}")]
pub struct ApprovalSourceError {
    /// Stable adapter-defined category without raw external details.
    pub code: String,
}

impl ApprovalSourceError {
    /// Create a bounded source error.
    pub fn new(code: impl Into<String>) -> Self {
        let code: String = code.into().chars().take(MAX_ID_LEN).collect();
        Self { code }
    }
}

/// Capability-gate failure before a policy decision can be produced.
#[derive(Debug, Error)]
pub enum GateError {
    /// Policy or intent input was invalid.
    #[error(transparent)]
    InvalidInput(#[from] PolicyInputError),
    /// Trusted approval state could not be read.
    #[error(transparent)]
    ApprovalSource(#[from] ApprovalSourceError),
}

fn evaluate_verified(
    policy: &CapabilityPolicy,
    intent: &ExecutionIntent,
    approved: &BTreeSet<String>,
) -> PolicyDecision {
    let mut reasons = Vec::new();
    for capability in &intent.requested {
        if !policy.allowed.contains(capability) {
            reasons.push(PolicyRejection::CapabilityNotAllowed {
                capability: capability.clone(),
            });
        } else if policy.approval_required.contains(capability) && !approved.contains(capability) {
            reasons.push(PolicyRejection::ApprovalRequired {
                capability: capability.clone(),
            });
        }
    }

    if reasons.is_empty() {
        PolicyDecision::Approved {
            capabilities: intent.requested.iter().cloned().collect(),
        }
    } else {
        reasons.sort();
        PolicyDecision::Rejected { reasons }
    }
}

fn validate_capabilities(capabilities: &BTreeSet<String>) -> Result<(), PolicyInputError> {
    for capability in capabilities {
        if capability.is_empty()
            || capability.len() > MAX_ID_LEN
            || !capability.bytes().all(|byte| {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || matches!(byte, b'.' | b'_' | b'-')
            })
        {
            return Err(PolicyInputError::InvalidCapability {
                capability: capability.clone(),
            });
        }
    }
    Ok(())
}

fn validate_id(field: &'static str, value: &str) -> Result<(), PolicyInputError> {
    if value.is_empty() {
        return Err(PolicyInputError::InvalidId {
            field,
            message: "must not be empty",
        });
    }
    if value.chars().count() > MAX_ID_LEN {
        return Err(PolicyInputError::InvalidId {
            field,
            message: "exceeds 128 characters",
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(values: &[&str]) -> BTreeSet<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    struct StaticApprovals {
        approved: BTreeSet<String>,
        fail: bool,
    }

    impl ApprovalSource for StaticApprovals {
        fn approved_capabilities(
            &self,
            _operation_id: &str,
        ) -> Result<BTreeSet<String>, ApprovalSourceError> {
            if self.fail {
                Err(ApprovalSourceError::new("board_unavailable"))
            } else {
                Ok(self.approved.clone())
            }
        }
    }

    fn gate(policy: CapabilityPolicy, approved: &[&str]) -> CapabilityGate<StaticApprovals> {
        CapabilityGate::new(
            policy,
            StaticApprovals {
                approved: set(approved),
                fail: false,
            },
        )
    }

    #[test]
    fn approves_allowed_capabilities_without_required_approval() {
        let policy = CapabilityPolicy::new(set(&["context.read"]), set(&[])).unwrap();
        let intent = ExecutionIntent {
            operation_id: "run-1".to_owned(),
            requested: set(&["context.read"]),
        };

        assert_eq!(
            gate(policy, &[]).authorize(&intent).unwrap(),
            PolicyDecision::Approved {
                capabilities: vec!["context.read".to_owned()]
            }
        );
    }

    #[test]
    fn rejects_missing_approval_before_runner_invocation() {
        let policy = CapabilityPolicy::new(
            set(&["context.read", "production.write"]),
            set(&["production.write"]),
        )
        .unwrap();
        let intent = ExecutionIntent {
            operation_id: "run-2".to_owned(),
            requested: set(&["production.write"]),
        };

        assert_eq!(
            gate(policy, &[]).authorize(&intent).unwrap(),
            PolicyDecision::Rejected {
                reasons: vec![PolicyRejection::ApprovalRequired {
                    capability: "production.write".to_owned()
                }]
            }
        );
    }

    #[test]
    fn rejects_disallowed_and_unapproved_capabilities_deterministically() {
        let policy =
            CapabilityPolicy::new(set(&["production.write"]), set(&["production.write"])).unwrap();
        let intent = ExecutionIntent {
            operation_id: "run-3".to_owned(),
            requested: set(&["filesystem.delete", "production.write"]),
        };

        assert_eq!(
            gate(policy, &[]).authorize(&intent).unwrap(),
            PolicyDecision::Rejected {
                reasons: vec![
                    PolicyRejection::CapabilityNotAllowed {
                        capability: "filesystem.delete".to_owned()
                    },
                    PolicyRejection::ApprovalRequired {
                        capability: "production.write".to_owned()
                    }
                ]
            }
        );
    }

    #[test]
    fn accepts_exact_approval_for_risky_capability() {
        let policy =
            CapabilityPolicy::new(set(&["production.write"]), set(&["production.write"])).unwrap();
        let intent = ExecutionIntent {
            operation_id: "run-4".to_owned(),
            requested: set(&["production.write"]),
        };

        assert!(matches!(
            gate(policy, &["production.write"])
                .authorize(&intent)
                .unwrap(),
            PolicyDecision::Approved { .. }
        ));
    }

    #[test]
    fn fails_closed_when_trusted_approval_source_is_unavailable() {
        let policy =
            CapabilityPolicy::new(set(&["production.write"]), set(&["production.write"])).unwrap();
        let gate = CapabilityGate::new(
            policy,
            StaticApprovals {
                approved: set(&["production.write"]),
                fail: true,
            },
        );
        let intent = ExecutionIntent {
            operation_id: "run-5".to_owned(),
            requested: set(&["production.write"]),
        };

        assert!(matches!(
            gate.authorize(&intent),
            Err(GateError::ApprovalSource(_))
        ));
    }

    #[test]
    fn rejects_invalid_policy_configuration() {
        let error = CapabilityPolicy::new(set(&[]), set(&["production.write"])).unwrap_err();
        assert_eq!(
            error,
            PolicyInputError::ApprovalForDisallowedCapability {
                capability: "production.write".to_owned()
            }
        );
    }

    #[test]
    fn examples_deserialize_without_caller_asserted_approval() {
        let policy: CapabilityPolicy = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../examples/capability-policy-v0.json"
        )))
        .unwrap();
        let intent: ExecutionIntent = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../examples/execution-intent-v0.json"
        )))
        .unwrap();

        assert!(matches!(
            gate(policy, &[]).authorize(&intent).unwrap(),
            PolicyDecision::Approved { .. }
        ));
    }
}
