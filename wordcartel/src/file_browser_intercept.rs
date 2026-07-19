//! File-browser input interception — moved out of `file_browser.rs` (C5 Task 18) onto the
//! `BrowseMode` axis. SELECT mode filters/commits on `fb.query`, unchanged. DESTINATION
//! mode's field is dual-duty — simultaneously the filename-to-be and the live listing
//! filter — so this module is where keystrokes are routed to the FIELD (via the shared
//! `minibuffer::text_*` cursor helpers) or to the SELECTION (via `list_window`), never both
//! at once: nav never edits the field, field edits never move the selection except to clamp
//! it.

use crate::app::Msg;
use crossterm::event::Event;
use crate::file_browser::BrowseMode;

/// File browser overlay intercepts KEY INPUT and PASTE. Non-key, non-paste messages
/// fall through to normal handling while the browser stays open (mirrors theme_picker).
#[allow(clippy::too_many_lines)] // a flat, exhaustive per-key dispatch table — every arm
// already delegates to a domain helper (file_browser/file_browser_commit/list_window/
// minibuffer); the count comes from arm cardinality, not fat logic in any one arm.
pub(crate) fn intercept(msg: crate::app::Msg, editor: &mut crate::editor::Editor,
    ctx: &crate::overlays::DispatchCtx) -> crate::app::Handled {
    if editor.file_browser.is_none() { return crate::app::Handled::Pass(msg); }
    // Drop an async clipboard-paste result that arrives while the browser is open —
    // it must not land in the document behind the overlay (Codex I6, mirror palette).
    if matches!(&msg, Msg::ClipboardPaste { .. }) {
        return crate::app::Handled::Done(crate::app::fold_and_continue(editor, ctx.ex, ctx.clock, ctx.msg_tx, ctx.fs));
    }
    // Both editor-owned filter options, read once — `rederive` derives the destination flag
    // and the filter text from `fb.mode` itself, so every call site below just forwards these
    // two and can never pass the wrong `destination`/text pairing (Task 18).
    let (show_clutter, types) = (editor.files_show_clutter, editor.files_type_filter);
    if let Msg::Input(Event::Paste(text)) = &msg {
        let ah = editor.active().view.area.1;
        if let Some(fb) = editor.file_browser.as_mut() {
            match &mut fb.mode {
                BrowseMode::Select | BrowseMode::Recents => fb.query.push_str(text),
                BrowseMode::Destination { field, field_cursor, .. } => {
                    for c in text.chars() { crate::minibuffer::text_insert(field, field_cursor, c); }
                }
            }
            crate::file_browser_listing::rederive(fb, show_clutter, types);
            crate::app::keep_overlay_visible(ah, fb.selected, fb.entries.len(), &mut fb.scroll_top);
        }
        return crate::app::Handled::Done(crate::app::fold_and_continue(editor, ctx.ex, ctx.clock, ctx.msg_tx, ctx.fs));
    }
    if let Msg::Input(Event::Key(k)) = &msg {
        if k.kind == crossterm::event::KeyEventKind::Press {
            use crossterm::event::KeyCode;
            match k.code {
                // Destination mode's Esc additionally aborts an in-progress quit drain and
                // clears the overwrite-pair state — see `file_browser::cancel_destination`.
                KeyCode::Esc => {
                    if editor.file_browser.as_ref().is_some_and(|fb| fb.mode.is_destination()) {
                        crate::file_browser::cancel_destination(editor);
                    } else {
                        editor.file_browser = None;
                    }
                }
                // Destination mode commits via `file_browser_commit::commit_destination` — THE
                // single place a picker commit becomes a write (Task 21).
                KeyCode::Enter => {
                    let is_destination = editor.file_browser.as_ref()
                        .is_some_and(|fb| fb.mode.is_destination());
                    if is_destination {
                        crate::file_browser_commit::commit_destination(
                            editor, ctx.fs, ctx.ex, ctx.clock, ctx.msg_tx);
                    } else {
                        crate::file_browser::file_browser_enter(editor, ctx.fs, ctx.msg_tx);
                    }
                }
                // The Tab gesture (destination mode only): copy a highlighted FILE's name
                // into the field. Never commits — see `file_browser_commit::copy_name_into_field`.
                KeyCode::Tab => {
                    if let Some(fb) = editor.file_browser.as_mut() {
                        let highlighted = fb.entries.get(fb.selected).cloned();
                        if let (BrowseMode::Destination { field, field_cursor, .. }, Some(entry)) =
                            (&mut fb.mode, highlighted.as_ref())
                        {
                            if matches!(entry.kind, crate::fsx::EntryKind::File) {
                                crate::file_browser_commit::copy_name_into_field(field, field_cursor, &entry.name);
                            }
                        }
                    }
                }
                c if crate::list_window::list_nav_key(c).is_some() => {
                    let ah = editor.active().view.area.1;
                    if let Some(fb) = editor.file_browser.as_mut() {
                        let before = fb.selected;
                        crate::list_window::apply_list_nav(crate::list_window::list_nav_key(c).unwrap(),
                            ah, fb.entries.len(), &mut fb.selected, &mut fb.scroll_top);
                        // Only a GENUINE move counts as the writer choosing the highlight — a
                        // recognised key that saturates at a boundary (Up at row 0, Down at
                        // the last row, PageUp/Home already at the top, …) is a no-op, not a
                        // choice, and must not arm Row 1 of `classify_destination_enter` on a
                        // highlight nobody actually moved onto. See `FileBrowser::navigated_name`.
                        if fb.selected != before {
                            fb.navigated_name = fb.entries.get(fb.selected).map(|e| e.name.clone());
                        }
                    }
                }
                KeyCode::Backspace => {
                    let ah = editor.active().view.area.1;
                    if let Some(fb) = editor.file_browser.as_mut() {
                        match &mut fb.mode {
                            BrowseMode::Select | BrowseMode::Recents => { fb.query.pop(); }
                            BrowseMode::Destination { field, field_cursor, .. } => {
                                crate::minibuffer::text_backspace(field, field_cursor);
                            }
                        }
                        crate::file_browser_listing::rederive(fb, show_clutter, types);
                        crate::app::keep_overlay_visible(ah, fb.selected, fb.entries.len(), &mut fb.scroll_top);
                    }
                }
                // Left/Right move the FIELD cursor in destination mode only — select mode has
                // no cursor to move (its query has none today), so this is a no-op there,
                // exactly as it was before this key had its own match arm.
                KeyCode::Left => {
                    if let Some(fb) = editor.file_browser.as_mut() {
                        if let BrowseMode::Destination { field, field_cursor, .. } = &mut fb.mode {
                            crate::minibuffer::text_left(field, field_cursor);
                        }
                    }
                }
                KeyCode::Right => {
                    if let Some(fb) = editor.file_browser.as_mut() {
                        if let BrowseMode::Destination { field, field_cursor, .. } = &mut fb.mode {
                            crate::minibuffer::text_right(field, field_cursor);
                        }
                    }
                }
                KeyCode::Char(c)
                    if !k.modifiers.contains(crossterm::event::KeyModifiers::CONTROL)
                        && !k.modifiers.contains(crossterm::event::KeyModifiers::ALT) =>
                {
                    let ah = editor.active().view.area.1;
                    if let Some(fb) = editor.file_browser.as_mut() {
                        match &mut fb.mode {
                            BrowseMode::Select | BrowseMode::Recents => fb.query.push(c),
                            BrowseMode::Destination { field, field_cursor, .. } => {
                                crate::minibuffer::text_insert(field, field_cursor, c);
                            }
                        }
                        crate::file_browser_listing::rederive(fb, show_clutter, types);
                        crate::app::keep_overlay_visible(ah, fb.selected, fb.entries.len(), &mut fb.scroll_top);
                    }
                }
                _ => {}
            }
        }
        return crate::app::Handled::Done(crate::app::fold_and_continue(editor, ctx.ex, ctx.clock, ctx.msg_tx, ctx.fs));
    }
    // Non-key msg falls through to normal handling while the browser stays open.
    crate::app::Handled::Pass(msg)
}
