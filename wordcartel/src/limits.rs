//! Central resource quotas (M5). The one auditable place for "the program is bounded
//! here". Fixed safety rails — refuse on input/output edges, degrade on caches.

/// Refuse opening a file larger than this (enforced by a bounded read, not just metadata).
pub const MAX_OPEN_BYTES: u64 = 64 * 1024 * 1024;
/// Ceiling on a single filter subprocess's output (raised from 1 MiB so a whole-document
/// filter on a large doc is not spuriously refused).
pub const MAX_FILTER_OUTPUT: usize = 64 * 1024 * 1024;
/// Ceiling on a single in-process transform's output.
pub const MAX_TRANSFORM_OUTPUT: usize = 64 * 1024 * 1024;
/// Stop collecting search matches past this (bounds the "everything matches" scan + vector).
pub const MAX_SEARCH_MATCHES: usize = 100_000;
/// Skip/trim persisting a serialized session larger than this; bound the load read at it too.
pub const MAX_SESSION_BYTES: usize = 8 * 1024 * 1024;

/// Max decoded paste size (canonical home; re-exported from clipboard.rs).
pub const PASTE_MAX_BYTES: usize = 8 * 1024 * 1024;
/// Max OSC-52 encoded clipboard payload (canonical home; re-exported from clipboard.rs).
pub const OSC52_MAX_ENCODED: usize = 100_000;
