# Gareji Core

Gareji Core is the local trust kernel for bounded, auditable agent execution. Its implemented Phase 0 foundation runs trusted local plugins through a strict Host-to-Plugin contract; policy enforcement, durable progress recording, and Runner Adapters are added behind separate Interfaces without moving Board-specific task semantics into Core.

The current Rust crates retain the `agentmesh-*` package names while the public Gareji Core identity is established. Renaming is a separate mechanical migration and must not be mixed with behavior changes.

## OpenAI Build Week

Gareji Core is one of three public repositories in the Gareji submission, together with [Gareji Board](https://github.com/Gareji-AI/gareji-board) and [Gareji MCP](https://github.com/Gareji-AI/gareji-mcp). Codex was used throughout the architecture work, Rust implementation, tests, release packaging, and review. GPT-5.6 was used to reason across repository boundaries and to implement and review the capability policy, Progress Recorder, Runner seam, and the safe first-run setup and diagnostics flow.

The submission uses Codex directly; it does not require an OpenAI API integration or API credits. The [OpenAI Build Week demo guide](docs/openai-build-week-demo.md) documents the bounded working demonstration and the evidence that judges can reproduce.

## What it provides

- JSON-RPC 2.0 over LSP-style `Content-Length` framing
- a one-shot `initialize → run → close` plugin lifecycle
- compact machine-readable stdout and bounded local audit sidecars
- deterministic failure categories, strict input validation, and structured redaction
- deterministic Capability Policy evaluation through a trusted Approval Source
- durable, idempotent Progress Checkpoint recording in SQLite
- independent projection delivery, partial success, conflict detection, and retry
- explicit project registration and per-project active-work references
- attributed context references returned without Core dereferencing local files
- a bounded local Core bridge reusable by MCP, Runner, CLI, and trusted Hooks
- an idempotent `gareji setup` flow for Core installation, project registration, and Codex Plugin installation

Plugins are trusted, absolute native executables. Gareji Core does not sandbox plugins, discover plugins from a registry, or support remote plugins.

## Local trust-kernel Modules

- **Implemented:** capability and approval policy evaluated before a Run.
- **Implemented:** Progress Recorder backed by SQLite with per-destination deliveries.
- **Implemented:** bounded project and Work item progress history through the reusable Core bridge.
- **Implemented:** fail-closed Board assessment before active Work item selection.
- **Implemented:** a policy-enforcing, runtime-neutral Runner seam with bounded requests and reports.
- **Implemented:** the first external Runner Adapter using non-interactive Codex JSONL.
- **Implemented:** a standalone first-run Setup Module with `setup`, `doctor`, and dry-run behavior.
- **Later:** optional Cloud synchronization that cannot bypass local policy.

See [the architecture](docs/architecture.md), [First-run setup v0](docs/setup-v0.md), [Gareji CLI presentation language v0](docs/ui-language-v0.md), [Capability Policy v0](docs/capability-policy-v0.md), [Runner execution v0](docs/runner-execution-v0.md), [Codex Runner Adapter v0](docs/codex-runner-adapter-v0.md), [Progress Recorder v0](docs/progress-recorder-v0.md), and [Core bridge v0](docs/core-bridge-v0.md) for current and planned behavior. The [OpenAI Build Week demo](docs/openai-build-week-demo.md) gives the bounded three-minute recording path.

## First-run setup

Release artifacts contain the two user-facing binaries, the local Marketplace, and the Gareji Progress Plugin in one directory. The Stop Hook requires `python` on Windows or `python3` on Linux and macOS; Setup and Doctor verify it. Extract the archive for your platform, inspect the plan, and then apply it:

```powershell
.\gareji setup --workspace "C:\absolute\path\to\project" --context "C:\absolute\path\to\project\PROJECT.md" --dry-run
.\gareji setup --workspace "C:\absolute\path\to\project" --context "C:\absolute\path\to\project\PROJECT.md"
.\gareji status --workspace "C:\absolute\path\to\project" --limit 3
.\gareji doctor --project-id project
```

For a source checkout, build the same layout locally:

```powershell
cargo build -p gareji-core-cli -p gareji-bootstrap
target\debug\gareji setup --workspace "C:\absolute\path\to\project" --context "C:\absolute\path\to\project\PROJECT.md" --dry-run
target\debug\gareji setup --workspace "C:\absolute\path\to\project" --context "C:\absolute\path\to\project\PROJECT.md"
target\debug\gareji status --workspace "C:\absolute\path\to\project" --limit 3
target\debug\gareji doctor --project-id project
```

Setup installs Core in the local application-data directory, registers reference-only Markdown context with no projection targets, adds the Gareji local Codex marketplace, and installs the Gareji Progress Plugin. Restart Codex after a changed installation and review the Stop Hook before trusting it.

Run `gareji status` from a registered workspace to see installation health, the number of configured context references, and the newest recorded project activity. The human view follows the target, state, system, activity, and next-action order defined by [Gareji CLI presentation language v0](docs/ui-language-v0.md); `gareji --json status` exposes the same bounded report for automation.

Gareji Core ships command-line and machine-readable JSON Interfaces only. Gareji Board owns the human control surface.

Maintainers can reproduce a release bundle and its clean-profile dry-run check with `python scripts/package_gareji.py --target <rust-target> --release-dir <release-directory> --output-dir dist --smoke-test`. Windows produces a ZIP; Linux and macOS produce a `tar.gz`. Every archive has a SHA-256 sidecar and an internal file manifest.

## Boundaries

Gareji Core intentionally contains no Board-, tracker-, or knowledge-provider-specific task model. Domain payloads remain owned by callers and plugins. Gareji Board owns Work item state and portfolio policy; Runner Adapters own runtime translation. A local Markdown path is project configuration, not a provider Adapter: Core returns the attributed reference and the trusted caller reads it with its normal filesystem access. Do not add credentials, real service data, personal paths, or audit sidecars to this repository.

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
