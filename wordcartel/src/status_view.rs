//! A17 T8: the `view_messages` command — render the bounded `StatusHistory` ring (spec §5) into a
//! read-only, path-less scratch-style buffer and switch to it (Q4: NOT a 12th overlay; the ring is
//! the source of truth). Re-invoking regenerates in place — it never stacks a second view, because
//! the read-only buffer IS the singleton marker (`read_only` is set `true` ONLY here in production —
//! pinned by `read_only_true_is_set_only_in_this_module` below).
//!
//! SCOPE OF THE READ-ONLY GUARANTEE (honest claim — A17 final gate). The read-only flag proves two
//! things and ONLY two: no CONTENT MUTATION (the closed set `Buffer::{apply,undo,redo}` no-op on a
//! read-only slot) and no CONTENT-INSTALL REPLACEMENT (`Editor::replace_buffer` rejects a read-only
//! slot). It does NOT forbid DISPOSING the buffer: a close/dispose (`workspace::close_buffer_now`'s
//! last-ordinary reset) is a distinct, SANCTIONED operation that swaps the slot for a FRESH writable
//! buffer with NO content carried over — safe precisely because `read_only ⟹ regenerable/ephemeral
//! content` (this view is a projection of the ring, never user data). Regenerating the view here is
//! likewise a DIRECT slot assignment, not a content edit. A future effort that ever marks a
//! CONTENT-BEARING buffer `read_only` breaks that implication and MUST revisit `close_buffer_now`.

use crate::editor::{Buffer, Editor};
use crate::status::{Status, StatusHistory, StatusKind, StatusSource};

/// Open (or regenerate) the message-history view and switch to it.
pub fn open(editor: &mut Editor) {
    let body = render(editor.status_history());
    let area = editor.active().view.area;
    // The read-only flag is the singleton marker: at most one message view exists, so re-invoking
    // regenerates its content in place rather than stacking a second buffer.
    if let Some(idx) = editor.buffers.iter().position(|b| b.read_only) {
        let id = editor.buffers[idx].id;
        let mut buf = Buffer::from_text(id, &body, None, area);
        buf.read_only = true;
        editor.buffers[idx] = buf; // regenerable view of the ring — never a content edit (see module doc).
        editor.switch_to_index(idx);
    } else {
        let id = editor.alloc_id();
        let mut buf = Buffer::from_text(id, &body, None, area);
        buf.read_only = true;
        editor.buffers.push(buf);
        let idx = editor.buffers.len() - 1;
        editor.switch_to_index(idx);
    }
    crate::derive::rebuild(editor);
    crate::nav::ensure_visible(editor);
    editor.clear_status();
}

/// Render the ring into the buffer body — oldest first (ring insertion order). Row format:
/// `"[<Kind>] <text>"`, plugin attribution `"[<Kind> · plugin:<label>]"`, `"(×N) "` prefix on the
/// text when the entry coalesced repeats (`repeat > 1`). Always ends with a trailing newline; an
/// empty ring yields a single newline (a valid, non-empty buffer).
fn render(history: &StatusHistory) -> String {
    let mut out = String::new();
    for s in history.entries() {
        out.push_str(&render_row(s));
        out.push('\n');
    }
    if out.is_empty() { out.push('\n'); }
    out
}

fn render_row(s: &Status) -> String {
    let kind = kind_label(s.kind());
    let tag = match s.source() {
        StatusSource::Host => format!("[{kind}]"),
        StatusSource::Plugin { label } => {
            let label = label.as_deref().unwrap_or("?");
            format!("[{kind} \u{00B7} plugin:{label}]")
        }
    };
    if s.repeat() > 1 {
        format!("{tag} (\u{00D7}{}) {}", s.repeat(), s.text())
    } else {
        format!("{tag} {}", s.text())
    }
}

