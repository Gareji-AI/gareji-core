//! AgentMesh Phase 0 host: framing, process supervision, lifecycle, audit.

pub mod audit;
pub mod failure_coord;
pub mod framing;
pub mod lifecycle;
pub mod process;
pub mod redaction;
pub mod run;
pub mod sidecar;

pub use audit::{AuditStore, FsAuditStore, InMemoryAuditStore};
pub use failure_coord::FailureCoordinator;
pub use framing::{FrameDecodeError, FrameDecoder, FrameEncoder, FrameLimits};
pub use lifecycle::{CancellationToken, RunConfig, RunOutcome};
pub use process::{PluginPath, PluginPathError, SpawnError};
pub use redaction::{RedactionError, RedactionPolicy};
pub use run::{execute_run, execute_run_with};
pub use sidecar::{CompactSink, SidecarDocument, StdoutCompactSink, WriteOnceCommit};
