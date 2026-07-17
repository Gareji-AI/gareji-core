# Capability Policy v0

Status: implemented in `gareji-core::policy`.

## Interface

```text
CapabilityGate::authorize(execution_intent) -> approved | rejected
```

An `ExecutionIntent` contains a stable `operation_id` and the capabilities the operation requests. It contains no caller-asserted approvals. `CapabilityGate` loads approvals bound to that operation through a trusted `ApprovalSource` and evaluates them against local `CapabilityPolicy`.

Capability names use lowercase ASCII letters, digits, `.`, `_`, or `-`, with a maximum of 128 bytes. Example names include `context.read`, `workspace.write`, and `production.write`.

## Decisions

- A requested capability absent from `allowed` produces `capability_not_allowed`.
- An allowed capability listed in `approval_required` produces `approval_required` unless the trusted source approves it for the exact operation.
- An unavailable Approval Source fails closed before Runner invocation.
- Reasons and approved capabilities are sorted for deterministic evidence.
- Approval-required capabilities must also be allowed; invalid policy configuration is rejected.

Board or a future Cloud Adapter may implement the Approval Source Interface, but the local Core gate makes the final decision. MCP and Runner never pass an `approved=true` shortcut.

See [the policy example](../examples/capability-policy-v0.json) and [execution intent example](../examples/execution-intent-v0.json).