/// Display spelling of a kind for the message list (capitalized; distinct from the lowercase
/// round-trip spelling `StatusKind::as_str`).
fn kind_label(kind: StatusKind) -> &'static str {
    match kind {
        StatusKind::Error => "Error",
        StatusKind::Warning => "Warning",
        StatusKind::Info => "Info",
        StatusKind::Log => "Log",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status::{StatusLifetime, StatusTopic};

    #[test]
    fn view_messages_opens_a_read_only_buffer_listing_history() {
        let mut e = Editor::new_from_text("x\n", None, (40, 6));
        e.set_status(StatusKind::Info, "first");
        e.set_status_full(StatusKind::Error, "boom", StatusLifetime::Sticky, StatusSource::Host, None);
        open(&mut e);
        assert!(e.active().read_only);
        let body = e.active().document.buffer.to_string();
        assert!(body.contains("first") && body.contains("boom"), "body: {body:?}");
    }

    #[test]
    fn re_invoking_view_messages_regenerates_in_place_without_stacking() {
        let mut e = Editor::new_from_text("x\n", None, (40, 6));
        e.set_status(StatusKind::Info, "first");
        open(&mut e);
        let n_after_first = e.buffers.len();
        let view_id = e.active().id;
        // A new message emitted while the read-only view is active is REJECTED (no false ack); emit
        // it against a fresh writable editor state is unnecessary — just re-open and confirm no stack.
        open(&mut e);
        assert_eq!(e.buffers.len(), n_after_first, "re-invoking must not stack a second view");
        assert_eq!(e.active().id, view_id, "regenerated in the same slot / id");
        assert!(e.active().read_only);
    }

    #[test]
    fn render_row_formats_host_plugin_and_repeat() {
        let host = Status::new(StatusKind::Info, "hi", StatusLifetime::Transient, StatusSource::Host, None, 0);
        assert_eq!(render_row(&host), "[Info] hi");
        let plug = Status::new(StatusKind::Info, "hi",
            StatusLifetime::Transient, StatusSource::Plugin { label: Some("demo".into()) }, None, 1);
        assert_eq!(render_row(&plug), "[Info \u{00B7} plugin:demo] hi");
        let mut rep = Status::new(StatusKind::Warning, "dup", StatusLifetime::Sticky, StatusSource::Host, None, 2);
        rep.bump_repeat(); rep.bump_repeat();
        assert_eq!(render_row(&rep), "[Warning] (\u{00D7}3) dup");
    }

    #[test]
    fn empty_history_renders_a_valid_non_empty_buffer() {
        let h = StatusHistory::new();
        assert_eq!(render(&h), "\n");
    }

    #[test]
    fn progress_topic_entry_renders() {
        let mut e = Editor::new_from_text("x\n", None, (40, 6));
        e.set_progress(StatusTopic::Filter, "filtering…");
        open(&mut e);
        assert!(e.active().document.buffer.to_string().contains("filtering"));
    }

    /// A17 final gate — pin the load-bearing assumption of the dispose-safety argument (module doc):
    /// the ONLY production site that sets `read_only = true` is this module's `open` path, so
    /// `read_only ⟹ regenerable/ephemeral content` holds tree-wide. `close_buffer_now`'s dispose
    /// safety depends on it; a future effort marking a content-bearing buffer read-only would trip
    /// this test and be forced to revisit that path.
    ///
    /// The scan is source-textual (not reflective): for each `src/**/*.rs` file it considers only the
    /// PRODUCTION prefix — everything before the first `#[cfg(test)]` marker — because tests
    /// legitimately flip `read_only` on ad-hoc buffers to exercise the guard. This crate keeps its
    /// tests in trailing `#[cfg(test)] mod tests` blocks, so the prefix is the production surface.
    #[test]
    fn read_only_true_is_set_only_in_this_module() {
        fn scan(dir: &std::path::Path, hits: &mut Vec<String>) {
            for entry in std::fs::read_dir(dir).expect("read src dir") {
                let path = entry.expect("dir entry").path();
                if path.is_dir() { scan(&path, hits); continue; }
                if path.extension().and_then(|e| e.to_str()) != Some("rs") { continue; }
                let src = std::fs::read_to_string(&path).expect("read source file");
                // Production prefix = everything before the first test-module marker.
                let prod = src.split("#[cfg(test)]").next().unwrap_or("");
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
                for (i, line) in prod.lines().enumerate() {
                    // Match an ASSIGNMENT `read_only = true` (a single `=`, not `==`), tolerant of
                    // spacing; skip anything after a `//` line comment so prose can mention it freely.
                    let code = line.split("//").next().unwrap_or(line);
                    let compact: String = code.split_whitespace().collect();
                    if compact.contains("read_only=true") && !compact.contains("read_only==true") {
                        hits.push(format!("{name}:{}", i + 1));
                    }
                }
            }
        }
        let src_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut hits = Vec::new();
        scan(&src_root, &mut hits);
        assert!(!hits.is_empty(), "sanity: the scan must find this module's own writers");
        assert!(hits.iter().all(|h| h.starts_with("status_view.rs:")),
            "read_only = true is set in production OUTSIDE status_view.rs — the dispose-safety \
             assumption (read_only ⟹ regenerable content) may be broken; revisit close_buffer_now. \
             Sites: {hits:?}");
    }
}
