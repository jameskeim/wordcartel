//! A17 T8: the `view_messages` command — render the bounded `StatusHistory` ring (spec §5) into a
//! read-only, path-less scratch-style buffer and switch to it (Q4: NOT a 12th overlay; the ring is
//! the source of truth). Re-invoking regenerates in place — it never stacks a second view, because
//! the read-only buffer IS the singleton marker (`read_only` is set `true` ONLY here).
//!
//! The buffer's content is immutable by construction: the two-category read-only guard (T8) refuses
//! every in-place edit (`Buffer::{apply,undo,redo}`) and every whole-buffer replacement
//! (`Editor::replace_buffer`) on a read-only slot. Regenerating the view is a DIRECT slot assignment
//! (a regenerable view of the ring, never a content edit) — the same principled exclusion as buffer
//! disposal (`workspace.rs`).

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
}
