# ADR 0003: Local Core bridge and operational registry

- Status: **Accepted**
- Date: 2026-07-17
- Accepted-at: local integration implementation review (2026-07-17)
- Deciders: Gareji Core maintainers

## Context

Board, MCP, Runner, CLI, and trusted lifecycle Hooks need the same project-to-workspace mapping, explicit grants, active Work item reference, and Progress Recorder behavior. Importing Core as a private source dependency into every separately released product would make local builds and private CI fragile. Reimplementing Core SQLite access in each product would create several state owners.

The product also needs low startup overhead without committing v0 to a background daemon or a Cloud dependency.

## Decision

Core owns a dependency-light `gareji-contracts` crate, a SQLite-backed Project Registry Module, and a bounded local Bridge Interface.

The Registry stores only operational configuration:

- stable project and Execution workspace references;
- explicit local grants;
- attributed context entries or evidence references;
- projection destination identities;
- one optional active Work item reference per project.

Board continues to own Work item state, project priority, and terminal-state validation. Until a Board Adapter is connected, selecting active work records an opaque reference and does not claim that the Work item exists or is eligible.

The `gareji-core bridge` command serves bounded newline-delimited JSON over stdio. A parent transport may keep one child process alive and reuse it for many operations. The Bridge owns Registry and Progress Recorder access; callers never query Core tables directly.

## Consequences

- MCP can remain a separate repository and thin transport without a private Rust source dependency.
- One child process amortizes startup and database initialization while retaining process isolation.
- A future local daemon or Cloud coordination Adapter can replace the process transport at the same Seam.
- The Core bridge protocol is local and versioned; it is not a generic remote control surface.
- Provider-backed context reads and Board Work item validation remain future Adapters rather than hidden behavior in the Registry.

## Rejected alternatives

- Direct SQLite access from MCP: duplicates Core rules and state ownership.
- A relative path dependency from MCP to a neighboring Core checkout: cannot build reliably for users or private CI.
- A mandatory daemon in v0: adds lifecycle, authentication, and upgrade work before a real multi-client need exists.
- Cloud-only coordination: violates the local-first trust-kernel invariant.
