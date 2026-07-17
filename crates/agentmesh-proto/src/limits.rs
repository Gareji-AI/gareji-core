//! Normative Phase 0 limits.

/// Host default limits. Overrides except run timeout are unstable/private in Phase 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Limits {
    /// Maximum input JSON bytes.
    pub input_max_bytes: usize,
    /// Maximum bytes per protocol frame body.
    pub frame_max_bytes: usize,
    /// Retained stderr bytes before discard-drain continues.
    pub stderr_retain_bytes: usize,
    /// Initialize deadline in milliseconds.
    pub initialize_timeout_ms: u64,
    /// Run deadline in milliseconds (configurable 1s..=1h).
    pub run_timeout_ms: u64,
    /// Exit grace after stdin close in milliseconds.
    pub exit_grace_ms: u64,
    /// Final sidecar hard cap.
    pub sidecar_max_bytes: usize,
    /// Maximum JSON nesting depth for host-owned and opaque JSON trees.
    pub json_max_depth: usize,
    /// Maximum JSON structural nodes.
    pub json_max_nodes: usize,
    /// Maximum framing header block size.
    pub header_block_max_bytes: usize,
    /// Maximum bytes per header line.
    pub header_line_max_bytes: usize,
    /// Maximum header lines.
    pub header_max_lines: usize,
    /// Maximum Content-Length digit length accepted before allocation.
    pub content_length_digits_max: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            input_max_bytes: 1 * 1024 * 1024,
            frame_max_bytes: 4 * 1024 * 1024,
            stderr_retain_bytes: 256 * 1024,
            initialize_timeout_ms: 5_000,
            run_timeout_ms: 60_000,
            exit_grace_ms: 2_000,
            sidecar_max_bytes: 10 * 1024 * 1024,
            json_max_depth: 64,
            json_max_nodes: 100_000,
            header_block_max_bytes: 8 * 1024,
            header_line_max_bytes: 1 * 1024,
            header_max_lines: 16,
            content_length_digits_max: 10,
        }
    }
}

impl Limits {
    /// Validate that a run timeout override is in the approved range.
    pub fn validate_run_timeout_ms(ms: u64) -> Result<u64, &'static str> {
        if (1_000..=3_600_000).contains(&ms) {
            Ok(ms)
        } else {
            Err("run timeout must be between 1 second and 1 hour")
        }
    }
}
