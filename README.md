# Gareji Core

Gareji Core is the local trust kernel for bounded, auditable agent execution. Its implemented Phase 0 foundation runs trusted local plugins through a strict Host-to-Plugin contract; policy enforcement, durable progress recording, and Runner Adapters are added behind separate Interfaces without moving Board-specific task semantics into Core.

The current Rust crates retain the `agentmesh-*` package names while the public Gareji Core identity is established. Renaming is a separate mechanical migration and must not be mixed with behavior changes.

## What it provides

- JSON-RPC 2.0 over LSP-style `Content-Length` framing
- a one-shot `initialize → run → close` plugin lifecycle
- compact machine-readable stdout and bounded local audit sidecars
- deterministic failure categories, strict input validation, and structured redaction
- deterministic Capability Policy evaluation through a trusted Approval Source
- durable, idempotent Progress Checkpoint recording in SQLite
- independent projection delivery, partial success, conflict detection, and retry

Plugins are trusted, absolute native executables. Gareji Core does not sandbox plugins, discover plugins from a registry, or support remote plugins.

## Local trust-kernel Modules

- **Implemented:** capability and approval policy evaluated before a Run.
- **Implemented:** Progress Recorder backed by SQLite with per-destination deliveries.
- **Next:** a stable Runner seam for Codex and future execution Adapters.
- **Later:** optional Cloud synchronization that cannot bypass local policy.

See [the architecture](docs/architecture.md), [Capability Policy v0](docs/capability-policy-v0.md), and [Progress Recorder v0](docs/progress-recorder-v0.md) for current and planned behavior.

## Boundaries

Gareji Core intentionally contains no Board-, tracker-, or knowledge-provider-specific task model. Domain payloads remain owned by callers and plugins. Gareji Board owns Work item state and portfolio policy; Runner Adapters own runtime translation. Do not add credentials, real service data, personal paths, or audit sidecars to this repository.

## Development

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

For a local roundtrip:

```bash
cargo build --release -p agentmesh-cli -p agentmesh-fixture-echo
./target/release/agentmesh run \
  --plugin "$(pwd)/target/release/agentmesh-fixture-echo" \
  --input ./examples/echo-input.json \
  --sidecar-dir ./.agentmesh/runs
```

## License

Licensed under [Apache License 2.0](LICENSE).

See [CONTRIBUTING.md](CONTRIBUTING.md), [SECURITY.md](SECURITY.md), and the [domain glossary](CONTEXT.md) before proposing changes.
