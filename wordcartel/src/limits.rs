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
/// Cap on a single plugin source file read by `discover` (Task 6) — generous headroom over
/// any real plugin script (plugin files are user CODE, not documents); an oversize file is
/// skipped + named in the returned report, never truncated or silently dropped.
pub const PLUGIN_MAX_SOURCE_BYTES: u64 = 1024 * 1024;
/// Max nesting depth converted from `[plugins.config.<name>]` into a Lua table.
pub const PLUGIN_MAX_CONFIG_DEPTH: usize = 8;
/// Max total nodes (keys + values) converted from one plugin's config table.
pub const PLUGIN_MAX_CONFIG_NODES: usize = 1024;
/// Max byte length of any single config string VALUE or table KEY — the pre-allocation byte
/// bound (resource-bound LAW) that depth+node counts miss: `config::load` reads the source
/// unbounded, so one giant string/key must be rejected BEFORE `lua.create_string` allocates it.
pub const PLUGIN_MAX_CONFIG_STR: usize = 64 * 1024;

/// P2 event-system caps (bounded-memory LAW, extending the P1 registration caps above).
/// Max `wc.on` hooks a single plugin may register — each hook stores a Lua function in the VM
/// registry plus an owned (never interned) Rust-side `HookEntry`.
pub const PLUGIN_MAX_HOOKS_PER_PLUGIN: usize = 64;
/// Max byte length of a `PluginEvent`'s captured path payload (`cap_status`-clamped at the fire
/// site) — the queue holds bounded owned data even for a pathological path.
pub const PLUGIN_MAX_EVENT_PAYLOAD: usize = 4096;

/// A17 — cap on a single `Status` message's displayed text (display-only truncation, char-boundary
/// safe). Deliberately reuses `PLUGIN_MAX_STATUS_LEN` so plugin `wc.status` and host messages share
/// one bound; T11 asserts this identity holds (a divergence would be a silent behavior change).
pub const MESSAGES_MAX_TEXT_LEN: usize = PLUGIN_MAX_STATUS_LEN;
/// A17 — max entries kept in the `StatusHistory` ring (M5 resource-cap ethos: fixed capacity,
/// oldest evicted, no growth at rest).
pub const MESSAGES_HISTORY_CAP: usize = 256;
/// A17 — max `seq` gap between two otherwise-identical adjacent messages for them to still
/// coalesce via `repeat` (spec §5.2). `1` means only a truly back-to-back repeat coalesces.
pub const MESSAGES_DEDUP_WINDOW: u64 = 1;
/// A17 T9 — display-slot throttle for the plugin emit path (spec §9.3): at most this many
/// `wc.status`/`wc.notify` slot updates per plugin (keyed by `InvokeState::current` label) per
/// pump tick (one `PluginHost::pump` cycle — `plugin::host::EmitThrottle::advance_tick`). Excess
/// emits within the same tick still reach history (`Editor::record_status_history_only`, subject
/// to §5.2 dedup) — only the display-slot write is dropped. A conservative v1 default: a looping
/// plugin (`while true do wc.status('x') end`) must not repaint the slot every callback.
pub const MESSAGES_EMIT_MAX_PER_TICK: usize = 1;

/// Max byte length of a `wc.command(name)` target — the longest possible registered id
/// (`<stem>.<name>`), so this cap can never reject a resolvable name (§5a).
pub const PLUGIN_MAX_COMMAND_REF: usize = PLUGIN_MAX_STEM_LEN + 1 + PLUGIN_MAX_NAME_LEN;
/// Max queued `wc.command` dispatches awaiting drain — a single callback looping on
/// `wc.command` must not grow an unbounded queue before the chain cap can even run (§5a).
pub const PLUGIN_MAX_PENDING_DISPATCH: usize = 64;
/// The pump's re-drain loop chain cap (§5c): the deterministic, testable bound on ping-pong
/// cascade length (one dispatch, one command callback, or one hook invocation = one unit).
pub const PLUGIN_PUMP_CHAIN_CAP: usize = 64;

/// P3 plugin-timer + parameterized-command caps.
/// Min timer interval — the spin defense: a repeating timer reschedules to `now + interval >= now +
/// 1000ms` from completion, so it wakes at most ~once/interval (a due deadline may yield ONE immediate
/// zero-timeout wake, then fires and moves 1s+ out — bounded cadence, not a spin). Sub-floor → typed error.
pub const PLUGIN_TIMER_MIN_INTERVAL_MS: u64 = 1000;
/// Max armed timers per plugin (heavier than a hook — each keeps a wall-clock wake alive). Over → typed error.
pub const PLUGIN_MAX_TIMERS_PER_PLUGIN: usize = 8;
/// Max bytes of a parameterized-command argument (wc.command arg / the PluginArg minibuffer line),
/// checked before the owning String allocation (resource-bound LAW).
pub const PLUGIN_MAX_COMMAND_ARG: usize = 4096;

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
        assert_eq!(PLUGIN_MAX_SOURCE_BYTES, 1024 * 1024);
        assert_eq!(PLUGIN_MAX_CONFIG_DEPTH, 8);
        assert_eq!(PLUGIN_MAX_CONFIG_NODES, 1024);
        assert_eq!(PLUGIN_MAX_CONFIG_STR, 64 * 1024);
        assert_eq!(PLUGIN_MAX_HOOKS_PER_PLUGIN, 64);
        assert_eq!(PLUGIN_MAX_EVENT_PAYLOAD, 4096);
        assert_eq!(PLUGIN_MAX_COMMAND_REF, PLUGIN_MAX_STEM_LEN + 1 + PLUGIN_MAX_NAME_LEN);
        assert_eq!(PLUGIN_MAX_PENDING_DISPATCH, 64);
        assert_eq!(PLUGIN_PUMP_CHAIN_CAP, 64);
        assert_eq!(PLUGIN_TIMER_MIN_INTERVAL_MS, 1000);
        assert_eq!(PLUGIN_MAX_TIMERS_PER_PLUGIN, 8);
        assert_eq!(PLUGIN_MAX_COMMAND_ARG, 4096);
    }

    // A17: guardrail so the messaging caps aren't drifted silently — the history ring is a
    // fixed-cap M5 resource bound, the text cap is shared with `wc.status`, and the emit
    // throttle/dedup windows are tuned constants a whole-branch/smoke pass may revisit.
    #[test]
    fn messages_caps_are_stable() {
        assert_eq!(MESSAGES_HISTORY_CAP, 256);
        assert_eq!(MESSAGES_MAX_TEXT_LEN, PLUGIN_MAX_STATUS_LEN);
        assert_eq!(MESSAGES_DEDUP_WINDOW, 1);
        assert_eq!(MESSAGES_EMIT_MAX_PER_TICK, 1);
    }
}
