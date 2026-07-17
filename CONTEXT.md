# Gareji Core

Gareji Core is the local trust kernel for bounded agent execution. This glossary separates stable Core concepts from Board, Runner, and knowledge-product concerns.

## Execution

**Gareji Core**:
The local authority that validates permitted agent work and preserves verifiable execution evidence through stable Interfaces.
_Avoid_: Board, Runner, scheduler

**Host**:
The Core role that starts one trusted Plugin, owns its lifecycle envelope, and converts its result into a bounded Core result.
_Avoid_: Plugin, shell wrapper

**Plugin**:
A trusted local executable that implements the Core Plugin Interface while keeping its domain payload opaque to the Host.
_Avoid_: remote service, shell command

**Runner**:
A replaceable execution Adapter that translates a bounded Core execution request into one agent-runtime invocation.
_Avoid_: Core, task-state authority

**Core Run**:
One bounded execution attempt with a unique identity, selected Plugin or Runner, outcome, and evidence references.
_Avoid_: Work item, project

## Trust and evidence

**Capability policy**:
The locally enforced declaration of what one Plugin, Runner, or Run may access or change.
_Avoid_: agent instructions, Board priority

**Compact result**:
The bounded machine-readable outcome returned to a caller after a Core Run.
_Avoid_: raw transcript, audit sidecar

**Audit sidecar**:
A potentially sensitive local evidence record containing details that do not belong in the Compact result.
_Avoid_: public log, task state

**Progress Recorder**:
The Core Module that durably accepts an immutable Progress Checkpoint and tracks its independent projection deliveries.
_Avoid_: Board transition, Hook

**Project Registry**:
The Core Module that stores explicit operational project-to-workspace configuration, grants, and active Work item references without owning Board state.
_Avoid_: Board project model, inferred repository scanner

**Core bridge**:
The versioned local Interface through which a trusted parent transport uses Registry and Progress Recorder behavior without accessing Core storage directly.
_Avoid_: daemon, generic remote API

**Board Adapter**:
The replaceable Core role that obtains Board-owned active-work assessments before Core stores an operational Work item reference.
_Avoid_: Work item state owner, Board database reader

## Integration

**App**:
A pinned, integrity-checked package that identifies a Plugin and its allowed runtime configuration.
_Avoid_: Board project, Plugin registry entry

**Opaque payload**:
Plugin-owned structured data that Core transports and bounds without promoting product-specific fields into the Host envelope.
_Avoid_: Core domain type
