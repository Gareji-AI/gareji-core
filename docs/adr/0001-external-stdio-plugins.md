# ADR 0001: External stdio plugins

- Status: **Accepted**
- Date: 2026-07-15
- Accepted-at: Phase 0 exit review (2026-07-15)
- Deciders: Gareji Core maintainers

## Context

AgentMesh must keep tracker/tool-specific behavior out of core while preserving crash isolation and language freedom. In-process dynamic libraries couple ABI/stability risks to the host.

## Decision

Communicate with plugins as **external OS processes** over **strict JSON-RPC 2.0** framed with LSP-style `Content-Length` headers on stdio. Phase 0 accepts only absolute native executable paths. The host owns lifecycle/audit envelopes; plugin payloads remain opaque.

## Consequences

- Clear crash isolation and language-neutral design path
- Requires process supervision, framing, timeouts, and audit persistence in host
- Phase 0 proves Rust fixtures only; polyglot evidence and protocol 1.0 remain later gates

## Acceptance evidence

- CI: `x86_64-pc-windows-msvc`, `x86_64-unknown-linux-gnu`, `aarch64-apple-darwin`, and security checks
- Artifact packaging and downloaded CLI ↔ echo plugin smoke
- Local: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`
- Local release smoke: `agentmesh run` ↔ `agentmesh-fixture-echo` ⇒ `outcome=ok`, bounded sidecar under `.agentmesh/runs/`

Deferred hardening explicitly does **not** reopen this ADR: quantitative coverage gate, full failure golden matrix, dedicated application-error/exit-nonzero conformance wiring, and Ctrl-C system tests.
