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
| Gareji Setup | first-run Core placement, explicit project registration, Codex Marketplace and Plugin installation | Core policy, direct SQLite access, silent Hook trust |
| Context consumer | dereferencing attributed context references with its existing access | Core storage, Board task authority |
| Knowledge projection Adapter | provider-specific Progress Checkpoint write-back when a real destination requires it | local path configuration, Core execution policy |
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

`gareji-core::registry::ProjectRegistry` persists explicit project-to-workspace registrations, user grants, configured context references, projection destination identities, and one optional active Work item reference per project. It does not own or transition Board Work item state. A context reference may be a local Markdown absolute path; Core returns it unchanged and does not read, index, watch, or normalize the referenced file.

### Local Core bridge

`gareji-core::bridge::CoreBridge` composes the Registry and Progress Recorder behind the versioned `gareji.core-bridge.v0` Interface. The `gareji-core bridge` command exposes it to one trusted parent process over bounded newline-delimited JSON. Transports reuse one child process and never access Core SQLite tables directly. Board-facing history uses a bounded, project-scoped `list_progress` operation; Core remains the checkpoint-ledger authority while Board owns presentation and reconciliation.

### Board active-work Adapter

`gareji-core::board::BoardPort` asks Board for one attributed active-work assessment before the Registry changes its operational selection. The production process Adapter reuses `gareji-board bridge`; test Adapters exercise the same Interface. Core does not interpret Board Work item states and fails closed when Board is unavailable.

### Runner seam

`gareji-core::runner::RunExecutor` validates one bounded opaque request, enforces the Capability Gate, and invokes an injected `RunnerAdapter` only after approval. The Adapter receives only the sorted capabilities Core authorized and returns the same bounded Core Run report regardless of runtime. Invalid Adapter reports are rejected at the seam instead of reaching callers.

gareji-codex-runner is the first external Adapter. It translates one authorized request into non-interactive Codex JSONL and returns a compact report through the same Interface. A future runtime satisfies that Interface rather than adding provider fields to shared envelopes. Model selection, Work item state, workspace and worktree preparation, and runtime event parsing remain outside Core.

### Context references and optional projection

Local Markdown, GBrain, and LLMWiki do not require separate Adapters when their only integration input is an absolute file path. The path is operational configuration at the existing Registry Interface; trusted callers and external indexers dereference it themselves.

Projects that use Core as the sole Progress Checkpoint ledger register an empty `delivery_targets` list. The `CheckpointProjector` seam remains available but dormant until a destination has provider-specific write-back behavior such as authentication, format translation, idempotency, or conflict handling. Path labels alone do not justify an Adapter.

### First-run Setup Module

The standalone `gareji` CLI owns installation orchestration outside the trust kernel. Its `setup` operation validates absolute workspace and Markdown paths, validates the generated registration through the supplied Core binary, verifies the Stop Hook runtime, installs Core in local application data, registers through the public `gareji-core project` Interface, and delegates Marketplace and Plugin changes to the installed Codex CLI. Its `doctor` operation reports the same layers without changing them.

Setup is safe to rerun: matching binaries, registrations, Marketplace sources, and enabled Plugins are left unchanged. `--dry-run` invokes no mutating command. Setup never reads or writes Core SQLite directly and cannot approve or silently trust a lifecycle Hook.

Setup and the packaged Stop Hook consume one versioned platform-layout contract for the installed Core location, avoiding separate operating-system path rules in Rust and Python.

## Invariants

- Core remains usable without Gareji Cloud.
- Cloud requests cannot bypass local capability policy.
- Board Work item state never becomes an opaque Plugin payload owned by Core.
- Raw transcripts, full diffs, credentials, and unbounded logs do not enter Compact results or Progress Checkpoints.
- Every external side effect is attributable to a bounded Run and evidence reference.
- Current and planned behavior are labeled separately in documentation.
