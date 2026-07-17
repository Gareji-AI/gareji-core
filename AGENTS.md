# Gareji Core contributor guide

## Scope

Gareji Core is the local trust kernel. The current implementation is a tool-neutral Host and Plugin contract; planned Core Modules add generic capability policy, progress recording, and local durable state without importing Board-specific Work item semantics.

Tracker-, Board-, runtime-, and knowledge-provider-specific integrations belong behind Interfaces in separate repositories or Adapter Modules. Keep their payloads opaque to `agentmesh-proto` and `agentmesh-host`.

Do not change Host envelope ownership, add product domain types to the protocol crates, or add a daemon, TUI, registry, remote Plugin, or Cloud authority without an accepted design decision. SQLite is permitted only inside the `gareji-core` Progress Recorder Module described by ADR 0002; do not couple it to the existing Host Interface.

## Workspace rules

- Fixture plugins depend on `agentmesh-fixture-support` and `agentmesh-proto` only.
- Malformed framing/JSON fixtures use independent raw writers.
- Never add credentials, user data, vault paths, or real service payloads to the repository.
- Audit sidecars are potentially sensitive; do not commit them or attach them to public issues.

## Checks before a pull request

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Change discipline

- One commit represents one independently reviewable and revertible meaning.
- Keep tests, schemas, fixtures, and documentation with the behavior they prove.
- Separate mechanical renames from behavior changes.
- Use an English Conventional Commit subject and explain intent, change, and verification in the body when the reason is not obvious.
- Never include private strategy, pricing, provenance investigations, credentials, personal paths, or audit sidecars in public history.

## Agent skills

### Issue tracker

Issues are tracked as local Markdown files under `.scratch/`. See `docs/agents/issue-tracker.md`.

### Triage labels

Use the default triage label vocabulary. See `docs/agents/triage-labels.md`.

### Domain docs

Use a single-context documentation layout. See `docs/agents/domain.md`.
