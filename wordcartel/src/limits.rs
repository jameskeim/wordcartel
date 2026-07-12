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

/// Effort A: harper-ls `maxFileLength` — raise well above the 120 KB default so real
/// long-form documents are checked (the server silently skips longer docs otherwise).
pub const HARPER_MAX_FILE_LENGTH: u64 = 10_000_000;
/// Effort A: client-side cap on the text shipped per recheck over stdio (full-document sync).
/// Comfortably under the server's 10 M-char limit; proportional-to-work discipline, not a
/// correctness need — an over-cap document is skipped with a status and no in-flight state.
pub const DIAG_MAX_SEND_BYTES: u64 = 8 * 1024 * 1024;
/// Effort A: inbound cap on a single LSP `Content-Length`-framed message read from harper-ls
/// (untrusted cross-process input). Comfortably above any real reply to an
/// `DIAG_MAX_SEND_BYTES`-sized document plus JSON-RPC/diagnostics overhead; a frame claiming
/// more is refused with an `io::Error` before any allocation — never a capacity-overflow panic.
pub const LSP_MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

/// P1 plugin registration caps (bounded-memory LAW — interned ids/labels are permanent leaks
/// that `set_memory_limit` does not bound; checked on the raw Lua String BEFORE interning).
pub const PLUGIN_MAX_COMMANDS_PER_PLUGIN: usize = 256;
/// The `<plugin>` file/dir stem.
pub const PLUGIN_MAX_STEM_LEN: usize = 64;
/// The plugin-local command name.
pub const PLUGIN_MAX_NAME_LEN: usize = 128;
/// The menu/palette label.
pub const PLUGIN_MAX_LABEL_LEN: usize = 256;
/// `wc.status` / `error(msg)` truncation (display-only).
pub const PLUGIN_MAX_STATUS_LEN: usize = 4096;
// Edit text reuses PASTE_MAX_BYTES (above) — plugin edits and user paste share one pre-alloc bound.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_caps_are_sane() {
        assert_eq!(PLUGIN_MAX_COMMANDS_PER_PLUGIN, 256);
        assert_eq!(PLUGIN_MAX_STEM_LEN, 64);
        assert_eq!(PLUGIN_MAX_NAME_LEN, 128);
        assert_eq!(PLUGIN_MAX_LABEL_LEN, 256);
        assert_eq!(PLUGIN_MAX_STATUS_LEN, 4096);
    }
}
