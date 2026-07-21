# Use a policy-enforcing Runner seam

Status: Accepted

Gareji Core exposes one in-process `RunExecutor::execute` operation that validates a bounded opaque request and positive execution deadline, evaluates the local Capability Gate, and invokes one injected Runner Adapter only after approval. The Adapter receives a Core-constructed request containing the deadline and sorted capabilities Core authorized; it never receives caller-asserted approval evidence.

The Runner seam is runtime-neutral. Codex flags, model selection, Work item state, workspace preparation, worktree lifecycle, scheduling, and Progress Checkpoint reconciliation remain outside Core. A Runner Adapter translates the opaque payload into exactly one runtime invocation and returns one bounded report containing a compact summary, opaque result payload, evidence references, and a stable failure when relevant.

Core validates both sides of the seam. Invalid requests fail before approval lookup, rejected policy decisions never invoke the Adapter, an unavailable Approval Source fails closed, and an invalid Adapter report is not forwarded to callers. The first Interface is synchronous and in-process; a caller that must keep a UI responsive runs it on a worker. A future process transport may carry the same request and result types without changing the Interface semantics.

The existing Plugin Host envelope remains unchanged. Host execution and Runner execution are separate Modules because they have different lifecycle and failure contracts, even though a later Runner Adapter may delegate part of its work to the Host.
