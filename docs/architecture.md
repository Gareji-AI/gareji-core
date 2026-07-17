# Gareji Core architecture

## Purpose

Gareji Core is a local trust kernel with a small set of stable Interfaces. It concentrates execution validation, policy enforcement, bounded results, evidence, and durable progress intake so Board, MCP, Hooks, and Runner implementations do not reproduce those rules.

## Responsibility map

| Product or Module | Owns | Does not own |
|---|---|---|
| Gareji Core | trusted execution envelopes, capability enforcement, compact results, evidence references, generic Progress Recorder behavior | Work item state, portfolio priority, provider-specific prompts |
| Gareji Board | Work item state, priority, approvals, portfolio policy, human control surface | Plugin process supervision, runtime event parsing |
| Runner Adapter | one runtime invocation, model/runtime configuration, worktree or managed environment setup | durable task truth, cross-project scheduling policy |
| Gareji MCP | Codex-facing transport and permission-scoped tool translation | duplicated Core state or business rules |
| Knowledge Adapter | sourced context reads and Progress Checkpoint projections | Board task authority, Core execution policy |
| Gareji Cloud | optional identity, synchronization, team administration, managed capacity | bypassing local Core policy, default source-code ownership |

## Implemented Phase 0

The existing Host Module exposes a strict one-shot Plugin lifecycle over framed JSON-RPC on stdio. It bounds input and output, clears inherited environment by default, classifies failures, emits one Compact result, and writes a bounded local Audit sidecar.

This Host Interface remains independently testable and does not learn Board or provider payloads.

## Implemented local-control foundation

### Capability gate

`gareji-core::policy::CapabilityGate` accepts a bounded execution intent and returns an approved capability set or structured rejection. Required approvals are loaded through a trusted `ApprovalSource`; Runner and MCP callers cannot assert their own approval set. An unavailable Approval Source fails closed.

### Progress Recorder

`gareji-core::progress::ProgressRecorder` validates and stores an immutable Progress Checkpoint in SQLite before projection. It provides idempotent `record`, `status`, and explicit `sync_pending` operations, with one delivery row per destination. Same-ID different-content records conflict, partial success is preserved, failed destinations retry, and conflict destinations require review.

The in-memory SQLite constructor is the local-substitutable test surface for the same implementation used with an application-data file.

### Project and active-work registry

`gareji-core::registry::ProjectRegistry` persists explicit project-to-workspace registrations, user grants, configured context references, projection destination identities, and one optional active Work item reference per project. It does not own or transition Board Work item state.

### Local Core bridge

`gareji-core::bridge::CoreBridge` composes the Registry and Progress Recorder behind the versioned `gareji.core-bridge.v0` Interface. The `gareji-core bridge` command exposes it to one trusted parent process over bounded newline-delimited JSON. Transports reuse one child process and never access Core SQLite tables directly. Board-facing history uses a bounded, project-scoped `list_progress` operation; Core remains the checkpoint-ledger authority while Board owns presentation and reconciliation.

### Board active-work Adapter

`gareji-core::board::BoardPort` asks Board for one attributed active-work assessment before the Registry changes its operational selection. The production process Adapter reuses `gareji-board bridge`; test Adapters exercise the same Interface. Core does not interpret Board Work item states and fails closed when Board is unavailable.

## Planned internal Modules

### Runner seam

Codex Runner is the first Adapter. A future runtime must satisfy the same execution Interface and return the same Core Run result rather than adding provider fields to shared envelopes.

## Invariants

- Core remains usable without Gareji Cloud.
- Cloud requests cannot bypass local capability policy.
- Board Work item state never becomes an opaque Plugin payload owned by Core.
- Raw transcripts, full diffs, credentials, and unbounded logs do not enter Compact results or Progress Checkpoints.
- Every external side effect is attributable to a bounded Run and evidence reference.
- Current and planned behavior are labeled separately in documentation.
