# ADR 0006: Treat local Markdown as a context reference

- Status: **Accepted**
- Date: 2026-07-18
- Deciders: Gareji Core maintainers

## Context

The planned Markdown Zettelkasten, GBrain, and LLMWiki integrations currently differ only by the absolute path of a Markdown file. Core already stores attributed `SourcedContext` entries and returns `evidence_ref` unchanged through `get_project_context`.

Adding one Adapter per product would create multiple shallow implementations with the same pass-through behavior. Deleting them would remove names but no complexity; the path is configuration rather than a behavioral seam.

## Decision

An absolute local Markdown path is registered as `SourcedContext.evidence_ref`. Core attributes, validates, stores, and returns the reference but does not dereference, copy, parse, index, watch, normalize, or write the file. Trusted callers use their existing filesystem access, and external products such as GBrain or LLMWiki may read or index the same file independently.

Machine-specific paths belong only in reviewed local registration data and must not be committed. Repository examples use synthetic absolute paths.

Projects that do not require external Progress Checkpoint write-back register an empty `delivery_targets` list. Core's SQLite Progress Recorder remains the authoritative ledger and `list_progress` remains the read Interface. The existing `CheckpointProjector` seam is dormant in this setup.

A provider Adapter is introduced only when provider-specific behavior exists, such as authentication, remote queries, format or link translation, freshness guarantees, watching, write-back idempotency, or conflict resolution. Two labels or paths with identical behavior do not create a real seam.

## Consequences

- Markdown Zettelkasten, GBrain, and LLMWiki Adapters are removed from the implementation roadmap.
- No schema or Core bridge change is required.
- Context content is read with the caller's existing permissions rather than copied into Core.
- Progress projection remains available for a future concrete destination without becoming a current completion requirement.
- A later provider Adapter requires a new accepted design decision describing the behavior that varies at its seam.

## Rejected alternatives

- One path-only Adapter per product: duplicates pass-through configuration without adding behavior.
- A generic Markdown reader inside Core: expands Core filesystem authority and duplicates capabilities already held by trusted callers.
- Mandatory Markdown checkpoint projection: creates a second progress authority and conflict surface without a current consumer requirement.
