# ADR 0007: Use a standalone idempotent Setup Module

- Status: **Accepted**
- Date: 2026-07-19
- Deciders: Gareji Core maintainers

## Context

The first usable local configuration requires coordinated Core placement, project registration with absolute Markdown references, Codex Marketplace configuration, Gareji Progress Plugin installation, and final Hook trust review. Requiring users to perform each step manually creates mismatched database, path, and Plugin state.

Putting Codex-specific installation inside gareji-core would expand the trust kernel into a product installer. Writing Core SQLite directly from an installer would create a second implementation of Registry rules.

## Decision

Provide a standalone `gareji-bootstrap` crate with the user-facing `gareji` binary. Its small Interface exposes `setup`, `setup --dry-run`, and `doctor`.

Setup installs Core in platform application data, registers projects through the public gareji-core CLI, and invokes Codex's supported Marketplace and Plugin commands. It leaves matching state unchanged and fails on differing project settings or a conflicting `gareji-local` Marketplace mapping. Replacing a project registration requires `--replace-project`. Doctor checks the same layers without invoking install, registration, or repair operations.

The Setup Module does not access Core SQLite directly, persist another authority file, modify repositories, change PATH, approve capabilities, or silently trust lifecycle Hooks. Codex restart and Hook review remain an explicit user action after a changed Plugin installation.

## Consequences

- One command replaces the previously separate first-run instructions.
- Re-running Setup repairs missing layers without duplicating matching state.
- Core remains independent of Codex Plugin management.
- The local Marketplace is a distribution concern and points at the existing Gareji Progress Plugin.
- Distribution packaging must place gareji and gareji-core together or supply their explicit locations.

## Rejected alternatives

- Add `setup` to gareji-core: mixes installer and Codex concerns into the trust kernel.
- Write registration rows directly: duplicates Registry validation and storage ownership.
- Edit PATH automatically: creates persistent machine-wide state that the Hook does not require.
- Trust the Stop Hook automatically: removes an intentional Codex security review.
