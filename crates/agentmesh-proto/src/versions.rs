//! Protocol and binary version constants.

/// Wire protocol date version for Phase 0.
pub const PROTOCOL_VERSION: &str = "2026-07-15";

/// Alias used in docs and manifests.
pub const PLUGIN_PROTOCOL_DATE: &str = PROTOCOL_VERSION;

/// Host/binary SemVer during private Phase 0.
pub const HOST_VERSION: &str = env!("CARGO_PKG_VERSION");
